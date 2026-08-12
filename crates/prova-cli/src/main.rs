//! `prova` CLI.
//!
//! Usage:
//!   prova <file-or-dir>...           run the given files/dirs (console output)
//!   prova                            run the suite declared in ./prova.toml
//!   prova --profile ci               run the `ci` profile from ./prova.toml
//!   prova --manifest path.toml       use a specific manifest
//!   prova --format json <path>       stream JSONL events (machine/GUI protocol)
//!   prova --format tap <path>        emit TAP (Test Anything Protocol) to stdout
//!   prova --junit results.xml <path> also write a JUnit XML report (for CI dashboards)
//!   prova --color always|never       force/disable ANSI color (default: auto — TTY only)
//!   prova -q <path>                  quiet: failures, the recap, and the tally only
//!   prova --gha on|off <path>        GitHub Actions annotations + step summary (default: auto)
//!   prova --list <path>              discover tests without running them
//!   prova --jobs N <path>            run up to N units concurrently (throughput only)
//!
//! CLI flags override manifest values; explicit path arguments override the manifest's SELECTION
//! (what runs) while the package environment (plugins, capabilities, run defaults) still applies —
//! home discovery anchors at the named paths, so a file runs the same from anywhere.

mod annotations;
mod broker;
// `claims` moved to prova-core (`prova_core::ledger::claims`) so an embedding host can
// read what a project owes; the CLI is one renderer over that ledger, not its owner.
use prova_core::ledger::claims;
mod catalog;
mod home;
mod ide;
mod init;
mod learn;
mod manifest;
mod mcp;
mod packages;
mod placement;
mod registry;
mod progress;
mod record;
mod report;
mod runstate;
mod var;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

use home::Home;
use manifest::{Manage, Manifest, SuiteDecl};
use prova_core::{
    discover_files, discover_suite, discover_suites, run_suites, JUnitReporter, JsonReporter,
    MultiReporter, PortMode, Reporter, RunConfig, Suite, SystemLayout, TapReporter,
    XdgSystemLayout,
};

/// The cwd-test serializer: tests that `set_current_dir` are safe under nextest
/// (process-per-test) but RACE each other under plain `cargo test` (threads, one process, one
/// cwd) — the release gate runs the latter and caught exactly that. Hold this for the whole
/// body and restore on drop; poisoning recovers (a failed cwd test must not cascade).
#[cfg(test)]
pub(crate) struct CwdGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
    saved: std::path::PathBuf,
}

#[cfg(test)]
impl CwdGuard {
    pub(crate) fn hold() -> Self {
        static CWD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let lock = CWD_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        CwdGuard {
            _lock: lock,
            saved: std::env::current_dir().expect("the test process has a cwd"),
        }
    }
}

#[cfg(test)]
impl Drop for CwdGuard {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.saved);
    }
}

/// One subcommand: its dispatch name, its `--help` lines, and its entry point — ONE row per
/// verb, so a verb cannot exist undocumented (the field is required) and the help text cannot
/// name a verb that doesn't dispatch (the same row does both). See docs/plans/autodidact.md §2.8.
struct Verb {
    name: &'static str,
    /// This verb's lines in `prova --help`, exactly as printed (including indentation).
    help: &'static str,
    run: fn(Vec<String>) -> ExitCode,
}

