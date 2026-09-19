//! The run journal — every settled leaf, written the moment it settles (docs/plans/resume.md).
//!
//! `last-run.json` is written once, at the end, so a run killed at minute seventeen (a daemon
//! restart, a sleep, an OOM) leaves nothing behind but the previous run's record. The journal is the
//! same account written as it happens: a header line naming the run and the tree it ran over, then
//! one line per leaf, then an end line that confirms the tree did not move underneath the run. A
//! killed run leaves a valid prefix, and `--resume` reads it.
//!
//! One file per invocation KEY (the lane, the selection, the thrown switches), under
//! `var/journals/`, so `prova run ut` never overwrites what `prova run all` could resume from. Only
//! an UNNARROWED run keeps one: resume is for lanes, and hashing the tree on every `-k` inner loop
//! would be a tax on the path that must stay instant.

use std::io::Write;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::home::Home;
use crate::var;

/// The directory under `var/` holding one journal per invocation key.
const JOURNALS: &str = "journals";

/// One line of a journal. Tagged, so a reader can tell a header from a row without guessing by shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Line {
    Header(Header),
    Row(Row),
    /// Written after the last leaf: the tree digest taken again at the end. A journal without one
    /// was killed; one whose `tree` is `None` saw the tree change mid-run.
    End { tree: Option<String> },
}

/// Who ran, over what.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Header {
    pub schema: u32,
    pub run_id: String,
    /// The invocation key, spelled — what `--resume` must match exactly.
    pub key: Vec<String>,
    pub tree: String,
    pub binary: String,
    pub started_at: String,
}

/// One settled leaf, file-qualified, in the record's vocabulary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Row {
    pub path: String,
    /// `passed` | `failed` | `promised` | `skipped` | `reused`.
    pub outcome: String,
    /// For `reused`: the run that executed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
}

/// A journal as read back.
#[derive(Debug, Clone)]
pub struct Journal {
    pub header: Header,
    pub rows: Vec<Row>,
    /// `None`: never ended (killed). `Some(None)`: the tree moved during the run.
    pub end: Option<Option<String>>,
}

impl Journal {
    /// Every pass this journal can lend a resume, keyed by path, valued by the run that executed it.
    /// The LAST row for a path is the verdict: an offered pass that then executed and failed (an
    /// executing leaf depended on it) lends nothing.
    pub fn passes(&self) -> std::collections::BTreeMap<String, String> {
        let mut out = std::collections::BTreeMap::new();
        for r in &self.rows {
            match r.outcome.as_str() {
                "passed" => {
                    out.insert(r.path.clone(), self.header.run_id.clone());
                }
                "reused" => {
                    let from = r.from.clone().unwrap_or_else(|| self.header.run_id.clone());
                    out.insert(r.path.clone(), from);
                }
                _ => {
                    out.remove(&r.path);
                }
            }
        }
        out
    }
}

/// The file name for a key: a short digest, because a spelled key holds `›`, spaces and slashes.
fn file_for(key: &[String]) -> String {
    let mut hasher = <Sha256 as Digest>::new();
    for part in key {
        hasher.update(part.as_bytes());
        hasher.update([0u8]);
    }
    format!("{}.jsonl", &hex::encode(hasher.finalize())[..16])
}

fn path(home: &Home, key: &[String]) -> PathBuf {
    var::path(home).join(JOURNALS).join(file_for(key))
}

/// A run's identity: its start in epoch millis and the process, which is unique on one machine and
/// reads as a time to the person who has to find it.
pub fn new_run_id() -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("run-{millis}-{}", std::process::id())
}

/// An open journal. Best-effort by construction, like the record: a verdict never depends on
/// whether its journal could be written, so every write failure is swallowed after the first
/// warning.
pub struct Writer {
    file: Option<std::fs::File>,
}

impl Writer {
    /// Start the journal for `header.key`, replacing the previous run's.
    pub fn create(home: &Home, header: &Header) -> Writer {
        let file = var::dir(home).ok().and_then(|dir| {
            let dir = dir.join(JOURNALS);
            std::fs::create_dir_all(&dir).ok()?;
            std::fs::File::create(dir.join(file_for(&header.key))).ok()
        });
        let mut w = Writer { file };
        w.line(&Line::Header(header.clone()));
        w
    }

    pub fn row(&mut self, row: Row) {
        self.line(&Line::Row(row));
    }

    pub fn end(&mut self, tree: Option<String>) {
        self.line(&Line::End { tree });
    }

