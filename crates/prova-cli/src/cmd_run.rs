//! `prova run`, the manifest-declared runner trampoline, and `prova switches`.

use super::*;

/// `prova run [<lane>]` — the lanes front door. A lane is a `[profiles.<name>]` table; the verb
/// is sugar for `--profile <lane>` (the composable primitive stays — it is what other verbs
/// compose with: `prova list -p ut`), never a second code path. `prova run --list` shows the
/// package's lanes offline, from the manifest alone — the "what can I run right now" answer,
/// mirroring `init --list`.
pub(crate) fn run_subcommand(args: Vec<String>) -> ExitCode {
    let mut args = args.into_iter().peekable();
    match args.peek().map(String::as_str) {
        Some("-h") | Some("--help") => {
            println!(
                "usage: prova run [<lane>] [run options]\n\
                 \x20      prova run --list\n\n\
                 A lane is a [profiles.<name>] table in prova.toml — a named way to run the\n\
                 suite (its own selection, guarantees, env). `prova run ut` is exactly\n\
                 `prova --profile ut`; everything after the lane composes as usual\n\
                 (`prova run ut -k orders`). With no lane, same as bare `prova`."
            );
            ExitCode::SUCCESS
        }
        Some("--list") => {
            let home = match resolve_home(None) {
                Ok(h) => h,
                Err(code) => return code,
            };
            let manifest = match read_manifest(&home) {
                Ok(m) => m,
                Err(code) => return code,
            };
            println!("  {:<12}  the [run] table — what bare `prova` runs", "(default)");
            for (name, p) in &manifest.profiles {
                println!("  {:<12}  {}", name, lane_line(p));
            }
            if manifest.profiles.is_empty() {
                println!();
                println!("  No lanes declared — add [profiles.<name>] tables to prova.toml.");
            }
            ExitCode::SUCCESS
        }
        // A leading non-flag argument is the lane. A path here is a common slip with a specific
        // fix, so it gets its own message instead of "no such profile".
        Some(first) if !first.starts_with('-') => {
            let lane = first.to_string();
            args.next();
            if lane.contains('/') || Path::new(&lane).exists() {
                eprintln!(
                    "prova: `run` takes a lane (a [profiles.<name>] from prova.toml), not a \
                     path — run files/dirs with `prova {lane}`"
                );
                return ExitCode::from(2);
            }
            let mut rest: Vec<String> = vec!["--profile".to_string(), lane];
            rest.extend(args);
            run(rest)
        }
        // No lane: same as bare `prova`, with whatever flags followed.
        _ => run(args.collect()),
    }
}

/// One `run --list` line for a lane: its declared description, else a summary of what it changes.
pub(crate) fn lane_line(p: &crate::manifest::Profile) -> String {
    if let Some(d) = p.description.as_deref().filter(|d| !d.trim().is_empty()) {
        return d.to_string();
    }
    let mut chips: Vec<String> = Vec::new();
    if !p.tags.is_empty() {
        chips.push(format!("tags: {}", p.tags.join(", ")));
    }
    if !p.proofs.is_empty() {
        chips.push(format!("proofs: {}", p.proofs.join(", ")));
    }
    if let Some(jobs) = p.jobs {
        chips.push(format!("jobs: {jobs}"));
    }
    if !p.must_run.is_empty() {
        chips.push(format!("must_run: {}", p.must_run.join(", ")));
    }
    match p.heed.resolve() {
        crate::manifest::Heed::None => {}
        crate::manifest::Heed::All => chips.push("heeds all reminders".to_string()),
        crate::manifest::Heed::Matching(sels) => {
            chips.push(format!("heeds: {}", sels.join(", ")))
        }
    }
    if !p.env.is_empty() {
        chips.push(format!("env: {} var(s)", p.env.len()));
    }
    if !p.dependencies.is_empty() {
        chips.push(format!("deps: {}", p.dependencies.len()));
    }
    if chips.is_empty() {
        "(no overrides — same as default)".to_string()
    } else {
        chips.join("; ")
    }
}

