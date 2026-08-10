//! Stamps the version prova reports at runtime.
//!
//! A `cargo install --git` build otherwise reports exactly the version of the release it was cut
//! from, with nothing to tell them apart. That is not hypothetical: a suite authored against an
//! unreleased 0.11.0 build passed locally and died in CI on `attempt to call a nil value (field
//! 'writes')`, because the released 0.11.0 did not have the API the local 0.11.0 did. `--version`
//! agreed with itself and was useless.
//!
//! So a build that is not a release is stamped `<version>+dev.<sha>`.
//!
//! **Build metadata, not a prerelease.** `0.14.0+dev.abc` and `0.14.0-dev.abc` look
//! interchangeable and are not: semver ignores build metadata when comparing, and *excludes*
//! prereleases from ranges that do not name one. A `-dev` suffix would make every dev build fail
//! every `[requires] prova` gate — verified, not assumed. `+dev` is visible to a human and
//! invisible to the comparison, which is exactly the split we want.

use std::process::Command;

// A build-script panic IS a build failure — exactly the right outcome for a missing
// cargo-provided variable, so the expect below is the design rather than a latent runtime panic.
#[allow(clippy::expect_used)]
fn main() {
    // HEAD moves on checkout/commit; re-stamp when it does.
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-env-changed=PROVA_RELEASE");

    let base = std::env::var("CARGO_PKG_VERSION").expect("cargo always sets this");
    println!("cargo:rustc-env=PROVA_VERSION={}", stamp(&base));
}

fn stamp(base: &str) -> String {
    // The release workflow says so explicitly. Inferring it from git alone is fragile: the release
    // job checks out a single ref shallowly, so the tag object it would need may not be local.
    if std::env::var("PROVA_RELEASE").is_ok_and(|v| v == "1") {
        return base.to_string();
    }

    // Building from a source tarball — no git, nothing to distinguish, assume a release.
    let Some(sha) = git(&["rev-parse", "--short=9", "HEAD"]) else {
        return base.to_string();
    };

    // Built at a release tag without the env var (a local `git checkout v0.14.0`) — still a
    // faithful copy of that release, so do not cry dev.
    if git(&["describe", "--tags", "--exact-match", "--match", "v*"]).is_some() {
        return base.to_string();
    }

    format!("{base}+dev.{sha}")
}

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}
