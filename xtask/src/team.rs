//! `--sign team` without a separate keychain (fabric `LIFECYCLE.md` §7a, the User's option (A),
//! 2026-11-04): when `~/Library/Keychains/anemnez-signing.keychain-db` is not there, the team identity is
//! the login keychain's, which an unattended Herald can use without a password (the User runs
//! `security set-key-partition-list` once per host so codesign may use the key without asking).
//!
//! Which login identity is the team's is decided by the signature itself, never by its name: each
//! "Apple Development" identity is tried, and the first whose signature meets the team requirement
//! (`anchor apple generic and certificate leaf[subject.OU] = "<team>"`) signs. One of another team
//! never meets it, so it is never used.

use std::path::Path;
use std::process::Command;

/// The SHA-1 of every "Apple Development" codesigning identity `security find-identity -v -p codesigning` lists.
pub fn apple_development(listed: &str) -> Vec<String> {
    listed
        .lines()
        .filter(|line| line.contains("\"Apple Development:"))
        .filter_map(|line| line.split_once(") ").map(|(_, rest)| rest))
        .filter_map(|rest| rest.split_whitespace().next())
        .filter(|sha| sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(str::to_owned)
        .collect()
}

/// The login keychain's identity of `team`: the first Apple Development identity whose signature of
/// `probe` (signed as `identifier`) meets the team requirement. `probe` is left signed by it.
pub fn login_identity(team: &str, probe: &Path, identifier: &str) -> Result<String, String> {
    let listed = Command::new("security")
        .args(["find-identity", "-v", "-p", "codesigning"])
        .output()
        .map_err(|e| format!("security find-identity: {e}\n"))?;
    let candidates = apple_development(&String::from_utf8_lossy(&listed.stdout));
    let requirement =
        format!(r#"anchor apple generic and certificate leaf[subject.OU] = "{team}" and identifier "{identifier}""#);
    for sha in &candidates {
        let signed = Command::new("codesign")
            .args(["--force", "--sign", sha, "--identifier", identifier, "--options", "runtime"])
            .arg(probe)
            .output()
            .map_err(|e| format!("codesign: {e}\n"))?;
        if !signed.status.success() {
            continue;
        }
        let meets = Command::new("codesign")
            .args(["--verify", "--strict", &format!("-R={requirement}")])
            .arg(probe)
            .output()
            .map_err(|e| format!("codesign --verify: {e}\n"))?;
        if meets.status.success() {
            return Ok(sha.clone());
        }
    }
    Err(format!(
        "no Apple Development identity of team {team} in the login keychain ({} tried), and no anemnez-signing keychain: \
         import the team identity, or `--sign local` for a build only this machine trusts\n",
        candidates.len()
    ))
}

#[cfg(test)]
mod tests {
    use super::apple_development;

    #[test]
    fn only_apple_development_identities_are_candidates_for_the_team() {
        let listed = "  1) 0123456789ABCDEF0123456789ABCDEF01234567 \"COS Local Signing\"\n\
                      \x20 2) 89ABCDEF0123456789ABCDEF0123456789ABCDEF \"Apple Development: A Person (622Q75Z3C6)\"\n\
                      \x20 3) FEDCBA9876543210FEDCBA9876543210FEDCBA98 \"Apple Development: Someone Else (XXXXXXXXXX)\"\n\
                      \x20 4) 1111 \"Apple Development: torn line\"\n\
                      \x20    3 valid identities found\n";
        assert_eq!(
            apple_development(listed),
            vec![
                "89ABCDEF0123456789ABCDEF0123456789ABCDEF".to_string(),
                "FEDCBA9876543210FEDCBA9876543210FEDCBA98".to_string()
            ],
            "every Apple Development identity is tried (its signature, not its name, decides the team); others never"
        );
        assert!(apple_development("     0 valid identities found\n").is_empty());
    }
}
