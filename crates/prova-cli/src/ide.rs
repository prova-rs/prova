//! `prova ide setup` — wire this project for editor support: install the shared LuaLS core stubs and
//! create/merge the project's `.luarc.json` pointer.
//!
//! This is the re-runnable IDE half that used to be welded into `prova init`. It stands alone because
//! it is a distinct, repeatable concern: regenerate the machine-local `.luarc.json` after a fresh
//! clone, or wire annotations into a project that was scaffolded some other way. `prova init` calls
//! the same [`wire`] helper as its finishing step, so the two never drift.
//!
//! ```text
//! prova ide setup                 # create-or-merge .luarc.json (default: manage = always)
//! prova ide setup --manage auto   # same, but an unmergeable (JSONC) file hints instead of erroring
//! prova ide setup --manage never  # install stubs only; leave .luarc.json to you
//! ```
//!
//! The core stubs land under the cache annotations dir keyed by prova's version and are shared by
//! every project on the machine; nothing per-project is written outside the repo except `.luarc.json`
//! itself. Plugin stubs are linked automatically on the next `prova` run that resolves them.

use std::process::ExitCode;

use prova_core::{SystemLayout, XdgSystemLayout};

use crate::annotations;
use crate::home::{self, Home};
use crate::manifest::Manage;

pub fn run(args: Vec<String>) -> ExitCode {
    // The only subcommand today is `setup`; accept it explicitly so the surface can grow.
    let mut it = args.into_iter().peekable();
    match it.peek().map(String::as_str) {
        Some("setup") => {
            it.next();
        }
        Some("-h") | Some("--help") | None => {
            print_help();
            return ExitCode::SUCCESS;
        }
        Some(other) => {
            eprintln!("prova ide: unknown subcommand {other:?} (expected: setup)");
            return ExitCode::from(2);
        }
    }

    let mut manage = Manage::Always;
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--manage" => {
                let v = it.next().unwrap_or_default();
                match Manage::parse(Some(v.as_str())) {
                    Ok(m) => manage = m,
                    Err(e) => {
                        eprintln!("prova ide setup: {e}");
                        return ExitCode::from(2);
                    }
                }
            }
            "-h" | "--help" => {
                print_help();
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("prova ide setup: unknown option {other:?}");
                return ExitCode::from(2);
            }
        }
    }

    let home = match home::find(std::path::Path::new(".")) {
        Ok(Some(h)) => h,
        Ok(None) => {
            eprintln!(
                "prova ide setup: no prova.toml found in this directory or any parent — run `prova init` first"
            );
            return ExitCode::from(2);
        }
        Err(e) => {
            eprintln!("prova ide setup: {e}");
            return ExitCode::from(2);
        }
    };

    let layout = match XdgSystemLayout::new() {
        Ok(l) => l,
        Err(err) => {
            eprintln!("prova ide setup: cannot locate cache directory: {err}");
            return ExitCode::from(2);
        }
    };

    match wire(&home, manage, &layout) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("prova ide setup: {err}");
            ExitCode::from(2)
        }
    }
}

/// Install the core stubs and reconcile `.luarc.json` per `manage`, printing a concise, honest
/// summary. Shared by the `ide setup` verb and by `prova init`'s finishing step so the two behaviors
/// are one. Plugin roots are left empty here — a plugin's stub is linked on the next `prova` run that
/// resolves it (the run path already syncs annotations).
pub fn wire(home: &Home, manage: Manage, layout: &dyn SystemLayout) -> Result<(), String> {
    let outcome = annotations::setup(
        home,
        &Default::default(),
        manage,
        layout,
        crate::PROVA_VERSION,
    )?;
    println!(
        "prova: core IDE annotations at {}",
        outcome.core_dir.display()
    );
    if outcome.luarc_created {
        println!("prova: wrote .luarc.json — open this project in your editor for completion");
        // The pointer holds absolute, machine-local paths, so it is not shareable and should not be
        // committed. prova won't edit the user's .gitignore — it says so once, here.
        //
        // ...unless the ignore file already covers it, which is the common case now: both init
        // archetypes ship a `.gitignore` with `/.luarc.json` in it. Telling someone to do a thing
        // that is already done is the kind of small wrongness that makes a tool feel careless, and
        // on a fresh `prova init` it was the only inaccurate line in the output.
        if !luarc_already_ignored(&home.dir) {
            println!("prova: note — .luarc.json holds machine-local paths; add it to .gitignore");
        }
    } else if outcome.luarc_updated {
        println!("prova: merged prova's annotation entries into .luarc.json");
    }
    if outcome.luarc_hint {
        println!(
            "prova: .luarc.json is not plain JSON — merge skipped; add prova's entries by hand, \
             or set [luals] manage = \"never\""
        );
    }
    println!(
        "prova: plugin annotations are linked automatically as you declare them and run `prova`"
    );
    Ok(())
}

