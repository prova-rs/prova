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
//! prova ide setup --manage auto   # polite: create if absent, else hint (don't edit a file you own)
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
        println!("prova: note — .luarc.json holds machine-local paths; add it to .gitignore");
    }
    if outcome.luarc_hint {
        println!(
            "prova: .luarc.json exists and is yours — run `prova ide setup --manage always` to merge prova's entries"
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
         --manage auto    create if absent; if you already own one, leave it and print a hint\n\
         --manage never   install stubs only; never touch .luarc.json"
    );
}