/// Every subcommand, in `--help` order. The run path (`prova [<file-or-dir>...]`) is the
/// fallback when no verb matches.
const VERBS: &[Verb] = &[
    Verb {
        name: "init",
        help: "  prova init [<key>]        render a catalog archetype into this package (interactive if no key),\n\
               \x20                           then wire LuaLS IDE support\n\
               \x20 prova init --list         list the init catalog: the archetypes prova can scaffold from",
        run: init::run,
    },
    Verb {
        name: "ide",
        help: "  prova ide setup           (re)wire this package's LuaLS support: core stubs + .luarc.json",
        run: ide::run,
    },
    Verb {
        name: "run",
        help: "  prova run [<lane>]        run the suite through a named lane ([profiles.<lane>]) — sugar\n\
               \x20                           for `--profile`; `prova run --list` lists this package's lanes",
        run: run_subcommand,
    },
    Verb {
        name: "eval",
        help: "  prova eval '<code>'       run a one-shot Lua snippet in the full prova environment and print\n\
               \x20                           the returned value (`-` reads the snippet from stdin)",
        run: eval_subcommand,
    },
    Verb {
        name: "skill",
        help: "  prova skill               print the agent skill (how to drive Prova); --install writes it\n\
               \x20                           to .claude/skills/prova/SKILL.md at the package root",
        run: skill_subcommand,
    },
    Verb {
        name: "learn",
        help: "  prova learn [<topic>]     the topic catalog: progressive disclosure of how Prova works\n\
               \x20                           (no topic lists them; slots render THIS package's facts)",
        run: learn::run,
    },
    Verb {
        name: "introspect",
        help: "  prova introspect [<filter>]  the API surface: every function/value as signature +\n\
               \x20                           summary (the CLI twin of `prova.help()`; learn's shapes sibling)",
        run: introspect_subcommand,
    },
    Verb {
        name: "owed",
        help: "  prova owed                what this package still owes: open promises, unproven claims,\n\
               \x20                           covers pointing at prose that is not there, and due reminders",
        run: owed_subcommand,
    },
    Verb {
        name: "specs",
        help: "  prova specs               the specs lane: claims + backlog items, state-tagged;\n\
               \x20                           --claims/--backlog narrow; drivers: `capture`, `promote <id>`, `backfill`",
        run: specs_subcommand,
    },
    Verb {
        name: "tests",
        help: "  prova tests               the tests lane: every node, state-tagged PROMISE/PROOF;\n\
               \x20                           --promises/--proofs narrow; `burndown`/`falsify` drive",
        run: tests_subcommand,
    },
    Verb {
        name: "reminders",
        help: "  prova reminders           the attention account: every `prova.remind` with its recorded\n\
               \x20                           state (DUE / WATCHING / UNEVALUATED); exits non-zero when any is due",
        run: reminders_subcommand,
    },
    Verb {
        name: "switches",
        help: "  prova switches            every declared opt-in class (`switch = ...`): how many tests it\n\
               \x20                           gates, and who throws it — a profile, [run], or ad-hoc only",
        run: switches_subcommand,
    },
    Verb {
        name: "attest",
        help: "  prova attest [<address>]  did the proof covering this claim actually RUN? Fails when it was\n\
               \x20                           skipped, deselected or absent; no address gates EVERY claim (CI)",
        run: attest_subcommand,
    },
    Verb {
        name: "evidence",
        help: "  prova evidence            the whole account: CLAIMED / BOUND / PROMISED / ATTESTED with\n\
               \x20                           counts, then what is owed — where does this project stand?",
        run: evidence_subcommand,
    },
    Verb {
        name: "capabilities",
        help: "  prova capabilities        what prova can detect on THIS host: the built-in capability\n\
               \x20                           vocabulary (docker, github, native clients…), each met or unmet",
        run: capabilities_subcommand,
    },
    Verb {
        name: "mcp",
        help: "  prova mcp                 serve an MCP stdio server whose tools mirror the CLI (run, list, eval)",
        run: mcp::run,
    },
    Verb {
        name: "up",
        help: "  prova up [<topology>] [<url>]  list/stand up a topology — local, or from a git repo that advertises it",
        run: up_subcommand,
    },
    Verb {
        name: "lock",
        help: "  prova lock <token> [--reads] [--machine] -- <cmd>  hold a package lock while <cmd> runs — join\n\
               \x20                           the suite's house rules from any tool (`prova learn locks`)",
        run: lock_subcommand,
    },
    Verb {
        name: "broker",
        help: "  prova broker              spec scaffolding: the single-machine reference placement broker the\n\
               \x20                           conformance suite spawns — not needed to use prova (see placement.md)",
        run: broker_subcommand,
    },
    Verb {
        name: "watch",
        help: "  prova watch <topology>    stand up a topology and re-apply on definition change (dev loop)",
        run: watch_subcommand,
    },
    Verb {
        name: "start",
        help: "  prova start <topology>    stand up a topology detached (returns; use `down` to stop)",
        run: start_subcommand,
    },
    Verb {
        name: "down",
        help: "  prova down <topology>     tear down a detached topology",
        run: down_subcommand,
    },
    Verb {
        name: "ps",
        help: "  prova ps                  list running topologies",
        run: ps_subcommand,
    },
    Verb {
        name: "package",
        help: "  prova package lint <f>... check package files against the namespacing grammar",
        run: package_subcommand,
    },
    Verb {
        name: "packages",
        help: "  prova packages [<query>]  search the configured package registries (no query lists all;\n\
               \x20                           `info <name>` details; `add <name>[@ref]` pins into [dependencies])",
        run: registry::run,
    },
];