fn print_help() {
    println!(
        "usage: prova ide setup [--manage auto|always|never]\n\
         \n\
         install the shared LuaLS core stubs and create/merge this project's .luarc.json so the\n\
         prova DSL and every declared plugin complete in your editor.\n\
         \n\
         --manage always  (default) create .luarc.json if absent, else merge prova's entries into it\n\
         --manage auto    same, but a file prova cannot parse (JSONC) gets a hint, not an error\n\
         --manage never   install stubs only; never touch .luarc.json"
    );
}

/// Whether the package's `.gitignore` already ignores `.luarc.json`, so the advisory note can stay
/// quiet. Deliberately shallow: it reads the ignore file at the package root and looks for the
/// entry, rather than evaluating gitignore semantics (negations, nested ignore files, the global
/// core.excludesFile). Getting this wrong in the permissive direction costs one redundant line of
/// advice; getting it wrong in the strict direction would mean suppressing a note someone needed.
/// So a miss must fall through to printing, and every branch here does.
fn luarc_already_ignored(root: &std::path::Path) -> bool {
    let Ok(text) = std::fs::read_to_string(root.join(".gitignore")) else {
        return false;
    };
    text.lines().any(|line| {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('!') {
            return false;
        }
        // The spellings that actually cover the root pointer. `.luarc.json` (unanchored, matches at
        // any depth) and `/.luarc.json` (anchored) are what the archetypes and hand-written ignore
        // files use; a bare `*.json` would too, but treating a broad glob as intent to ignore this
        // specific file is a guess, and the cost of guessing wrong is silence where advice was due.
        matches!(line, ".luarc.json" | "/.luarc.json" | "./.luarc.json")
    })
}

#[cfg(test)]
mod tests {
    use super::luarc_already_ignored;

    fn at(body: Option<&str>) -> tempdir::Dir {
        let d = tempdir::Dir::new();
        if let Some(b) = body {
            std::fs::write(d.path().join(".gitignore"), b).unwrap();
        }
        d
    }

    #[test]
    fn no_gitignore_means_the_note_is_still_useful() {
        assert!(!luarc_already_ignored(at(None).path()));
    }

    #[test]
    fn the_anchored_and_unanchored_spellings_both_count() {
        assert!(luarc_already_ignored(at(Some("/.luarc.json\n")).path()));
        assert!(luarc_already_ignored(at(Some(".DS_Store\n.luarc.json\n")).path()));
    }

    // A comment mentioning the file is not an ignore rule — the archetypes' own .gitignore has
    // exactly this shape (a comment paragraph above the entry), so a naive `contains` would have
    // reported ignored for a file that merely talks about it.
    #[test]
    fn a_comment_mentioning_it_does_not_count() {
        assert!(!luarc_already_ignored(
            at(Some("# .luarc.json is machine-local\n")).path()
        ));
    }

    // A negation re-includes it; suppressing the note there would be exactly backwards.
    #[test]
    fn a_negation_does_not_count() {
        assert!(!luarc_already_ignored(at(Some("!.luarc.json\n")).path()));
    }

    /// A throwaway directory, removed on drop.
    mod tempdir {
        use std::path::{Path, PathBuf};
        use std::sync::atomic::{AtomicU32, Ordering};

        static N: AtomicU32 = AtomicU32::new(0);

        pub struct Dir(PathBuf);
        impl Dir {
            pub fn new() -> Dir {
                let p = std::env::temp_dir().join(format!(
                    "prova-ide-{}-{}",
                    std::process::id(),
                    N.fetch_add(1, Ordering::Relaxed)
                ));
                std::fs::create_dir_all(&p).unwrap();
                Dir(p)
            }
            pub fn path(&self) -> &Path {
                &self.0
            }
        }
        impl Drop for Dir {
            fn drop(&mut self) {
                std::fs::remove_dir_all(&self.0).ok();
            }
        }
    }
}
