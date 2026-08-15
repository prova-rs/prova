//! `prova falsify`, `prova eval`, and the broker verb.

use super::*;

/// `prova tests falsify [<sel>]` — the falsification pass. Selects only tests declaring
/// `falsified_by`, applies each mutation before its body, and inverts the verdict: going red is the
/// proof succeeding, and a body that survives its own falsifier is reported vacuous.
///
/// `--allow-empty` because a suite where nothing declares a mutation is not an error — most proofs
/// never will. It is, however, worth noticing, which is what the empty tally says. (The tests-lane
/// driver; the retired top-level `prova falsify` is gone.)
pub(crate) fn falsify_subcommand(args: Vec<String>) -> ExitCode {
    let mut full = vec!["--falsify".to_string(), "--allow-empty".to_string()];
    full.extend(args);
    run(full)
}

/// `prova eval '<code>'` — run a one-shot Lua snippet in the FULL prova environment (built-in
/// modules, manifest-declared plugins via `require`, a real transient `ctx`) and print the returned
/// value. Goes through the same manifest/home/plugins resolution as the run path, so
/// `require("postgres")` works from a package directory; without a manifest it still runs with the
/// built-ins. Exit 0 on success, 1 if the snippet raises, 2 on usage errors.
/// What `prova eval`'s flag loop yields: the snippet plus the environment knobs.
struct EvalCli {
    code: String,
    profile: Option<String>,
    manifest_path: Option<String>,
    packages: Vec<String>,
    force_json: bool,
}

/// `prova eval --help`. Its own function because the text is longer than the parser around
/// it, and a usage string is the one part of an argument loop that never shares its logic.
fn print_eval_help() {
    println!(
        "usage: prova eval '<lua code>' [--format json] [--profile NAME] [--manifest PATH] [-P name=source]\n\
         \n\
         run a one-shot Lua snippet in the full prova environment — built-in modules\n\
         (fs, shell, docker, http, …), manifest-declared plugins via require(), and a\n\
         real transient `ctx` (anything it provisions is torn down afterwards) — then\n\
         print the returned value and exit.\n\
         \n\
         the snippet may be a bare expression (`1 + 1`) or statements with an explicit\n\
         `return`. pass `-` to read the snippet from stdin.\n\
         \n\
         a snippet whose FIRST line is a Lua comment starts with `--`, which is parsed\n\
         as a flag — pass it after `--` (`prova eval -- '-- a note\\nreturn 1'`).\n\
         \n\
         examples:\n\
         \x20 prova eval 'return 1 + 1'\n\
         \x20 prova eval 'return fs.exists(\"Cargo.toml\")'\n\
         \x20 prova eval 'local db = require(\"postgres\").container(ctx); return db.url'"
    );
}

