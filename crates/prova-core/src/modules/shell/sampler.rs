//! `shell.run{ sample = … }`: a process that lives too long under a conduct leaves its stacks
//! and open files behind before anything kills it.
//!
//! Flake class F (evidence lost): nextest kills a hung test at its slow-timeout and keeps only
//! "has been running for over 60 seconds", so a stall that never reproduces leaves nothing to
//! read. A conduct that names what is worth sampling (`matching`, a plain substring of the
//! command line, e.g. nextest's `/deps/` test binaries) and when (`after`) gets one sample per
//! process, written to `into`, while the process is still alive to be read.
//!
//! The tree walk and the due decision are pure ([`descendants`], [`Watch::due`]); the host part
//! is one `ps` per look and one sampler run per due process, off the conduct's own task.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// How often the conduct's tree is looked at: half of `after`, between 200 ms and 5 s. A process
/// is first seen at most this late, so its age reads at most this short.
pub(crate) fn look_every(after: Duration) -> Duration {
    (after / 2).clamp(Duration::from_millis(200), Duration::from_secs(5))
}

/// What a conduct asked to sample.
#[derive(Clone, Debug)]
pub(crate) struct SampleOpts {
    pub after: Duration,
    pub matching: String,
    pub into: PathBuf,
}

/// One row of a process listing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Proc {
    pub pid: u32,
    pub ppid: u32,
    pub command: String,
}

/// `ps -A -o pid=,ppid=,command=` read into rows; a line that is not `pid ppid command` is
/// skipped.
pub(crate) fn parse_ps(text: &str) -> Vec<Proc> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let pid = parts.next()?.parse().ok()?;
            let ppid = parts.next()?.parse().ok()?;
            let command = parts.collect::<Vec<_>>().join(" ");
            (!command.is_empty()).then_some(Proc { pid, ppid, command })
        })
        .collect()
}

/// Every process below `root` in `rows` (not `root` itself), at any depth.
pub(crate) fn descendants(rows: &[Proc], root: u32) -> Vec<Proc> {
    let mut below = Vec::new();
    let mut frontier = vec![root];
    let mut visited = HashSet::from([root]);
    while let Some(parent) = frontier.pop() {
        for p in rows.iter().filter(|p| p.ppid == parent) {
            if visited.insert(p.pid) {
                frontier.push(p.pid);
                below.push(p.clone());
            }
        }
    }
    below
}

/// When each matching process was first seen, and which were sampled. Keyed by pid AND command,
/// so a recycled pid running something else is a new process.
#[derive(Default)]
pub(crate) struct Watch {
    seen: HashMap<(u32, String), Instant>,
    sampled: HashSet<(u32, String)>,
}

impl Watch {
    /// The processes of `below` that match and have now lived past `after`, each returned once.
    /// Forgets processes no longer listed.
    pub(crate) fn due(&mut self, below: &[Proc], opts: &SampleOpts, now: Instant) -> Vec<Proc> {
        let live: HashSet<(u32, String)> = below.iter().map(|p| (p.pid, p.command.clone())).collect();
        self.seen.retain(|k, _| live.contains(k));
        self.sampled.retain(|k| live.contains(k));
        let mut due = Vec::new();
        for p in below.iter().filter(|p| p.command.contains(&opts.matching)) {
            let key = (p.pid, p.command.clone());
            let first = *self.seen.entry(key.clone()).or_insert(now);
            if now.duration_since(first) >= opts.after && self.sampled.insert(key) {
                due.push(p.clone());
            }
        }
        due
    }
}

/// The conduct's process listing, or nothing when `ps` cannot be read (sampling is evidence,
/// never a reason to fail the conduct).
pub(crate) async fn list() -> Vec<Proc> {
    match tokio::process::Command::new("/bin/ps").args(["-A", "-o", "pid=,ppid=,command="]).output().await {
        Ok(out) if out.status.success() => parse_ps(&String::from_utf8_lossy(&out.stdout)),
        _ => Vec::new(),
    }
}

/// Sample `p` into `opts.into` on a blocking thread; says where on stderr.
pub(crate) fn sample(p: Proc, opts: &SampleOpts) {
    let into = opts.into.clone();
    let after = opts.after;
    tokio::task::spawn_blocking(move || {
        let path = into.join(format!("{}-{}.txt", p.pid, short_name(&p.command)));
        match write_sample(&p, after, &path) {
            Ok(()) => eprintln!(
                "prova: sampled pid {} past {}s, its stacks and open files: {}",
                p.pid,
                after.as_secs(),
                path.display()
            ),
            Err(e) => eprintln!("prova: could not sample pid {} into {}: {e}", p.pid, path.display()),
        }
    });
}