/// Deprecated verb spellings — one release's bridge to the `package` vocabulary, retiring at 1.0
/// with the other pre-1.0 spellings. Dispatch still lands on the canonical verb; the warning is
/// what teaches the rename.
const DEPRECATED_VERBS: &[(&str, &str)] = &[("plugin", "package"), ("plugins", "packages")];

/// Retired verb spellings — the state-verb surface the lane grammar replaced (query consolidation).
/// Unlike a deprecation, these do NOT dispatch: a state is a `--flag` on its lane now, not its own
/// verb. The tombstone refuses and names the new spelling — kinder to muscle memory (agents most of
/// all) than the run path's "no such file", and honest that the old command is gone.
const RETIRED_VERBS: &[(&str, &str)] = &[
    ("promises", "prova tests --promises"),
    ("burndown", "prova tests burndown"),
    ("falsify", "prova tests falsify"),
    // `backlog` was the specs lane's cold-state view; it is now `prova specs --backlog` (and its
    // one write moved to `prova specs promote <id>`).
    ("backlog", "prova specs --backlog"),
    // `list` was the tests-lane node listing; `prova tests` is its lane-named, state-tagged
    // successor (and the MCP `list` tool renamed to `tests` in step).
    ("list", "prova tests"),
];

/// MCP tool names whose CLI spelling differs — the parity contract's teaching half
/// (docs/design/mcp-mode.md#cli-mcp-verb-parity): every tool name typed at the CLI dispatches or
/// teaches, never falls through to the run path's "no such file". Unlike RETIRED_VERBS these were
/// never CLI verbs; the message names the CLI twin. `mcp_tools_are_real_verbs` holds this table
/// and the router's KNOWN_MCP_ONLY set equal, so a new divergence cannot ship untaught.
const MCP_SPELLINGS: &[(&str, &str)] = &[
    // The verified write is lane-scoped at the CLI (drivers are `prova <lane> <driver>`; MCP
    // tools are flat).
    ("capture", "the MCP `capture` tool's CLI spelling is `prova specs capture <id> \"<prose>\" --file <doc>`"),
    // `status` is the MCP server's held-topology view (warmth). The CLI's view of detached
    // topologies is `ps`; increment 7 (docs/plans/query-consolidation.md) unifies the vocabulary.
    ("status", "the MCP `status` tool lists topologies the server holds; detached topologies list via `prova ps`"),
];

/// `prova --help`, assembled from the verb table so the two cannot disagree.
fn help_text() -> String {
    let verbs: Vec<&str> = VERBS.iter().map(|v| v.help).collect();
    format!(
        "usage:\n\
         \x20 prova <file-or-dir>...    run the given files/dirs\n\
         \x20 prova                     run the suite declared in prova.toml (found by walking up)\n\
         {}\n\n{OPTIONS}",
        verbs.join("\n")
    )
}

const OPTIONS: &str = "\
options:
  -p, --profile NAME        run a profile from the manifest
      --manifest PATH       use a specific manifest (default ./prova.toml)
      --format console|json|tap  output format (--json is shorthand)
      --color auto|always|never  color console output (default auto: TTY only; honors NO_COLOR)
  -q, --quiet               only print failures, the recap, and the summary
      --heed[=SEL,...]      fail the run on DUE reminders — bare heeds all; =SEL heeds only those
                            matching a reminder name/tag. Ad-hoc form of a profile's `heed`
      --junit PATH          also write a JUnit XML report to PATH (for CI; composes with --format)
      --gha auto|on|off     GitHub Actions annotations + step summary (default auto: when in GHA)
  -j, --jobs N              run up to N units concurrently — tests contending on one tool
                            (cargo, a port) declare locks instead of dialing this to 1: the
                            scheduler serializes only the holders (`prova learn locks`)
  -P, --package name=source add an ad-hoc package (repeatable; layers over the manifest)
  -k PATTERN                select nodes whose path contains PATTERN (repeatable; !PAT excludes)
      --tags a,b            select nodes tagged with any listed tag (repeatable; !tag excludes)
      --node PATH           select an exact node path (repeatable) — re-run what a report named
                            (naming a switched test implies its switch)
  -s, --switch a,b          throw opt-in switches: run tests marked `switch = ...`, which are
                            otherwise held back (repeatable; unions with [run]/profile `switches`)
      --last-failed         select only the nodes that failed in the previous run
      --topology NAME       require attaching to the held topology NAME (error when not running) —
                            judge the LIVE environment, never a silently fresh one
      --fresh               ignore held topologies: always provision fresh (the CI behavior)
      --promises            select only promised tests — the open surface (composes with --list)
      --proofs              select only settled proofs — the mirror of --promises (composes; the
                            two are mutually exclusive)
      --due                 promises fall due: open promises report as real failures (burndown's mode;
                            alone, the whole suite tolerates no open promise)
      --allow-empty         a selection matching no tests is OK (default: that is an error)
  -u, --update-snapshots    (re)write snapshots instead of comparing (matches_snapshot)
      --update-baseline     move ratchet baselines toward this run's measurements (tightens only;
                            refuses to loosen) — .prova/baselines/ (measure.ratchet)
      --unreferenced M      snapshots no test used: ignore (default) | warn | delete (full runs only)
  -U, --update              force-refresh git plugin sources (skip the freshness cache)
      --offline             never fetch git plugin sources; use only what is already cached
      --list                discover tests without running them (respects selection)
      --record PATH         also write the run record here (always written to .prova/var/) — what
                            executed and, named individually, what did NOT: see `prova attest`
  -V, --version             print version
  -h, --help                print this help";

