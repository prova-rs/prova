//! `prova init` — scaffold a prova project: a `prova.toml` manifest, its home directory, and (unless
//! opted out) the LuaLS IDE integration (a root `.luarc.json` pointing at the shared core stubs).
//!
//! ```text
//! prova init                 # home in ./prova/ (visible — tests + config in one dir)
//! prova init --hidden        # home in ./.prova/ (tucked away)
//! prova init --flat          # manifest at ./prova.toml (no nesting)
//! prova init --no-luals      # skip IDE wiring (sets [luals] manage = "never")
//! ```
//!
//! `init` refuses to run if the project already has a manifest — it never clobbers an existing
//! layout. IDE annotations are created immediately (core stubs); each plugin's stub is added on the
//! first `prova` run that resolves it.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::annotations;
use crate::home::Home;

/// Where the manifest goes, relative to the project root.
enum Layout {
    /// `./prova/prova.toml` (default).
    Visible,
    /// `./.prova/prova.toml`.
    Hidden,
    /// `./prova.toml`.
    Flat,
}

impl Layout {
    /// The home directory for this layout under `root`.
    fn home_dir(&self, root: &Path) -> PathBuf {
        match self {
            Layout::Visible => root.join("prova"),
            Layout::Hidden => root.join(".prova"),
            Layout::Flat => root.to_path_buf(),
        }
    }
}

pub fn run(args: Vec<String>) -> ExitCode {
    let mut layout = Layout::Visible;
    let mut luals = true;
    for arg in &args {
        match arg.as_str() {
            "--hidden" => layout = Layout::Hidden,
            "--flat" => layout = Layout::Flat,
            "--no-luals" => luals = false,
            "-h" | "--help" => {
                println!(
                    "usage: prova init [--hidden | --flat] [--no-luals]\n\
                     \n\
                     scaffold a prova.toml manifest and (unless --no-luals) LuaLS IDE support.\n\
                     default home is ./prova/ ; --hidden uses ./.prova/ ; --flat uses ./prova.toml"
                );
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("prova init: unknown option {other:?}");
                return ExitCode::from(2);
            }
        }
    }

    let root = PathBuf::from(".");

    // Refuse to clobber: any of the three manifest locations already present means initialized.
    for existing in ["prova.toml", "prova/prova.toml", ".prova/prova.toml"] {
        if root.join(existing).is_file() {
            eprintln!("prova init: already initialized ({existing} exists)");
            return ExitCode::from(2);
        }
    }

    let home_dir = layout.home_dir(&root);
    if let Err(e) = std::fs::create_dir_all(&home_dir) {
        eprintln!("prova init: cannot create {}: {e}", home_dir.display());
        return ExitCode::from(2);
    }

    let manifest = home_dir.join("prova.toml");
    if let Err(e) = std::fs::write(&manifest, manifest_template(luals)) {
        eprintln!("prova init: cannot write {}: {e}", manifest.display());
        return ExitCode::from(2);
    }

    let home = Home {
        root: root.clone(),
        dir: home_dir.clone(),
        manifest: manifest.clone(),
    };

    println!("prova: wrote {}", manifest.display());

    if luals {
        let sys_layout = match prova_core::XdgSystemLayout::new() {
            Ok(l) => l,
            Err(err) => {
                eprintln!("prova init: cannot locate cache directory: {err}");
                return ExitCode::from(2);
            }
        };
        match annotations::init(
            &home,
            &Default::default(),
            &sys_layout,
            crate::PROVA_VERSION,
        ) {
            Ok(outcome) => {
                println!(
                    "prova: core IDE annotations at {}",
                    outcome.core_dir.display()
                );
                if outcome.luarc_created {
                    println!("prova: wrote .luarc.json — open this project in your editor for completion");
                    // The pointer is an absolute, machine-local path, so it is not shareable and
                    // should not be committed. prova won't edit the user's .gitignore for them —
                    // it says so once, here, and leaves the choice where it belongs.
                    println!(
                        "prova: note — .luarc.json holds machine-local paths; add it to .gitignore"
                    );
                }
                println!("prova: plugin annotations are added automatically as you declare them and run `prova`");
            }
            Err(err) => {
                eprintln!("prova init: IDE annotations: {err}");
                return ExitCode::from(2);
            }
        }
    }

    println!(
        "\nnext: add a test at {}/example_test.lua and run `prova`",
        home.dir.display()
    );
    ExitCode::SUCCESS
}

/// The default `prova.toml`. `paths = ["."]` discovers any `*_test.lua` under the home dir, so a test
/// dropped anywhere in it just runs; organize into `suites/` subdirs when a project grows.
fn manifest_template(luals: bool) -> String {
    let mut s = String::from(
        "# prova test suite manifest. Run `prova` from anywhere in this project.\n\
         # Docs: https://github.com/prova-rs/prova\n\
         \n\
         [run]\n\
         paths = [\".\"]          # discover *_test.lua here (organize into suites/ as you grow)\n\
         \n\
         # Declare plugins for real resources; `require(\"<name>\")` then works in tests, and its\n\
         # editor completions appear automatically after the next run.\n\
         # [plugins]\n\
         # postgres = \"prova-rs/prova-postgres@v0.2.0\"\n",
    );
    if !luals {
        s.push_str(
            "\n[luals]\n\
             manage = \"never\"    # prova will not create or edit .luarc.json\n",
        );
    }
    s
}
