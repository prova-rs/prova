//! `prova init` — scaffold a prova package by **rendering a catalog archetype** into the current
//! directory, then wiring LuaLS IDE support as a finishing step.
//!
//! ```text
//! prova init                 # interactive select among catalog entries (default pre-highlighted)
//! prova init --list          # the catalog: which archetypes prova can scaffold from
//! prova init <key>           # render the named catalog entry
//! prova init <key> --answer name=value --switch ci   # feed the render (repeatable)
//! prova init <key> --defaults        # take each prompt's default instead of asking
//! prova init <key> --headless        # never prompt; an unanswerable prompt is an error, not a hang
//! prova init <key> --no-ide          # skip the IDE-wiring finishing step (alias: --no-luals)
//! ```
//!
//! The scaffold is selected from a [catalog](crate::catalog) — prova's built-in entries plus any
//! `[init.*]` in `~/.config/prova/config.toml`. The catalog and the target key are resolved *before*
//! anything touches the filesystem, so a typo'd key or a broken config never leaves a half-scaffolded
//! package behind. `init` refuses to run if the package already has a manifest — it never clobbers an
//! existing layout — unless the entry declares `in_package = "allow"` (it augments a package rather
//! than creating one). Every render also receives generic package-state (an in-package switch, the
//! package root, `plugin_root`) so any archetype can adapt to where it is running; see the catalog
//! module docs.
//!
//! Answer precedence (highest first): CLI `--answer` → the entry's baked `answers` → an interactive
//! prompt (unless `--headless`) → the archetype's own default (via `--defaults`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// The catalog key the interactive picker pre-highlights.
const DEFAULT_KEY: &str = "project";

/// Package state discovered before a render — what `init` knows about WHERE it is running, injected
/// generically into every archetype (see the catalog module docs on state injection).
struct PackageState {
    /// The package root, relative to the cwd (`.` when they coincide).
    package_root: String,
    /// The manifest's `[run] plugin_root`, verbatim (package-root relative), when declared.
    packages_dir: Option<String>,
}

/// Discover the enclosing package, if any, walking up from the cwd exactly like `prova` itself.
/// A manifest that exists but fails to parse still counts as "in a package" (the switch and root are
/// facts); only its `plugin_root` is unknowable — warn and carry on rather than fail the init.
fn package_state() -> Option<PackageState> {
    let cwd = std::env::current_dir().ok()?;
    let home = crate::home::find(&cwd).ok().flatten()?;
    // `home.dir` is canonicalized by discovery (on Windows that's the `\\?\` verbatim form, on macOS
    // symlinked temp dirs resolve) — canonicalize the cwd the same way or strip_prefix can't match.
    let cwd = cwd.canonicalize().unwrap_or(cwd);
    let package_root = match cwd.strip_prefix(&home.dir) {
        Ok(rel) => {
            let depth = rel.components().count();
            if depth == 0 {
                ".".to_string()
            } else {
                vec![".."; depth].join("/")
            }
        }
        Err(_) => home.dir.display().to_string(), // unrelated roots (symlinks) — absolute is still true
    };
    let packages_dir = match std::fs::read_to_string(&home.manifest)
        .map_err(|e| e.to_string())
        .and_then(|text| crate::manifest::Manifest::parse(&text))
        .and_then(|m| m.resolve(None))
    {
        Ok(resolved) => resolved.packages_dir,
        Err(err) => {
            eprintln!(
                "prova init: note — could not read {}: {err} (rendering without `prova_plugin_root`)",
                home.manifest.display()
            );
            None
        }
    };
    Some(PackageState {
        package_root,
        packages_dir,
    })
}

/// Everything `prova init`'s flag loop can set.
struct InitCli {
    luals: bool,
    list: bool,
    headless: bool,
    defaults: bool,
    // Freshness knob, matching `prova` (run) and `archetect`.
    //
    // `-U`/`--update` is deliberately NOT here yet. It needs `Configuration::with_force_update`,
    // which archetect only grows in the release after the pinned v3.4.1 — shipping the flag now
    // would make it a silent no-op, which is worse than its absence for a knob whose whole job is
    // "I do not trust the cache".
    offline: bool,
    key: Option<String>,
    answers: Vec<(String, String)>,
    switches: Vec<String>,
}