/// A file-name-safe stem from a command line: the program's base name and its first argument
/// (nextest passes the test name first), at most 100 characters.
fn short_name(command: &str) -> String {
    let mut words = command.split_whitespace();
    let program = words.next().unwrap_or("proc").rsplit('/').next().unwrap_or("proc");
    let first = words.next().unwrap_or("");
    format!("{program}-{first}")
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') { c } else { '_' })
        .take(100)
        .collect()
}

fn write_sample(p: &Proc, after: Duration, path: &std::path::Path) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut file = std::fs::File::create(path)?;
    writeln!(file, "# pid {} (parent {}), alive past {}s\n# {}\n", p.pid, p.ppid, after.as_secs(), p.command)?;
    for (title, argv) in stack_tools(p.pid) {
        writeln!(file, "## {title}")?;
        match std::process::Command::new(&argv[0]).args(&argv[1..]).stdin(std::process::Stdio::null()).output() {
            Ok(out) => {
                file.write_all(&out.stdout)?;
                file.write_all(&out.stderr)?;
            }
            Err(e) => writeln!(file, "({} could not run: {e})", argv[0])?,
        }
        writeln!(file)?;
    }
    Ok(())
}

/// The stack sampler for this platform, then the open files.
fn stack_tools(pid: u32) -> Vec<(&'static str, Vec<String>)> {
    let pid = pid.to_string();
    let mut tools: Vec<(&'static str, Vec<String>)> = Vec::new();
    if cfg!(target_os = "macos") {
        tools.push(("stacks (sample, 2 s)", vec!["/usr/bin/sample".into(), pid.clone(), "2".into()]));
    } else {
        tools.push((
            "stacks (gdb)",
            vec!["gdb".into(), "-p".into(), pid.clone(), "-batch".into(), "-ex".into(), "thread apply all bt".into()],
        ));
    }
    tools.push(("open files (lsof)", vec!["lsof".into(), "-p".into(), pid]));
    tools
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(after: u64) -> SampleOpts {
        SampleOpts { after: Duration::from_secs(after), matching: "/deps/".into(), into: PathBuf::from("/tmp") }
    }

    #[test]
    fn a_listing_reads_into_rows_and_the_tree_below_a_root_is_walked_at_any_depth() {
        let rows = parse_ps(
            "  1     0 /sbin/launchd\n 10     1 cargo nextest run\n 11    10 cargo-nextest nextest run\n \
             12    11 /w/target/debug/deps/a-1f tests::slow --exact\n 13    12 sleep 300\n 14     1 other\nnot a row\n",
        );
        assert_eq!(rows.len(), 6, "the malformed line is skipped");
        let below: Vec<u32> = descendants(&rows, 10).iter().map(|p| p.pid).collect();
        assert_eq!(below.iter().copied().collect::<HashSet<_>>(), HashSet::from([11, 12, 13]));
        assert_eq!(rows[3].command, "/w/target/debug/deps/a-1f tests::slow --exact");
    }

    #[test]
    fn a_matching_process_is_due_once_past_its_age_and_a_recycled_pid_is_new() {
        let t0 = Instant::now();
        let test = Proc { pid: 12, ppid: 11, command: "/t/deps/a tests::slow".into() };
        let nextest = Proc { pid: 11, ppid: 10, command: "cargo-nextest run".into() };
        let mut w = Watch::default();
        let o = opts(150);
        assert!(w.due(&[nextest.clone(), test.clone()], &o, t0).is_empty(), "first seen: not yet due");
        assert!(w.due(&[nextest.clone(), test.clone()], &o, t0 + Duration::from_secs(149)).is_empty());
        let due = w.due(&[nextest.clone(), test.clone()], &o, t0 + Duration::from_secs(150));
        assert_eq!(due, vec![test.clone()], "only the matching process, once it is old enough");
        assert!(w.due(&[nextest.clone(), test.clone()], &o, t0 + Duration::from_secs(300)).is_empty(), "never twice");
        let recycled = Proc { pid: 12, ppid: 11, command: "/t/deps/a tests::other".into() };
        let at = t0 + Duration::from_secs(301);
        assert!(w.due(&[recycled.clone()], &o, at).is_empty(), "a recycled pid starts its own clock");
        assert_eq!(w.due(&[recycled.clone()], &o, at + Duration::from_secs(150)), vec![recycled]);
    }

    #[test]
    fn a_sample_file_is_named_by_pid_program_and_first_argument() {
        assert_eq!(short_name("/w/target/debug/deps/cos_daemon-1f2e tests::a b"), "cos_daemon-1f2e-tests__a");
        assert_eq!(short_name(""), "proc-");
    }
}
