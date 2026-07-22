//! `prova init` — scaffold a prova package by **rendering a catalog archetype** into the current
//! directory, then wiring LuaLS IDE support as a finishing step.
//!
//! ```text
//! prova init                 # interactive select among catalog entries (default pre-highlighted)
//! prova init --list          # the catalog: which archetypes prova can scaffold from
//! prova init <key>           # render the named catalog entry
//! prova init <key> --answer name=value --switch ci   # feed the render (repeatable)
//! prova init <key> --defaults        # take each prompt's default instead of asking
//! prova init <key> --headless        # never prompt; an unanswerable prompt is an error, not a hang
//! prova init <key> --no-ide          # skip the IDE-wiring finishing step (alias: --no-luals)
//! ```
//!
//! The scaffold is selected from a [catalog](crate::catalog) — prova's built-in entries plus any
//! `[init.*]` in `~/.config/prova/config.toml`. The catalog and the target key are resolved *before*
//! anything touches the filesystem, so a typo'd key or a broken config never leaves a half-scaffolded
//! package behind. `init` refuses to run if the package already has a manifest — it never clobbers an
//! existing layout — unless the entry declares `in_package = "allow"` (it augments a package rather
//! than creating one). Every render also receives generic package-state (an in-package switch, the
//! package root, `plugin_root`) so any archetype can adapt to where it is running; see the catalog
//! module docs.
//!
//! Answer precedence (highest first): CLI `--answer` → the entry's baked `answers` → an interactive
//! prompt (unless `--headless`) → the archetype's own default (via `--defaults`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// The catalog key the interactive picker pre-highlights.
const DEFAULT_KEY: &str = "project";

/// Package state discovered before a render — what `init` knows about WHERE it is running, injected
/// generically into every archetype (see the catalog module docs on state injection).
struct PackageState {
    /// The package root, relative to the cwd (`.` when they coincide).
    package_root: String,
    /// The manifest's `[run] plugin_root`, verbatim (package-root relative), when declared.
    plugin_root: Option<String>,
}

