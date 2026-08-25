//! Run-state for held topologies — the thin bit of persistence that turns `prova up` into a
//! supervisable environment. A running `prova up` **self-registers** a record here (pid + endpoints)
//! when it comes up and removes it on clean teardown, so `prova down` / `prova ps` are pure consumers
//! of these files — they never need a resource inventory. Detached mode (`prova start`) is then just
//! "spawn `prova up` in the background"; the child owns its record exactly as an attached `up` does.
//!
//! Records live under the package's state directory — `<var>/running/<name>.json`, where `<var>` is
//! `<home>/.prova/var/` by default (see `var`) — project-scoped and self-gitignored, so nothing leaks
//! into the user's tree or git status. The detached child's stdio goes to `<var>/running/<name>.log`
//! (by convention — not stored).
//!
//! One location, no fallback. These records briefly lived in a visible `<home>/running/` directory;
//! pre-announcement there is no deployed binary whose held topologies need finding, so reads do not
//! consult it. If an upgrade ever needs to survive a live hold, that is a migration to write then, with
//! a reason — not a branch carried indefinitely on the read path.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::home::Home;
use crate::var;

/// A resource endpoint as recorded in run-state (name → connect URL).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Endpoint {
    pub name: String,
    pub url: String,
}

/// How far along a holder is. A record used to exist only once a topology was fully up, which made
/// a topology *coming* up invisible: `ps` said "no topologies running" while the machine was busy
/// creating a cluster, and a second `start` sailed past the guard straight into the factory
/// (docs/design/agent-ergonomics.md#starting-is-a-visible-state).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// The factory is running. Endpoints and `value` are not populated yet.
    Starting,
    /// Provisioned: endpoints and the rehydration payload are real.
    Ready,
}

impl Default for Status {
    /// **The migration, and the reason it is `Ready`**
    /// (docs/design/agent-ergonomics.md#run-state-is-a-versioned-contract). Every record written
    /// before this field existed was written on success and only on success, so an absent `status`
    /// means ready. Getting this wrong is not cosmetic: without the default, a record from a live
    /// pre-upgrade holder fails to deserialize, `read` returns `None`, and that holder becomes
    /// invisible to `ps`, `down` and the double-provision guard — the exact defect this field was
    /// added to fix, reintroduced by its own fix, for anyone upgrading with something held.
    fn default() -> Status {
        Status::Ready
    }
}

impl Status {
    /// The word `ps` and the refusals use.
    pub fn label(self) -> &'static str {
        match self {
            Status::Starting => "starting",
            Status::Ready => "running",
        }
    }
}

/// One held topology's record: what it is, the pid holding it, when it came up, and its endpoints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    pub name: String,
    pub pid: u32,
    /// Unix seconds when the topology came up (for `ps` uptime).
    pub started_at: u64,
    /// Whether the holder is still provisioning. Defaulted so pre-`status` records parse — see
    /// [`Status::default`], which is load-bearing rather than tidy.
    #[serde(default)]
    pub status: Status,
    pub endpoints: Vec<Endpoint>,
    /// The holder's JSON projection of the factory's returned value — the rehydration payload an
    /// attaching run seeds into its scope caches instead of provisioning
    /// (docs/design/topologies.md#attach-binds-by-name). Defaulted so pre-attach records parse.
    #[serde(default)]
    pub value: serde_json::Value,
}

/// The run-state directory for a project (`<var>/running/`), created on demand. The self-ignoring
/// `.gitignore` comes from `var::dir`, one level up, which covers this and every other kind of
/// generated state at once — so prova still owns its whole footprint with no edits to the user's own
/// ignore files.
pub fn dir(home: &Home) -> std::io::Result<PathBuf> {
    let d = var::dir(home)?.join("running");
    std::fs::create_dir_all(&d)?;
    Ok(d)
}

/// The record path for topology `name`. Resolves without creating anything.
pub fn path(home: &Home, name: &str) -> PathBuf {
    var::path(home).join("running").join(format!("{name}.json"))
}

/// The detached-child log path for topology `name` (by convention).
pub fn log_path(home: &Home, name: &str) -> PathBuf {
    var::path(home).join("running").join(format!("{name}.log"))
}

