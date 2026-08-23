//! The package / capabilities / introspect verbs.

use super::*;

/// `prova package lint <file>...` — check each package file against the namespacing grammar.
pub(crate) fn package_subcommand(args: Vec<String>) -> ExitCode {
    let mut args = args.into_iter();
    match args.next().as_deref() {
        Some("lint") => {
            let files: Vec<String> = args.collect();
            if files.is_empty() {
                eprintln!("usage: prova package lint <file>...");
                return ExitCode::from(2);
            }
            let mut ok = true;
            for file in &files {
                // Lint loads each plugin with the same primitives + archetect module a run would
                // install, plus the plugin's own namespace so its intra-plugin `require`s resolve.
                let path = Path::new(file);
                let ns = packages::namespace_for_file(path);
                let mut config = RunConfig::new(1).with_module(prova_archetect::install);
                if let Some(bin) = prova_bin() {
                    config = config.with_prova_bin(bin);
                }
                if let Some((canonical, dir)) = &ns {
                    config = config.with_package_namespace(canonical.clone(), dir.clone());
                }
                match prova_core::inspect_package(path, &config) {
                    Ok(report) if report.issues.is_empty() => {
                        // A plugin is any Lua namespace: a resource (has facets) or a helper library
                        // (none) — both valid. Report the shape rather than requiring facets.
                        let detail = match report.shape {
                            Some(prova_core::PackageShape::Resource) => {
                                format!("resource; facets: {}", report.facets.join(", "))
                            }
                            Some(prova_core::PackageShape::Library) => "library".to_string(),
                            None => "namespace".to_string(),
                        };
                        println!("ok   {file}  ({detail})");
                        // Advisory (non-fatal): a published plugin should ship a LuaCATS stub so
                        // consumers of `require("<name>")` get editor completion. The archetype
                        // generates it; warn when it's absent so the ecosystem stays IDE-ready.
                        if let Some(warning) = missing_stub_warning(&ns) {
                            println!("     warn: {warning}");
                        }
                    }
                    Ok(report) => {
                        ok = false;
                        println!("FAIL {file}");
                        for issue in &report.issues {
                            println!("       - {issue}");
                        }
                    }
                    Err(err) => {
                        ok = false;
                        println!("FAIL {file}");
                        println!("       - could not load: {err}");
                    }
                }
            }
            if ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Some(other) => {
            eprintln!("prova: unknown package subcommand {other:?} (expected: lint)");
            ExitCode::from(2)
        }
        None => {
            eprintln!("usage: prova package lint <file>...");
            ExitCode::from(2)
        }
    }
}

/// `prova capabilities` — what in my world that VARIES is available to me? The variable host
/// probes (docker/github/OS), then what THIS package references (profiles' `must_run`, topology
/// `requires`, companion registrations), each MET or UNMET with the reason. A report, exit 0 —
/// never a gate; the gate is `must_run` at run time. Always-available facts are not checks:
/// compiled-in batteries appear only when a slim build lacks one, and unprobed assumptions
/// (network/internet) are not reported at all.
/// The package context, when a manifest is in reach: the declared `[capabilities]` vocabulary, plus
/// every capability this package *references* (profiles' `must_run`, topology `requires`) as
/// `(expr, where it comes from)` rows.
///
/// Parsed raw — never `resolve_from_manifest`, whose `must_run` gate would FAIL on exactly the unmet
/// guarantee this report exists to show. `caps` is replaced with the project's own vocabulary, so
/// every row below probes with the factory a run would use.
fn package_capability_refs(caps: &mut prova_core::Capabilities) -> Vec<(String, String)> {
    let mut package: Vec<(String, String)> = Vec::new();
    let Some(home) = home::find(std::path::Path::new(".")).ok().flatten() else {
        return package;
    };
    let Ok(text) = std::fs::read_to_string(&home.manifest) else {
        return package;
    };
    let Ok(m) = Manifest::parse(&text) else {
        return package;
    };
    for cap in &m.run.must_run {
        package.push((cap.clone(), "must_run: [run]".to_string()));
    }
    for (name, profile) in &m.profiles {
        for cap in &profile.must_run {
            package.push((cap.clone(), format!("must_run: profile `{name}`")));
        }
    }
    for (name, topo) in &m.topologies {
        for cap in &topo.requires {
            package.push((cap.clone(), format!("topology `{name}`")));
        }
    }
    // The declared vocabulary, from the one resolver the MCP tool also calls — so the two surfaces
    // cannot disagree about what a name means here.
    let (vocabulary, origins) = crate::capabilities::project_vocabulary(&home, &m);
    package.extend(origins);
    *caps = vocabulary;
    package
}

/// `prova capabilities <name>` — everything known about ONE capability: what the name means here,
/// what was run to decide, the raw output, and the parsed version.
///
/// This closes a real diagnostic gap. An unmet capability used to report only `"foo" is
/// unavailable`, and a wrong-version skip only the numbers — with no way to see which command
/// produced them. The version came from somewhere, and "somewhere" was unprintable.
fn explain_capability(name: &str) -> ExitCode {
    let mut caps = prova_core::Capabilities::default();
    let _ = package_capability_refs(&mut caps);
    let e = caps.explain(name);
    // One column width for every row, wide enough for the longest label the explanation can carry —
    // a ragged report is harder to scan than a wide one.
    const W: usize = 14;
    println!("{}", e.name);
    println!("  {:<W$}{}", "kind", e.kind);
    for (label, value) in &e.detail {
        // Multi-line command output stays readable: indent continuation lines under the column.
        let value = value.replace('\n', &format!("\n  {:<W$}", ""));
        println!("  {label:<W$}{value}");
    }
    println!(
        "  {:<W$}{}",
        "status",
        if e.available { "MET" } else { "UNMET" }
    );
    match e.version {
        Some(v) => println!("  {:<W$}{v}", "version"),
        None => println!(
            "  {:<W$}(none — a constraint cannot be confirmed)",
            "version"
        ),
    }
    ExitCode::SUCCESS
}

pub(crate) fn capabilities_subcommand(args: Vec<String>) -> ExitCode {
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!(
            "usage: prova capabilities [<name>]\n\n\
             With no argument: lists what VARIES on this host, plus everything this package's\n\
             manifest declares or references, each MET or UNMET with the reason.\n\n\
             With a name: explains that one capability — what the name means here (declared how, or\n\
             a built-in, or undeclared), what prova ran to decide, the raw output, and the parsed\n\
             version.\n\n\
             Beyond the built-ins, any executable on PATH is a capability, and a package declares\n\
             its own in `[capabilities]`. A report, never a gate (the gate is `must_run` at run\n\
             time). See `prova learn capabilities`."
        );
        return ExitCode::SUCCESS;
    }
    // One positional argument is the explain form; two is a mistake worth naming, since a
    // capability expression with a constraint (`"dotnet >= 9"`) must be quoted as one word.
    let positional: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();
    if positional.len() > 1 {
        eprintln!(
            "prova: capabilities: expected at most one capability name, got {} — quote an \
             expression with a constraint as one argument (\"dotnet >= 9\")",
            positional.len()
        );
        return ExitCode::from(2);
    }
    if let Some(name) = positional.first() {
        // The name, not the expression: `capabilities "dotnet >= 9"` explains `dotnet` and reports
        // the version it found, which is what the caller is trying to see.
        let bare = name
            .split(|c: char| c.is_whitespace() || "<>=^~".contains(c))
            .next()
            .unwrap_or(name);
        return explain_capability(bare);
    }
    // The report answers "what in my world that VARIES is available to me?" — a fact that cannot
    // be false on any machine is not a capability check. So: the variable host probes, then what
    // THIS package's manifest and companion reference (probed the same way), and the compiled-in
    // batteries only when a slim build actually lacks one.
    let mut caps = prova_core::Capabilities::default();

    let package = package_capability_refs(&mut caps);

    // Host probes that genuinely vary by machine or environment. `network`/`internet` are
    // deliberately absent: prova assumes them today (no probe), and an always-MET row is noise.
    const VARIABLE_HOST: &[&str] = &["docker", "github", "unix", "windows"];
    println!("what varies on this host (any binary on PATH is also a capability):");
    println!();
    let mut met = 0usize;
    let mut total = 0usize;
    let mut report = |name: &str, origin: Option<&str>, caps: &prova_core::Capabilities| {
        total += 1;
        let suffix = origin.map(|o| format!("   ({o})")).unwrap_or_default();
        match caps.expr_status(name) {
            Ok(None) => {
                met += 1;
                println!("  MET    {name:<16}{suffix}");
            }
            Ok(Some(reason)) => println!("  UNMET  {name:<16} {reason}{suffix}"),
            Err(e) => println!("  ERROR  {name:<16} {e}{suffix}"),
        }
    };
    for name in VARIABLE_HOST {
        // A built-in this package REDEFINED must never print as an ordinary row: the name no longer
        // means what a reader of another repo would assume, and that is exactly what makes declaring
        // an override safe (docs/design/capabilities.md#overriding-a-builtin-is-declared).
        let origin = caps.declaration(name).map(|kind| {
            if caps.overrides_builtin(name) {
                format!("{} — OVERRIDES the built-in", kind.label())
            } else {
                format!("declared: {}", kind.label())
            }
        });
        report(name, origin.as_deref(), &caps);
    }
    // What this package references — deduped, first origin wins the label.
    let mut seen: std::collections::BTreeSet<String> = VARIABLE_HOST.iter().map(|s| s.to_string()).collect();
    if !package.is_empty() {
        println!();
        println!("what this package references:");
        println!();
        for (expr, origin) in &package {
            if seen.insert(expr.clone()) {
                report(expr, Some(origin), &caps);
            }
        }
    }
    // Compiled-in batteries: a capability only when a slim build lacks one — always-available
    // is not a check, so a full build shows a single footnote instead of rows.
    let natives: Vec<&str> = prova_core::builtin_capability_names()
        .iter()
        .filter(|n| !VARIABLE_HOST.contains(n) && !matches!(**n, "network" | "internet"))
        .copied()
        .collect();
    let missing: Vec<&str> = natives
        .iter()
        .filter(|n| matches!(caps.expr_status(n), Ok(Some(_))))
        .copied()
        .collect();
    println!();
    if missing.is_empty() {
        println!(
            "  compiled in (always available in this build): {} — batteries, not checks",
            natives.join(", ")
        );
    } else {
        println!("  missing from THIS BUILD (feature-gated):");
        for name in &missing {
            report(name, Some("compiled-in module"), &caps);
        }
    }
    println!();
    println!(
        "  {met}/{total} met · per-test `requires` gate at run time (`prova learn capabilities`)"
    );
    ExitCode::SUCCESS
}