/// Discover the enclosing package, if any, walking up from the cwd exactly like `prova` itself.
/// A manifest that exists but fails to parse still counts as "in a package" (the switch and root are
/// facts); only its `plugin_root` is unknowable — warn and carry on rather than fail the init.
fn package_state() -> Option<PackageState> {
    let cwd = std::env::current_dir().ok()?;
    let home = crate::home::find(&cwd).ok().flatten()?;
    // `home.dir` is canonicalized by discovery (on Windows that's the `\\?\` verbatim form, on macOS
    // symlinked temp dirs resolve) — canonicalize the cwd the same way or strip_prefix can't match.
    let cwd = cwd.canonicalize().unwrap_or(cwd);
    let package_root = match cwd.strip_prefix(&home.dir) {
        Ok(rel) => {
            let depth = rel.components().count();
            if depth == 0 {
                ".".to_string()
            } else {
                vec![".."; depth].join("/")
            }
        }
        Err(_) => home.dir.display().to_string(), // unrelated roots (symlinks) — absolute is still true
    };
    let plugin_root = match std::fs::read_to_string(&home.manifest)
        .map_err(|e| e.to_string())
        .and_then(|text| crate::manifest::Manifest::parse(&text))
        .and_then(|m| m.resolve(None))
    {
        Ok(resolved) => resolved.plugin_root,
        Err(err) => {
            eprintln!(
                "prova init: note — could not read {}: {err} (rendering without `prova_plugin_root`)",
                home.manifest.display()
            );
            None
        }
    };
    Some(PackageState {
        package_root,
        plugin_root,
    })
}

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
                     [--defaults] [--headless] [--no-ide]\n\
                     \n\
                     render a catalog archetype into the current directory, then wire LuaLS IDE\n\
                     support. <key> names a catalog entry (see `prova init --list`); omit it to\n\
                     choose interactively. --headless never prompts (an unanswered, undefaulted\n\
                     prompt is an error); --defaults takes each prompt's default; --no-ide skips\n\
                     IDE wiring."
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
    // config.toml fails before a half-scaffolded package can exist.
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

    // No key means "choose interactively" — prova always presents the catalog (which always contains
    // at least `default`), rather than silently picking for you. Without a terminal to prompt on,
    // that's an error, not a hang.
    let key = match key {
        Some(k) => k,
        None => match select_key(&catalog) {
            Ok(k) => k,
            Err(code) => return code,
        },
    };
    let entry = match catalog.get(&key) {
        Ok(e) => e,
        Err(err) => {
            eprintln!("prova init: {err}");
            return ExitCode::from(2);
        }
    };

    // Refuse to clobber — unless the entry declares it AUGMENTS an initialized package
    // (`in_package = "allow"`), in which case the archetype decides what to write.
    let root = Path::new(".");
    if entry.in_package == crate::catalog::InPackage::Deny {
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
    }

    // Package-state injection: tell the archetype where it is running. Lowest precedence — the
    // entry's baked answers/switches and the CLI both win, so an entry can override the facts.
    let state = package_state();
    let mut merged: BTreeMap<String, String> = BTreeMap::new();
    if let Some(state) = &state {
        merged.insert("prova_package_root".to_string(), state.package_root.clone());
        if let Some(plugin_root) = &state.plugin_root {
            merged.insert("prova_plugin_root".to_string(), plugin_root.clone());
        }
    }

    // Answers: baked entry answers over the injected state, CLI `--answer` over both. Switches:
    // state ∪ entry ∪ CLI. Defaults: either the entry opts in or `--defaults` is passed.
    merged.extend(entry.answers.clone());
    for (k, v) in cli_answers {
        merged.insert(k, v);
    }
    let mut switches = Vec::new();
    if state.is_some() {
        switches.push("prova:in-package".to_string());
    }
    for s in entry.switches.iter().cloned().chain(cli_switches) {
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
    // produced no prova.toml isn't a prova package layout — say so rather than fail.
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

    if state.is_some() && entry.in_package == crate::catalog::InPackage::Allow {
        println!("\nprova: rendered {key:?} into the existing package. Run `prova` to execute the suite.");
    } else {
        println!("\nprova: initialized. Run `prova` to execute the suite.");
    }
    ExitCode::SUCCESS
}

/// Present the catalog interactively and return the chosen key. A keyless `prova init` always offers
/// the catalog (which always contains at least `default`) rather than choosing silently — but that
/// needs a terminal to prompt on. In a non-interactive context (CI, a pipe) it's a usage error that
/// names the alternatives, never a prompt that hangs.
fn select_key(catalog: &crate::catalog::Catalog) -> Result<String, ExitCode> {
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() {
        eprintln!(
            "prova init: no archetype given, and stdin is not a terminal to choose from — pass a \
             key (see `prova init --list`), or run in an interactive terminal"
        );
        return Err(ExitCode::from(2));
    }

    let choices: Vec<Choice> = catalog
        .entries
        .iter()
        .map(|(key, entry)| Choice {
            key: key.clone(),
            description: entry.description.clone(),
        })
        .collect();
    // Start the cursor on `default` when it's present, so Enter takes the common path.
    let start = choices
        .iter()
        .position(|c| c.key == DEFAULT_KEY)
        .unwrap_or(0);

    match inquire::Select::new("Select a prova init archetype:", choices)
        .with_starting_cursor(start)
        .prompt()
    {
        Ok(choice) => Ok(choice.key),
        Err(
            inquire::InquireError::OperationCanceled | inquire::InquireError::OperationInterrupted,
        ) => {
            eprintln!("prova init: cancelled");
            Err(ExitCode::from(130))
        }
        Err(err) => {
            eprintln!("prova init: selection failed: {err}");
            Err(ExitCode::from(2))
        }
    }
}

/// One row in the interactive picker: `key  —  description`.
struct Choice {
    key: String,
    description: String,
}

impl std::fmt::Display for Choice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}  —  {}", self.key, self.description)
    }
}