/// Parse `prova init`'s arguments; `--help` prints usage and exits successfully.
fn parse_args(args: Vec<String>) -> Result<InitCli, ExitCode> {
    let mut cli = InitCli {
        luals: true,
        list: false,
        headless: false,
        defaults: false,
        offline: false,
        key: None,
        answers: Vec::new(),
        switches: Vec::new(),
    };
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--no-luals" | "--no-ide" => cli.luals = false,
            "--list" => cli.list = true,
            "--headless" => cli.headless = true,
            "--defaults" => cli.defaults = true,
            "--offline" => cli.offline = true,
            "--answer" | "-a" => {
                let Some(pair) = it.next() else {
                    eprintln!("prova init: --answer expects key=value");
                    return Err(ExitCode::from(2));
                };
                match pair.split_once('=') {
                    Some((k, v)) => cli.answers.push((k.to_string(), v.to_string())),
                    None => {
                        eprintln!("prova init: --answer expects key=value, got {pair:?}");
                        return Err(ExitCode::from(2));
                    }
                }
            }
            "--switch" | "-s" => {
                let Some(name) = it.next() else {
                    eprintln!("prova init: --switch expects a name");
                    return Err(ExitCode::from(2));
                };
                cli.switches.push(name);
            }
            "-h" | "--help" => {
                println!(
                    "usage: prova init [<key>] [--list] [--answer k=v]... [--switch name]... \
                     [--defaults] [--headless] [--no-ide] [--offline]\n\
                     \n\
                     render a catalog archetype into the current directory, then wire LuaLS IDE\n\
                     support. <key> names a catalog entry (see `prova init --list`); omit it to\n\
                     choose interactively. --headless never prompts (an unanswered, undefaulted\n\
                     prompt is an error); --defaults takes each prompt's default; --no-ide skips\n\
                     IDE wiring.\n\
                     \n\
                     --offline uses only what is already cached, never the network.\n\
                     \n\
                     NOTE: a moved tag (the floating-major `v1` convention) stays cached until\n\
                     archetect's update interval lapses — a day by default — so a freshly\n\
                     published archetype can render stale. Until `prova init -U` lands, lower\n\
                     `updates.interval` in archetect's config, or run `archetect -U` once."
                );
                return Err(ExitCode::SUCCESS);
            }
            other if other.starts_with('-') => {
                eprintln!("prova init: unknown option {other:?}");
                return Err(ExitCode::from(2));
            }
            other => {
                if let Some(prior) = &cli.key {
                    eprintln!("prova init: expected one catalog key, got {prior:?} and {other:?}");
                    return Err(ExitCode::from(2));
                }
                cli.key = Some(other.to_string());
            }
        }
    }
    Ok(cli)
}

/// Answers: baked entry answers over the injected package state, CLI `--answer` over both.
/// Switches: state ∪ entry ∪ CLI.
fn merged_inputs(
    state: &Option<PackageState>,
    entry: &crate::catalog::Resolved,
    cli: &InitCli,
) -> (BTreeMap<String, String>, Vec<String>) {
    // Package-state injection: tell the archetype where it is running. Lowest precedence — the
    // entry's baked answers/switches and the CLI both win, so an entry can override the facts.
    let mut merged: BTreeMap<String, String> = BTreeMap::new();
    if let Some(state) = state {
        merged.insert("prova_package_root".to_string(), state.package_root.clone());
        if let Some(dir) = &state.packages_dir {
            merged.insert("prova_packages_dir".to_string(), dir.clone());
            // The deprecated answer name — archetype pins from before the package vocabulary
            // still read it; both are served until it retires at 1.0.
            merged.insert("prova_plugin_root".to_string(), dir.clone());
        }
    }
    merged.extend(entry.answers.clone());
    for (k, v) in &cli.answers {
        merged.insert(k.clone(), v.clone());
    }
    let mut switches = Vec::new();
    if state.is_some() {
        switches.push("prova:in-package".to_string());
    }
    for s in entry.switches.iter().chain(cli.switches.iter()) {
        if !switches.contains(s) {
            switches.push(s.clone());
        }
    }
    (merged, switches)
}

/// Resolve the render target before any filesystem work — a bad key or a malformed config.toml
/// fails before a half-scaffolded package can exist. No key means "choose interactively": prova
/// always presents the catalog (which always contains at least `default`) rather than silently
/// picking. Registry tolerance messages print rather than sink a render the remaining registries
/// can serve. `Err(ExitCode::SUCCESS)` is the `--list` early exit.
fn resolve_entry(
    cli: &InitCli,
    sys_layout: &prova_core::XdgSystemLayout,
) -> Result<(String, crate::catalog::Resolved), ExitCode> {
    let catalog = match crate::catalog::Catalog::load(sys_layout) {
        Ok(c) => c,
        Err(err) => {
            eprintln!("prova init: {err}");
            return Err(ExitCode::from(2));
        }
    };
    if cli.list {
        catalog.print_list();
        return Err(ExitCode::SUCCESS);
    }
    let key = match cli.key.clone() {
        Some(k) => k,
        None => select_key(&catalog)?,
    };
    let mut warnings = Vec::new();
    let entry = match crate::catalog::resolve(&catalog, &key, sys_layout, &mut warnings) {
        Ok(e) => e,
        Err(err) => {
            for w in &warnings {
                eprintln!("prova init: {w}");
            }
            eprintln!("prova init: {err}");
            return Err(ExitCode::from(2));
        }
    };
    for w in &warnings {
        eprintln!("prova init: {w}");
    }
    // Refuse to clobber — unless the entry declares it AUGMENTS an initialized package
    // (`in_package = "allow"`), in which case the archetype decides what to write.
    if entry.in_package == crate::catalog::InPackage::Deny {
        for existing in [
            "prova.toml",
            ".prova.toml",
            "prova/prova.toml",
            ".prova/prova.toml",
        ] {
            if Path::new(".").join(existing).is_file() {
                eprintln!("prova init: already initialized ({existing} exists)");
                return Err(ExitCode::from(2));
            }
        }
    }
    Ok((key, entry))
}

