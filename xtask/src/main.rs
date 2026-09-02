//! Build automation for Prova — the same `cargo xtask <command>` front door archetect uses, so the
//! two projects drive the same way. Run `cargo xtask <command>` (wired by the `xtask` alias in
//! `.cargo/config.toml`).
//!
//! Note: there is deliberately **no `fmt` task**. Prova's tree is not `rustfmt`-clean — a blanket
//! `cargo fmt` churns ~17 unrelated files — so formatting is hand-matched to the surrounding style
//! rather than automated here.

use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

/// The binary crate the `prova` executable is built and installed from.
const BIN_CRATE: &str = "crates/prova-cli";
/// The cargo package name of that crate (for `--package`).
const BIN_PACKAGE: &str = "prova-cli";

#[derive(Parser)]
#[command(name = "xtask")]
#[command(about = "Build automation for Prova")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Install the `prova` binary to ~/.cargo/bin (refreshes the user-scoped MCP build)
    Install {
        /// Statically compile OpenSSL into the binary (portable across machines)
        #[arg(long = "static-openssl", visible_alias = "static-ssl", default_value_t = true)]
        openssl_static: bool,
    },

    /// Run prova with arguments (e.g. `cargo xtask run -- init --list`)
    Run {
        /// Arguments to pass to prova
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },



    /// Build the release binary
    Build,

    /// Check code without building
    Check,

    /// Sweep stale build artifacts from target/
    Sweep,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Join the suite's house rule for the commands whose WORK is cargo: install/build/check/
    // sweep hold the package "cargo" lock (the same flock prova's conducts and the [runner]
    // provision hold — .prova/var/locks/cargo.lock; the file is the contract, see
    // prova_core::locks). Scoped per command, deliberately: `xtask run` delegates to PROVA,
    // which takes its own locks — holding here would deadlock the child against its ancestor.
    // Known edge, accepted: the `cargo xtask` BOOTSTRAP compile runs before any of this code
    // can lock; it is tiny, and cargo's own target-dir lock serializes same-dir builds.
    let _cargo_lock = match cli.command {
        Commands::Run { .. } => None,
        _ => hold_cargo_lock(),
    };


    match cli.command {
        Commands::Install { openssl_static } => {
            sweep()?;
            if openssl_static {
                cargo_env(&["install", "--path", BIN_CRATE], &[("OPENSSL_STATIC", "1")])?;
            } else {
                cargo(&["install", "--path", BIN_CRATE])?;
            }
        }

        Commands::Run { args } => {
            let mut cmd_args = vec!["run", "--package", BIN_PACKAGE, "--"];
            cmd_args.extend(args.iter().map(|s| s.as_str()));
            cargo(&cmd_args)?;
        }

        Commands::Build => {
            sweep()?;
            cargo(&["build", "--release"])?;
        }

        Commands::Check => {
            sweep()?;
            cargo(&["check", "--workspace", "--all-targets"])?;
        }

        Commands::Sweep => {
            sweep()?;
        }
    }

    Ok(())
}

fn cargo(args: &[&str]) -> Result<()> {
    cargo_env(args, &[])
}

fn cargo_env(args: &[&str], env: &[(&str, &str)]) -> Result<()> {
    println!("cargo {}", args.join(" "));

    let mut command = Command::new("cargo");
    command.args(args);
    for (key, value) in env {
        command.env(key, value);
    }

    let status = command.status()?;
    if !status.success() {
        anyhow::bail!("cargo command failed with status: {}", status);
    }

    Ok(())
}

/// Sweep build artifacts older than 7 days. Installs cargo-sweep if missing.
fn sweep() -> Result<()> {
    ensure_cargo_sweep()?;
    println!("==> Sweeping stale artifacts (>7 days)...");
    let status = Command::new("cargo")
        .args(["sweep", "--time", "7"])
        .status()
        .context("failed to run cargo sweep")?;
    if !status.success() {
        eprintln!("    Warning: cargo sweep failed, continuing anyway");
    }
    Ok(())
}

/// Install cargo-sweep if it isn't already present.
fn ensure_cargo_sweep() -> Result<()> {
    let cargo_bin = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".cargo/bin/cargo-sweep");

    if cargo_bin.exists() {
        return Ok(());
    }

    println!("==> Installing cargo-sweep...");
    let status = Command::new("cargo")
        .args(["install", "cargo-sweep"])
        .status()
        .context("failed to install cargo-sweep")?;
    if !status.success() {
        anyhow::bail!("cargo install cargo-sweep failed (exit {})", status);
    }
    Ok(())
}

/// A held package "cargo" lock, plus the holder record that names who is holding it. Dropping
/// releases the flock (kernel) and removes the record (here) — record first, so the file never
/// outlives the hold it describes.
struct CargoLock {
    _file: std::fs::File,
    record: Option<std::path::PathBuf>,
}

impl Drop for CargoLock {
    fn drop(&mut self) {
        if let Some(path) = self.record.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Block until this package's "cargo" lock is held (see prova_core::locks — the flock file is
/// the cross-tool contract). Failure degrades visibly to unlocked, never silently blocks work.
fn hold_cargo_lock() -> Option<CargoLock> {
    let path = std::path::Path::new(".prova/var/locks/cargo.lock");
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let file = match std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("xtask: cargo lock unavailable ({e}) — proceeding without the cross-instance hold");
            return None;
        }
    };
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        // LOCK_NB first so a held lock is announced rather than silently waited on.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            println!("==> Waiting for the package cargo lock (held by a prova conduct or another xtask)...");
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
                eprintln!("xtask: flock failed — proceeding without the cross-instance hold");
                return None;
            }
        }
    }
    Some(CargoLock { record: register_holder(path), _file: file })
}

/// Write the holder record beside the lock, by hand.
///
/// xtask deliberately does not depend on prova-core — it is the bootstrap tool, and pulling the
/// engine in would mean compiling it before you can build anything. Joining the record convention
/// with a dozen lines of hand-written JSON is the point rather than a compromise: the format is
/// part of the same public contract the flock file is, and xtask is this repo's most frequent
/// non-prova holder, so a waiter behind `cargo xtask build` should read "xtask build" instead of
/// "an unregistered holder" (docs/plans/lock-starvation.md).
fn register_holder(lock_path: &std::path::Path) -> Option<std::path::PathBuf> {
    let dir = lock_path.with_extension("holders");
    std::fs::create_dir_all(&dir).ok()?;
    let pid = std::process::id();
    let package = std::env::current_dir().ok()?.display().to_string();
    let what = std::env::args().collect::<Vec<_>>().join(" ");
    let acquired_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let json = format!(
        r#"{{"pid":{pid},"token":"cargo","shared":false,"package":{},"what":{},"acquired_at":{acquired_at}}}"#,
        quote(&package),
        quote(&what),
    );
    let path = dir.join(format!("{pid}.json"));
    std::fs::write(&path, json).ok()?;
    Some(path)
}

/// The JSON string rules, in full: escape the two structural characters and the control range.
/// A path or an argument is user-supplied text, so "it will never contain a quote" is the kind of
/// assumption that produces an unparseable record exactly once, in someone else's tree.
fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