/// The running prova version, checked against each plugin's `requires.prova` compatibility range.
const PROVA_VERSION: &str = prova_core::VERSION;

#[derive(Clone, Copy)]
enum Format {
    Console,
    Json,
    Tap,
}

/// Match `--name value` / `--name=value` (and any aliases); returns the value if `arg` is one.
fn value_flag(
    arg: &str,
    args: &mut impl Iterator<Item = String>,
    names: &[&str],
) -> Option<String> {
    for name in names {
        if let Some(v) = arg.strip_prefix(&format!("{name}=")) {
            return Some(v.to_string());
        }
        if arg == *name {
            return Some(args.next().unwrap_or_default());
        }
    }
    None
}

/// Read and parse the package manifest at `home.manifest` — the shared front half of every verb
/// that starts from the manifest. Prints the error and yields exit 2 on failure.
fn read_manifest(home: &home::Home) -> Result<Manifest, ExitCode> {
    std::fs::read_to_string(&home.manifest)
        .map_err(|e| e.to_string())
        .and_then(|text| Manifest::parse(&text))
        .map_err(|e| {
            eprintln!("prova: {e}");
            ExitCode::from(2)
        })
}

fn main() -> ExitCode {
    // The state-root escape hatch is validated for EVERY invocation, before any dispatch — a
    // misconfigured `PROVA_VAR_DIR` must fail identically whether or not this particular command
    // would have recorded anything (see `var`). An active override then announces itself on stderr,
    // so relocated state can never be an invisible machine difference.
    if let Err(diagnostic) = var::check_env() {
        eprintln!("prova: {diagnostic}");
        return ExitCode::from(2);
    }
    var::announce();

    // Nothing re-execs (docs/design/manifest.md#runner-is-the-subject-not-the-conductor): the
    // binary you invoke conducts. `[runner]` names the binary UNDER TEST, provisioned just in
    // time by the run path and injected as `prova.bin` — see `cmd_run::provision_subject`.

    // Subcommands dispatch through the verb table; everything else is the run path.
    let mut raw = std::env::args().skip(1).peekable();
    if let Some(first) = raw.peek() {
        if let Some(verb) = VERBS.iter().find(|v| v.name == *first) {
            raw.next();
            return (verb.run)(raw.collect());
        }
        if let Some((old, new)) = DEPRECATED_VERBS.iter().find(|(old, _)| old == first) {
            eprintln!("prova: `prova {old}` is deprecated — use `prova {new}` (retires at 1.0)");
            let Some(verb) = VERBS.iter().find(|v| v.name == *new) else {
                eprintln!("prova: internal: deprecated verb `{old}` maps to unknown `{new}`");
                return ExitCode::from(2);
            };
            raw.next();
            return (verb.run)(raw.collect());
        }
        if let Some((old, replacement)) = RETIRED_VERBS.iter().find(|(old, _)| old == first) {
            eprintln!("prova: `prova {old}` was retired — use `{replacement}` (a state is a flag on its lane now)");
            return ExitCode::from(2);
        }
        if let Some((name, teach)) = MCP_SPELLINGS.iter().find(|(name, _)| name == first) {
            eprintln!("prova: `{name}` is not a CLI verb — {teach}");
            return ExitCode::from(2);
        }
    }
    run(std::env::args().skip(1).collect())
}

