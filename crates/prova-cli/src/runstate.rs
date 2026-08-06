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

/// One held topology's record: what it is, the pid holding it, when it came up, and its endpoints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    pub name: String,
    pub pid: u32,
    /// Unix seconds when the topology came up (for `ps` uptime).
    pub started_at: u64,
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

/// Write (or overwrite) a topology's record.
pub fn write(home: &Home, record: &Record) -> std::io::Result<()> {
    dir(home)?;
    let text = serde_json::to_string_pretty(record).map_err(std::io::Error::other)?;
    std::fs::write(path(home, &record.name), text)
}

/// Read a topology's record, if present and parseable.
pub fn read(home: &Home, name: &str) -> Option<Record> {
    let text = std::fs::read_to_string(path(home, name)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Remove a topology's record (ignored if absent).
pub fn remove(home: &Home, name: &str) {
    let _ = std::fs::remove_file(path(home, name));
}

/// Every recorded topology in this project, sorted by name.
pub fn list(home: &Home) -> Vec<Record> {
    let mut out = Vec::new();
    let d = var::path(home).join("running");
    if let Ok(entries) = std::fs::read_dir(&d) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) == Some("json") {
                if let Some(rec) = std::fs::read_to_string(&p)
                    .ok()
                    .and_then(|t| serde_json::from_str::<Record>(&t).ok())
                {
                    out.push(rec);
                }
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
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

        let got = read(&home, "orders").expect("record present");
        assert_eq!(got.pid, 4242);
        assert_eq!(got.endpoints[0].url, "postgres://x");

        let all = list(&home);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "orders");

        remove(&home, "orders");
        assert!(read(&home, "orders").is_none());
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

        assert!(read(&home, "orders").is_none(), "not a record location");
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
