//! The tree fingerprint — the key `--resume` reuses a verdict under (docs/plans/resume.md#phase-1a).
//!
//! A digest of every file the VCS does not ignore: each path and its bytes, in path order. Content, not a
//! commit id. Describing a jj change must not invalidate a resume, and editing a tracked file always
//! must. The VCS answers only "which files"; the bytes are read here, so an edit the VCS has not
//! recorded as a commit yet still moves the digest.
//!
//! Ignored files are out by construction: build output, caches and prova's own `var/` are not the
//! subject of a proof, and hashing a `target/` would cost more than the run. A file git does not track
//! yet but does not ignore is IN, as jj's snapshot already counts it: a new proof is part of the tree it
//! runs in, so adding or editing one moves the digest (a new, untracked proof once read as FLAKY across
//! two runs whose trees really differed).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use sha2::{Digest, Sha256};

/// Which VCS lists the tracked files, rooted where it lives.
#[derive(Debug, PartialEq, Eq)]
enum Vcs {
    Jj(PathBuf),
    Git(PathBuf),
}

/// The nearest enclosing repository. jj first: a colocated repo carries both, and jj's view is the
/// one that includes a new file before anyone has told git about it.
fn detect(start: &Path) -> Option<Vcs> {
    for dir in start.ancestors() {
        if dir.join(".jj").is_dir() {
            return Some(Vcs::Jj(dir.to_path_buf()));
        }
        if dir.join(".git").exists() {
            return Some(Vcs::Git(dir.to_path_buf()));
        }
    }
    None
}

/// The tracked paths, relative to the repository root, sorted.
fn tracked(vcs: &Vcs, var: Option<&Path>) -> Result<(PathBuf, Vec<String>), String> {
    let (root, output, split) = match vcs {
        // `jj file list` snapshots the working copy first, which is what makes a new file count.
        Vcs::Jj(root) => (
            root,
            Command::new("jj")
                .args(["file", "list", "--color", "never"])
                .current_dir(root)
                .output(),
            b'\n',
        ),
        Vcs::Git(root) => (
            root,
            // Tracked, and untracked but not ignored: the same files jj's snapshot counts.
            Command::new("git")
                .args(["ls-files", "-z", "--cached", "--others", "--exclude-standard"])
                .current_dir(root)
                .output(),
            b'\0',
        ),
    };
    let tool = match vcs {
        Vcs::Jj(_) => "jj file list",
        Vcs::Git(_) => "git ls-files --cached --others --exclude-standard",
    };
    let out = output.map_err(|e| format!("{tool} could not start in {}: {e}", root.display()))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!(
            "{tool} failed in {} ({}): {}",
            root.display(),
            out.status,
            err.lines().next().unwrap_or("")
        ));
    }
    let paths: Vec<String> = out
        .stdout
        .split(|b| *b == split)
        .filter(|p| !p.is_empty())
        .map(|p| String::from_utf8_lossy(p).into_owned())
        .collect();
    Ok((root.clone(), the_tree(root, paths, var)))
}

/// Which of a repository's listed `paths` are the tree, sorted: never what prova writes itself, nor
/// anything under the RESOLVED state directory `var`. `PROVA_VAR_DIR` may put that inside the
/// repository, where the default's name match never sees it; compared canonically, so a directory
/// that does not exist holds no file to exclude. One rule for the worktree and a revision alike.
fn the_tree(root: &Path, mut paths: Vec<String>, var: Option<&Path>) -> Vec<String> {
    paths.retain(|p| !prova_writes(p));
    if let Some(var) = var.and_then(|v| v.canonicalize().ok()) {
        let base = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        if let Ok(inside) = var.strip_prefix(&base) {
            let prefix = format!("{}/", inside.to_string_lossy());
            if !inside.as_os_str().is_empty() {
                paths.retain(|p| !p.starts_with(&prefix));
            }
        }
    }
    paths.sort();
    paths
}

/// What prova itself writes into a package (`.luarc.json`, annotations.rs; `.prova/var/`, var.rs's
/// default): its own state, never the subject of a proof, so never the tree. Counting it made every run
/// a new tree in a checkout that does not ignore it (prova's own resume and flake sandboxes), once
/// untracked files counted. Matched at any depth, since a package can sit below its repository's root.
fn prova_writes(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    name == ".luarc.json" || path.starts_with(".prova/var/") || path.contains("/.prova/var/")
}

