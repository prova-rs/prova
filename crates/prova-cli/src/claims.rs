//! Claims — obligations that enter from outside prova, and the ledger that reconciles them.
//!
//! A `<!-- claim: id -->` anchor in prose is a deliberate act: *this sentence is normative and I
//! intend it to be proven*. That single act admits an obligation into the system from a design doc,
//! a README, anywhere. A proof discharges it with `covers = "path#id"`.
//!
//! `prova owed` answers the one question an agent orienting in a repo should ask — **what is owed
//! here?** — across every origin, because an answer that lives in two places has one that goes
//! stale. Open specs and unproven claims are the same kind of thing: work someone scoped and
//! nobody finished.
//!
//! Reported, never fatal — with one exception. A duplicate id makes an address ambiguous, so
//! nothing can discharge it and the ledger is incoherent rather than merely behind. That is a
//! defect, and it errors.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A normative statement anchored in prose.
#[derive(Debug, Clone)]
pub struct Claim {
    /// `path#id`, package-relative — the address a proof names in `covers`.
    pub address: String,
    pub file: PathBuf,
    pub line: usize,
}

/// What the ledger found. Ordered worst-first so the actionable rows are read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    /// A `covers` naming an address with no anchor. Two situations produce this — prose not
    /// written yet, and prose deleted once the proof captured the contract — and the remedies
    /// differ, so the message names both rather than guessing.
    Unbound,
    /// An anchored claim nothing covers. The intake half: an obligation with no proof.
    Unproven,
    /// A proof authored ahead of its implementation.
    Spec,
}

impl Status {
    pub fn tag(self) -> &'static str {
        match self {
            Status::Unbound => "UNBOUND",
            Status::Unproven => "UNPROVEN",
            Status::Spec => "SPEC",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Owed {
    pub status: Status,
    /// Where the obligation lives: a claim address, or the proof's node path.
    pub subject: String,
    /// The reason, remedy, or claim text — whatever the reader needs next.
    pub detail: String,
}

#[derive(Debug)]
pub enum ClaimError {
    /// Same id twice in one file. Errors, because the address it forms cannot be discharged.
    Duplicate { id: String, file: PathBuf, first: usize, again: usize },
    Io { path: PathBuf, source: std::io::Error },
}

impl std::fmt::Display for ClaimError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClaimError::Duplicate { id, file, first, again } => write!(
                f,
                "duplicate claim id '{id}' in {} (lines {first} and {again}) — an ambiguous \
                 address cannot be discharged by anything; rename one",
                file.display()
            ),
            ClaimError::Io { path, source } => {
                write!(f, "reading {}: {source}", path.display())
            }
        }
    }
}

/// Scan the configured docs for anchors.
///
/// The anchor is an HTML comment so it renders as nothing: prose carrying a machine-readable
/// obligation must still read as prose, or authors stop writing it.
pub fn scan(root: &Path, docs: &[String]) -> Result<Vec<Claim>, ClaimError> {
    let mut claims = Vec::new();
    for entry in docs {
        let path = root.join(entry);
        if path.is_dir() {
            collect_dir(root, &path, &mut claims)?;
        } else if path.exists() {
            collect_file(root, &path, &mut claims)?;
        }
        // A declared-but-missing path is not an error here: docs get moved, and `owed` reporting a
        // missing directory as a hard failure would block the very reconciliation you ran it for.
    }
    Ok(claims)
}

fn collect_dir(root: &Path, dir: &Path, out: &mut Vec<Claim>) -> Result<(), ClaimError> {
    let entries = std::fs::read_dir(dir).map_err(|source| ClaimError::Io {
        path: dir.to_path_buf(),
        source,
    })?;
    let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok()).map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            collect_dir(root, &path, out)?;
        } else if path.extension().is_some_and(|e| e == "md" || e == "markdown") {
            collect_file(root, &path, out)?;
        }
    }
    Ok(())
}

fn collect_file(root: &Path, path: &Path, out: &mut Vec<Claim>) -> Result<(), ClaimError> {
    let text = std::fs::read_to_string(path).map_err(|source| ClaimError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let relative = path.strip_prefix(root).unwrap_or(path).to_path_buf();

    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for (index, line) in text.lines().enumerate() {
        let Some(id) = parse_anchor(line) else { continue };
        let line_no = index + 1;
        if let Some(&first) = seen.get(&id) {
            return Err(ClaimError::Duplicate {
                id,
                file: relative,
                first,
                again: line_no,
            });
        }
        out.push(Claim {
            address: format!("{}#{id}", relative.display()),
            file: relative.clone(),
            line: line_no,
        });
        seen.insert(id, line_no);
    }
    Ok(())
}

/// `<!-- claim: some-id -->` → `some-id`. Tolerant of spacing, strict about shape: anything else
/// is ordinary prose and must stay invisible.
fn parse_anchor(line: &str) -> Option<String> {
    let rest = line.trim().strip_prefix("<!--")?.trim_start();
    let rest = rest.strip_prefix("claim:")?.trim_start();
    let id = rest.strip_suffix("-->")?.trim();
    (!id.is_empty() && !id.contains(char::is_whitespace)).then(|| id.to_string())
}

/// Reconcile anchors against what the proofs claim to discharge.
pub fn reconcile(claims: &[Claim], proofs: &[prova_core::ProofObligation]) -> Vec<Owed> {
    let mut owed = Vec::new();
    let anchored: Vec<&str> = claims.iter().map(|c| c.address.as_str()).collect();

    for proof in proofs {
        for address in &proof.covers {
            // External addresses (`jira:PROVA-142`) are opaque to this pass — unresolvable is not
            // unbound, and reporting one as the other would send an agent hunting for prose that
            // was never supposed to be local.
            if address.contains(':') && !address.contains('#') {
                continue;
            }
            if !anchored.contains(&address.as_str()) {
                owed.push(Owed {
                    status: Status::Unbound,
                    subject: address.clone(),
                    detail: format!(
                        "{} covers it, but no anchor exists — write the prose, or retire the \
                         reference into `proves`",
                        proof.path
                    ),
                });
            }
        }
        if let Some(reason) = &proof.spec {
            owed.push(Owed {
                status: Status::Spec,
                subject: proof.path.clone(),
                detail: reason.clone(),
            });
        }
    }

    for claim in claims {
        let covered = proofs.iter().any(|p| p.covers.iter().any(|a| a == &claim.address));
        if !covered {
            owed.push(Owed {
                status: Status::Unproven,
                subject: claim.address.clone(),
                detail: format!("{}:{} — no proof covers it", claim.file.display(), claim.line),
            });
        }
    }

    owed.sort_by(|a, b| a.status.cmp(&b.status).then(a.subject.cmp(&b.subject)));
    owed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_anchor_is_recognised_regardless_of_spacing() {
        assert_eq!(parse_anchor("<!-- claim: busy-not-absent -->").as_deref(), Some("busy-not-absent"));
        assert_eq!(parse_anchor("<!--claim:tight-->").as_deref(), Some("tight"));
        assert_eq!(parse_anchor("   <!-- claim: indented -->  ").as_deref(), Some("indented"));
    }

    #[test]
    fn ordinary_prose_is_invisible() {
        // Unanchored prose must never become an obligation. Inferring claims from unmarked text
        // is how this pattern turns into ritual and gets routed around.
        assert!(parse_anchor("This claim: is just a sentence.").is_none());
        assert!(parse_anchor("<!-- a normal comment -->").is_none());
        assert!(parse_anchor("<!-- claim: two words -->").is_none());
        assert!(parse_anchor("<!-- claim: -->").is_none());
    }
}
