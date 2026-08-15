//! `docker.run{ files = … }` — content a container reads from disk, carried IN rather than baked
//! (docs/design/agent-ergonomics.md#containerized-mounts).
//!
//! **Why not a bind mount.** `binds` is one defaulted field away in bollard's `HostConfig`, and it
//! is the wrong answer. `docker.run` talks to whatever daemon `DOCKER_HOST` resolves to; a bind
//! names a path on the DAEMON's filesystem, and a scope tempdir does not exist over there. Docker's
//! classic response to a missing bind source is to create an empty directory, so the container
//! boots, finds no realm/config/catalog, and fails later as an auth error or a 404 that names
//! nothing about mounts. That is a silent wrong wearing a configured face — the failure class this
//! module has been paid to remove twice already.
//!
//! **What this does instead.** `PUT /containers/{id}/archive` streams a tar into a CREATED but
//! not-yet-started container, so the content travels over the same API as everything else: it works
//! against a remote or rootless daemon, needs no image build, and the bytes come from the proof
//! rather than from ambient host state. The container sees the files at boot because the upload
//! happens between create and start.
//!
//! **The shape follows `docker.build`'s `secrets`.** One of `text` / `file` / `dir` per entry, and
//! a bare string is refused: it is ambiguous between a path and a literal, and guessing wrong
//! either writes the path as content or reads a file the author never meant. That reasoning is
//! already load-bearing one verb over, so the two content-carrying surfaces read the same.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use mlua::{Table, Value};

/// Where one entry's bytes come from. No bare-string form, deliberately — see the module note.
pub(super) enum FileSource {
    /// A literal, written verbatim. The common case: a realm, a config, a fixture document.
    Text(String),
    /// A file already on disk, copied at provision time.
    File(PathBuf),
    /// A directory, copied recursively — an archetype catalog, a seed corpus.
    Dir(PathBuf),
}

/// One `files` entry: an absolute path INSIDE the container, and what lands there.
pub(super) struct FileEntry {
    pub(super) path: String,
    pub(super) source: FileSource,
    /// Unix mode, for the case that actually needs it: something the container will execute.
    pub(super) mode: Option<u32>,
}