/// Parse `prova eval`'s arguments; `--help` prints usage and exits successfully, and `-` reads
/// the snippet from stdin.
fn parse_eval_args(args: Vec<String>) -> Result<EvalCli, ExitCode> {
    let mut code: Option<String> = None;
    let mut profile: Option<String> = None;
    let mut manifest_path: Option<String> = None;
    let mut packages: Vec<String> = Vec::new();
    let mut force_json = false;

    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        if let Some(v) = value_flag(&arg, &mut it, &["--profile", "-p"]) {
            profile = Some(v);
            continue;
        }
        if let Some(v) = value_flag(&arg, &mut it, &["--manifest"]) {
            manifest_path = Some(v);
            continue;
        }
        if let Some(v) = value_flag(&arg, &mut it, &["--package", "-P", "--plugin"]) {
            if arg.starts_with("--plugin") { eprintln!("prova: `--plugin` is deprecated — use `--package` (retires at 1.0)"); }
            packages.push(v);
            continue;
        }
        if let Some(v) = value_flag(&arg, &mut it, &["--format"]) {
            match v.as_str() {
                "json" => force_json = true,
                "console" => {}
                other => {
        eprintln!("prova eval: unknown format {other:?} (expected console|json)");
        return Err(ExitCode::from(2));
                }
            }
            continue;
        }
        match arg.as_str() {
            "--json" => force_json = true,
            "-h" | "--help" => {
                print_eval_help();
                return Err(ExitCode::SUCCESS);
            }
            "-" if code.is_none() => {
                use std::io::Read;
                let mut buf = String::new();
                if let Err(e) = std::io::stdin().read_to_string(&mut buf) {
        eprintln!("prova eval: cannot read snippet from stdin: {e}");
        return Err(ExitCode::from(2));
                }
                code = Some(buf);
            }
            // `--` ends flag parsing, exactly as `prova lock <token> -- <cmd>` already spells it.
            // The snippet after it is taken verbatim, which is the only reliable way to pass code
            // that opens with a Lua comment.
            "--" if code.is_none() => match it.next() {
                Some(snippet) => code = Some(snippet),
                None => {
        eprintln!("prova eval: `--` with no snippet after it");
        return Err(ExitCode::from(2));
                }
            },
            // A leading `--` on something that is plainly CODE, not a flag. This is the whole
            // reason `--` above exists: a snippet whose first line is a comment arrives as one
            // argv element starting with `--`, and reporting it as an unknown flag names no Lua
            // at all — so the author goes looking at their script instead of at the argument
            // boundary (docs/design/agent-ergonomics.md#eval-snippet-starting-with-a-comment).
            // A real flag is one word; whitespace or a newline means we are holding source.
            other if other.starts_with("--") && other.chars().any(char::is_whitespace) => {
                eprintln!(
        "prova eval: this looks like Lua beginning with a comment, not a flag — an \
         argument starting with `--` is parsed as one. Pass the snippet after a `--` \
         separator (`prova eval -- '<code>'`), read it from stdin (`-`), or put a \
         statement on the first line."
                );
                return Err(ExitCode::from(2));
            }
            other if other.starts_with('-') && other.len() > 1 => {
                eprintln!("prova eval: unknown flag {other}");
                return Err(ExitCode::from(2));
            }
            other if code.is_none() => code = Some(other.to_string()),
            other => {
                eprintln!("prova eval: unexpected argument {other:?} (expected one snippet)");
                return Err(ExitCode::from(2));
            }
        }
    }

    let Some(code) = code else {
        eprintln!(
            "usage: prova eval '<lua code>'   (or `prova eval -` to read the snippet from stdin)"
        );
        return Err(ExitCode::from(2));
    };
    if code.trim().is_empty() {
        eprintln!("prova eval: the snippet is empty");
        return Err(ExitCode::from(2));
    }
    Ok(EvalCli { code, profile, manifest_path, packages, force_json })
}

