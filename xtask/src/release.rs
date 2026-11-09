//! Prova's side of the publish contract every product repository answers the same way (artichoke `docs/specs/RELEASE.md`
//! §4), which Herald runs after a green gate:
//!
//!   cargo xtask release --out <dir> [--sign local|team|<identity>] [--commit <sha>]
//!       build the release binary named for the commit, sign it with the product's identifier, check
//!       the signature, and write <dir>/release.json; Herald then `artichoke release`s it
//!
//! `--commit` is how Herald names the commit it checked out; without it, a person's jj working copy
//! must be clean and its parent is the commit. `--sign team` signs with team P9DJA3JCKP's Apple
//! Development identity (the `anemnez-signing` keychain, else the login keychain's: `team.rs`, fabric
//! LIFECYCLE §7a option A); `local` with this machine's COS Local Signing.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{run_command as run, workspace_root};

/// The cargo package that holds the binary.
const PACKAGE: &str = "prova-cli";
/// The repository's name, as Herald knows it.
const REPO: &str = "prova";
/// What one release publishes: the CLI `prova`, a tool Fabric places at `~/.anemnez/bin/prova`
/// (fabric TOOLBOX.md), built and team-signed by Herald on our own hosts (D, 2026-11-09).
const PRODUCTS: [(&str, &str); 1] = [("prova", "cli")];

const TEAM_ID: &str = "P9DJA3JCKP";
const SIGNING_KEYCHAIN: &str = "anemnez-signing.keychain-db";
const LOCAL_IDENTITY: &str = "COS Local Signing";

/// `cargo xtask release`.
pub fn release(args: &[String]) -> Result<(), String> {
    let mut out = None;
    let mut sign = "local".to_string();
    let mut commit = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--out" => out = Some(PathBuf::from(it.next().ok_or("--out takes a directory\n")?)),
            "--sign" => sign = it.next().ok_or("--sign takes local, team or an identity\n")?.clone(),
            "--commit" => commit = Some(it.next().ok_or("--commit takes a commit id\n")?.clone()),
            other => return Err(format!("unknown argument {other:?}\n")),
        }
    }
    let out = out.ok_or("say where: --out <dir>\n")?;
    build_release(&out, &sign, commit)
}

fn build_release(out: &Path, sign: &str, commit: Option<String>) -> Result<(), String> {
    let root = workspace_root();
    let commit = match commit {
        Some(c) if c.len() >= 12 && c.bytes().all(|b| b.is_ascii_hexdigit()) => c,
        Some(c) => return Err(format!("--commit {c:?} is not a commit id\n")),
        None => clean_commit(&root)?,
    };
    let short = &commit[..12];
    let version = env!("CARGO_PKG_VERSION");
    let build = format!("{version}+{short}");
    let mut cargo = Command::new("cargo");
    cargo.args(["build", "--release", "--locked", "--package", PACKAGE]);
    for (product, _) in PRODUCTS {
        cargo.args(["--bin", product]);
    }
    // OpenSSL compiled in, as `cargo xtask install` builds it: the binary runs on any host.
    run(cargo.env("PRODUCT_COMMIT", short).env("PRODUCT_DIRTY", "false").env("OPENSSL_STATIC", "1").current_dir(&root))?;
    let target_dir = std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| root.join("target"), PathBuf::from);
    std::fs::create_dir_all(out).map_err(|e| format!("{}: {e}\n", out.display()))?;
    let mut staged = Vec::new();
    for (product, _) in PRODUCTS {
        let built = target_dir.join("release").join(product);
        let at = out.join(product);
        std::fs::copy(&built, &at).map_err(|e| format!("{}: {e}\n", built.display()))?;
        staged.push((at, product));
    }
    let requirements = sign_all(sign, &staged)?;
    let triple = host_triple()?;
    let mut entries = Vec::new();
    for (((at, product), requirement), (_, kind)) in staged.iter().zip(&requirements).zip(PRODUCTS) {
        run(Command::new("codesign").args(["--verify", "--strict", &format!("-R={requirement}")]).arg(at))?;
        // A tool is smoke-checked before Fabric places it (fabric TOOLBOX.md §6): so here first.
        if kind == "cli" {
            run(Command::new(at).arg("--version"))?;
        }
        let tool =
            if kind == "cli" { format!(", \"kind\": \"cli\", \"commands\": [\"{product}\"]") } else { String::new() };
        entries.push(format!(
            "    {{ \"product\": \"{product}\", \"build\": \"{build}\", \"target\": \"{triple}\", \"file\": \"{product}\", \"identifier\": \"ai.anemnez.{product}\"{tool} }}"
        ));
    }
    let lock = sha256_of(&root.join("Cargo.lock"))?;
    // Hand-written JSON: xtask stays dependency-free. Every value here is plain ASCII.
    let manifest = format!(
        "{{\n  \"repo\": \"{REPO}\",\n  \"commit\": \"{commit}\",\n  \"cargo_lock_sha256\": \"{lock}\",\n  \"products\": [\n{}\n  ]\n}}\n",
        entries.join(",\n")
    );
    std::fs::write(out.join("release.json"), manifest).map_err(|e| format!("release.json: {e}\n"))?;
    eprintln!("→ released prova {build} for {triple} into {}", out.display());
    Ok(())
}