mod cmd_meta;
use cmd_meta::*;

mod cmd_specs;
use cmd_specs::*;

mod cmd_run;
use cmd_run::*;

mod cmd_attest;
use cmd_attest::*;

mod cmd_eval;
use cmd_eval::*;

mod cmd_topo;
use cmd_topo::*;

mod dispatch;
use dispatch::*;

mod suites;
use suites::*;

#[cfg(test)]
#[allow(clippy::items_after_test_module)] // the helpers below are shared with mcp.rs, not test-only
mod tests {
    use super::*;

    #[test]
    fn missing_stub_warning_fires_only_without_a_stub() {
        let base = std::env::temp_dir().join(format!("prova-lint-stub-{}", std::process::id()));
        let dir = base.join("plugin");
        std::fs::create_dir_all(&dir).unwrap();
        let ns = Some(("postgres".to_string(), dir.clone()));

        // No library/ → warns.
        let w = missing_stub_warning(&ns).expect("should warn without a stub");
        assert!(w.contains("library/postgres.lua"), "{w}");

        // With library/postgres.lua → silent.
        std::fs::create_dir_all(dir.join("library")).unwrap();
        std::fs::write(
            dir.join("library").join("postgres.lua"),
            "---@meta postgres\n",
        )
        .unwrap();
        assert!(missing_stub_warning(&ns).is_none());

        // No namespace at all (headless file with no parent info) → nothing to advise.
        assert!(missing_stub_warning(&None).is_none());

        std::fs::remove_dir_all(&base).ok();
    }

    /// Collect every verb a document tells the agent to RUN — the word after "prova " in
    /// backticked/fenced command position — skipping non-verb shapes: flags (`prova --list`),
    /// placeholders (`prova <verb>`), file arguments, and `prova.toml`-style dotted names.
    /// Plain-prose "prova" (the product name) is not a command and is not linted.
    fn verbs_uttered(doc: &str) -> Vec<String> {
        let mut chunks: Vec<&str> = doc.split("`prova ").skip(1).collect();
        // Fenced examples put commands at line start with no inline backticks.
        chunks.extend(
            doc.lines()
                .map(str::trim_start)
                .filter_map(|l| l.strip_prefix("prova ")),
        );
        let mut out = Vec::new();
        for chunk in chunks {
            let word: String = chunk
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                .collect();
            if !word.is_empty()
                && word.chars().next().is_some_and(|c| c.is_ascii_lowercase())
                && !chunk.starts_with(&format!("{word}."))
            {
                out.push(word);
            }
        }
        out
    }

    /// The reference lint (docs/plans/autodidact.md §2.6.7): every verb the skill and the topics
    /// tell an agent to run must exist in the verb table — the docs cannot advertise a command
    /// the binary would reject.
    #[test]
    fn skill_and_topics_only_name_real_verbs() {
        let known: std::collections::BTreeSet<&str> =
            VERBS.iter().map(|v| v.name).collect();
        // Words that follow `prova ` without being subcommand verbs: run-path/general usage.
        let non_verbs = ["run", "package", "environment", "release"];
        let docs: Vec<(&str, &str)> = std::iter::once(("skill.md", SKILL))
            .chain(
                learn::Topic::ALL
                    .iter()
                    .map(|t| (t.key(), t.rendered_source_for_lint())),
            )
            .collect();
        for (name, doc) in docs {
            for verb in verbs_uttered(doc) {
                assert!(
                    known.contains(verb.as_str()) || non_verbs.contains(&verb.as_str()),
                    "{name} tells the agent to run `prova {verb}`, which is not a verb the \
                     binary dispatches — fix the doc or add the verb to VERBS"
                );
            }
        }
    }

    /// Predictability as a gate: every verb teaches itself — `prova learn <verb>` must resolve and
    /// never dead-end. The mirror of `skill_and_topics_only_name_real_verbs` (that one: every verb a
    /// doc names is real; this one: every verb a user can type is documented).
    ///
    /// A UNIT test on purpose, not a black-box `.prova.lua` proof: the invariant is a correspondence
    /// between two in-process source tables (`VERBS` ↔ `Topic::resolve`). Recovering the verb list
    /// black-box would mean parsing `--help` — fragile and indirect for no gain. Adding a verb with
    /// no learn home fails HERE, so the invariant cannot silently rot (docs/plans/terminology.md).
    #[test]
    fn every_verb_resolves_in_learn() {
        for verb in VERBS {
            // `learn` is the catalog verb itself: `prova learn` (no topic) IS its documentation, so
            // it needs no topic of its own.
            if verb.name == "learn" {
                continue;
            }
            assert!(
                learn::Topic::resolve(verb.name).is_some(),
                "`prova {name}` has no learn topic — `prova learn {name}` would dead-end. Give it a \
                 topic key, or a command-keyword alias in learn.rs (docs/plans/terminology.md).",
                name = verb.name,
            );
        }
    }

