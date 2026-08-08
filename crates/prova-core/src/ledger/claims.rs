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
//! Reported, never fatal — with two exceptions, both defects that make an anchor impossible to
//! trust rather than merely behind: a **duplicate id** (an ambiguous address nothing can discharge)
//! and a **malformed anchor** (the `claim:`/`backlog:` keyword is there, so the line means to be an
//! anchor, but it cannot be read — a no-id or a mistyped date). Both error, naming file and line,
//! because the alternative — silently treating the line as prose — is an obligation the author
//! wrote and then hunts for and cannot find.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Serialize;
use sha2::{Digest, Sha256};

/// The two states of one prose obligation, distinguished only by the anchor's keyword.
///
/// `claim` is owed: it is reconciled, it can be bound by a proof, and an uncovered one reports as
/// `UNPROVEN`. `backlog` is the cold state of the very same thing — captured in place, but muted:
/// out of `owed`, never failing CI, invisible to an agent driving the doc. It is a not-yet-claim a
/// human promotes when its time comes. The two share one id namespace, so promotion is a keyword
/// flip in place (`prova backlog promote`) and a stray duplicate across the states is still caught.
///
/// The invariant that keeps the state machine legible: **only a claim can be bound.** A backlog
/// item is unbound by definition; a proof that `covers` one is pointing at something still cold,
/// and the ledger says so (`BACKLOGGED`) rather than silently discharging it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Claim,
    Backlog,
}

impl Kind {
    /// The anchor keyword — `claim` or `backlog` — as it appears after `<!--`.
    pub fn keyword(self) -> &'static str {
        match self {
            Kind::Claim => "claim",
            Kind::Backlog => "backlog",
        }
    }
}