pub(crate) fn eval_subcommand(args: Vec<String>) -> ExitCode {
    let EvalCli { code, profile, manifest_path, packages: cli_packages, force_json } =
        match parse_eval_args(args) {
            Ok(cli) => cli,
            Err(code) => return code,
        };

    let layout = match XdgSystemLayout::new() {
        Ok(layout) => layout,
        Err(err) => {
            eprintln!("prova: cannot determine home directories: {err}");
            return ExitCode::from(2);
        }
    };

    // Same home/manifest resolution as the run path — but a missing manifest is fine here: the
    // snippet then runs with just the built-ins (no manifest-declared plugins).
    let home: Option<Home> = if let Some(path) = &manifest_path {
        Some(home::from_manifest_path(Path::new(path)))
    } else {
        match home::find(Path::new(".")) {
            Ok(h) => h,
            Err(e) => {
                eprintln!("prova: {e}");
                return ExitCode::from(2);
            }
        }
    };
    let (mut packages_resolved, sources) = match &home {
        Some(home) => {
            match resolve_from_manifest(home, profile, None, None, None, &layout, false, false, false) {
                Ok(r) => (r.dependencies, r.sources),
                Err(code) => return code,
            }
        }
        None => (packages::ResolvedPackages::default(), BTreeMap::new()),
    };
    if let Err(code) = layer_cli_packages(&cli_packages, &layout, &sources, &mut packages_resolved) {
        return code;
    }
    let config = engine_config(1, &packages_resolved, home.as_ref(), prova_core::progress::null());

    match prova_core::eval_snippet(&code, &config) {
        Ok(value) => {
            print_eval_value(&value, force_json);
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("prova eval: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Print an eval result: scalars plainly (a string without quotes, so the value is shell-friendly),
/// nothing for null, pretty JSON for tables/arrays. `--format json` forces JSON for everything.
pub(crate) fn print_eval_value(value: &serde_json::Value, force_json: bool) {
    use serde_json::Value as J;
    if force_json {
        println!(
            "{}",
            serde_json::to_string_pretty(value).unwrap_or_else(|_| "null".into())
        );
        return;
    }
    match value {
        J::Null => {}
        J::Bool(b) => println!("{b}"),
        J::Number(n) => println!("{n}"),
        J::String(s) => println!("{s}"),
        other => println!(
            "{}",
            serde_json::to_string_pretty(other).unwrap_or_else(|_| "null".into())
        ),
    }
}

/// `prova broker` — the reference placement broker (unix-only: the placement transport IS a unix
/// socket, so there is nothing meaningful to serve elsewhere).
pub(crate) fn broker_subcommand(args: Vec<String>) -> ExitCode {
    #[cfg(unix)]
    {
        broker::run(args)
    }
    #[cfg(not(unix))]
    {
        let _ = args;
        eprintln!("prova broker: the placement transport is a unix socket — not available on this platform");
        ExitCode::FAILURE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    /// The snippet is the one positional; the knobs land in their slots; `--json` and
    /// `--format json` are the same request; an empty or missing snippet is a usage error.
    #[test]
    fn eval_args_parse_the_snippet_and_knobs() {
        let cli = parse_eval_args(args(&["return 1", "-p", "ci", "--json", "-P", "x=./pkg"])).unwrap();
        assert_eq!(cli.code, "return 1");
        assert_eq!(cli.profile.as_deref(), Some("ci"));
        assert!(cli.force_json);
        assert_eq!(cli.packages, vec!["x=./pkg".to_string()]);

        let cli = parse_eval_args(args(&["--format", "json", "return 2"])).unwrap();
        assert!(cli.force_json);
        assert!(parse_eval_args(args(&["--format", "yaml", "x"])).is_err(), "unknown format");
        assert!(parse_eval_args(args(&[])).is_err(), "no snippet");
        assert!(parse_eval_args(args(&["  "])).is_err(), "empty snippet");
        assert!(parse_eval_args(args(&["a", "b"])).is_err(), "one snippet only");
    }
}

#[cfg(test)]
mod separator_tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<EvalCli, ExitCode> {
        parse_eval_args(args.iter().map(|s| s.to_string()).collect())
    }

    /// The refusal code, or `None` when the parse was accepted. `EvalCli` is not `Debug`, and
    /// deriving it only so a test can `unwrap_err` would put a trait on shipping code for the
    /// test's convenience.
    fn refused(args: &[&str]) -> Option<ExitCode> {
        parse(args).err()
    }

    /// The whole point of the separator: a snippet whose first line is a Lua comment arrives as one
    /// argv element starting with `--`, which every argument loop reads as a flag.
    #[test]
    fn a_commented_snippet_survives_the_separator() {
        let code = "-- what this does\nreturn 1";
        let cli = parse(&["--", code]).ok().unwrap();
        assert_eq!(cli.code, code, "taken verbatim, comment and all");
    }

    /// Without it the refusal has to recognize SOURCE, or it reports a flag problem to someone
    /// holding valid Lua — who then audits their script instead of the argument boundary.
    #[test]
    fn a_commented_snippet_without_the_separator_is_refused_as_source() {
        assert_eq!(refused(&["-- a note\nreturn 1"]), Some(ExitCode::from(2)), "refused, not guessed at");
    }

    /// The control that keeps the heuristic from becoming a worse error than the one it replaced:
    /// a real typo'd flag is one word, and must still read as a flag.
    #[test]
    fn a_genuine_unknown_flag_is_still_a_flag() {
        assert_eq!(refused(&["--bogus"]), Some(ExitCode::from(2)));
        // …and a lone `--` with nothing behind it is a usage error, not an empty snippet.
        assert_eq!(refused(&["--"]), Some(ExitCode::from(2)));
    }

    /// The paths that already worked must keep working — a parser fix earns its keep only if it
    /// costs nothing that was already correct.
    #[test]
    fn ordinary_snippets_and_flags_are_untouched() {
        assert_eq!(parse(&["return 1 + 1"]).ok().unwrap().code, "return 1 + 1");

        let cli = parse(&["--profile", "ci", "return 1"]).ok().unwrap();
        assert_eq!(cli.profile.as_deref(), Some("ci"));
        assert_eq!(cli.code, "return 1");

        assert!(parse(&["--json", "return 1"]).ok().unwrap().force_json);
        // A trailing comment never needed the separator; only the leading position bites.
        assert!(parse(&["return 1\n-- trailing"]).is_ok());
        // Two snippets is a usage error, not a silent last-one-wins.
        assert_eq!(refused(&["return 1", "return 2"]), Some(ExitCode::from(2)));
        // An empty snippet is refused rather than run as a no-op.
        assert_eq!(refused(&["   "]), Some(ExitCode::from(2)));
    }
}