    /// Every verb's help text names the verb it dispatches — the row documents itself.
    #[test]
    fn every_verb_documents_itself() {
        for verb in VERBS {
            assert!(
                verb.help.contains(&format!("prova {}", verb.name)),
                "verb `{}` has help text that never mentions it",
                verb.name
            );
        }
        // And the assembled help is the verbs, in order.
        let help = help_text();
        for verb in VERBS {
            assert!(help.contains(verb.help), "help_text() dropped `{}`", verb.name);
        }
    }

    /// Lane–surface parity (docs/plans/query-consolidation.md, increment 1). The three lanes
    /// (`prova_core::LANES`) are prova's top-level vocabulary; each must be reachable the same way on
    /// every surface it fronts — a `prova <lane>` verb and a `prova learn <lane>` topic. This is the
    /// sibling of `every_verb_resolves_in_learn`: a correspondence between in-process source tables
    /// (`LANES` ↔ `VERBS` ↔ `Topic::resolve`), so a unit test reads it directly rather than parsing
    /// `--help`.
    ///
    /// Three legs now: a `prova <lane>` verb, a `prova learn <lane>` topic, AND a same-named MCP
    /// tool — the correspondence is `LANES` ↔ `VERBS` ↔ `Topic::resolve` ↔ the MCP router. The MCP
    /// leg landed in increment 8 once tool-per-lane was settled (the alternative, one
    /// lane-parameterized `query` tool, was rejected — matching verb names is what lets an agent and
    /// a user share one lane vocabulary). A lane that loses any leg fails here.
    ///
    /// Legs the plan wires in a later increment live in `KNOWN_GAPS`. A row there is a promise, not a
    /// mute: the minimality check below FAILS once the leg is wired, so closing a gap forces deleting
    /// its row — graduation, exactly as an honored promise fails until its flag is removed.
    #[test]
    fn lane_surface_parity() {
        // All three legs are wired, so KNOWN_GAPS is empty. A row here is a promise, not a mute: the
        // minimality check below FAILS once a listed leg is wired, forcing its deletion (graduation).
        const KNOWN_GAPS: &[(&str, &str)] = &[];
        let has_verb = |lane: &str| VERBS.iter().any(|v| v.name == lane);
        let has_topic = |lane: &str| learn::Topic::resolve(lane).is_some();
        // The live MCP tool surface, read from the router (never a hand-kept list). Every lane must
        // be reachable by a same-named tool, so an agent and a user share one lane vocabulary.
        let tools: std::collections::BTreeSet<String> = mcp::ProvaMcpServer::tool_router()
            .list_all()
            .into_iter()
            .map(|t| t.name.into_owned())
            .collect();
        let has_tool = |lane: &str| tools.contains(lane);

        for lane in prova_core::LANES {
            if !KNOWN_GAPS.contains(&(lane.key, "verb")) {
                assert!(
                    has_verb(lane.key),
                    "lane `{key}` has no `prova {key}` verb — add it to VERBS, or list \
                     (\"{key}\", \"verb\") in KNOWN_GAPS with the increment that will \
                     (docs/plans/query-consolidation.md).",
                    key = lane.key,
                );
            }
            // Topic leg: no known gaps — every lane must teach itself today, like every verb does.
            assert!(
                has_topic(lane.key),
                "lane `{key}` has no `prova learn {key}` topic — `prova learn {key}` would dead-end.",
                key = lane.key,
            );
            if !KNOWN_GAPS.contains(&(lane.key, "mcp")) {
                assert!(
                    has_tool(lane.key),
                    "lane `{key}` has no MCP tool named `{key}` — an agent cannot reach the {key} \
                     lane, breaking CLI↔MCP parity (docs/plans/query-consolidation.md).",
                    key = lane.key,
                );
            }
        }

        // Minimality / graduation: every listed gap must still be an actual gap.
        for (lane, surface) in KNOWN_GAPS {
            let still_gapped = match *surface {
                "verb" => !has_verb(lane),
                "topic" => !has_topic(lane),
                "mcp" => !has_tool(lane),
                other => panic!("KNOWN_GAPS names an unknown surface {other:?}"),
            };
            assert!(
                still_gapped,
                "lane `{lane}` surface `{surface}` is now wired — delete its KNOWN_GAPS row (the gap \
                 is closed; graduate it).",
            );
        }
    }