/// Digest the tracked tree containing `start`: `<vcs>:<24 hex>`. `Err` says why there is no
/// fingerprint (no repository, or the VCS refused), in words a refusal can quote.
pub fn fingerprint(start: &Path) -> Result<String, String> {
    fingerprint_excluding(start, None)
}

/// [`fingerprint`], never counting what lies under `var` (this package's resolved state directory).
fn fingerprint_excluding(start: &Path, var: Option<&Path>) -> Result<String, String> {
    let vcs = detect(start).ok_or_else(|| {
        format!(
            "{} is not inside a jj or git repository, so there is no tree to fingerprint",
            start.display()
        )
    })?;
    let kind = match vcs {
        Vcs::Jj(_) => "jj",
        Vcs::Git(_) => "git",
    };
    let (root, paths) = tracked(&vcs, var)?;
    Ok(format!("{kind}:{}", digest(&root, &paths)))
}

/// The fingerprint of the tree containing `start` AND of every extra root (`[resume] roots`) —
/// each root digested as its own repository's tracked tree, in the order given. Any root that
/// cannot be fingerprinted fails the whole: a resume must never proceed on a partial key. The
/// home's own state directory ([`crate::var::path`]) is never the tree, wherever it resolves.
pub fn fingerprint_with(home: &crate::home::Home, roots: &[PathBuf]) -> Result<String, String> {
    let start = home.dir.as_path();
    let var = crate::var::path(home);
    let mut parts = vec![fingerprint_excluding(start, Some(&var))?];
    for root in roots {
        let part = fingerprint(root).map_err(|why| format!("[resume] root {}: {why}", root.display()))?;
        parts.push(part);
    }
    Ok(parts.join("+"))
}

/// How long one git command reading a revision may take before the read is given up, named: a rev's
/// listing and its blobs come from the local object store, so a minute is a hung git, never a slow one.
pub const REV_PATIENCE: Duration = Duration::from_secs(60);

/// The fingerprint a run at `rev` records for `home` (E m/18479): the same digest as [`fingerprint_with`]
/// over a clean checkout of `rev`, read from that revision's tracked files through git (`ls-tree`, then
/// `cat-file --batch`), never the working copy, so an edit in progress never moves it. A package with
/// `[resume] roots` is refused by name: its runs digest those repositories too, which no one revision of
/// this one names. Each git call is bounded by `patience`.
pub fn fingerprint_rev(
    home: &crate::home::Home,
    roots: &[String],
    rev: &str,
    patience: Duration,
) -> Result<String, String> {
    if !roots.is_empty() {
        return Err(format!(
            "this package's runs also digest its [resume] roots ({}), which no revision of this repository names",
            roots.join(", ")
        ));
    }
    let var = crate::var::path(home);
    rev_fingerprint(&home.dir, rev, Some(&var), patience)
}

/// [`fingerprint_rev`]'s digest of the repository containing `start`, at `rev`, never counting `var`.
fn rev_fingerprint(
    start: &Path,
    rev: &str,
    var: Option<&Path>,
    patience: Duration,
) -> Result<String, String> {
    if rev.is_empty() || rev.starts_with('-') {
        return Err(format!("{rev:?} isn't a revision"));
    }
    let vcs = detect(start).ok_or_else(|| {
        format!(
            "{} is not inside a jj or git repository, so there is no tree to fingerprint",
            start.display()
        )
    })?;
    let (kind, root) = match &vcs {
        Vcs::Jj(root) => ("jj", root),
        Vcs::Git(root) => ("git", root),
    };
    if !root.join(".git").exists() {
        return Err(format!(
            "{} has no git store to read revision {rev} from (a jj repository not colocated)",
            root.display()
        ));
    }
    let git = |args: &[&str], input: Option<Vec<u8>>| {
        let mut cmd = Command::new("git");
        cmd.args(args).current_dir(root);
        bounded_output(&mut cmd, input, patience)
            .map_err(|why| format!("git {}: {why}", args.join(" ")))
    };
    let listing = git(&["ls-tree", "-r", "-z", "--full-tree", rev], None)
        .map_err(|why| format!("revision {rev} can't be read: {why}"))?;
    let entries = listed(&listing)?;
    let paths = the_tree(root, entries.keys().cloned().collect(), var);
    let shas: Vec<&str> = paths
        .iter()
        .filter_map(|p| entries.get(p))
        .filter(|e| e.mode != "160000")
        .map(|e| e.sha.as_str())
        .collect();
    let mut wanted = String::new();
    for sha in &shas {
        wanted.push_str(sha);
        wanted.push('\n');
    }
    let blobs = blobs(&git(&["cat-file", "--batch"], Some(wanted.into_bytes()))?)?;
    let bytes_of = |path: &str| resolved(&entries, &blobs, path);
    Ok(format!(
        "{kind}:{}",
        digest_entries(paths.iter().map(|p| (p.as_str(), bytes_of(p))))
    ))
}

