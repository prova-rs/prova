//! `prova init` — scaffold a prova project by **rendering a catalog archetype** into the current
//! directory, then wiring LuaLS IDE support as a finishing step.
//!
//! ```text
//! prova init                 # render the `default` entry (interactive select among entries is M6)
//! prova init --list          # the catalog: which archetypes prova can scaffold from
//! prova init <key>           # render the named catalog entry
//! prova init <key> --answer name=value --switch ci   # feed the render (repeatable)
//! prova init <key> --defaults        # take each prompt's default instead of asking
//! prova init <key> --headless        # never prompt; an unanswerable prompt is an error, not a hang
//! prova init <key> --no-luals        # skip the IDE-wiring finishing step
//! ```
//!
//! The scaffold is selected from a [catalog](crate::catalog) — prova's built-in entries plus any
//! `[init.*]` in `~/.config/prova/config.toml`. The catalog and the target key are resolved *before*
//! anything touches the filesystem, so a typo'd key or a broken config never leaves a half-scaffolded
//! project behind. `init` refuses to run if the project already has a manifest — it never clobbers an
//! existing layout.
//!
//! Answer precedence (highest first): CLI `--answer` → the entry's baked `answers` → an interactive
//! prompt (unless `--headless`) → the archetype's own default (via `--defaults`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// The catalog key used when none is given on the command line.
const DEFAULT_KEY: &str = "default";

pub fn run(args: Vec<String>) -> ExitCode {
    let mut luals = true;
    let mut list = false;
    let mut headless = false;
    let mut defaults = false;
    let mut key: Option<String> = None;
    let mut cli_answers: Vec<(String, String)> = Vec::new();
    let mut cli_switches: Vec<String> = Vec::new();

    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--no-luals" | "--no-ide" => luals = false,
            "--list" => list = true,
            "--headless" => headless = true,
            "--defaults" => defaults = true,
            "--answer" | "-a" => {
                let Some(pair) = it.next() else {
                    eprintln!("prova init: --answer expects key=value");
                    return ExitCode::from(2);
                };
                match pair.split_once('=') {
                    Some((k, v)) => cli_answers.push((k.to_string(), v.to_string())),
                    None => {
                        eprintln!("prova init: --answer expects key=value, got {pair:?}");
                        return ExitCode::from(2);
                    }
                }
            }
            "--switch" | "-s" => {
                let Some(name) = it.next() else {
                    eprintln!("prova init: --switch expects a name");
                    return ExitCode::from(2);
                };
                cli_switches.push(name);
            }
            "-h" | "--help" => {
                println!(
                    "usage: prova init [<key>] [--list] [--answer k=v]... [--switch name]... \
                     [--defaults] [--headless] [--no-luals]\n\
                     \n\
                     render a catalog archetype into the current directory, then wire LuaLS IDE\n\
                     support. <key> names a catalog entry (see `prova init --list`); omit it for the\n\
                     default. --headless never prompts (an unanswered, undefaulted prompt is an\n\
                     error); --defaults takes each prompt's default; --no-luals skips IDE wiring."
                );
                return ExitCode::SUCCESS;
            }
            other if other.starts_with('-') => {
                eprintln!("prova init: unknown option {other:?}");
                return ExitCode::from(2);
            }
            other => {
                if let Some(prior) = &key {
                    eprintln!("prova init: expected one catalog key, got {prior:?} and {other:?}");
                    return ExitCode::from(2);
                }
                key = Some(other.to_string());
            }
        }
    }

    // Resolve the catalog and target entry before any filesystem work — a bad key or a malformed
    // config.toml fails before a half-scaffolded project can exist.
    let sys_layout = match prova_core::XdgSystemLayout::new() {
        Ok(l) => l,
        Err(err) => {
            eprintln!("prova init: cannot locate config directory: {err}");
            return ExitCode::from(2);
        }
    };
    let catalog = match crate::catalog::Catalog::load(&sys_layout) {
        Ok(c) => c,
        Err(err) => {
            eprintln!("prova init: {err}");
            return ExitCode::from(2);
        }
    };
    if list {
        catalog.print_list();
        return ExitCode::SUCCESS;
    }

    // No key means the default entry. (Interactive selection among several entries is M6.)
    let key = key.unwrap_or_else(|| DEFAULT_KEY.to_string());
    let entry = match catalog.get(&key) {
        Ok(e) => e,
        Err(err) => {
            eprintln!("prova init: {err}");
            return ExitCode::from(2);
        }
    };

    // Refuse to clobber: any known manifest location already present means this project is initialized.
    let root = Path::new(".");
    for existing in [
        "prova.toml",
        ".prova.toml",
        "prova/prova.toml",
        ".prova/prova.toml",
    ] {
        if root.join(existing).is_file() {
            eprintln!("prova init: already initialized ({existing} exists)");
            return ExitCode::from(2);
        }
    }

    // Answers: baked entry answers first, CLI `--answer` overrides. Switches: entry ∪ CLI. Defaults:
    // either the entry opts in or `--defaults` is passed.
    let mut merged: BTreeMap<String, String> = entry.answers.clone();
    for (k, v) in cli_answers {
        merged.insert(k, v);
    }
    let mut switches = entry.switches.clone();
    for s in cli_switches {
        if !switches.contains(&s) {
            switches.push(s);
        }
    }
    let use_defaults = entry.defaults || defaults;

    let destination = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let answers = prova_archetect::string_answers(merged);
    println!("prova: rendering {key:?} from {}", entry.source);
    if let Err(err) = prova_archetect::render_interactive(
        &entry.source,
        &destination,
        answers,
        switches,
        use_defaults,
        headless,
    ) {
        eprintln!("prova init: render failed: {err}");
        return ExitCode::from(2);
    }

    // IDE wiring, as a finishing step, over whatever manifest the archetype rendered. A render that
    // produced no prova.toml isn't a prova project layout — say so rather than fail.
    if luals {
        match crate::home::find(root) {
            Ok(Some(home)) => {
                if let Err(err) =
                    crate::ide::wire(&home, crate::manifest::Manage::Always, &sys_layout)
                {
                    eprintln!("prova init: IDE annotations: {err}");
                    return ExitCode::from(2);
                }
            }
            Ok(None) => {
                println!(
                    "prova: no prova.toml was rendered — skipping IDE wiring (run `prova ide setup` later)"
                );
            }
            Err(err) => {
                eprintln!("prova init: {err}");
                return ExitCode::from(2);
            }
        }
    }

    println!("\nprova: initialized. Run `prova` to execute the suite.");
    ExitCode::SUCCESS
}
