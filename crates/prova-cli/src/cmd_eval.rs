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
pub(crate) fn eval_subcommand(args: Vec<String>) -> ExitCode {
    let mut code: Option<String> = None;
    let mut profile: Option<String> = None;
    let mut manifest_path: Option<String> = None;
    let mut cli_packages: Vec<String> = Vec::new();
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
            cli_packages.push(v);
            continue;
        }
        if let Some(v) = value_flag(&arg, &mut it, &["--format"]) {
            match v.as_str() {
                "json" => force_json = true,
                "console" => {}
                other => {
                    eprintln!("prova eval: unknown format {other:?} (expected console|json)");
                    return ExitCode::from(2);
                }
            }
            continue;
        }
        match arg.as_str() {
            "--json" => force_json = true,
            "-h" | "--help" => {
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
                     examples:\n\
                     \x20 prova eval 'return 1 + 1'\n\
                     \x20 prova eval 'return fs.exists(\"Cargo.toml\")'\n\
                     \x20 prova eval 'local db = require(\"postgres\").container(ctx); return db.url'"
                );
                return ExitCode::SUCCESS;
            }
            "-" if code.is_none() => {
                use std::io::Read;
                let mut buf = String::new();
                if let Err(e) = std::io::stdin().read_to_string(&mut buf) {
                    eprintln!("prova eval: cannot read snippet from stdin: {e}");
                    return ExitCode::from(2);
                }
                code = Some(buf);
            }
            other if other.starts_with('-') && other.len() > 1 => {
                eprintln!("prova eval: unknown flag {other}");
                return ExitCode::from(2);
            }
            other if code.is_none() => code = Some(other.to_string()),
            other => {
                eprintln!("prova eval: unexpected argument {other:?} (expected one snippet)");
                return ExitCode::from(2);
            }
        }
    }

    let Some(code) = code else {
        eprintln!(
            "usage: prova eval '<lua code>'   (or `prova eval -` to read the snippet from stdin)"
        );
        return ExitCode::from(2);
    };
    if code.trim().is_empty() {
        eprintln!("prova eval: the snippet is empty");
        return ExitCode::from(2);
    }

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