/// `prova tree --rev <rev> [--manifest PATH]`: the fingerprint a run of this package at `rev` records,
/// printed alone on stdout (E m/18479), so a reader compares two values prova made and never carries a
/// copy of the digest. A revision git can't read, or a package whose runs digest `[resume] roots` too,
/// is refused by name with nothing on stdout.
pub(crate) fn tree_subcommand(args: Vec<String>) -> std::process::ExitCode {
    let (mut rev, mut manifest_path) = (None, None);
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        let slot = match arg.as_str() {
            "-h" | "--help" => {
                println!("usage: prova tree --rev <rev> [--manifest PATH]");
                return std::process::ExitCode::SUCCESS;
            }
            "--rev" => &mut rev,
            "--manifest" => &mut manifest_path,
            other => {
                eprintln!("prova tree: unexpected argument {other:?}");
                return crate::exit::config();
            }
        };
        match it.next() {
            Some(v) => *slot = Some(v),
            None => {
                eprintln!("prova tree: {arg} needs a value");
                return crate::exit::config();
            }
        }
    }
    let Some(rev) = rev else {
        eprintln!("prova tree: --rev <rev> names the revision to fingerprint");
        return crate::exit::config();
    };
    if let Err(why) = crate::var::check_env() {
        eprintln!("prova: {why}");
        return crate::exit::config();
    }
    let home = match crate::cmd_topo::resolve_home(manifest_path.as_deref()) {
        Ok(h) => h,
        Err(code) => return code,
    };
    let manifest = match crate::read_manifest(&home) {
        Ok(m) => m,
        Err(code) => return code,
    };
    match fingerprint_rev(&home, &manifest.resume.roots, &rev, REV_PATIENCE) {
        Ok(tree) => {
            println!("{tree}");
            std::process::ExitCode::SUCCESS
        }
        Err(why) => {
            eprintln!("prova tree: {why}");
            crate::exit::config()
        }
    }
}

/// One `ls-tree` row: its mode and object.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    mode: String,
    sha: String,
}

/// `git ls-tree -r -z` rows by path: `<mode> <type> <sha>\t<path>\0`.
fn listed(out: &[u8]) -> Result<std::collections::BTreeMap<String, Entry>, String> {
    let mut entries = std::collections::BTreeMap::new();
    for row in out.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        let row = String::from_utf8_lossy(row);
        let (head, path) = row
            .split_once('\t')
            .ok_or_else(|| format!("git ls-tree row {row:?} has no path"))?;
        let mut fields = head.split(' ');
        let (Some(mode), Some(_kind), Some(sha)) = (fields.next(), fields.next(), fields.next())
        else {
            return Err(format!("git ls-tree row {row:?} isn't <mode> <type> <sha>"));
        };
        entries.insert(
            path.to_owned(),
            Entry {
                mode: mode.to_owned(),
                sha: sha.to_owned(),
            },
        );
    }
    Ok(entries)
}

/// `git cat-file --batch` output by object: `<sha> <type> <size>\n<bytes>\n`, or `<sha> missing\n`.
fn blobs(out: &[u8]) -> Result<std::collections::HashMap<String, Vec<u8>>, String> {
    let mut found = std::collections::HashMap::new();
    let mut at = 0;
    while at < out.len() {
        let end = out[at..]
            .iter()
            .position(|b| *b == b'\n')
            .map(|n| at + n)
            .ok_or("git cat-file: a header with no end")?;
        let header = String::from_utf8_lossy(&out[at..end]).into_owned();
        at = end + 1;
        let fields: Vec<&str> = header.split(' ').collect();
        if fields.len() == 2 && fields[1] == "missing" {
            continue;
        }
        let [sha, _kind, size] = fields[..] else {
            return Err(format!(
                "git cat-file: header {header:?} isn't <sha> <type> <size>"
            ));
        };
        let size: usize = size
            .parse()
            .map_err(|_| format!("git cat-file: size in {header:?}"))?;
        let bytes = out
            .get(at..at + size)
            .ok_or("git cat-file: an object shorter than its size")?;
        found.insert(sha.to_owned(), bytes.to_vec());
        at += size + 1;
    }
    Ok(found)
}