    /// CLI↔MCP alignment: every tool the MCP server exposes maps to a real CLI verb of the same name
    /// — the two surfaces name the same capabilities identically. The mirror of
    /// `skill_and_topics_only_name_real_verbs` (the docs an agent reads) for the tools an agent
    /// calls. Reads the LIVE router (`mcp::tool_names`), so a tool added or renamed in a `#[tool]`
    /// attribute is caught without touching this test.
    ///
    /// Deliberately MCP-only tools live in `KNOWN_MCP_ONLY` with their reason; the minimality check
    /// fails if the router stops exposing one, so the list cannot rot. (docs/plans/query-consolidation.md)
    #[test]
    fn mcp_tools_are_real_verbs() {
        // MCP tools with no same-named TOP-LEVEL CLI verb, and why — every one of these MUST also
        // sit in MCP_SPELLINGS so the name teaches at the CLI instead of falling through to the
        // run path's "no such file" (docs/design/mcp-mode.md#cli-mcp-verb-parity).
        // `capture`'s CLI spelling is the lane driver `prova specs capture` (one verified write,
        // two spellings; drivers are lane-scoped, tools are flat). `status` is the held-topology
        // view whose CLI twin is `ps`; increment 7 unifies the topology lifecycle vocabulary.
        // (`introspect` graduated: it now has a `prova introspect` CLI verb — increment 8.)
        const KNOWN_MCP_ONLY: &[&str] = &["status", "capture"];
        let known: std::collections::BTreeSet<&str> = VERBS.iter().map(|v| v.name).collect();

        // Read the LIVE router (never a hand-kept list) — a tool added or renamed in a `#[tool]`
        // attribute is caught without touching this test. Building the router is side-effect-free.
        let tools: Vec<String> = mcp::ProvaMcpServer::tool_router()
            .list_all()
            .into_iter()
            .map(|t| t.name.into_owned())
            .collect();
        assert!(!tools.is_empty(), "the MCP router exposes no tools — the wiring is broken");

        for tool in &tools {
            assert!(
                known.contains(tool.as_str()) || KNOWN_MCP_ONLY.contains(&tool.as_str()),
                "MCP exposes a `{tool}` tool with no `prova {tool}` CLI verb — add the verb, rename \
                 the tool to match, or list it in KNOWN_MCP_ONLY with a reason \
                 (docs/plans/query-consolidation.md).",
            );
        }

        // Minimality: don't let KNOWN_MCP_ONLY carry a name the router no longer exposes.
        for name in KNOWN_MCP_ONLY {
            assert!(
                tools.iter().any(|t| t == name),
                "KNOWN_MCP_ONLY lists `{name}`, which the MCP router no longer exposes — delete the \
                 row.",
            );
        }

        // The teaching half of the parity contract: the allowlist and the redirect table are the
        // same set. A divergent tool name that does not teach is exactly the first-try miss the
        // field report described; a teaching row for a name that dispatches (or is gone) is rot.
        let taught: std::collections::BTreeSet<&str> =
            MCP_SPELLINGS.iter().map(|(name, _)| *name).collect();
        let allowed: std::collections::BTreeSet<&str> = KNOWN_MCP_ONLY.iter().copied().collect();
        assert_eq!(
            taught, allowed,
            "MCP_SPELLINGS (teaches at the CLI) and KNOWN_MCP_ONLY (allowed to diverge) must name \
             the same tools — a divergence must teach, and only divergences may.",
        );
    }

    /// `prova capabilities` enumerates from `builtin_capability_names()`; that list must not drift
    /// from what the engine actually treats as built-in — every name it advertises must satisfy
    /// `is_builtin_capability`, or the report would claim a probe prova cannot perform.
    #[test]
    fn builtin_capability_names_are_all_builtin() {
        for name in prova_core::builtin_capability_names() {
            assert!(
                prova_core::is_builtin_capability(name),
                "`{name}` is in builtin_capability_names() but is_builtin_capability disagrees",
            );
        }
    }
}

