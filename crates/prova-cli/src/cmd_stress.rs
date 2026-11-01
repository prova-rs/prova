//! `prova stress` — reproduce a flake on purpose (docs/design/flakes.md#stress-mode).
//!
//! Each selected nextest case runs as its own process, the way nextest runs it (`<binary> --exact
//! <name> --nocapture --test-threads=1` in the suite's cwd), N copies, P at a time, beside M CPU
//! burners. Every copy gets its own SHORT `TMPDIR` (macOS caps a Unix socket path at 104 bytes) and
//! its own process group, killed when the copy ends so whatever the test spawned dies with it. The
//! burners are started and killed BY PID — at the end, and on SIGINT/SIGTERM.
//!
//! A stress run is a probe, not a gate run: it writes `var/stress/<run>/summary.json` and the logs
//! of failed copies, appends a `source: "stress"` line to the flake ledger for every case whose
//! copies disagreed, exits 1 if any copy failed — and writes no `last-run.json`.

use std::process::ExitCode;

const USAGE: &str = "\
usage: prova stress [-k PAT]... [--node \"<binary-id> <name>\"]... [-E FILTERSET]
                    [--copies N] [--par P] [--burners M] [--timeout S] [--include-excluded]
                    [--listing FILE] [-- <cargo nextest list args>]

Runs every selected nextest case as its own process, N copies (default 8), P at once (default:
the machine's parallelism), beside M `yes` burners (default 0). Each copy has its own short TMPDIR
and process group. Cases whose copies both passed and failed are FLAKY and go to the flake ledger.

  -k PAT              keep cases whose `<binary-id> <name>` contains PAT (case-insensitive;
                      repeatable; `!PAT` excludes)
  --node ID           keep exactly the case `<binary-id> <name>` (repeatable)
  -E FILTERSET        a nextest filterset, handed to `cargo nextest list`
  --copies N          copies of each case (default 8)
  --par P             copies running at once (default: available parallelism)
  --burners M         CPU burners (`yes > /dev/null`) beside the copies (default 0)
  --timeout S         seconds before a copy is killed and counted failed (default 240)
  --include-excluded  also run cases the nextest profile's default-filter excludes
  --listing FILE      read a saved `cargo nextest list --message-format json` instead of listing
  -- ARGS             `cargo nextest list` args (else the manifest's `[stress] nextest`) — build
                      exactly as the gate builds, e.g. --workspace --features x/y --profile prova
";

/// The parsed command line.
#[derive(Debug, Default)]
struct Args {
    keywords: Vec<String>,
    excludes: Vec<String>,
    nodes: Vec<String>,
    filterset: Option<String>,
    copies: usize,
    par: usize,
    burners: usize,
    timeout_secs: u64,
    include_excluded: bool,
    listing: Option<String>,
    nextest: Option<Vec<String>>,
}

fn parse(argv: Vec<String>) -> Result<Args, ExitCode> {
    let mut a = Args {
        copies: 8,
        par: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4),
        timeout_secs: 240,
        ..Args::default()
    };
    let bad = |why: String| -> ExitCode {
        eprintln!("prova stress: {why}\n\n{USAGE}");
        ExitCode::from(2)
    };
    let mut it = argv.into_iter();
    while let Some(arg) = it.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f.to_string(), Some(v.to_string())),
            _ => (arg.clone(), None),
        };
        let mut value = |name: &str| -> Result<String, ExitCode> {
            inline.clone().or_else(|| it.next()).ok_or_else(|| bad(format!("{name} needs a value")))
        };
        let count = |name: &str, v: String, min: usize| -> Result<usize, ExitCode> {
            match v.parse::<usize>() {
                Ok(n) if n >= min => Ok(n),
                _ => Err(bad(format!("{name} expects an integer >= {min}, got {v:?}"))),
            }
        };
        match flag.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                return Err(ExitCode::SUCCESS);
            }
            "-k" => {
                let v = value("-k")?;
                match v.strip_prefix('!') {
                    Some(rest) => a.excludes.push(rest.to_lowercase()),
                    None => a.keywords.push(v.to_lowercase()),
                }
            }
            "--node" => a.nodes.push(value("--node")?),
            "-E" | "--filterset" => a.filterset = Some(value("-E")?),
            "--copies" => a.copies = count("--copies", value("--copies")?, 1)?,
            "--par" => a.par = count("--par", value("--par")?, 1)?,
            "--burners" => a.burners = count("--burners", value("--burners")?, 0)?,
            "--timeout" => {
                let v = value("--timeout")?;
                a.timeout_secs = match v.trim_end_matches('s').parse::<u64>() {
                    Ok(n) if n >= 1 => n,
                    _ => return Err(bad(format!("--timeout expects seconds, got {v:?}"))),
                };
            }
            "--include-excluded" => a.include_excluded = true,
            "--listing" => a.listing = Some(value("--listing")?),
            "--" => {
                a.nextest = Some(it.by_ref().collect());
            }
            other => return Err(bad(format!("unknown argument {other:?}"))),
        }
    }
    Ok(a)
}