/// What `fs::read` of `path` gives in a clean checkout: a file's bytes; a symlink's target's, followed
/// inside the revision (a link out of it, to a directory or to nothing is unreadable, as reading it
/// in a checkout would fail or reach outside the tree); a submodule is a directory, so unreadable.
fn resolved(
    entries: &std::collections::BTreeMap<String, Entry>,
    blobs: &std::collections::HashMap<String, Vec<u8>>,
    path: &str,
) -> Option<Vec<u8>> {
    let mut at = path.to_owned();
    for _ in 0..40 {
        let entry = entries.get(&at)?;
        let bytes = blobs.get(&entry.sha)?;
        if entry.mode != "120000" {
            return (entry.mode != "160000").then(|| bytes.clone());
        }
        let target = String::from_utf8_lossy(bytes).into_owned();
        let base = at.rsplit_once('/').map_or("", |(dir, _)| dir);
        at = normalized(base, &target)?;
    }
    None
}

/// `target` read from directory `base`, both repository-relative, as a path inside the repository.
fn normalized(base: &str, target: &str) -> Option<String> {
    if target.starts_with('/') {
        return None;
    }
    let mut parts: Vec<&str> = base.split('/').filter(|p| !p.is_empty()).collect();
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            p => parts.push(p),
        }
    }
    Some(parts.join("/"))
}

/// `cmd`'s stdout, `input` written to its stdin, within `patience`: past it the child is killed and
/// the wait is an error naming the bound, never a hang. A nonzero exit is an error with its first
/// stderr line. The wait is a channel receive with a deadline, never a poll.
fn bounded_output(
    cmd: &mut Command,
    input: Option<Vec<u8>>,
    patience: Duration,
) -> Result<Vec<u8>, String> {
    use std::io::{Read, Write};
    use std::process::Stdio;
    cmd.stdin(if input.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("could not start: {e}"))?;
    if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
        std::thread::spawn(move || {
            let _ = stdin.write_all(&input);
        });
    }
    let mut stdout = child.stdout.take().ok_or("no stdout")?;
    let mut stderr = child.stderr.take().ok_or("no stderr")?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut out = Vec::new();
        let read = stdout.read_to_end(&mut out).map(|_| out);
        let _ = tx.send(read);
    });
    let errs = std::thread::spawn(move || {
        let mut err = String::new();
        let _ = stderr.read_to_string(&mut err);
        err
    });
    let out = match rx.recv_timeout(patience) {
        Ok(read) => read.map_err(|e| format!("reading its output: {e}"))?,
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "gave no answer within {patience:?}, so it was stopped"
            ));
        }
    };
    let status = child.wait().map_err(|e| format!("waiting on it: {e}"))?;
    let err = errs.join().unwrap_or_default();
    if !status.success() {
        return Err(format!("{status}: {}", err.lines().next().unwrap_or("")));
    }
    Ok(out)
}

/// [`digest_entries`] over the working copy: each of `paths` read from under `root`.
fn digest(root: &Path, paths: &[String]) -> String {
    digest_entries(
        paths
            .iter()
            .map(|p| (p.as_str(), std::fs::read(root.join(p)).ok())),
    )
}

