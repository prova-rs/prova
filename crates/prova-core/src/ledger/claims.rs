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

use sha2::{Digest, Sha256};

/// A normative statement anchored in prose.
#[derive(Debug, Clone)]
pub struct Claim {
    /// `path#id`, package-relative — the address a proof names in `covers`.
    pub address: String,
    pub file: PathBuf,
    pub line: usize,
    /// Short digest of the claim's normalized text — what a pinned binding compares against.
    pub digest: String,
}

/// The prose an anchor labels: the lines under it, to the next blank line. A paragraph is what an
/// author thinks of as "the claim", and taking more would make every neighbouring edit churn.
fn claim_text(lines: &[&str], anchor_index: usize) -> String {
    lines
        .iter()
        .skip(anchor_index + 1)
        .take_while(|l| !l.trim().is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Collapse whitespace before hashing.
///
/// A pin that fired when someone reflowed a paragraph would be switched off within a week, so
/// wrapping and indentation must not count. Case and punctuation DO count: "must" and "may" are
/// the whole content of a normative claim.
pub fn digest(text: &str) -> String {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut hasher = <Sha256 as Digest>::new();
    hasher.update(normalized.as_bytes());
    hex::encode(hasher.finalize())[..8].to_string()
}

/// Split `path#id@digest` into its address and pin. No `@` means an unpinned binding — bound, but
/// not watching the text.
pub fn split_pin(address: &str) -> (&str, Option<&str>) {
    match address.rsplit_once('@') {
        Some((addr, pin)) if !pin.is_empty() => (addr, Some(pin)),
        _ => (address, None),
    }
}

/// Every claim whose bare id matches `id` — the resolution behind `prova attest <id>`.
///
/// The full `path#id` address is a machine coordinate: an agent has it in its buffer, a human
/// does not. Ids are the memorable half, so a unique one resolves; the same id anchored in two
/// documents is legal, and then the caller gets the candidates, never a coin flip.
pub fn matching_id<'c>(claims: &'c [Claim], id: &str) -> Vec<&'c Claim> {
    let suffix = format!("#{id}");
    claims.iter().filter(|c| c.address.ends_with(&suffix)).collect()
}

/// What the ledger found. Ordered worst-first so the actionable rows are read.
///
/// The tags are the negations of the lifecycle stages (docs/design/lifecycle.md), and one
/// grammar: past participles about the obligation. `DANGLING` and `UNPROVEN` are the two
/// directions of a broken link — a proof pointing at prose that is not there, and prose no proof
/// points at — and the earlier names (`UNBOUND` for the first) did not say which was which.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    /// A `covers` naming an address with no anchor. Two situations produce this — prose not
    /// written yet, and prose deleted once the proof captured the contract — and the remedies
    /// differ, so the message names both rather than guessing.
    Dangling,
    /// An anchored claim nothing covers. The intake half: an obligation with no proof.
    Unproven,
    /// A pinned claim whose text changed. The drift that keeps everything green: the anchor still
    /// resolves and the proof still passes, but the claim now says something the proof may not
    /// check. Only the text can catch it.
    Stale,
    /// A proof authored ahead of its implementation — flagged `promises`, not yet kept.
    Promised,
}

impl Status {
    pub fn tag(self) -> &'static str {
        match self {
            Status::Dangling => "DANGLING",
            Status::Unproven => "UNPROVEN",
            Status::Stale => "STALE",
            Status::Promised => "PROMISED",
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

    let all: Vec<&str> = text.lines().collect();
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
            digest: digest(&claim_text(&all, index)),
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
pub fn reconcile(claims: &[Claim], proofs: &[crate::ProofObligation]) -> Vec<Owed> {
    let mut owed = Vec::new();

    for proof in proofs {
        for raw in &proof.covers {
            // External addresses (`jira:PROVA-142`) are opaque to this pass — unresolvable is not
            // unbound, and reporting one as the other would send an agent hunting for prose that
            // was never supposed to be local.
            if raw.contains(':') && !raw.contains('#') {
                continue;
            }
            let (address, pin) = split_pin(raw);
            let Some(claim) = claims.iter().find(|c| c.address == address) else {
                owed.push(Owed {
                    status: Status::Dangling,
                    subject: address.to_string(),
                    detail: format!(
                        "{} covers it, but no anchor exists — write the prose, or retire the \
                         reference into `proves`",
                        proof.path
                    ),
                });
                continue;
            };
            // A pin is opt-in per binding: unpinned bindings are bound but not watching the text,
            // so a wording change on a claim whose exact phrasing is not the contract costs
            // nobody a re-confirmation.
            if pin.is_some_and(|pin| pin != claim.digest) {
                owed.push(Owed {
                    status: Status::Stale,
                    subject: address.to_string(),
                    detail: format!(
                        "the claim's text changed since {} pinned it — re-read it and confirm the \
                         proof still discharges it, then `prova owed --pin`",
                        proof.path
                    ),
                });
            }
        }
        if let Some(reason) = &proof.spec {
            owed.push(Owed {
                status: Status::Promised,
                subject: proof.path.clone(),
                detail: reason.clone(),
            });
        }
    }

    for claim in claims {
        let covered = proofs
            .iter()
            .any(|p| p.covers.iter().any(|a| split_pin(a).0 == claim.address));
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


/// Write the current digest onto every unpinned binding in `files`, and refresh any pin whose
/// claim has since been edited.
///
/// The pin is written by the tool, into the proof source. A digest a human types is one nobody
/// verifies; a digest in a lockfile is invisible in review. In the source it shows up in the diff
/// as "this claim's text changed and someone re-accepted it" — which is exactly the signal a
/// reviewer wants, and is worth the churn it costs.
pub fn pin(files: &[PathBuf], claims: &[Claim]) -> Result<usize, ClaimError> {
    let mut pinned = 0;
    for file in files {
        let text = std::fs::read_to_string(file).map_err(|source| ClaimError::Io {
            path: file.clone(),
            source,
        })?;
        let mut updated = text.clone();
        for claim in claims {
            // Quoted so one address cannot match inside a longer one (`#claim` in `#claim-2`).
            for quote in ['"', '\''] {
                let bare = format!("{quote}{}{quote}", claim.address);
                let want = format!("{quote}{}@{}{quote}", claim.address, claim.digest);
                if updated.contains(&bare) {
                    updated = updated.replace(&bare, &want);
                    continue;
                }
                // An existing pin on this address, stale or not, is replaced wholesale.
                let prefix = format!("{quote}{}@", claim.address);
                if let Some(at) = updated.find(&prefix) {
                    let rest = &updated[at + prefix.len()..];
                    if let Some(end) = rest.find(quote) {
                        let old = &updated[at..at + prefix.len() + end + 1];
                        updated = updated.replace(old, &want);
                    }
                }
            }
        }
        if updated != text {
            std::fs::write(file, &updated).map_err(|source| ClaimError::Io {
                path: file.clone(),
                source,
            })?;
            pinned += 1;
        }
    }
    Ok(pinned)
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
