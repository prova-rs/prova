//! The tree fingerprint — the key `--resume` reuses a verdict under (docs/plans/resume.md#phase-1a).
//!
//! A digest of every file the VCS tracks: each path and its bytes, in path order. Content, not a
//! commit id. Describing a jj change must not invalidate a resume, and editing a tracked file always
//! must. The VCS answers only "which files"; the bytes are read here, so an edit the VCS has not
//! recorded as a commit yet still moves the digest.
//!
//! Ignored and untracked-by-git files are out by construction: build output, caches and prova's own
//! `var/` are not the subject of a proof, and hashing a `target/` would cost more than the run.

use std::path::{Path, PathBuf};
use std::process::Command;

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
fn tracked(vcs: &Vcs) -> Result<(PathBuf, Vec<String>), String> {
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
            Command::new("git")
                .args(["ls-files", "-z"])
                .current_dir(root)
                .output(),
            b'\0',
        ),
    };
    let tool = match vcs {
        Vcs::Jj(_) => "jj file list",
        Vcs::Git(_) => "git ls-files",
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
    let mut paths: Vec<String> = out
        .stdout
        .split(|b| *b == split)
        .filter(|p| !p.is_empty())
        .map(|p| String::from_utf8_lossy(p).into_owned())
        .collect();
    paths.sort();
    Ok((root.clone(), paths))
}

/// Digest the tracked tree containing `start`: `<vcs>:<24 hex>`. `Err` says why there is no
/// fingerprint (no repository, or the VCS refused), in words a refusal can quote.
pub fn fingerprint(start: &Path) -> Result<String, String> {
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
    let (root, paths) = tracked(&vcs)?;
    Ok(format!("{kind}:{}", digest(&root, &paths)))
}

/// Path, length and bytes of each file, in order. A tracked path that cannot be read (deleted in the
/// working copy, a dangling link) digests as its own marker, so deleting a file moves the digest too.
fn digest(root: &Path, paths: &[String]) -> String {
    let mut hasher = <Sha256 as Digest>::new();
    for p in paths {
        hasher.update(p.as_bytes());
        hasher.update([0u8]);
        match std::fs::read(root.join(p)) {
            Ok(bytes) => {
                hasher.update((bytes.len() as u64).to_le_bytes());
                hasher.update(&bytes);
            }
            Err(_) => hasher.update(b"\x01unreadable"),
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