/// Path, length and bytes of each file, in the order given. A path whose bytes could not be had
/// (`None`: deleted in the working copy, a dangling link, a submodule) digests as its own marker, so
/// deleting a file moves the digest too. The one digest: the worktree and a revision both feed it.
fn digest_entries<'a>(entries: impl IntoIterator<Item = (&'a str, Option<Vec<u8>>)>) -> String {
    let mut hasher = <Sha256 as Digest>::new();
    for (p, bytes) in entries {
        hasher.update(p.as_bytes());
        hasher.update([0u8]);
        match bytes {
            Some(bytes) => {
                hasher.update((bytes.len() as u64).to_le_bytes());
                hasher.update(&bytes);
            }
            None => hasher.update(b"\x01unreadable"),
        }
    }
    hex::encode(hasher.finalize())[..24].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("prova-tree-ut-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The digest is content: the same bytes digest alike, and an edit, a rename or a deletion each
    /// move it.
    #[test]
    fn the_digest_moves_with_bytes_names_and_deletions() {
        let dir = tempdir("digest");
        std::fs::write(dir.join("a.txt"), "one").unwrap();
        std::fs::write(dir.join("b.txt"), "two").unwrap();
        let paths = vec!["a.txt".to_string(), "b.txt".to_string()];
        let base = digest(&dir, &paths);
        assert_eq!(base, digest(&dir, &paths), "same bytes, same digest");

        std::fs::write(dir.join("a.txt"), "uno").unwrap();
        let edited = digest(&dir, &paths);
        assert_ne!(base, edited, "an edit moves the digest");

        std::fs::rename(dir.join("b.txt"), dir.join("c.txt")).unwrap();
        let renamed = digest(&dir, &["a.txt".to_string(), "c.txt".to_string()]);
        assert_ne!(edited, renamed, "a rename moves the digest");

        let deleted = digest(&dir, &paths);
        assert_ne!(edited, deleted, "a tracked file gone from disk moves the digest");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// In git, a file not yet added but not ignored is part of the tree: creating it and editing it
    /// each move the fingerprint, while an ignored one never does. Before this, an untracked new proof
    /// left the digest unmoved, so two runs of really different trees read as one, and a fix in
    /// progress was named FLAKY (E, 2026-11-14).
    #[test]
    fn an_untracked_file_git_does_not_ignore_moves_the_fingerprint() {
        let dir = tempdir("git-untracked");
        let git = |args: &[&str]| {
            let out = Command::new("git").args(args).current_dir(&dir).output().expect("git starts");
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        git(&["init", "-q"]);
        std::fs::write(dir.join(".gitignore"), "/target\n").unwrap();
        std::fs::write(dir.join("a.lua"), "one").unwrap();
        git(&["add", "."]);
        let base = fingerprint(&dir).unwrap();
        assert!(base.starts_with("git:"), "{base}");
        std::fs::create_dir_all(dir.join("target")).unwrap();
        std::fs::write(dir.join("target/out"), "build output").unwrap();
        assert_eq!(fingerprint(&dir).unwrap(), base, "an ignored file is not the tree");
        std::fs::write(dir.join("new_test.lua"), "draft").unwrap();
        let added = fingerprint(&dir).unwrap();
        assert_ne!(added, base, "an untracked, unignored file is part of the tree");
        std::fs::write(dir.join("new_test.lua"), "fixed").unwrap();
        let edited = fingerprint(&dir).unwrap();
        assert_ne!(edited, added, "and editing it moves the digest");
        // What prova writes itself is never the tree, ignored or not (its own sandboxes ignore nothing).
        std::fs::write(dir.join(".luarc.json"), "{}").unwrap();
        std::fs::create_dir_all(dir.join("pkg/.prova/var")).unwrap();
        std::fs::write(dir.join("pkg/.prova/var/last-run.json"), "{}").unwrap();
        assert_eq!(fingerprint(&dir).unwrap(), edited, "prova's own .luarc.json and var/ are not the tree");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The RESOLVED state directory is never the tree, even where the default's name match cannot
    /// see it: `PROVA_VAR_DIR` put inside the repository (D's review of 5394e270). A directory of the
    /// same name elsewhere still counts: only the resolved one is excluded.
    #[test]
    fn the_resolved_state_directory_is_never_the_tree_wherever_it_lives() {
        let dir = tempdir("git-var");
        let out = Command::new("git").args(["init", "-q"]).current_dir(&dir).output().expect("git starts");
        assert!(out.status.success());
        std::fs::write(dir.join("a.lua"), "one").unwrap();
        let var = dir.join("cache/prova/pkg-0123");
        std::fs::create_dir_all(&var).unwrap();
        let base = fingerprint_excluding(&dir, Some(&var)).unwrap();
        std::fs::write(var.join("last-run.json"), "{}").unwrap();
        assert_eq!(fingerprint_excluding(&dir, Some(&var)).unwrap(), base, "the resolved var dir is not the tree");
        assert_ne!(fingerprint(&dir).unwrap(), base, "without it named, the same file would move the digest");
        std::fs::create_dir_all(dir.join("cache/prova/pkg-01234")).unwrap();
        std::fs::write(dir.join("cache/prova/pkg-01234/x"), "a sibling with a longer name").unwrap();
        assert_ne!(fingerprint_excluding(&dir, Some(&var)).unwrap(), base, "a prefix of the name is not the dir");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn committed(tag: &str) -> PathBuf {
        let dir = tempdir(tag);
        let git = |args: &[&str]| {
            let out = Command::new("git")
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(args)
                .current_dir(&dir)
                .output()
                .expect("git starts");
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git(&["init", "-q"]);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.lua"), "one").unwrap();
        std::fs::write(dir.join("sub/b.lua"), "two").unwrap();
        std::os::unix::fs::symlink("../a.lua", dir.join("sub/to-a")).unwrap();
        std::os::unix::fs::symlink("to-a", dir.join("sub/to-to-a")).unwrap();
        std::os::unix::fs::symlink("nowhere", dir.join("dangling")).unwrap();
        std::os::unix::fs::symlink("sub", dir.join("to-dir")).unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "one"]);
        dir
    }

    /// A revision digests as a clean checkout of it does: files, a link followed inside the tree (through
    /// another link too), a dangling link and a link to a directory each as `fs::read` reads them.
    #[test]
    fn a_revision_digests_as_its_clean_checkout_does() {
        let dir = committed("rev-clean");
        let worktree = fingerprint(&dir).unwrap();
        assert_eq!(
            rev_fingerprint(&dir, "HEAD", None, REV_PATIENCE).unwrap(),
            worktree
        );
        std::fs::write(dir.join("a.lua"), "edited").unwrap();
        std::fs::write(dir.join("untracked.lua"), "new").unwrap();
        assert_ne!(fingerprint(&dir).unwrap(), worktree, "the worktree moved");
        assert_eq!(
            rev_fingerprint(&dir, "HEAD", None, REV_PATIENCE).unwrap(),
            worktree,
            "the revision didn't"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// What prova writes, and the resolved state directory, are never a revision's tree either, even
    /// committed: one rule for both ([`the_tree`]).
    #[test]
    fn a_revision_never_counts_what_prova_writes() {
        let dir = committed("rev-var");
        let base = rev_fingerprint(&dir, "HEAD", None, REV_PATIENCE).unwrap();
        let var = dir.join("state");
        std::fs::create_dir_all(dir.join(".prova/var")).unwrap();
        std::fs::create_dir_all(&var).unwrap();
        std::fs::write(dir.join(".prova/var/last-run.json"), "{}").unwrap();
        std::fs::write(var.join("x"), "state").unwrap();
        let out = Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "add",
                "-f",
                "-A",
            ])
            .current_dir(&dir)
            .output()
            .unwrap();
        assert!(out.status.success());
        let out = Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "-m",
                "state",
            ])
            .current_dir(&dir)
            .output()
            .unwrap();
        assert!(out.status.success());
        assert_eq!(
            rev_fingerprint(&dir, "HEAD", Some(&var), REV_PATIENCE).unwrap(),
            base
        );
        assert_ne!(
            rev_fingerprint(&dir, "HEAD", None, REV_PATIENCE).unwrap(),
            base,
            "unnamed, the dir counts"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_revision_git_lacks_or_an_option_is_refused_by_name() {
        let dir = committed("rev-refused");
        let err = rev_fingerprint(&dir, "no-such-rev", None, REV_PATIENCE).unwrap_err();
        assert!(err.contains("no-such-rev"), "{err}");
        let err = rev_fingerprint(&dir, "--output=x", None, REV_PATIENCE).unwrap_err();
        assert!(err.contains("isn't a revision"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A command past its bound is stopped and named, never waited on.
    #[test]
    fn a_command_past_its_bound_is_stopped_and_named() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "exec sleep 30"]);
        let err = bounded_output(&mut cmd, None, Duration::from_millis(50)).unwrap_err();
        assert!(err.contains("within 50ms"), "{err}");
    }

    #[test]
    fn a_links_target_is_read_inside_the_repository() {
        assert_eq!(normalized("sub", "../a.lua").as_deref(), Some("a.lua"));
        assert_eq!(normalized("", "./sub/b").as_deref(), Some("sub/b"));
        assert_eq!(normalized("", "../outside"), None, "out of the repository");
        assert_eq!(normalized("sub", "/abs"), None);
    }

    /// jj is preferred when a repository is colocated, and the NEAREST repository wins. (Whether the
    /// temp root itself sits inside some repository is the host's business, so "none" is not
    /// asserted here.)
    #[test]
    fn detection_prefers_jj_and_the_nearest_repository() {
        let dir = tempdir("detect");
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::create_dir_all(dir.join(".jj")).unwrap();
        std::fs::create_dir_all(dir.join("pkg/sub")).unwrap();
        assert_eq!(detect(&dir.join("pkg/sub")), Some(Vcs::Jj(dir.clone())));
        std::fs::remove_dir_all(dir.join(".jj")).unwrap();
        assert_eq!(detect(&dir.join("pkg/sub")), Some(Vcs::Git(dir.clone())));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
