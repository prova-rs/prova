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
    if let Some(budget) = p.budget.as_deref() {
        chips.push(format!("budget: {budget}"));
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
    // The build IS a critical section under the package's declared locks (`[runner] locks =
    // ["cargo"]`): the provision must never race a proof holding `writes("cargo")` in another
    // prova instance — the exact house rule the suite encodes. Blocking hold: waiting for the
    // other holder is the point, and dropping the handles releases.
    let mut held: Vec<std::fs::File> = Vec::new();
    for token in &runner.locks {
        // Narrated with its duration (docs/design/agent-ergonomics.md#narrate-lock-waits): the
        // provision is the first thing a run does, so a silent blocking hold here reads as prova
        // hanging before it has said anything at all.
        match prova_core::locks::hold_timed(token, false, false, Some(&home.dir)) {
            Ok((handle, waited)) => {
                if waited >= std::time::Duration::from_millis(400) {
                    eprintln!(
                        "prova: waited {:.1}s for lock {token:?} (held by another prova instance) \
                         before provisioning the subject",
                        waited.as_secs_f64()
                    );
                }
                held.push(handle);
            }
            Err(e) => eprintln!(
                "prova: [runner] lock {token:?}: {e} — provisioning without the cross-instance \
                 hold (visible degradation, never silent)"
            ),
        }
    }
    let status = if cfg!(windows) {
        std::process::Command::new("cmd").args(["/C", build]).current_dir(&home.dir).status()
    } else {
        std::process::Command::new("sh").args(["-c", build]).current_dir(&home.dir).status()
    };
    drop(held);
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
        locks: table
            .get("locks")
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

/// The env override naming the subject outright: an absolute path to the binary `prova.bin` must
/// resolve to, ahead of any declared `[runner]`.
///
/// For the one caller that builds a DIFFERENT artifact of this same tree and needs the suite to
/// exercise THAT one — coverage. The layered conduct runs the suite through an instrumented build
/// and depends on `prova.bin` children being instrumented too, because the recursion is where the
/// runtime actually executes; with the declared `[runner]` winning, every child was the ordinary
/// uninstrumented `target/debug/prova`, contributing nothing. Measured: a 197-second conduct wrote
/// TWO profraws and the layer read 45% against a 69% floor, with no coverage having been lost.
///
/// An ENV var rather than a flag because the recursion is arbitrarily deep: a flag stops at the
/// first child, while every descendant inherits this (the same reason `LLVM_PROFILE_FILE` works).
/// Precedent: `PROVA_RUN_DEPTH`, also run-scoped mechanism rather than configuration.
///
/// Deliberately NOT a general configuration surface — the manifest is where a package says what it
/// tests, and reading the subject from the ambient environment is exactly the silent split
/// `[runner]` exists to remove. Nothing in prova sets it; a conduct that means it sets it, and the
/// coverage proof asserts afterwards that the recursion was actually measured, so this cannot fail
/// quietly the way the arrangement it replaces did.
const SUBJECT_BIN_ENV: &str = "PROVA_SUBJECT_BIN";

/// The binary nested proofs reach as `prova.bin`: the explicit override when one is set, else the
/// declared subject when the manifest names one, else this executable. Resolution only —
/// provisioning is the run path's explicit act.
pub(crate) fn subject_bin(home: Option<&Home>) -> Option<std::path::PathBuf> {
    // Empty counts as unset, matching `PROVA_RUN_DEPTH`: a child that clears the variable rather
    // than removing it means "no override", not "the subject is the empty path".
    if let Some(path) = std::env::var_os(SUBJECT_BIN_ENV).filter(|v| !v.is_empty()) {
        return Some(std::path::PathBuf::from(path));
    }
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

/// `prova lock <token> [--reads] [--machine] -- <command…>` — the shell-portable spelling of
/// the lock contract (docs/design/architecture.md#lock-wrapper-verb): hold the token, run the
/// command, forward its exit code, release on exit. The suite's own vocabulary: a bare token
/// is a WRITE hold (exclusive, like `locks = { "cargo" }`); `--reads` is the concurrent hold;
/// `--machine` widens past the package. Exists because macOS ships no flock(1) — a Makefile or
/// CI step joins the house rule with this one incantation, and never provisions anything.
pub(crate) fn lock_subcommand(args: Vec<String>) -> ExitCode {
    let usage = || {
        eprintln!("usage: prova lock <token> [--reads] [--machine] -- <command> [args…]");
        ExitCode::from(2)
    };
    let mut token: Option<String> = None;
    let mut shared = false;
    let mut machine = false;
    let mut it = args.into_iter();
    let command: Vec<String> = loop {
        match it.next().as_deref() {
            Some("-h") | Some("--help") => {
                println!(
                    "usage: prova lock <token> [--reads] [--machine] -- <command> [args…]\n\n\
                     Hold the package lock <token> while <command> runs — the same flock the\n\
                     suite's `locks = {{ … }}` and the [runner] provision hold, so an external\n\
                     build joins the house rule (`prova learn locks`). A bare token is a WRITE\n\
                     hold; --reads is the concurrent hold; --machine spans every repo on the\n\
                     box. Blocks until held (says so when it waits), forwards the command's\n\
                     exit code, and the kernel releases on exit — crashes included."
                );
                return ExitCode::SUCCESS;
            }
            Some("--reads") => shared = true,
            Some("--writes") => shared = false, // the explicit spelling of the default
            Some("--machine") => machine = true,
            Some("--") => break it.collect(),
            Some(word) if token.is_none() && !word.starts_with('-') => {
                token = Some(word.to_string());
            }
            Some(_) | None => return usage(),
        }
    };
    let (Some(token), false) = (token, command.is_empty()) else {
        return usage();
    };

    let home = home::find(std::path::Path::new(".")).ok().flatten();
    if home.is_none() && !machine {
        eprintln!(
            "prova: no prova.toml found walking up — a package lock needs a package \
             (use --machine for a box-wide token, or run inside a package)"
        );
        return ExitCode::from(2);
    }
    let project_dir = home.as_ref().map(|h| h.dir.as_path());
    // Try without blocking first, purely so a wait is SAID — then block for real.
    let mut waited = std::time::Duration::ZERO;
    let held = match prova_core::locks::try_hold(&token, shared, machine, project_dir) {
        Ok(Some(f)) => Ok(f),
        _ => {
            eprintln!("prova: waiting for lock {token:?} (held elsewhere)…");
            prova_core::locks::hold_timed(&token, shared, machine, project_dir).map(|(f, w)| {
                waited = w;
                f
            })
        }
    };
    let _held = match held {
        Ok(f) => f,
        Err(e) => {
            eprintln!("prova: lock {token:?}: {e}");
            return ExitCode::from(2);
        }
    };
    let ran_from = std::time::Instant::now();
    match std::process::Command::new(&command[0]).args(&command[1..]).status() {
        Ok(status) => {
            // The split is the whole point (docs/design/agent-ergonomics.md#narrate-lock-waits):
            // this wrapper is where an operator watches a queued build, and "done in 841.8s"
            // sends them to profile a command that ran for 190s.
            if !waited.is_zero() {
                eprintln!(
                    "prova: waited {:.1}s for lock {token:?}, ran {:.1}s",
                    waited.as_secs_f64(),
                    ran_from.elapsed().as_secs_f64()
                );
            }
            ExitCode::from(status.code().unwrap_or(1) as u8)
        }
        Err(e) => {
            eprintln!("prova: lock: cannot run {:?}: {e}", command[0]);
            ExitCode::from(2)
        }
    }
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