/// Parse `files = { ["/abs/path"] = { text|file|dir = …, mode? = "0755" } }`.
///
/// Everything that can be checked without a daemon is checked HERE — absolute paths, exactly one
/// source, a source file that exists — because the alternative is a container that starts and then
/// misbehaves for a reason no message connects back to this table.
pub(super) fn parse(opts: &Table) -> mlua::Result<Vec<FileEntry>> {
    let Some(tbl) = opts.get::<Option<Table>>("files")? else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for pair in tbl.pairs::<String, Value>() {
        let (path, spec) = pair?;
        if !path.starts_with('/') {
            return Err(mlua::Error::RuntimeError(format!(
                "docker.run `files`: {path:?} must be an ABSOLUTE path inside the container — a \
                 relative one has no meaning here, since there is no working directory to resolve \
                 it against until the image's own entrypoint runs"
            )));
        }
        let Value::Table(spec) = spec else {
            return Err(mlua::Error::RuntimeError(format!(
                "docker.run `files`: {path:?} must be a table with one of `text`, `file`, or \
                 `dir` — a bare string is ambiguous between a literal and a path, and guessing \
                 wrong either writes the path as content or reads a file you did not mean (the \
                 same rule `docker.build`'s `secrets` follows), got {}",
                spec.type_name()
            )));
        };
        crate::opts::reject_unknown(
            &spec,
            &["dir", "file", "mode", "text"],
            &format!("docker.run `files[{path:?}]`"),
        )?;

        let text = spec.get::<Option<String>>("text")?;
        let file = spec.get::<Option<String>>("file")?;
        let dir = spec.get::<Option<String>>("dir")?;
        let source = match (text, file, dir) {
            (Some(t), None, None) => FileSource::Text(t),
            (None, Some(f), None) => {
                let p = PathBuf::from(&f);
                if !p.is_file() {
                    return Err(mlua::Error::RuntimeError(format!(
                        "docker.run `files`: {path:?} reads `file` {f:?}, which does not exist"
                    )));
                }
                FileSource::File(p)
            }
            (None, None, Some(d)) => {
                let p = PathBuf::from(&d);
                if !p.is_dir() {
                    return Err(mlua::Error::RuntimeError(format!(
                        "docker.run `files`: {path:?} reads `dir` {d:?}, which is not a directory"
                    )));
                }
                FileSource::Dir(p)
            }
            _ => {
                return Err(mlua::Error::RuntimeError(format!(
                    "docker.run `files`: {path:?} needs exactly one of `text`, `file`, or `dir`"
                )))
            }
        };

        // A string, because `0755` in Lua is decimal 755 and would be a surprising 0o1363. Taking
        // the octal text keeps the number the author wrote and the mode the container gets the
        // same thing.
        let mode = match spec.get::<Option<String>>("mode")? {
            Some(m) => Some(u32::from_str_radix(m.trim_start_matches("0o"), 8).map_err(|_| {
                mlua::Error::RuntimeError(format!(
                    "docker.run `files`: {path:?} has mode {m:?} — expected octal digits like \
                     \"0755\""
                ))
            })?),
            None => None,
        };
        out.push(FileEntry { path, source, mode });
    }
    // Lua table order is unspecified; a stable order keeps the tar (and any failure naming an
    // entry) identical run to run.
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

/// Build one tar carrying every entry, rooted at `/`.
///
/// Rooted at `/` rather than per-entry-parent on purpose: the archive endpoint extracts into a
/// directory that must ALREADY exist, and `/opt/keycloak/data/import` generally does not. Emitting
/// the parents as directory entries makes the tar self-sufficient, so one upload places everything
/// regardless of what the image happens to have laid down.
pub(super) fn tar_bytes(entries: &[FileEntry]) -> mlua::Result<Vec<u8>> {
    let mut builder = tar::Builder::new(Vec::new());
    let mut made: BTreeSet<String> = BTreeSet::new();

    for entry in entries {
        let rel = entry.path.trim_start_matches('/');
        ensure_parents(&mut builder, rel, &mut made)?;
        match &entry.source {
            FileSource::Text(text) => {
                let mut header = tar::Header::new_gnu();
                header.set_size(text.len() as u64);
                header.set_mode(entry.mode.unwrap_or(0o644));
                header.set_cksum();
                builder
                    .append_data(&mut header, rel, text.as_bytes())
                    .map_err(|e| tar_err(&entry.path, e))?;
            }
            FileSource::File(src) => {
                let bytes = std::fs::read(src).map_err(|e| tar_err(&entry.path, e))?;
                let mut header = tar::Header::new_gnu();
                header.set_size(bytes.len() as u64);
                // Default to the source file's own mode, so an executable stays executable
                // without the author having to say so.
                header.set_mode(entry.mode.unwrap_or_else(|| source_mode(src)));
                header.set_cksum();
                builder
                    .append_data(&mut header, rel, bytes.as_slice())
                    .map_err(|e| tar_err(&entry.path, e))?;
            }
            FileSource::Dir(src) => {
                builder
                    .append_dir_all(rel, src)
                    .map_err(|e| tar_err(&entry.path, e))?;
            }
        }
    }
    builder.into_inner().map_err(|e| {
        mlua::Error::RuntimeError(format!("docker.run `files`: building the archive: {e}"))
    })
}

/// Emit a directory entry for each missing ancestor of `rel`, once each.
fn ensure_parents(
    builder: &mut tar::Builder<Vec<u8>>,
    rel: &str,
    made: &mut BTreeSet<String>,
) -> mlua::Result<()> {
    let parts: Vec<&str> = rel.split('/').collect();
    let mut prefix = String::new();
    for part in &parts[..parts.len().saturating_sub(1)] {
        if part.is_empty() {
            continue;
        }
        prefix.push_str(part);
        prefix.push('/');
        if !made.insert(prefix.clone()) {
            continue;
        }
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Directory);
        header.set_size(0);
        header.set_mode(0o755);
        header.set_cksum();
        builder
            .append_data(&mut header, &prefix, std::io::empty())
            .map_err(|e| tar_err(&prefix, e))?;
    }
    Ok(())
}

#[cfg(unix)]
fn source_mode(p: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).map(|m| m.permissions().mode() & 0o7777).unwrap_or(0o644)
}

/// Windows has no unix mode to carry, and the container's filesystem does — so a file authored
/// there lands readable rather than with whatever a translation layer invented.
#[cfg(not(unix))]
fn source_mode(_p: &Path) -> u32 {
    0o644
}