/// `prova reminders` — the attention account, read back from the run record.
///
/// A query verb: executes nothing, evaluates nothing (conditions evaluate during RUNS — see
/// docs/design/reminders.md). Reports every reminder with its recorded state, DUE first, and exits
/// non-zero when any is due — the `attest` pattern, so "is anything owed attention?" is one exit
/// code for a pipeline.
/// Is any file at/under `root` modified after `stamped`? The freshness sweep behind the
/// `[runner] sources` skip — cheap (hundreds of stats), and errs toward "newer" on any
/// unreadable entry so doubt always rebuilds.
pub(crate) fn newer_than(root: &std::path::Path, stamped: std::time::SystemTime) -> bool {
    let meta = match std::fs::symlink_metadata(root) {
        Ok(m) => m,
        Err(_) => return true,
    };
    if meta.is_dir() {
        let entries = match std::fs::read_dir(root) {
            Ok(e) => e,
            Err(_) => return true,
        };
        for entry in entries {
            match entry {
                Ok(e) => {
                    if newer_than(&e.path(), stamped) {
                        return true;
                    }
                }
                Err(_) => return true,
            }
        }
        false
    } else {
        meta.modified().map(|m| m > stamped).unwrap_or(true)
    }
}

/// When the subject was last made current: the LATER of the provision stamp and the bin's own
/// mtime. A direct `cargo build` of the subject produces exactly the artifact `prova.bin`
/// injects — it IS a provision, and the stamp not knowing about it once sent an MCP handshake
/// into a redundant multi-second build (measured live, under the retired re-exec model).
fn provision_reference(
    home: &Home,
    runner: &crate::manifest::RunnerSection,
) -> Option<std::time::SystemTime> {
    let read = |p: std::path::PathBuf| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let stamp = read(home.dir.join("target").join(".prova-runner-stamp"));
    let bin = read(home.dir.join(&runner.bin));
    match (stamp, bin) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    }
}

fn provision_runner(
    home: &Home,
    runner: &crate::manifest::RunnerSection,
    force: bool,
) -> Option<ExitCode> {
    let build = runner.build.as_deref()?;
    let stamp = home.dir.join("target").join(".prova-runner-stamp");
    let fresh = !force
        && !runner.sources.is_empty()
        && provision_reference(home, runner)
            .map(|stamped| {
                let mut roots: Vec<std::path::PathBuf> =
                    runner.sources.iter().map(|s| home.dir.join(s)).collect();
                roots.push(home.manifest.clone());
                !roots.iter().any(|r| newer_than(r, stamped))
            })
            .unwrap_or(false);
    if fresh {
        return None;
    }
    let status = if cfg!(windows) {
        std::process::Command::new("cmd").args(["/C", build]).current_dir(&home.dir).status()
    } else {
        std::process::Command::new("sh").args(["-c", build]).current_dir(&home.dir).status()
    };
    match status {
        Ok(s) if s.success() => {
            if !runner.sources.is_empty() {
                if let Some(dir) = stamp.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let _ = std::fs::write(&stamp, b"");
            }
            None
        }
        Ok(s) => {
            eprintln!(
                "prova: [runner] build failed ({s}) — the declared runner could not be \
                 provisioned; fix the build, or invoke a prova without this manifest"
            );
            Some(ExitCode::from(2))
        }
        Err(e) => {
            eprintln!("prova: [runner] build could not start: {e}");
            Some(ExitCode::from(2))
        }
    }
}

/// The `[runner]` section, read leniently (bridging version skew is its job — a field this
/// binary predates must not silently disarm it). `None` when the manifest declares none.
pub(crate) fn declared_subject(home: &Home) -> Option<Result<crate::manifest::RunnerSection, ExitCode>> {
    let text = std::fs::read_to_string(&home.manifest).ok()?;
    let value: toml::Value = toml::from_str(&text).ok()?;
    let table = value.get("runner")?.as_table()?;
    // A DECLARED [runner] that cannot be read is loud, never a silent proceed: a typo'd `bin`
    // would put the conductor in the subject's seat — the installed binary judging the tree,
    // the exact footgun this section exists to kill.
    let Some(bin) = table.get("bin").and_then(|v| v.as_str()) else {
        eprintln!(
            "prova: [runner] declares no readable `bin` — the subject cannot be resolved; fix \
             the manifest ([runner] bin = \"<home-relative path>\")"
        );
        return Some(Err(ExitCode::from(2)));
    };
    Some(Ok(crate::manifest::RunnerSection {
        build: table.get("build").and_then(|v| v.as_str()).map(String::from),
        bin: bin.to_string(),
        sources: table
            .get("sources")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str()).map(String::from).collect())
            .unwrap_or_default(),
    }))
}