/// Write (or overwrite) a topology's record **atomically** — full contents to a temp file in the
/// same directory, then `rename` over the target.
///
/// A plain `fs::write` truncates and then writes, so a holder killed in that window leaves a
/// half-written file while its process may still be alive
/// (docs/design/agent-ergonomics.md#unparseable-runstate-record-reads-as-no-hold). A truncated
/// record does not parse, and an unparseable record used to read as *no record at all* — so the
/// crash that produced it also hid the holder it belonged to. Rename is atomic on the same
/// filesystem, so a reader sees either the old record or the new one, never a prefix.
pub fn write(home: &Home, record: &Record) -> std::io::Result<()> {
    let d = dir(home)?;
    let text = serde_json::to_string_pretty(record).map_err(std::io::Error::other)?;
    // Same directory, so the rename cannot cross a filesystem. The pid keeps concurrent writers
    // (a holder and a supervisor) off each other's temp file.
    let tmp = d.join(format!(".{}.{}.tmp", record.name, std::process::id()));
    std::fs::write(&tmp, text)?;
    match std::fs::rename(&tmp, path(home, &record.name)) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// **Claim** a topology's name: write the record only if no file is there yet, atomically.
///
/// `Ok(true)` means this process owns the name; `Ok(false)` means someone else got there first.
/// This is what makes the double-provision guard a guarantee rather than a narrow window
/// (docs/design/agent-ergonomics.md#second-start-joins-or-refuses): read-then-write leaves two
/// starts in the same spawn window both seeing nothing and both proceeding, which is exactly the
/// collision the guard exists to prevent, just harder to hit. `create_new` is `O_EXCL` — the
/// kernel picks one winner.
pub fn claim(home: &Home, record: &Record) -> std::io::Result<bool> {
    use std::io::Write;
    dir(home)?;
    let text = serde_json::to_string_pretty(record).map_err(std::io::Error::other)?;
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path(home, &record.name))
    {
        Ok(mut f) => {
            f.write_all(text.as_bytes())?;
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(e),
    }
}

/// What is at a record's path. Absent and unreadable are **different answers**, and conflating them
/// is the fail-open direction (docs/design/agent-ergonomics.md#unparseable-runstate-record-reads-as-no-hold):
/// a file that exists but will not parse cannot be liveness-checked, so the honest reading is
/// "something may be held here", reported — not "nothing is held here", which sends the guards
/// straight past a live holder into a second instance.
#[derive(Debug)]
pub enum Held {
    /// No record at this name.
    Absent,
    /// A record that parsed.
    Record(Box<Record>),
    /// A file is there and cannot be read or parsed. Carries the reason, for a message that names
    /// what to look at rather than leaving someone to guess.
    Unreadable(String),
}

impl Held {
    /// The record, if it parsed. Callers that use this are choosing to treat unreadable as absent,
    /// which is only correct where "not ready yet" and "not there" lead to the same action.
    pub fn record(self) -> Option<Record> {
        match self {
            Held::Record(r) => Some(*r),
            _ => None,
        }
    }
}

/// Read a topology's record, distinguishing absent from unreadable.
pub fn read(home: &Home, name: &str) -> Held {
    let p = path(home, name);
    let text = match std::fs::read_to_string(&p) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Held::Absent,
        Err(e) => return Held::Unreadable(format!("{}: {e}", p.display())),
    };
    match serde_json::from_str::<Record>(&text) {
        Ok(rec) => Held::Record(Box::new(rec)),
        Err(e) => Held::Unreadable(format!("{}: {e}", p.display())),
    }
}

/// Remove a topology's record (ignored if absent).
pub fn remove(home: &Home, name: &str) {
    let _ = std::fs::remove_file(path(home, name));
}

/// One entry in the run-state directory. The name comes from the FILE NAME, which is legible even
/// when the contents are not — so an unreadable record can still be reported by name.
#[derive(Debug)]
pub struct Entry {
    pub name: String,
    pub held: Held,
}