/// One selected case.
#[derive(Debug, Clone, PartialEq)]
struct Case {
    binary_id: String,
    binary: String,
    cwd: String,
    name: String,
}

impl Case {
    fn key(&self) -> String {
        format!("{} {}", self.binary_id, self.name)
    }
}

/// The cases a `cargo nextest list --message-format json` listing offers this selection. Ignored
/// cases never run; cases the profile's default-filter excludes (a quarantine) run only under
/// `--include-excluded`; cases a filterset (`-E`) rejected never do.
fn select(listing: &serde_json::Value, a: &Args) -> Result<Vec<Case>, String> {
    let suites = listing
        .get("rust-suites")
        .and_then(|s| s.as_object())
        .ok_or("the listing has no \"rust-suites\" — is it `cargo nextest list --message-format json`?")?;
    let mut cases = Vec::new();
    for (bid, suite) in suites {
        let str_of = |k: &str| suite.get(k).and_then(|v| v.as_str()).unwrap_or_default().to_string();
        let (binary, cwd) = (str_of("binary-path"), str_of("cwd"));
        let Some(tcs) = suite.get("testcases").and_then(|t| t.as_object()) else { continue };
        for (name, tc) in tcs {
            if tc.get("ignored").and_then(|v| v.as_bool()).unwrap_or(false) {
                continue;
            }
            let fm = tc.get("filter-match");
            let status = fm.and_then(|f| f.get("status")).and_then(|v| v.as_str()).unwrap_or("matches");
            let reason = fm.and_then(|f| f.get("reason")).and_then(|v| v.as_str()).unwrap_or("");
            let admitted = status == "matches" || (a.include_excluded && reason == "default-filter");
            if !admitted {
                continue;
            }
            let case = Case { binary_id: bid.clone(), binary: binary.clone(), cwd: cwd.clone(), name: name.clone() };
            let key = case.key();
            let lower = key.to_lowercase();
            let included = (a.keywords.is_empty() && a.nodes.is_empty())
                || a.keywords.iter().any(|k| lower.contains(k))
                || a.nodes.contains(&key);
            if included && !a.excludes.iter().any(|k| lower.contains(k)) {
                cases.push(case);
            }
        }
    }
    Ok(cases)
}

/// A file-name-safe spelling of a case key, bounded so a deep module path cannot overflow NAME_MAX.
fn sanitize(key: &str) -> String {
    let s: String = key
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') { c } else { '_' })
        .collect();
    s.chars().take(150).collect()
}

/// A short temp root: the system temp dir when its path is short, else `/tmp`. A copy's `TMPDIR`
/// sits two components below it, and a Unix socket path under that must fit in 104 bytes.
fn short_temp_base() -> std::path::PathBuf {
    let sys = std::env::temp_dir();
    if sys.as_os_str().len() <= 48 {
        sys
    } else {
        std::path::PathBuf::from("/tmp")
    }
}

/// The failure's label from a failed copy's log: a Rust panic's `panicked at` line and the message
/// beneath it, else a timeout, else the log's last non-comment line.
fn first_failure(log: &str) -> Option<String> {
    let lines: Vec<&str> = log.lines().collect();
    if let Some(i) = lines.iter().position(|l| l.contains("panicked at")) {
        let next = lines.get(i + 1).map(|l| l.trim()).unwrap_or("");
        return Some(format!("{} {next}", lines[i].trim()).trim().to_string());
    }
    if let Some(l) = lines.iter().find(|l| l.starts_with("TIMEOUT")) {
        return Some(l.to_string());
    }
    lines.iter().rev().find(|l| !l.trim().is_empty() && !l.starts_with('#')).map(|l| l.trim().to_string())
}