    /// One line, one write: the file is unbuffered, so a line is on disk before the next leaf starts.
    fn line(&mut self, line: &Line) {
        let Some(file) = self.file.as_mut() else { return };
        let Ok(mut text) = serde_json::to_string(line) else { return };
        text.push('\n');
        if let Err(e) = file.write_all(text.as_bytes()) {
            eprintln!("prova: the run journal could not be written ({e}); this run cannot be resumed");
            self.file = None;
        }
    }
}

/// The journal for `key`, if one exists and its header parses. Rows that do not parse end the read
/// there: a line cut by a kill is the one place a partial write can land.
pub fn load(home: &Home, key: &[String]) -> Option<Journal> {
    let text = std::fs::read_to_string(path(home, key)).ok()?;
    let mut lines = text.lines();
    let Ok(Line::Header(header)) = serde_json::from_str(lines.next()?) else {
        return None;
    };
    let mut rows = Vec::new();
    let mut end = None;
    for raw in lines {
        match serde_json::from_str(raw) {
            Ok(Line::Row(r)) => rows.push(r),
            Ok(Line::End { tree }) => end = Some(tree),
            Ok(Line::Header(_)) | Err(_) => break,
        }
    }
    Some(Journal { header, rows, end })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home_in(tag: &str) -> Home {
        let dir = std::env::temp_dir().join(format!("prova-journal-ut-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Home { manifest: dir.join("prova.toml"), dir }
    }

    fn header(key: &str, run: &str) -> Header {
        Header {
            schema: 1,
            run_id: run.into(),
            key: vec![key.into()],
            tree: "jj:abc".into(),
            binary: "bin".into(),
            started_at: "2026-09-22T00:00:00Z".into(),
        }
    }

    fn row(path: &str, outcome: &str, from: Option<&str>) -> Row {
        Row { path: path.into(), outcome: outcome.into(), from: from.map(str::to_string) }
    }

    /// Written line by line, read back whole — and a KILLED run (no end line, a torn last line)
    /// still yields every row that landed before the tear.
    #[test]
    fn a_killed_journal_reads_back_its_prefix() {
        let home = home_in("prefix");
        let mut w = Writer::create(&home, &header("all", "run-1"));
        w.row(row("f › a", "passed", None));
        w.row(row("f › b", "failed", None));
        drop(w);
        let p = path(&home, &["all".to_string()]);
        let mut torn = std::fs::read_to_string(&p).unwrap();
        torn.push_str("{\"kind\":\"row\",\"path\":\"f › c\",\"outc");
        std::fs::write(&p, torn).unwrap();

        let j = load(&home, &["all".to_string()]).expect("the header parses");
        assert_eq!(j.header.run_id, "run-1");
        assert_eq!(j.rows.len(), 2, "the torn line is where the read stops");
        assert!(j.end.is_none(), "no end line: the run was killed");
        let _ = std::fs::remove_dir_all(&home.dir);
    }

    /// A pass lends itself to a resume under the run that executed it: this journal's own run for a
    /// `passed`, the ORIGIN for a `reused` — so a resume of a resume still names who ran it.
    #[test]
    fn passes_carry_the_run_that_executed_them() {
        let j = Journal {
            header: header("all", "run-2"),
            rows: vec![
                row("f › a", "passed", None),
                row("f › b", "reused", Some("run-1")),
                row("f › c", "failed", None),
                row("f › d", "skipped", None),
                // Offered as reused, then executed after all — and failed: the later row wins.
                row("f › e", "reused", Some("run-1")),
                row("f › e", "failed", None),
            ],
            end: Some(Some("jj:abc".into())),
        };
        let p = j.passes();
        assert_eq!(p.len(), 2, "only passes are lent — a failure always runs again");
        assert!(!p.contains_key("f › e"), "the last row for a path is its verdict");
        assert_eq!(p["f › a"], "run-2");
        assert_eq!(p["f › b"], "run-1");
    }

    /// Keys never collide on disk: `ut` and `all` each keep their own journal.
    #[test]
    fn each_key_keeps_its_own_journal() {
        let home = home_in("keys");
        let mut all = Writer::create(&home, &header("all", "run-a"));
        all.end(Some("jj:abc".into()));
        let mut ut = Writer::create(&home, &header("ut", "run-u"));
        ut.end(Some("jj:abc".into()));
        assert_eq!(load(&home, &["all".to_string()]).unwrap().header.run_id, "run-a");
        assert_eq!(load(&home, &["ut".to_string()]).unwrap().header.run_id, "run-u");
        let _ = std::fs::remove_dir_all(&home.dir);
    }
}