/// A normative statement anchored in prose — a claim, or its cold-state counterpart, a backlog item.
#[derive(Debug, Clone)]
pub struct Claim {
    /// Which state this anchor is in: an owed `claim`, or a muted `backlog` item.
    pub kind: Kind,
    /// `path#id`, package-relative — the address a proof names in `covers`.
    pub address: String,
    pub file: PathBuf,
    pub line: usize,
    /// Short digest of the claim's normalized text — what a pinned binding compares against.
    pub digest: String,
    /// An optional `YYYY-MM-DD` draw-down date carried on the anchor: the deadline by which a
    /// backlog item should be promoted, or a claim discharged. A reminder condition compares it
    /// against `now` to draw it down (docs/plans/deprecation-drawdown.md). Optional — but agents
    /// are nudged to set one, and `prova backlog --undated` finds the items that lack it.
    pub date: Option<String>,
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

/// The cold shelf: every backlog-state anchor, in scan order. The muted counterpart to the owed
/// claims `reconcile` reports — captured work a human has not yet decided to make active.
pub fn backlog(claims: &[Claim]) -> Vec<&Claim> {
    claims.iter().filter(|c| c.kind == Kind::Backlog).collect()
}

/// What the ledger found. Ordered worst-first so the actionable rows are read.
///
/// The tags are the negations of the lifecycle stages (docs/design/lifecycle.md), and one
/// grammar: past participles about the obligation. `DANGLING` and `UNPROVEN` are the two
/// directions of a broken link — a proof pointing at prose that is not there, and prose no proof
/// points at — and the earlier names (`UNBOUND` for the first) did not say which was which.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum Status {
    /// A `covers` naming an address with no anchor. Two situations produce this — prose not
    /// written yet, and prose deleted once the proof captured the contract — and the remedies
    /// differ, so the message names both rather than guessing.
    Dangling,
    /// A `covers` naming an anchor that is still in `backlog` state. The address resolves, but a
    /// backlog item is unbound by definition — the proof is trying to discharge something nobody
    /// has promoted to a claim yet. The remedy is one keyword: `prova backlog promote <id>`.
    Backlogged,
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
            Status::Backlogged => "BACKLOGGED",
            Status::Unproven => "UNPROVEN",
            Status::Stale => "STALE",
            Status::Promised => "PROMISED",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
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
    /// A line that is clearly an anchor attempt — the `claim:` / `backlog:` keyword is there — but
    /// cannot be read (no id, a mistyped date, trailing junk). Errors rather than silently becoming
    /// prose: an author who wrote `<!-- backlog: … -->` expects it to appear, so a mistake must say
    /// why, not vanish into a thing they hunt for and cannot find.
    Malformed { file: PathBuf, line: usize, reason: String },
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
            ClaimError::Malformed { file, line, reason } => write!(
                f,
                "malformed anchor at {}:{} — {reason}",
                file.display(),
                line
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
    // One namespace across both states: an id anchored twice is ambiguous whether the second is a
    // claim or a backlog item, so the duplicate check spans them. It is also what makes promotion a
    // safe in-place flip — the id is the identity, the keyword is only the state.
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for (index, line) in text.lines().enumerate() {
        let line_no = index + 1;
        let (kind, id, date) = match parse_anchor(line) {
            Anchor::Prose => continue,
            Anchor::Malformed(reason) => {
                return Err(ClaimError::Malformed {
                    file: relative,
                    line: line_no,
                    reason,
                });
            }
            Anchor::Found { kind, id, date } => (kind, id, date),
        };
        if let Some(&first) = seen.get(&id) {
            return Err(ClaimError::Duplicate {
                id,
                file: relative,
                first,
                again: line_no,
            });
        }
        out.push(Claim {
            kind,
            address: format!("{}#{id}", relative.display()),
            file: relative.clone(),
            line: line_no,
            digest: digest(&claim_text(&all, index)),
            date,
        });
        seen.insert(id, line_no);
    }
    Ok(())
}

/// What one line is, read as an anchor.
enum Anchor {
    /// No `claim:`/`backlog:` keyword — ordinary prose, and it must stay invisible.
    Prose,
    /// A well-formed anchor: `<!-- claim: id -->`, optionally `<!-- backlog: id 2026-09-01 -->`.
    Found { kind: Kind, id: String, date: Option<String> },
    /// The keyword is there, so the line MEANS to be an anchor, but it cannot be read — with why.
    Malformed(String),
}

/// Read a line as an anchor. The keyword (`claim:`/`backlog:`) is the line of intent: without it,
/// the line is prose and stays invisible; WITH it, a mistake is reported (`Malformed`), never
/// silently dropped. Tolerant of spacing, strict about shape: an id, then an OPTIONAL `YYYY-MM-DD`,
/// and nothing else.
fn parse_anchor(line: &str) -> Anchor {
    let Some(rest) = line.trim().strip_prefix("<!--").map(str::trim_start) else {
        return Anchor::Prose;
    };
    let (kind, rest) = if let Some(rest) = rest.strip_prefix("claim:") {
        (Kind::Claim, rest)
    } else if let Some(rest) = rest.strip_prefix("backlog:") {
        (Kind::Backlog, rest)
    } else {
        return Anchor::Prose; // an HTML comment, but not an anchor keyword
    };
    // Past the keyword, the author meant an anchor — every failure from here is Malformed, not Prose.
    let kw = kind.keyword();
    let Some(body) = rest.trim_start().strip_suffix("-->").map(str::trim) else {
        return Anchor::Malformed(format!("`{kw}:` anchor is not closed with `-->` on this line"));
    };
    let mut parts = body.split_whitespace();
    let Some(id) = parts.next() else {
        return Anchor::Malformed(format!("`{kw}:` has no id — write `<!-- {kw}: some-id -->`"));
    };
    let date = match parts.next() {
        None => None,
        Some(tok) if valid_iso_date(tok) => Some(tok.to_string()),
        Some(tok) => {
            return Anchor::Malformed(format!(
                "{tok:?} after the id `{id}` is not a valid YYYY-MM-DD date — the only thing that \
                 may follow an id (an id itself has no spaces)"
            ))
        }
    };
    if let Some(extra) = parts.next() {
        return Anchor::Malformed(format!(
            "unexpected {extra:?} — an anchor is an id and an optional YYYY-MM-DD date, nothing more"
        ));
    }
    Anchor::Found { kind, id: id.to_string(), date }
}

/// A strict `YYYY-MM-DD` check: exact shape plus plausible month/day ranges, no date crate. The
/// reminder condition does the real calendar arithmetic against `now`; here we only refuse an
/// obviously malformed date so a typo does not silently ride along as a deadline.
fn valid_iso_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    let digits = |r: &[u8]| r.iter().all(u8::is_ascii_digit);
    if !(digits(&b[0..4]) && digits(&b[5..7]) && digits(&b[8..10])) {
        return false;
    }
    let month: u32 = s[5..7].parse().unwrap_or(0);
    let day: u32 = s[8..10].parse().unwrap_or(0);
    (1..=12).contains(&month) && (1..=31).contains(&day)
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
            // Only a claim can be bound. The anchor resolves, but it is still in backlog state —
            // the proof is discharging something nobody promoted to a claim yet. Report it rather
            // than treat a cold item as covered; the remedy is one keyword.
            if claim.kind == Kind::Backlog {
                owed.push(Owed {
                    status: Status::Backlogged,
                    subject: address.to_string(),
                    detail: format!(
                        "{} covers it, but it is still a backlog item — `prova backlog promote {}` \
                         to make it a claim a proof can discharge",
                        proof.path,
                        address.rsplit('#').next().unwrap_or(address),
                    ),
                });
                continue;
            }
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