/// Every entry in this project's run-state directory, sorted by name — **including the ones that
/// do not parse**. `list` used to drop those silently, which made a corrupt record equivalent to no
/// record for every consumer at once: `ps` omitted it, the guards sailed past it, and nothing
/// anywhere said a file was unreadable.
pub fn list(home: &Home) -> Vec<Entry> {
    let mut out = Vec::new();
    let d = var::path(home).join("running");
    if let Ok(entries) = std::fs::read_dir(&d) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Some(name) = p.file_stem().and_then(|s| s.to_str()).map(str::to_string) else {
                continue;
            };
            let held = match std::fs::read_to_string(&p) {
                Ok(t) => match serde_json::from_str::<Record>(&t) {
                    Ok(rec) => Held::Record(Box::new(rec)),
                    Err(e) => Held::Unreadable(format!("{}: {e}", p.display())),
                },
                Err(e) => Held::Unreadable(format!("{}: {e}", p.display())),
            };
            out.push(Entry { name, held });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// The parsed records only — for consumers where an unreadable entry has already been handled, or
/// genuinely does not change the answer.
pub fn records(home: &Home) -> Vec<Record> {
    list(home)
        .into_iter()
        .filter_map(|e| e.held.record())
        .collect()
}

/// Current unix seconds.
pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Is a process alive? Uses `kill -0` (no dependency on libc/nix). On non-unix, best-effort `true`.
pub fn is_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        Command::new("kill")
            .arg("-0")
            .arg(pid.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

/// Send SIGTERM to a process (graceful — the held `prova up` runs its normal teardown). Returns
/// whether the signal was delivered.
pub fn terminate(pid: u32) -> bool {
    #[cfg(unix)]
    {
        Command::new("kill")
            .arg("-TERM")
            .arg(pid.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

/// The last `n` lines of a topology's detached log (for surfacing a provisioning failure).
pub fn log_tail(home: &Home, name: &str, n: usize) -> String {
    let text = std::fs::read_to_string(log_path(home, name)).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

/// Configure a `Command` to run fully detached from this process: its own process group (so it does
/// not receive the parent shell's Ctrl-C and survives the parent exiting), with its stdio pointed at
/// the topology's log.
///
/// "Detached" carries a second requirement that is invisible on Unix and load-bearing on Windows:
/// the child must not keep the PARENT's stdio alive. `prova start` is routinely run by something
/// capturing its output — `shell.run` in a proof, a CI step — so this process's stdout is often the
/// write end of a pipe someone is blocked reading. Redirecting the CHILD's stdio (above) does not
/// settle that on Windows, because `CreateProcessW` is called with `bInheritHandles = TRUE` and the
/// child receives every INHERITABLE handle this process holds, whatever `STARTUPINFO` says. The
/// detached `prova up` outlives `prova start` by design, so it sat on that pipe's write end
/// indefinitely: `prova start` exited, the reader never saw EOF, and the caller hung forever.
///
/// That cost two six-hour Windows CI timeouts before it was diagnosed. It is the same
/// grandchild-holds-the-pipe shape `Process::stop()` already tree-kills for `shell.spawn`, on the
/// one path that never got the fix — which is why the flag is cleared on the handles themselves
/// here rather than papered over at any single call site.
///
/// Clearing `HANDLE_FLAG_INHERIT` governs only what CHILDREN receive; this process keeps writing to
/// its own stdout normally afterwards, which `prova start` does — it prints the endpoints once the
/// child self-registers.
pub fn detach(cmd: &mut Command, log: &Path) -> std::io::Result<()> {
    let out = std::fs::File::create(log)?;
    let err = out.try_clone()?;
    cmd.stdin(Stdio::null()).stdout(out).stderr(err);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::Foundation::{SetHandleInformation, HANDLE_FLAG_INHERIT};

        // DETACHED_PROCESS: no console of its own, and none borrowed from ours.
        // CREATE_NEW_PROCESS_GROUP: a console Ctrl-C must not reach a topology deliberately meant to
        // outlive the command that started it — the counterpart to `process_group(0)` above.
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);

        // The half that actually unwedges a capturing caller. Best-effort by design: a failure means
        // the handle is already non-inheritable or is not a real handle, neither of which is worth
        // refusing to start a topology over.
        for handle in [
            std::io::stdout().as_raw_handle(),
            std::io::stderr().as_raw_handle(),
            std::io::stdin().as_raw_handle(),
        ] {
            unsafe { SetHandleInformation(handle as _, HANDLE_FLAG_INHERIT, 0) };
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_home(tag: &str) -> (Home, PathBuf) {
        let root =
            std::env::temp_dir().join(format!("prova-runstate-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let home = Home {
            dir: root.clone(),
            manifest: root.join("prova.toml"),
        };
        (home, root)
    }

    #[test]
    fn write_read_list_remove_roundtrip() {
        let (home, root) = tmp_home("rt");
        let rec = Record {
            name: "orders".into(),
            pid: 4242,
            started_at: 100,
            status: Status::Ready,
            endpoints: vec![Endpoint {
                name: "db".into(),
                url: "postgres://x".into(),
            }],
            value: serde_json::json!({ "db": { "url": "postgres://x" } }),
        };
        write(&home, &rec).unwrap();
        // Records live in the package's state dir, and the self-ignore sits one level up in `var/`
        // — covering run-state and every other kind of generated state with one file.
        assert!(root.join(".prova/var/running/orders.json").is_file());
        assert!(
            std::fs::read_to_string(root.join(".prova/var/.gitignore"))
                .unwrap()
                .contains('*'),
            "the state dir must ignore itself"
        );
        assert!(
            !root.join("running").exists(),
            "nothing generated at the package root"
        );

        let got = read(&home, "orders").record().expect("record present");
        assert_eq!(got.pid, 4242);
        assert_eq!(got.endpoints[0].url, "postgres://x");

        let all = list(&home);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "orders");

        remove(&home, "orders");
        assert!(read(&home, "orders").record().is_none());
        std::fs::remove_dir_all(&root).ok();
    }

    /// The migration (docs/design/agent-ergonomics.md#run-state-is-a-versioned-contract): a record
    /// written by a holder from before `status` existed must still parse, and must read as READY —
    /// those holders wrote on success and only on success. If this regresses, the symptom is not a
    /// parse error anyone sees; it is `read` returning `None` for a live holder, which makes it
    /// invisible to `ps`, `down` and the guard.
    #[test]
    fn a_pre_status_record_parses_and_reads_as_ready() {
        let old = r#"{"name":"orders","pid":7,"started_at":100,
                      "endpoints":[{"name":"db","url":"postgres://x"}],
                      "value":{"db":{"url":"postgres://x"}}}"#;
        let rec: Record = serde_json::from_str(old).expect("a pre-status record must still parse");
        assert_eq!(rec.status, Status::Ready, "no status means it came up");
        assert_eq!(rec.endpoints[0].url, "postgres://x");
    }

    /// And the new spelling round-trips, so a `starting` record survives write→read.
    #[test]
    fn status_round_trips_through_the_file() {
        let (home, root) = tmp_home("status");
        let rec = Record {
            name: "orders".into(),
            pid: 4242,
            started_at: 100,
            status: Status::Starting,
            endpoints: vec![],
            value: serde_json::Value::Null,
        };
        write(&home, &rec).unwrap();
        assert_eq!(read(&home, "orders").record().unwrap().status, Status::Starting);
        std::fs::remove_dir_all(&root).ok();
    }

    // Records live in exactly one place. A stray file in the old visible `running/` directory is not
    // run-state any more, and must not resurrect a topology in `ps` — one location means one answer.
    #[test]
    fn a_record_outside_the_state_dir_is_not_run_state() {
        let (home, root) = tmp_home("one-location");
        let stray = root.join("running");
        std::fs::create_dir_all(&stray).unwrap();
        std::fs::write(
            stray.join("orders.json"),
            r#"{"name":"orders","pid":7,"started_at":1,"endpoints":[]}"#,
        )
        .unwrap();

        assert!(read(&home, "orders").record().is_none(), "not a record location");
        assert!(list(&home).is_empty(), "ps must not see it");
        std::fs::remove_dir_all(&root).ok();
    }

    // Unix-only, because the assertion below is only meaningful where `is_alive` is real: off unix
    // it is a stub that assumes every pid is alive (detached `up` is a unix story — SIGTERM is how
    // the held process runs its teardown). Asserting a bogus pid is dead on Windows was asserting
    // behavior the platform stub does not have, which is why this test — not the code — was the
    // thing that had been red there.
    #[cfg(unix)]
    #[test]
    fn is_alive_reports_self_and_not_a_bogus_pid() {
        assert!(is_alive(std::process::id()));
        // A pid that (almost certainly) isn't ours and isn't alive.
        assert!(!is_alive(999_999_999));
    }
}