pub(crate) fn run(argv: Vec<String>) -> ExitCode {
    // Stress is heavy by design (copies beside burners): it holds one of the machine's slots.
    let _slot = crate::machine_slot::take("prova stress");
    let a = match parse(argv) {
        Ok(a) => a,
        Err(code) => return code,
    };
    #[cfg(unix)]
    {
        unix::stress(a)
    }
    #[cfg(not(unix))]
    {
        let _ = a;
        eprintln!("prova stress: unix only — it needs process groups and signals");
        ExitCode::from(2)
    }
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::collections::{BTreeMap, HashSet};
    use std::io::Write;
    use std::os::unix::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    /// What happened to one case's copies.
    #[derive(Default)]
    struct Tally {
        pass: usize,
        fail: usize,
        logs: Vec<String>,
        first_failure: Option<String>,
    }

    /// The live copies' process groups, and whether a stop has begun — one lock, so a copy is never
    /// spawned after the stop has killed everything it knew about.
    #[derive(Default)]
    struct Live {
        groups: HashSet<i32>,
        stopping: bool,
    }

    fn kill_group(pgid: i32) {
        // SAFETY: killpg with a pid this process spawned as a group leader; an exited group answers
        // ESRCH (or EPERM on macOS), which is the "already gone" we want.
        unsafe {
            libc::killpg(pgid, libc::SIGKILL);
        }
    }

    fn kill_pid(pid: i32) {
        // SAFETY: kill with a pid this process spawned and has not yet reaped, so it cannot have been
        // recycled for another process.
        unsafe {
            libc::kill(pid, libc::SIGKILL);
        }
    }

    fn listing(a: &Args, home: &crate::home::Home, manifest_args: Vec<String>) -> Result<serde_json::Value, String> {
        if let Some(path) = &a.listing {
            let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read --listing {path}: {e}"))?;
            return serde_json::from_str(&text).map_err(|e| format!("--listing {path} is not JSON: {e}"));
        }
        let mut argv = vec!["nextest".to_string(), "list".into(), "--message-format".into(), "json".into()];
        if let Some(f) = &a.filterset {
            argv.push("-E".into());
            argv.push(f.clone());
        }
        argv.extend(a.nextest.clone().unwrap_or(manifest_args));
        // Listing BUILDS the test binaries, so it holds the package's cargo lock like every prova
        // cargo lane — a sweep or a conduct elsewhere never races it over target/.
        let _hold = prova_core::locks::hold("cargo", false, false, Some(&home.dir))
            .map_err(|e| format!("cannot hold the cargo lock: {e}"))?;
        eprintln!("prova stress: cargo {}", argv.join(" "));
        let out = std::process::Command::new("cargo")
            .args(&argv)
            .current_dir(&home.dir)
            .stderr(std::process::Stdio::inherit())
            .output()
            .map_err(|e| format!("cannot run cargo nextest list: {e}"))?;
        if !out.status.success() {
            return Err(format!("cargo {} exited {}", argv.join(" "), out.status));
        }
        serde_json::from_slice(&out.stdout).map_err(|e| format!("cargo nextest list printed no JSON listing: {e}"))
    }

    /// How many times a spawn refused "text file busy" is tried again, [`BUSY_PAUSE`] apart.
    const BUSY_TRIES: u32 = 100;
    /// The pause between two such tries.
    const BUSY_PAUSE: Duration = Duration::from_millis(10);

    /// Spawn `command`, past Linux's ETXTBSY. A binary written moments ago is refused as busy
    /// while any process still holds a write descriptor to it — a sibling forked mid-write keeps
    /// one until it execs — and nothing signals the moment it lets go. That refusal is the OS,
    /// never the test: counted as a failed copy, it made a stress run report a flake the test did
    /// not have (prova's own stress proof, release CI 2026-10-31). Every other error is returned
    /// at once, and so is a binary still busy after [`BUSY_TRIES`] tries.
    fn spawn_past_busy(command: &mut std::process::Command) -> std::io::Result<std::process::Child> {
        let mut tries = 1;
        loop {
            match command.spawn() {
                Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy && tries < BUSY_TRIES => {
                    tries += 1;
                    std::thread::sleep(BUSY_PAUSE);
                }
                spawned => return spawned,
            }
        }
    }

    /// Run one copy to its end, its group killed afterwards. `Ok(true)` passed.
    fn copy(case: &Case, n: usize, tmp: &Path, log: &Path, timeout: Duration, live: &Mutex<Live>) -> bool {
        let _ = std::fs::create_dir_all(tmp);
        let Ok(mut file) = std::fs::File::create(log) else { return false };
        let _ = writeln!(file, "# {}\n# copy {n}\n# TMPDIR {}/", case.key(), tmp.display());
        let (Ok(out), Ok(err)) = (file.try_clone(), file.try_clone()) else { return false };
        let started = Instant::now();
        let child = {
            let mut guard = live.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if guard.stopping {
                return false;
            }
            let mut command = std::process::Command::new(&case.binary);
            command
                .args(["--exact", &case.name, "--nocapture", "--test-threads=1"])
                .current_dir(&case.cwd)
                .env("TMPDIR", format!("{}/", tmp.display()))
                .env("NEXTEST", "1")
                .env("RUST_BACKTRACE", "1")
                .stdin(std::process::Stdio::null())
                .stdout(out)
                .stderr(err)
                .process_group(0);
            match spawn_past_busy(&mut command) {
                Ok(c) => {
                    guard.groups.insert(c.id() as i32);
                    c
                }
                Err(e) => {
                    let _ = writeln!(file, "cannot spawn {}: {e}", case.binary);
                    return false;
                }
            }
        };
        let mut child = child;
        let pgid = child.id() as i32;
        let deadline = started + timeout;
        let ok = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status.success(),
                Ok(None) if Instant::now() >= deadline => {
                    kill_group(pgid);
                    let _ = child.wait();
                    let _ = writeln!(file, "TIMEOUT after {}s", timeout.as_secs());
                    break false;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(20)),
                Err(_) => break false,
            }
        };
        // Reap whatever the test left in its group — the daemons it spawned.
        kill_group(pgid);
        live.lock().unwrap_or_else(std::sync::PoisonError::into_inner).groups.remove(&pgid);
        let _ = writeln!(file, "# {:.1}s, {}", started.elapsed().as_secs_f64(), if ok { "passed" } else { "FAILED" });
        ok
    }

    /// The package, its manifest, and the selected cases — or the refusal (exit 2) that says why
    /// there is nothing to stress.
    fn prepare(a: &Args) -> Result<(crate::home::Home, crate::manifest::Manifest, Vec<Case>), ExitCode> {
        let refuse = |why: String| {
            eprintln!("prova stress: {why}");
            ExitCode::from(2)
        };
        let cwd = std::env::current_dir().unwrap_or_default();
        let Ok(Some(home)) = crate::home::find(&cwd) else {
            return Err(refuse(
                "no prova.toml found walking up — stress writes its account under the package's .prova/var/".into(),
            ));
        };
        let manifest = std::fs::read_to_string(&home.manifest)
            .ok()
            .and_then(|t| crate::manifest::Manifest::parse(&t).ok())
            .unwrap_or_default();
        let listed = listing(a, &home, manifest.stress.nextest.clone()).map_err(refuse)?;
        let cases = select(&listed, a).map_err(refuse)?;
        if cases.is_empty() {
            return Err(refuse("the selection matched no case — a stress of nothing is vacuous".into()));
        }
        Ok((home, manifest, cases))
    }

    /// Start `n` burners (`yes > /dev/null`), announcing their PIDs — the PIDs are how they die.
    fn start_burners(n: usize) -> Vec<std::process::Child> {
        let mut burners = Vec::new();
        for _ in 0..n {
            match std::process::Command::new("yes")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                Ok(c) => burners.push(c),
                Err(e) => eprintln!("prova stress: cannot start a burner: {e}"),
            }
        }
        println!("stress: burner pids: {}", pid_list(&burners));
        let _ = std::io::stdout().flush();
        burners
    }

    fn pid_list(children: &[std::process::Child]) -> String {
        children.iter().map(|c| c.id().to_string()).collect::<Vec<_>>().join(" ")
    }

    /// Kill the burners BY PID and reap them.
    fn stop_burners(burners: &mut [std::process::Child]) {
        for b in burners.iter() {
            kill_pid(b.id() as i32);
        }
        for b in burners.iter_mut() {
            let _ = b.wait();
        }
        if !burners.is_empty() {
            println!("stress: burners killed: {}", pid_list(burners));
        }
    }

    /// A stop (SIGINT/SIGTERM/SIGHUP) kills every live copy's group and every burner, then exits 130.
    /// The handler only counts (crate::interrupt); this thread does the killing.
    fn arm_stop(live: Arc<Mutex<Live>>, done: Arc<AtomicBool>, pids: Vec<i32>, tmp_root: PathBuf) -> std::thread::JoinHandle<()> {
        crate::interrupt::arm();
        std::thread::spawn(move || {
            while !done.load(Ordering::SeqCst) {
                if crate::interrupt::raised() {
                    let mut guard = live.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                    guard.stopping = true;
                    let n = guard.groups.len();
                    for g in guard.groups.iter() {
                        kill_group(*g);
                    }
                    for p in &pids {
                        kill_pid(*p);
                    }
                    let _ = std::fs::remove_dir_all(&tmp_root);
                    println!("stress: stopped by a signal; killed {n} copy group(s) and the burners");
                    let _ = std::io::stdout().flush();
                    std::process::exit(130);
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        })
    }

    /// Run every copy, `par` at once, and tally them per case.
    fn run_copies(a: &Args, cases: Vec<Case>, tmp_root: &Path, fail_dir: &Path, live: &Arc<Mutex<Live>>) -> BTreeMap<String, Tally> {
        let jobs: Vec<(usize, usize)> =
            (0..cases.len()).flat_map(|c| (0..a.copies).map(move |_| c)).enumerate().collect();
        let tallies: Arc<Mutex<BTreeMap<String, Tally>>> = Arc::default();
        let next = Arc::new(AtomicUsize::new(0));
        let timeout = Duration::from_secs(a.timeout_secs);
        let (cases, jobs) = (Arc::new(cases), Arc::new(jobs));
        let workers: Vec<_> = (0..a.par.min(jobs.len()))
            .map(|_| {
                let (cases, jobs, next, tallies, live) = (cases.clone(), jobs.clone(), next.clone(), tallies.clone(), live.clone());
                let (tmp_root, fail_dir) = (tmp_root.to_path_buf(), fail_dir.to_path_buf());
                std::thread::spawn(move || loop {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    let Some(&(seq, c)) = jobs.get(i) else { break };
                    let case = &cases[c];
                    let n = seq + 1;
                    let tmp = tmp_root.join(n.to_string());
                    let key = case.key();
                    let log = fail_dir.join(format!("{}.{n}.log", sanitize(&key)));
                    let started = Instant::now();
                    let ok = copy(case, n, &tmp, &log, timeout, &live);
                    let _ = std::fs::remove_dir_all(&tmp);
                    let mut t = tallies.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                    let tally = t.entry(key.clone()).or_default();
                    if ok {
                        tally.pass += 1;
                        let _ = std::fs::remove_file(&log);
                    } else {
                        tally.fail += 1;
                        if tally.first_failure.is_none() {
                            tally.first_failure = std::fs::read_to_string(&log).ok().as_deref().and_then(first_failure);
                        }
                        tally.logs.push(log.display().to_string());
                        println!("FAIL {key} ({:.1}s) -> {}", started.elapsed().as_secs_f64(), log.display());
                    }
                })
            })
            .collect();
        for w in workers {
            let _ = w.join();
        }
        let tallies = std::mem::take(&mut *tallies.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        tallies
    }

    pub(super) fn stress(a: Args) -> ExitCode {
        let (home, manifest, cases) = match prepare(&a) {
            Ok(p) => p,
            Err(code) => return code,
        };
        let run_id = crate::journal::new_run_id();
        let roots: Vec<PathBuf> = manifest.resume.roots.iter().map(|r| home.dir.join(r)).collect();
        let tree = crate::tree::fingerprint_with(&home.dir, &roots).ok();
        let binary = crate::record::binary_fingerprint();
        let out_dir = crate::var::path(&home).join("stress").join(&run_id);
        let fail_dir = out_dir.join("fail");
        if let Err(e) = crate::var::dir(&home).and_then(|_| std::fs::create_dir_all(&fail_dir)) {
            eprintln!("prova stress: cannot create {}: {e}", fail_dir.display());
            return ExitCode::from(2);
        }
        let tmp_root = short_temp_base().join(format!("ps{}", std::process::id()));
        let _ = std::fs::create_dir_all(&tmp_root);
        println!(
            "stress: {} case(s) x {} copies = {} runs, par {}, burners {} (run {run_id})",
            cases.len(),
            a.copies,
            cases.len() * a.copies,
            a.par,
            a.burners
        );
        println!("stress: tmp root {}", tmp_root.display());

        let mut burners = start_burners(a.burners);
        let live: Arc<Mutex<Live>> = Arc::default();
        let done = Arc::new(AtomicBool::new(false));
        let pids = burners.iter().map(|b| b.id() as i32).collect();
        let watcher = arm_stop(live.clone(), done.clone(), pids, tmp_root.clone());
        let selected = cases.clone();
        let tallies = run_copies(&a, cases, &tmp_root, &fail_dir, &live);
        done.store(true, Ordering::SeqCst);
        let _ = watcher.join();
        stop_burners(&mut burners);
        let _ = std::fs::remove_dir_all(&tmp_root);
        let cases = selected;
        account(&a, &home, &out_dir, &run_id, tree, binary, &cases, tallies)
    }

    /// Write the summary and the flake ledger lines, print the tally, and choose the exit.
    #[allow(clippy::too_many_arguments)]
    fn account(
        a: &Args,
        home: &crate::home::Home,
        out_dir: &Path,
        run_id: &str,
        tree: Option<String>,
        binary: String,
        cases: &[Case],
        tallies: BTreeMap<String, Tally>,
    ) -> ExitCode {
        let mut summary_cases = serde_json::Map::new();
        let mut flakes = Vec::new();
        for case in cases {
            let key = case.key();
            let Some(t) = tallies.get(&key) else { continue };
            summary_cases.insert(key.clone(), serde_json::json!({ "pass": t.pass, "fail": t.fail, "logs": t.logs }));
            if t.pass > 0 && t.fail > 0 {
                flakes.push(prova_core::ledger::FlakeEvent {
                    schema: prova_core::ledger::FLAKE_SCHEMA,
                    kind: "flake".into(),
                    test: format!("{}#{}", case.binary_id, case.name),
                    deputed: Some(prova_core::ledger::DeputedRef {
                        verifier: "junit".into(),
                        suite: case.binary_id.clone(),
                        name: case.name.clone(),
                    }),
                    tree: tree.clone(),
                    binary: binary.clone(),
                    failed_run: run_id.to_string(),
                    passed_run: run_id.to_string(),
                    first_failure: t.first_failure.clone(),
                    source: "stress".into(),
                    at: humantime::format_rfc3339_seconds(std::time::SystemTime::now()).to_string(),
                    node: None,
                });
            }
        }
        let summary = serde_json::json!({
            "run": run_id,
            "tree": tree,
            "binary": binary,
            "copies": a.copies,
            "par": a.par,
            "burners": a.burners,
            "cases": summary_cases,
        });
        let summary_path = out_dir.join("summary.json");
        if let Ok(text) = serde_json::to_string_pretty(&summary) {
            let _ = std::fs::write(&summary_path, text);
        }
        if let Ok(var) = crate::var::dir(home) {
            let ledger = var.join(prova_core::ledger::FLAKES_LEDGER);
            if let Err(e) = prova_core::ledger::append_flakes(&ledger, &flakes) {
                eprintln!("prova stress: the flake ledger {} could not be written ({e})", ledger.display());
            }
        }
        let failing: Vec<(&String, &Tally)> = tallies.iter().filter(|(_, t)| t.fail > 0).collect();
        println!("\nstress: {} of {} case(s) failed at least once", failing.len(), cases.len());
        let mut sorted = failing.clone();
        sorted.sort_by(|x, y| y.1.fail.cmp(&x.1.fail));
        for (key, t) in &sorted {
            let label = if t.pass > 0 { "FLAKY" } else { "RED" };
            println!("  {label}  {}/{}  {key}", t.fail, t.pass + t.fail);
            if let Some(m) = &t.first_failure {
                println!("        ↳ {m}");
            }
        }
        println!("stress: summary {}", summary_path.display());
        if failing.is_empty() {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn scratch(tag: &str) -> PathBuf {
            let d = std::env::temp_dir().join(format!("prova-stress-ut-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(d.join("fail")).unwrap();
            d
        }

        fn case(binary: &str, name: &str) -> Case {
            Case { binary_id: "b".into(), binary: binary.into(), cwd: "/".into(), name: name.into() }
        }

        /// Every copy runs and is tallied per case: a passing copy leaves no log, a failing one keeps
        /// its output, and the pool never runs more copies than asked.
        #[test]
        fn copies_are_run_and_tallied_per_case() {
            let d = scratch("copies");
            let a = Args { copies: 3, par: 2, timeout_secs: 30, ..Args::default() };
            let live: Arc<Mutex<Live>> = Arc::default();
            let t = run_copies(&a, vec![case("/usr/bin/true", "ok"), case("/usr/bin/false", "red")], &d.join("tmp"), &d.join("fail"), &live);
            assert_eq!((t["b ok"].pass, t["b ok"].fail), (3, 0));
            assert_eq!((t["b red"].pass, t["b red"].fail, t["b red"].logs.len()), (0, 3, 3));
            assert!(std::fs::read_to_string(&t["b red"].logs[0]).unwrap().contains("# b red"));
            assert!(live.lock().unwrap().groups.is_empty(), "every group reaped");
            let _ = std::fs::remove_dir_all(&d);
        }

        /// A binary still open for writing when its copy starts (ETXTBSY on Linux) runs once the
        /// writer lets go: the refusal is the OS's, never a failed copy.
        #[cfg(target_os = "linux")]
        #[test]
        fn a_copy_whose_binary_is_busy_runs_once_it_is_free() {
            use std::os::unix::fs::PermissionsExt;
            let d = scratch("busy");
            let bin = d.join("fake-test");
            let mut writer = std::fs::File::create(&bin).unwrap();
            writer.write_all(b"#!/bin/sh\nexit 0\n").unwrap();
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
            let still_busy = std::process::Command::new(&bin).status();
            assert_eq!(
                still_busy.map_err(|e| e.kind()).err(),
                Some(std::io::ErrorKind::ExecutableFileBusy),
                "the writer's descriptor makes the binary busy"
            );
            let release = std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(100));
                drop(writer);
            });
            let live: Arc<Mutex<Live>> = Arc::default();
            let ran = copy(
                &case(bin.to_str().unwrap(), "ok"),
                1,
                &d.join("t"),
                &d.join("fail/busy.log"),
                Duration::from_secs(10),
                &live,
            );
            release.join().unwrap();
            assert!(ran, "{}", std::fs::read_to_string(d.join("fail/busy.log")).unwrap_or_default());
            let _ = std::fs::remove_dir_all(&d);
        }

        /// A copy past its timeout is killed and failed; a copy after a stop never starts.
        #[test]
        fn a_hung_copy_is_killed_and_a_stopped_run_spawns_nothing() {
            let d = scratch("timeout");
            let live: Mutex<Live> = Mutex::default();
            let sleeper = Case { binary_id: "b".into(), binary: "/bin/sleep".into(), cwd: "/".into(), name: "30".into() };
            // `sleep --exact 30 …` is a usage error on some platforms; a real hang needs a shell.
            let hang = d.join("hang.sh");
            std::fs::write(&hang, "#!/bin/sh\nexec sleep 30\n").unwrap();
            std::process::Command::new("chmod").arg("+x").arg(&hang).status().unwrap();
            let hung = Case { binary: hang.display().to_string(), ..sleeper };
            let log = d.join("fail/h.log");
            let started = Instant::now();
            assert!(!copy(&hung, 1, &d.join("t1"), &log, Duration::from_secs(1), &live));
            assert!(started.elapsed() < Duration::from_secs(20));
            assert!(std::fs::read_to_string(&log).unwrap().contains("TIMEOUT after 1s"));
            live.lock().unwrap().stopping = true;
            assert!(!copy(&case("/usr/bin/true", "ok"), 2, &d.join("t2"), &d.join("fail/s.log"), Duration::from_secs(5), &live));
            let _ = std::fs::remove_dir_all(&d);
        }

        /// Burners start, run, and die by PID.
        #[test]
        fn burners_die_by_pid() {
            let mut b = start_burners(2);
            assert_eq!(b.len(), 2);
            let pids: Vec<i32> = b.iter().map(|c| c.id() as i32).collect();
            // SAFETY: signal 0 only probes for existence.
            assert!(pids.iter().all(|p| unsafe { libc::kill(*p, 0) } == 0), "running");
            stop_burners(&mut b);
            assert!(pids.iter().all(|p| unsafe { libc::kill(*p, 0) } != 0), "gone and reaped");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Args {
        parse(v.iter().map(|s| s.to_string()).collect()).unwrap()
    }

    const LISTING: &str = r#"{"rust-suites":{
      "cos-daemon::uds":{"binary-path":"/b/uds","cwd":"/c","testcases":{
        "tests::bind":{"ignored":false,"filter-match":{"status":"matches"}},
        "tests::slow":{"ignored":true,"filter-match":{"status":"matches"}},
        "tests::quarantined":{"ignored":false,"filter-match":{"status":"mismatch","reason":"default-filter"}},
        "tests::filtered":{"ignored":false,"filter-match":{"status":"mismatch","reason":"expression"}}}},
      "cos-daemon":{"binary-path":"/b/lib","cwd":"/c","testcases":{
        "tests::ok":{"ignored":false,"filter-match":{"status":"matches"}}}}}}"#;

    fn names(cases: &[Case]) -> Vec<String> {
        let mut v: Vec<String> = cases.iter().map(Case::key).collect();
        v.sort();
        v
    }

    /// Ignored never runs; a quarantine runs only under --include-excluded; a filterset's reject never.
    #[test]
    fn selection_honours_ignore_quarantine_and_filtersets() {
        let l: serde_json::Value = serde_json::from_str(LISTING).unwrap();
        assert_eq!(names(&select(&l, &args(&[])).unwrap()), ["cos-daemon tests::ok", "cos-daemon::uds tests::bind"]);
        assert_eq!(
            names(&select(&l, &args(&["--include-excluded"])).unwrap()),
            ["cos-daemon tests::ok", "cos-daemon::uds tests::bind", "cos-daemon::uds tests::quarantined"]
        );
        assert_eq!(names(&select(&l, &args(&["-k", "UDS"])).unwrap()), ["cos-daemon::uds tests::bind"]);
        assert_eq!(names(&select(&l, &args(&["-k", "!uds"])).unwrap()), ["cos-daemon tests::ok"]);
        assert_eq!(names(&select(&l, &args(&["--node", "cos-daemon tests::ok"])).unwrap()), ["cos-daemon tests::ok"]);
        assert!(select(&serde_json::json!({}), &args(&[])).is_err(), "not a listing");
    }

    #[test]
    fn the_command_line_parses_and_refuses() {
        let a = args(&["--copies", "3", "--par=2", "--burners", "1", "--timeout", "30s", "-E", "test(x)", "--", "--workspace"]);
        assert_eq!((a.copies, a.par, a.burners, a.timeout_secs), (3, 2, 1, 30));
        assert_eq!(a.filterset.as_deref(), Some("test(x)"));
        assert_eq!(a.nextest, Some(vec!["--workspace".to_string()]));
        assert!(parse(vec!["--copies".into(), "0".into()]).is_err());
        assert!(parse(vec!["--bogus".into()]).is_err());
    }

    #[test]
    fn a_failed_copy_is_labelled_by_its_panic() {
        let log = "# k\nrunning 1 test\nthread 'tests::bind' panicked at src/uds.rs:9:5:\nAddress already in use\n";
        assert_eq!(
            first_failure(log).as_deref(),
            Some("thread 'tests::bind' panicked at src/uds.rs:9:5: Address already in use")
        );
        assert_eq!(first_failure("# k\nTIMEOUT after 5s\n").as_deref(), Some("TIMEOUT after 5s"));
        assert!(sanitize("a::b c/d").chars().all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c)));
    }
}