pub fn run(args: Vec<String>) -> ExitCode {
    let cli = match parse_args(args) {
        Ok(cli) => cli,
        Err(code) => return code,
    };
    let InitCli { luals, headless, defaults, offline, .. } = &cli;
    let (luals, headless, defaults, offline) = (*luals, *headless, *defaults, *offline);

    let sys_layout = match prova_core::XdgSystemLayout::new() {
        Ok(l) => l,
        Err(err) => {
            eprintln!("prova init: cannot locate config directory: {err}");
            return ExitCode::from(2);
        }
    };
    let (key, entry) = match resolve_entry(&cli, &sys_layout) {
        Ok(resolved) => resolved,
        Err(code) => return code,
    };
    let root = Path::new(".");

    // Defaults: either the entry opts in or `--defaults` is passed.
    let state = package_state();
    let (merged, switches) = merged_inputs(&state, &entry, &cli);
    let use_defaults = entry.defaults || defaults;

    let destination = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let answers = prova_archetect::string_answers(merged);
    println!(
        "prova: rendering {key:?} from {} ({})",
        entry.source, entry.origin
    );
    if !entry.description.is_empty() {
        println!("prova: {}", entry.description);
    }
    if let Err(err) = prova_archetect::render_interactive(
        &entry.source,
        &destination,
        answers,
        switches,
        use_defaults,
        headless,
        offline,
    ) {
        eprintln!("prova init: render failed: {err}");
        return ExitCode::from(2);
    }

    // IDE wiring, as a finishing step, over whatever manifest the archetype rendered. A render that
    // produced no prova.toml isn't a prova package layout — say so rather than fail.
    if luals {
        match crate::home::find(root) {
            Ok(Some(home)) => {
                if let Err(err) =
                    crate::ide::wire(&home, crate::manifest::Manage::Always, &sys_layout)
                {
                    eprintln!("prova init: IDE annotations: {err}");
                    return ExitCode::from(2);
                }
            }
            Ok(None) => {
                println!(
                    "prova: no prova.toml was rendered — skipping IDE wiring (run `prova ide setup` later)"
                );
            }
            Err(err) => {
                eprintln!("prova init: {err}");
                return ExitCode::from(2);
            }
        }
    }

    if state.is_some() && entry.in_package == crate::catalog::InPackage::Allow {
        println!("\nprova: rendered {key:?} into the existing package. Run `prova` to execute the suite.");
    } else {
        println!("\nprova: initialized. Run `prova` to execute the suite.");
    }
    ExitCode::SUCCESS
}

/// Present the catalog interactively and return the chosen key. A keyless `prova init` always offers
/// the catalog (which always contains at least `default`) rather than choosing silently — but that
/// needs a terminal to prompt on. In a non-interactive context (CI, a pipe) it's a usage error that
/// names the alternatives, never a prompt that hangs.
fn select_key(catalog: &crate::catalog::Catalog) -> Result<String, ExitCode> {
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() {
        eprintln!(
            "prova init: no archetype given, and stdin is not a terminal to choose from — pass a \
             key (see `prova init --list`), or run in an interactive terminal"
        );
        return Err(ExitCode::from(2));
    }

    let choices: Vec<Choice> = catalog
        .entries
        .iter()
        .map(|(key, entry)| Choice {
            key: key.clone(),
            description: entry.description.clone(),
        })
        .collect();
    // Start the cursor on `default` when it's present, so Enter takes the common path.
    let start = choices
        .iter()
        .position(|c| c.key == DEFAULT_KEY)
        .unwrap_or(0);

    match inquire::Select::new("Select a prova init archetype:", choices)
        .with_starting_cursor(start)
        .prompt()
    {
        Ok(choice) => Ok(choice.key),
        Err(
            inquire::InquireError::OperationCanceled | inquire::InquireError::OperationInterrupted,
        ) => {
            eprintln!("prova init: cancelled");
            Err(ExitCode::from(130))
        }
        Err(err) => {
            eprintln!("prova init: selection failed: {err}");
            Err(ExitCode::from(2))
        }
    }
}

/// One row in the interactive picker: `key  —  description`.
struct Choice {
    key: String,
    description: String,
}

impl std::fmt::Display for Choice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}  —  {}", self.key, self.description)
    }
}