/// Provision the binary under test — just in time, only when a RUN asks
/// (docs/design/manifest.md#runner-is-the-subject-not-the-conductor). Nothing re-execs anymore:
/// the binary you invoke conducts, and `[runner]` names the SUBJECT — the build `prova.bin`
/// injects, so nested proofs judge this tree's build while your installed prova stays the tool
/// in your hand. Freshness compares sources against the LATER of the provision stamp and the
/// bin's own mtime (a direct `cargo build` of the subject IS a provision); `force` (-U) builds
/// unconditionally. `Some(code)` is a provisioning failure to exit with.
pub(crate) fn provision_subject(home: &Home, force: bool) -> Option<ExitCode> {
    // A `prova.bin` child inside a proof IS the subject — rebuilding underneath a live suite
    // would thrash it. Empty counts as unset, so a sandbox proof re-arms provisioning.
    let nested = std::env::var("PROVA_RUN_DEPTH").map(|v| !v.is_empty()).unwrap_or(false);
    if nested {
        return None;
    }
    let runner = match declared_subject(home)? {
        Ok(r) => r,
        Err(code) => return Some(code),
    };
    provision_runner(home, &runner, force)
}

/// The binary nested proofs reach as `prova.bin`: the declared subject when the manifest names
/// one, else this executable. Resolution only — provisioning is the run path's explicit act.
pub(crate) fn subject_bin(home: Option<&Home>) -> Option<std::path::PathBuf> {
    if let Some(home) = home {
        if let Some(Ok(runner)) = declared_subject(home) {
            return Some(home.dir.join(runner.bin));
        }
    }
    std::env::current_exe().ok()
}

pub(crate) fn switches_subcommand(args: Vec<String>) -> ExitCode {
    for arg in &args {
        if arg == "-h" || arg == "--help" {
            println!(
                "usage: prova switches\n\n\
                 Lists every declared opt-in class (`switch = \"<class>\"` on a test, group, or\n\
                 suite.config): how many tests it gates, and who throws it — [run] (every run),\n\
                 the profiles listing it in `switches = [...]`, or nobody (ad-hoc only: `-s`).\n\n\
                 Collects the suite but executes nothing. An ad-hoc-only class is a legitimate\n\
                 posture, surfaced so it is a visible fact rather than an accident."
            );
            return ExitCode::SUCCESS;
        }
    }
    let mut full = vec!["--switches-list".to_string()];
    full.extend(args);
    run(full)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A lane's `--list` line: the declared description verbatim when present, else a chip
    /// summary of exactly what the lane overrides — and an honest "same as default" when nothing.
    #[test]
    fn lane_line_prefers_description_then_summarizes_overrides() {
        let mut p = crate::manifest::Profile::default();
        assert_eq!(lane_line(&p), "(no overrides — same as default)");

        p.tags = vec!["unit".into()];
        p.jobs = Some(4);
        p.env.insert("RUST_LOG".into(), "debug".into());
        assert_eq!(lane_line(&p), "tags: unit; jobs: 4; env: 1 var(s)");

        p.description = Some("the fast lane".into());
        assert_eq!(lane_line(&p), "the fast lane");
        p.description = Some("   ".into());
        assert_ne!(lane_line(&p), "   ", "a blank description is no description");
    }

    /// The `[runner] sources` freshness sweep: older trees say no, any newer file says yes,
    /// and an unreadable root errs toward "newer" so doubt always rebuilds.
    #[test]
    fn newer_than_finds_the_one_fresh_file_and_doubts_toward_rebuild() {
        let root = std::env::temp_dir().join(format!("prova-newer-ut-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "x").unwrap();

        let future = std::time::SystemTime::now() + std::time::Duration::from_secs(3600);
        assert!(!newer_than(&root, future), "nothing is newer than a future stamp");

        let past = std::time::SystemTime::UNIX_EPOCH;
        assert!(newer_than(&root, past), "everything is newer than the epoch");

        assert!(newer_than(&root.join("absent"), future), "unreadable ⇒ assume newer");
        let _ = std::fs::remove_dir_all(&root);
    }
}
