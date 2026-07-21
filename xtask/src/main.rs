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

    /// Run all tests across the workspace
    Test,

    /// Run tests for a specific crate (e.g. prova-core, prova-cli)
    TestCrate {
        /// Crate name
        name: String,
    },

    /// Build the release binary
    Build,

    /// Check code without building
    Check,

    /// Run clippy lints (deny warnings)
    Clippy,

    /// Sweep stale build artifacts from target/
    Sweep,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

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

        Commands::Test => {
            sweep()?;
            cargo(&["test", "--workspace"])?;
        }

        Commands::TestCrate { name } => {
            sweep()?;
            cargo(&["test", "-p", &name])?;
        }

        Commands::Build => {
            sweep()?;
            cargo(&["build", "--release"])?;
        }

        Commands::Check => {
            sweep()?;
            cargo(&["check", "--workspace", "--all-targets"])?;
        }

        Commands::Clippy => {
            sweep()?;
            cargo(&["clippy", "--workspace", "--all-targets", "--all-features", "--", "-D", "warnings"])?;
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
