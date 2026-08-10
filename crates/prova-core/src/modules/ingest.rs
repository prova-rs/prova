//! The verifier ingestion seam, shared by every deputed-verifier module (`junit`, `sarif`):
//! pattern → files expansion and the report's file-provenance rows. A third verifier should
//! cost a parser, not another copy of this plumbing (docs/design/verifiers.md).

use std::path::{Path, PathBuf};

use mlua::{Lua, Table};

/// Expand `pattern` (a literal path or a glob, resolved against `cwd` when relative) to files.
/// `who` names the calling verb in errors (`junit.load`, `sarif.load`).
pub(super) fn resolve_files(
    pattern: &str,
    cwd: Option<&str>,
    who: &str,
) -> Result<Vec<PathBuf>, String> {
    let full = match cwd {
        Some(base) if !Path::new(pattern).is_absolute() => {
            format!("{}/{}", base.trim_end_matches('/'), pattern)
        }
        _ => pattern.to_string(),
    };
    if !full.contains(['*', '?', '[']) {
        let p = PathBuf::from(&full);
        return Ok(if p.is_file() { vec![p] } else { Vec::new() });
    }
    let mut files: Vec<PathBuf> = glob::glob(&full)
        .map_err(|e| format!("{who}: bad glob {full:?}: {e}"))?
        .filter_map(Result::ok)
        .filter(|p| p.is_file())
        .collect();
    files.sort();
    Ok(files)
}

/// The report's `files` rows — path + mtime provenance, so a stale ingested report is detectable
/// (the record carries these, and `attest` reads them).
pub(super) fn files_table(lua: &Lua, files: &[PathBuf]) -> mlua::Result<Table> {
    let files_t = lua.create_table()?;
    for (i, f) in files.iter().enumerate() {
        let row = lua.create_table()?;
        row.set("path", f.to_string_lossy())?;
        let mtime = std::fs::metadata(f)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        row.set("mtime", mtime)?;
        files_t.set(i + 1, row)?;
    }
    Ok(files_t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::make_tempdir;

    /// A literal path answers itself when the file exists and empty when it does not — never an
    /// error, because "no results yet" is the verifier recipe's own vacuity check to make loud.
    #[test]
    fn literal_paths_answer_existence() {
        let dir = make_tempdir().unwrap();
        let file = dir.join("results.xml");
        std::fs::write(&file, "<x/>").unwrap();
        let hits = resolve_files(&file.to_string_lossy(), None, "junit.load").unwrap();
        assert_eq!(hits, vec![file]);
        let misses = resolve_files(&dir.join("absent.xml").to_string_lossy(), None, "junit.load");
        assert_eq!(misses.unwrap(), Vec::<std::path::PathBuf>::new());
    }

    /// A relative pattern resolves against `cwd`; globs expand sorted, files only.
    #[test]
    fn globs_resolve_against_cwd_sorted() {
        let dir = make_tempdir().unwrap();
        std::fs::write(dir.join("b.xml"), "").unwrap();
        std::fs::write(dir.join("a.xml"), "").unwrap();
        std::fs::create_dir(dir.join("c.xml")).unwrap(); // a DIRECTORY the glob must skip
        let hits = resolve_files("*.xml", Some(&dir.to_string_lossy()), "sarif.load").unwrap();
        assert_eq!(hits, vec![dir.join("a.xml"), dir.join("b.xml")]);
    }

    /// A malformed glob is the caller's error, named after the verb that asked.
    #[test]
    fn bad_glob_names_the_verb()  {
        let err = resolve_files("[", None, "junit.load").unwrap_err();
        assert!(err.contains("junit.load"), "error names the verb: {err}");
    }
}