fn tar_err(path: &str, e: impl std::fmt::Display) -> mlua::Error {
    mlua::Error::RuntimeError(format!("docker.run `files`: packing {path:?}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, source: FileSource, mode: Option<u32>) -> FileEntry {
        FileEntry { path: path.to_string(), source, mode }
    }

    /// Read the archive back as (path, mode, entry_type, contents).
    fn unpack(bytes: &[u8]) -> Vec<(String, u32, tar::EntryType, String)> {
        let mut archive = tar::Archive::new(bytes);
        let mut out = Vec::new();
        for e in archive.entries().unwrap() {
            let mut e = e.unwrap();
            let path = e.path().unwrap().to_string_lossy().to_string();
            let (mode, kind) = (e.header().mode().unwrap(), e.header().entry_type());
            let mut body = String::new();
            use std::io::Read;
            let _ = e.read_to_string(&mut body);
            out.push((path, mode, kind, body));
        }
        out
    }

    /// The archive endpoint extracts into a directory that must ALREADY exist, and
    /// `/opt/keycloak/data/import` does not. Without parent entries the upload silently places
    /// nothing — which is the failure this whole feature exists to avoid.
    #[test]
    fn parents_are_emitted_so_a_deep_path_needs_no_help_from_the_image() {
        let tar = tar_bytes(&[entry(
            "/opt/deep/nested/realm.json",
            FileSource::Text("{}".into()),
            None,
        )])
        .unwrap();
        let got = unpack(&tar);
        let dirs: Vec<&String> = got
            .iter()
            .filter(|(_, _, k, _)| *k == tar::EntryType::Directory)
            .map(|(p, _, _, _)| p)
            .collect();
        assert_eq!(dirs, vec!["opt/", "opt/deep/", "opt/deep/nested/"], "every ancestor: {got:?}");
        assert!(got.iter().any(|(p, _, _, b)| p == "opt/deep/nested/realm.json" && b == "{}"));
    }

    /// Each ancestor exactly once, however many entries share it — a duplicate directory header is
    /// accepted by some extractors and rejected by others, so "it worked here" would not travel.
    #[test]
    fn a_shared_parent_is_emitted_once() {
        let tar = tar_bytes(&[
            entry("/etc/app/a.conf", FileSource::Text("a".into()), None),
            entry("/etc/app/b.conf", FileSource::Text("b".into()), None),
        ])
        .unwrap();
        let dirs = unpack(&tar)
            .into_iter()
            .filter(|(_, _, k, _)| *k == tar::EntryType::Directory)
            .map(|(p, _, _, _)| p)
            .collect::<Vec<_>>();
        assert_eq!(dirs, vec!["etc/", "etc/app/"], "no duplicate ancestors: {dirs:?}");
    }

    /// An explicit mode is what makes a carried-in script runnable; the default must NOT be
    /// executable, or every config file would land with a bit it has no business having.
    #[test]
    fn mode_defaults_to_read_only_and_an_explicit_one_wins() {
        let tar = tar_bytes(&[
            entry("/a/plain.txt", FileSource::Text("x".into()), None),
            entry("/a/hook.sh", FileSource::Text("#!/bin/sh\n".into()), Some(0o755)),
        ])
        .unwrap();
        let got = unpack(&tar);
        let mode = |name: &str| {
            got.iter().find(|(p, _, _, _)| p == name).map(|(_, m, _, _)| *m).unwrap()
        };
        assert_eq!(mode("a/plain.txt"), 0o644, "a plain file is not executable");
        assert_eq!(mode("a/hook.sh"), 0o755, "an explicit mode is honored");
    }

    /// Lua table order is unspecified, so without the sort the archive — and any failure naming an
    /// entry — would differ run to run for the same input.
    #[test]
    fn entries_are_packed_in_a_stable_order() {
        let mk = || {
            vec![
                entry("/z.conf", FileSource::Text("z".into()), None),
                entry("/a.conf", FileSource::Text("a".into()), None),
            ]
        };
        let mut sorted = mk();
        sorted.sort_by(|a, b| a.path.cmp(&b.path));
        let files: Vec<String> = unpack(&tar_bytes(&sorted).unwrap())
            .into_iter()
            .filter(|(_, _, k, _)| *k == tar::EntryType::Regular)
            .map(|(p, _, _, _)| p)
            .collect();
        assert_eq!(files, vec!["a.conf", "z.conf"]);
    }
}