/// Forwards every event and records the paths of failed nodes, so `--last-failed` can select
/// exactly them next run.
/// Watches the event stream and remembers what became of every leaf — for `--last-failed` (the
/// failures alone) and for the run record (all of it, including the skips and their reasons).
///
/// A reporter is the right seam: the executor already emits exactly one `NodeFinished` per leaf
/// that ran, so recording is an observation of the ordinary path rather than a second pass over it.
/// What never reaches this stream is precisely what never ran — the deselected — and those come
/// from the summary instead.
struct FailureRecorder {
    inner: Box<dyn Reporter>,
    failed: Vec<String>,
    executed: std::collections::BTreeMap<String, record::Executed>,
    skipped: Vec<record::Skipped>,
}

impl Reporter for FailureRecorder {
    fn event(&mut self, event: &prova_core::Event) {
        if let prova_core::Event::NodeFinished {
            path,
            outcome,
            message,
            file,
            ..
        } = event
        {
            // `--last-failed` re-selects with `--node`, which matches the raw path; the record is
            // keyed on the file-qualified one, so two files' same-named tests stay distinct.
            let key = record::qualified(path, *file);
            match outcome {
                prova_core::Outcome::Failed => {
                    self.failed.push(path.to_string());
                    self.executed.insert(key, record::Executed::Failed);
                }
                prova_core::Outcome::Passed => {
                    self.executed.insert(key, record::Executed::Passed);
                }
                prova_core::Outcome::Promised => {
                    self.executed.insert(key, record::Executed::Promised);
                }
                // Verbatim: a reason paraphrased by the recorder is a reason nobody can act on.
                prova_core::Outcome::Skipped => self.skipped.push(record::Skipped {
                    path: key,
                    reason: message.unwrap_or("skipped").to_string(),
                }),
            }
        }
        self.inner.event(event);
    }
}

/// Where `--last-failed` state lives: a small JSON list of node paths in the package's state
/// directory (`var`). Runs without a manifest home have nowhere durable to record, so the feature
/// quietly no-ops there.
///
/// READ path — resolves without creating anything, so `--last-failed` on a package that has never
/// recorded a failure leaves no directory behind.
fn last_failed_file(home: &Option<home::Home>) -> Option<std::path::PathBuf> {
    home.as_ref().map(|h| var::path(h).join(LAST_FAILED))
}

/// The record's basename. No leading dot: it already sits in a hidden, self-ignoring directory, and
/// a second layer of hiding only makes it harder to inspect while debugging.
const LAST_FAILED: &str = "last-failed.json";

fn load_last_failed(home: &Option<home::Home>) -> Option<Vec<String>> {
    let path = last_failed_file(home)?;
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn store_last_failed(home: &Option<home::Home>, failed: &[String]) {
    let Some(home) = home.as_ref() else {
        return;
    };
    if failed.is_empty() {
        // Nothing failed: drop the record but leave the directory. Recreating it on every red run
        // would churn the `.gitignore` write for no gain.
        if let Some(path) = last_failed_file(&Some(home.clone())) {
            let _ = std::fs::remove_file(path);
        }
        return;
    }
    // WRITE path — this is the call that materializes `var/` and its self-ignoring `.gitignore`.
    let Ok(dir) = var::dir(home) else {
        return;
    };
    if let Ok(text) = serde_json::to_string_pretty(failed) {
        let _ = std::fs::write(dir.join(LAST_FAILED), text);
    }
}

/// The embedded agent skill — versioned with the binary so it can never drift from the features.
const SKILL: &str = include_str!("skill.md");

/// `prova skill` prints the skill; `prova skill --install` writes it into the package's
/// `.claude/skills/prova/SKILL.md` (next to the manifest's package root) so the repo carries it.
fn skill_subcommand(args: Vec<String>) -> ExitCode {
    let install = args.iter().any(|a| a == "--install");
    if let Some(bad) = args.iter().find(|a| *a != "--install") {
        eprintln!("prova: skill: unknown argument {bad:?} (expected --install or nothing)");
        return ExitCode::from(2);
    }
    if !install {
        print!("{SKILL}");
        return ExitCode::SUCCESS;
    }
    let root = match home::find(&std::env::current_dir().unwrap_or_default()) {
        Ok(Some(h)) => h.dir,
        _ => std::env::current_dir().unwrap_or_default(),
    };
    let dir = root.join(".claude/skills/prova");
    let path = dir.join("SKILL.md");
    if let Err(err) = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, SKILL)) {
        eprintln!("prova: skill: could not write {}: {err}", path.display());
        return ExitCode::from(2);
    }
    println!("wrote {}", path.display());
    ExitCode::SUCCESS
}