/// `prova introspect [<filter>]` — the API surface: every function/method/value prova exposes, as
/// its signature + one-line summary, parsed from the LuaCATS stubs (so it cannot drift from editor
/// completion). The CLI twin of the MCP `introspect` tool and the human-facing form of
/// `prova.help()`; paired with `prova learn`, it is prova's discovery duo — shapes and concepts.
/// `<filter>` narrows by substring over name + summary. v1 shows the CORE surface; declared-plugin
/// APIs (which need package resolution) are a tracked follow-up.
pub(crate) fn introspect_subcommand(args: Vec<String>) -> ExitCode {
    let mut filter: Option<String> = None;
    for arg in args {
        if arg == "-h" || arg == "--help" {
            println!(
                "usage: prova introspect [<filter>]\n\n\
                 The prova API surface — every function/method/value as its signature + one-line\n\
                 summary, parsed from the LuaCATS stubs (never drifts from editor completion).\n\
                 `<filter>` narrows by substring (e.g. `shell`, `postgres`, `tempdir`). The CLI twin\n\
                 of the MCP `introspect` tool and `prova.help()`; pair it with `prova learn`\n\
                 (concepts). Shows the core surface; declared-plugin APIs are a follow-up."
            );
            return ExitCode::SUCCESS;
        }
        if arg.starts_with('-') {
            eprintln!("prova: introspect: unexpected flag {arg:?}\nusage: prova introspect [<filter>]");
            return ExitCode::from(2);
        }
        if filter.is_some() {
            eprintln!("prova: introspect: one filter at a time\nusage: prova introspect [<filter>]");
            return ExitCode::from(2);
        }
        filter = Some(arg);
    }
    let all = prova_core::help::core_entries();
    let entries = match filter.as_deref() {
        Some(n) => prova_core::help::filter(&all, n),
        None => all,
    };
    if entries.is_empty() {
        match &filter {
            Some(n) => println!("prova: nothing in the API surface matches {n:?}"),
            None => println!("prova: no API entries"),
        }
        return ExitCode::SUCCESS;
    }
    for e in &entries {
        // name + signature on one line (`shell.run(cmd, opts?) -> string`); the summary indented
        // under it. Without the name the signature is unattributable — the bug this format avoids.
        println!("  {}{}", e.name, e.signature);
        if !e.summary.is_empty() {
            println!("      {}", e.summary);
        }
    }
    println!();
    match &filter {
        Some(n) => println!("  {} entries matching {n:?}", entries.len()),
        None => println!("  {} entries in the core API surface", entries.len()),
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ExitCode exposes no accessor; its Debug form is the one honest window for asserting
    /// which code a verb chose.
    fn code(c: ExitCode) -> String {
        format!("{c:?}")
    }

    fn argv(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    /// Each verb's argument grammar refuses malformed invocations with exit 2 (usage), before
    /// any probe or filesystem work — and help/no-match forms exit 0.
    #[test]
    fn meta_verbs_refuse_malformed_arguments_with_usage() {
        let usage = code(ExitCode::from(2));
        assert_eq!(code(package_subcommand(argv(&[]))), usage, "package with no subcommand");
        assert_eq!(code(package_subcommand(argv(&["lint"]))), usage, "lint with no files");
        assert_eq!(code(package_subcommand(argv(&["publish"]))), usage, "unknown subcommand");

        // `capabilities` takes an OPTIONAL name (the explain form), so one positional is valid and
        // two is the shell-quoting mistake — a constraint must arrive as one argument
        // (`capabilities "dotnet >= 9"`).
        assert_eq!(
            code(capabilities_subcommand(argv(&["docker", "unix"]))),
            usage,
            "at most one capability name"
        );
        assert_eq!(
            code(capabilities_subcommand(argv(&["docker"]))),
            code(ExitCode::SUCCESS),
            "explaining one capability is a report"
        );
        assert_eq!(code(capabilities_subcommand(argv(&["--help"]))), code(ExitCode::SUCCESS));

        assert_eq!(code(introspect_subcommand(argv(&["--bogus"]))), usage, "unknown flag");
        assert_eq!(code(introspect_subcommand(argv(&["a", "b"]))), usage, "one filter at a time");
        assert_eq!(code(introspect_subcommand(argv(&["--help"]))), code(ExitCode::SUCCESS));
    }

    /// The introspect surface is real: unfiltered it serves the core API; an impossible filter
    /// still exits 0 (an empty answer is an answer, not an error).
    #[test]
    fn introspect_serves_the_core_surface_and_tolerates_no_match() {
        assert_eq!(code(introspect_subcommand(argv(&[]))), code(ExitCode::SUCCESS));
        assert_eq!(
            code(introspect_subcommand(argv(&["zz-no-such-api-zz"]))),
            code(ExitCode::SUCCESS)
        );
        assert!(
            !prova_core::help::core_entries().is_empty(),
            "the embedded LuaCATS stubs parse into a non-empty API surface"
        );
    }
}