fn sha256_of(path: &Path) -> Result<String, String> {
    let out = output(Command::new("shasum").args(["-a", "256"]).arg(path))?;
    out.split_whitespace().next().map(str::to_owned).ok_or_else(|| format!("no digest for {}\n", path.display()))
}

/// The commit a clean working copy names: the parent of an empty `@`. Anything uncommitted refuses,
/// because a dirty build is never published.
fn clean_commit(root: &Path) -> Result<String, String> {
    let changed = output(Command::new("jj").args(["diff", "-r", "@", "--summary"]).current_dir(root))?;
    if !changed.trim().is_empty() {
        return Err(format!(
            "the working copy has changes; commit them first (a dirty build is never published):\n{changed}"
        ));
    }
    let commit =
        output(Command::new("jj").args(["log", "-r", "@-", "--no-graph", "-T", "commit_id"]).current_dir(root))?;
    let commit = commit.trim().to_string();
    if commit.len() != 40 || !commit.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("jj named the commit {commit:?}, not 40 hex digits\n"));
    }
    Ok(commit)
}

/// Sign each `(binary, product)` per `sign`, under one keychain unlock; answers the requirement each
/// must now satisfy, in order.
fn sign_all(sign: &str, binaries: &[(PathBuf, &str)]) -> Result<Vec<String>, String> {
    let codesign = |identity: &str, keychain: Option<&Path>, binary: &Path, identifier: &str| {
        let mut c = Command::new("codesign");
        c.args(["--force", "--sign", identity, "--identifier", identifier, "--options", "runtime"]);
        if let Some(kc) = keychain {
            c.arg("--keychain").arg(kc);
        }
        run(c.arg(binary))
    };
    let mut requirements = Vec::new();
    match sign {
        "team" => {
            let home = std::env::var_os("HOME").ok_or("HOME is not set\n")?;
            let keychain = PathBuf::from(home).join("Library/Keychains").join(SIGNING_KEYCHAIN);
            // The separate keychain when it is there (unlocked once, from a terminal); else the login
            // keychain's team identity, which an unattended Herald uses with no prompt (crate::team).
            let (identity, keychain, _unlocked) = if keychain.is_file() {
                let identity = team_identity(&keychain)?;
                let unlocked = Unlocked::keychain(&keychain)?;
                (identity, Some(keychain), Some(unlocked))
            } else {
                let (probe, product) = binaries.first().ok_or("nothing to sign\n")?;
                let identity = crate::team::login_identity(TEAM_ID, probe, &format!("ai.anemnez.{product}"))?;
                (identity, None, None)
            };
            for (binary, product) in binaries {
                let identifier = format!("ai.anemnez.{product}");
                codesign(&identity, keychain.as_deref(), binary, &identifier)?;
                requirements.push(format!(
                    r#"anchor apple generic and certificate leaf[subject.OU] = "{TEAM_ID}" and identifier "{identifier}""#
                ));
            }
        }
        named => {
            let identity = if named == "local" { LOCAL_IDENTITY } else { named };
            for (binary, product) in binaries {
                let identifier = format!("ai.anemnez.{product}");
                codesign(identity, None, binary, &identifier)?;
                requirements.push(if named == "local" {
                    format!(r#"identifier "{identifier}" and certificate leaf[subject.CN] = "{LOCAL_IDENTITY}""#)
                } else {
                    format!(r#"identifier "{identifier}""#)
                });
            }
        }
    }
    Ok(requirements)
}

/// The first Apple Development identity in the signing keychain, by SHA-1. Whether it is the team's
/// is checked after signing, against the requirement.
fn team_identity(keychain: &Path) -> Result<String, String> {
    let found = output(Command::new("security").args(["find-identity", "-v", "-p", "codesigning"]).arg(keychain))?;
    found
        .lines()
        .find(|line| line.contains("\"Apple Development:"))
        .and_then(|line| line.split_once(") ").map(|(_, rest)| rest))
        .and_then(|rest| rest.split_whitespace().next())
        .map(str::to_string)
        .ok_or_else(|| format!("no Apple Development identity in {}\n", keychain.display()))
}

/// The signing keychain, unlocked from the terminal for one signing and locked again after.
struct Unlocked(PathBuf);

impl Unlocked {
    fn keychain(keychain: &Path) -> Result<Self, String> {
        eprintln!("==> signing needs the anemnez-signing keychain: ONE password prompt follows");
        run(Command::new("security").arg("unlock-keychain").arg(keychain))?;
        Ok(Self(keychain.to_path_buf()))
    }
}

impl Drop for Unlocked {
    fn drop(&mut self) {
        drop(Command::new("security").arg("lock-keychain").arg(&self.0).status());
    }
}

fn host_triple() -> Result<String, String> {
    let info = output(Command::new("rustc").arg("-vV"))?;
    info.lines()
        .find_map(|l| l.strip_prefix("host: "))
        .map(str::to_string)
        .ok_or_else(|| "rustc -vV named no host\n".to_string())
}

fn output(command: &mut Command) -> Result<String, String> {
    let shown = format!("{:?}", command);
    let out = command.output().map_err(|e| format!("cannot run {shown}: {e}\n"))?;
    if !out.status.success() {
        return Err(format!("{shown} failed: {}\n", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}