    // Backlog items are muted: a cold item is not owed, so it never enters this pass. That muting
    // is the whole point — a bug or half-formed spec can be parked in a doc being actively driven
    // without adding to what that doc owes right now.
    for claim in claims.iter().filter(|c| c.kind == Kind::Claim) {
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

/// Promote a backlog item to a claim in place: flip `<!-- backlog: id -->` to `<!-- claim: id -->`
/// on its own line. The id and the prose beneath it do not move — only the state changes, so the
/// diff reads as exactly what happened, and the address a future proof will name is already the one
/// the reader sees. Demotion (claim → backlog) is the inverse and deliberately not offered here: it
/// is only safe when nothing binds the claim, a check that belongs to the caller with the proofs in
/// hand, not to a keyword flip that cannot see them.
pub fn promote(claim: &Claim, root: &Path) -> Result<(), ClaimError> {
    let path = root.join(&claim.file);
    let text = std::fs::read_to_string(&path).map_err(|source| ClaimError::Io {
        path: path.clone(),
        source,
    })?;
    // Rewrite by line, preserving every other byte — split_inclusive keeps line endings and any
    // missing trailing newline, so a promotion never reflows or reformats the surrounding document.
    let idx = claim.line.saturating_sub(1);
    let mut out = String::with_capacity(text.len());
    for (i, segment) in text.split_inclusive('\n').enumerate() {
        if i == idx {
            out.push_str(&segment.replacen("backlog:", "claim:", 1));
        } else {
            out.push_str(segment);
        }
    }
    std::fs::write(&path, &out).map_err(|source| ClaimError::Io { path, source })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(line: &str) -> Option<(Kind, String, Option<String>)> {
        match parse_anchor(line) {
            Anchor::Found { kind, id, date } => Some((kind, id, date)),
            _ => None,
        }
    }
    fn is_prose(line: &str) -> bool {
        matches!(parse_anchor(line), Anchor::Prose)
    }
    fn is_malformed(line: &str) -> bool {
        matches!(parse_anchor(line), Anchor::Malformed(_))
    }

    #[test]
    fn an_anchor_is_recognised_regardless_of_spacing() {
        assert_eq!(ids("<!-- claim: busy-not-absent -->"), Some((Kind::Claim, "busy-not-absent".into(), None)));
        assert_eq!(ids("<!--claim:tight-->"), Some((Kind::Claim, "tight".into(), None)));
        assert_eq!(ids("   <!-- claim: indented -->  "), Some((Kind::Claim, "indented".into(), None)));
    }

    #[test]
    fn a_backlog_anchor_is_the_same_shape_with_a_different_keyword() {
        assert_eq!(ids("<!-- backlog: flaky-teardown -->"), Some((Kind::Backlog, "flaky-teardown".into(), None)));
        assert_eq!(ids("<!--backlog:tight-->"), Some((Kind::Backlog, "tight".into(), None)));
    }

    #[test]
    fn an_anchor_carries_an_optional_iso_date() {
        assert_eq!(
            ids("<!-- backlog: flaky-teardown 2026-09-01 -->"),
            Some((Kind::Backlog, "flaky-teardown".into(), Some("2026-09-01".into())))
        );
        assert_eq!(
            ids("<!-- claim: never-preempt 2026-12-31 -->"),
            Some((Kind::Claim, "never-preempt".into(), Some("2026-12-31".into())))
        );
    }

    #[test]
    fn ordinary_prose_is_invisible() {
        // Unanchored prose must never become an obligation. Without the keyword there is no intent
        // to anchor, so the line is prose — silently, no error.
        assert!(is_prose("This claim: is just a sentence."));
        assert!(is_prose("<!-- a normal comment -->"));
        assert!(is_prose("Just a paragraph about claims and backlog items."));
    }

    #[test]
    fn a_malformed_anchor_is_reported_not_silently_dropped() {
        // The keyword IS there, so the author meant an anchor — a mistake must say why rather than
        // vanish into prose and become a thing hunted for and not found.
        assert!(is_malformed("<!-- claim: two words -->"));
        assert!(is_malformed("<!-- claim: -->"));
        assert!(is_malformed("<!-- backlog: -->"));
        assert!(is_malformed("<!-- backlog: id 2026-13-01 -->")); // impossible month
        assert!(is_malformed("<!-- backlog: id 2026-9-1 -->")); // not zero-padded
        assert!(is_malformed("<!-- backlog: id notadate -->"));
        assert!(is_malformed("<!-- backlog: id 2026-09-01 extra -->"));
    }
}
