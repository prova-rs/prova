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
/// The self-hosting trampoline (docs/design/manifest.md#manifest-declared-runner). When the
/// nearest manifest declares `[runner]`, provision it (`build`, loud on failure) and re-exec the
/// declared `bin` with the original argv — so any prova invoked at this home judges through the
/// binary the manifest names, and freshness/identity are mechanism rather than prose.
///
/// `Some(code)` means the invocation was handled here (a completed re-exec on Windows, or a
/// failed provision); `None` means proceed as the runner. Guards, in order:
///   - `PROVA_TRAMPOLINED` non-empty: we ARE the hop's child (the flag rides the exec env and
///     inherits to every descendant, so nested work never re-provisions).
///   - `PROVA_RUN_DEPTH` non-empty: a `prova.bin` sub-run inside a proof — already the right
///     binary by injection; rebuilding underneath a live suite would thrash it.
///   - current exe IS the declared bin: invoking the artifact directly means that artifact;
///     re-execing it into itself buys nothing (and on Windows the build could not replace a
///     running exe anyway).
///
/// Empty-string guards count as unset, so a proof can re-arm the trampoline in a sandbox.
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

/// Run the `[runner]` build when the sources say it is owed. `sources` is the speed opt-in:
/// when the manifest names the build's input roots and nothing under them (nor the manifest
/// itself) is newer than the last successful provision, the multi-second no-op build is skipped —
/// freshness still holds, because any edit under a declared root re-arms the build. Undeclared
/// sources always build. `Some(code)` is a provisioning failure to exit with.
fn provision_runner(home: &Home, runner: &crate::manifest::RunnerSection) -> Option<ExitCode> {
    let build = runner.build.as_deref()?;
    let stamp = home.dir.join("target").join(".prova-runner-stamp");
    let fresh = !runner.sources.is_empty()
        && std::fs::metadata(&stamp)
            .and_then(|m| m.modified())
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

pub(crate) fn runner_trampoline() -> Option<ExitCode> {
    let non_empty = |k: &str| std::env::var(k).map(|v| !v.is_empty()).unwrap_or(false);
    if non_empty("PROVA_TRAMPOLINED") || non_empty("PROVA_RUN_DEPTH") {
        return None;
    }
    let home = home::find(std::path::Path::new(".")).ok()??;
    // Parse `[runner]` LENIENTLY, never through the strict Manifest schema: the whole point of
    // the hop is bridging version skew, and a manifest field this binary predates must not
    // silently disarm the trampoline (deny_unknown_fields would fail the full parse, `.ok()?`
    // would proceed as self, and the stale binary would answer wearing the repo's face — the
    // exact footgun the mechanism exists to kill). Unknown keys inside [runner] are the future's
    // business; build/bin/sources are read by name and everything else is ignored.
    let text = std::fs::read_to_string(&home.manifest).ok()?;
    let value: toml::Value = toml::from_str(&text).ok()?;
    let table = value.get("runner")?.as_table()?;
    // A DECLARED [runner] that cannot be understood is loud, never a silent proceed-as-self:
    // a typo'd `bin` disarming the hop would put the stale binary back in the judge's seat.
    let Some(bin_rel) = table.get("bin").and_then(|v| v.as_str()) else {
        eprintln!(
            "prova: [runner] declares no readable `bin` — the trampoline cannot hop; fix the \
             manifest ([runner] bin = \"<home-relative path>\")"
        );
        return Some(ExitCode::from(2));
    };
    let runner = crate::manifest::RunnerSection {
        build: table.get("build").and_then(|v| v.as_str()).map(String::from),
        bin: bin_rel.to_string(),
        sources: table
            .get("sources")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str()).map(String::from).collect())
            .unwrap_or_default(),
    };
    let bin = home.dir.join(&runner.bin);
    if let Ok(me) = std::env::current_exe() {
        let same = match (me.canonicalize(), bin.canonicalize()) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        };
        if same {
            return None;
        }
    }
    if let Some(code) = provision_runner(&home, &runner) {
        return Some(code);
    }
    if !bin.is_file() {
        eprintln!(
            "prova: [runner] bin {} does not exist after the build — the manifest names a \
             runner the provision step never produced",
            bin.display()
        );
        return Some(ExitCode::from(2));
    }
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = std::process::Command::new(&bin)
            .args(&args)
            .env("PROVA_TRAMPOLINED", "1")
            .exec();
        eprintln!("prova: [runner] exec {} failed: {err}", bin.display());
        Some(ExitCode::from(2))
    }
    #[cfg(not(unix))]
    {
        match std::process::Command::new(&bin)
            .args(&args)
            .env("PROVA_TRAMPOLINED", "1")
            .status()
        {
            Ok(s) => Some(ExitCode::from(s.code().unwrap_or(1) as u8)),
            Err(e) => {
                eprintln!("prova: [runner] exec {} failed: {e}", bin.display());
                Some(ExitCode::from(2))
            }
        }
    }
}

/// `prova switches` — the opt-in classes, listed: every declared `switch = "<class>"` with how
/// many leaves it gates and WHO throws it ([run], the profiles that list it, or nobody — ad-hoc
/// only). The ledger view that keeps a switched class from becoming a hidden test population
/// (docs/design/manifest.md#switches-are-discoverable). A reporter: collects, executes nothing,
/// exits 0.
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
