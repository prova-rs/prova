//! The lane registry — prova's top-level vocabulary, in one place.
//!
//! A **lane** is a *medium* that holds obligations, and every obligation in it is in one of two
//! states. There are exactly three lanes, and together they are the whole account:
//!
//! | lane        | medium              | latent state | active state | reconciled/gated by |
//! |-------------|---------------------|--------------|--------------|---------------------|
//! | `specs`     | prose (`[specs]` docs) | `backlog`    | `claim`      | `owed`              |
//! | `tests`     | `*.prova.lua`       | `promise`    | `proof`      | `attest`            |
//! | `reminders` | `prova.remind`      | `watching`   | `due`        | `heed`              |
//!
//! A lane is named for its **medium** (so `specs`, not `claims`; `tests`, not `proofs`) — the medium
//! holds *both* states, so naming it after one under-describes it. The two states are the duality:
//! `backlog ⇄ claim`, `promise ⇄ proof`, `watching ⇄ due`.
//!
//! This table is the single source the surfaces must agree with: for each lane there is a
//! `prova <lane>` verb, an MCP tool, and a `prova learn <lane>` topic (parity enforced by unit tests
//! in `prova-cli`). See `docs/plans/query-consolidation.md`.

/// One lane: a medium and its two-state duality. See the module docs for the model.
pub struct Lane {
    /// The lane key — the medium's plural noun, and the spelling every surface uses: the
    /// `prova <key>` verb, the MCP tool, and the `prova learn <key>` topic.
    pub key: &'static str,
    /// A one-phrase description of the medium the lane's items live in.
    pub medium: &'static str,
    /// The latent / cold / open state of an item (`backlog` / `promise` / `watching`).
    pub latent: &'static str,
    /// The active / owed / demonstrated state of an item (`claim` / `proof` / `due`).
    pub active: &'static str,
}

/// The specs lane, named — so a caller wiring a literal lane reaches it at compile time,
/// with no lookup to unwrap.
pub const SPECS: Lane = Lane {
    key: "specs",
    medium: "prose ([specs] docs)",
    latent: "backlog",
    active: "claim",
};

/// The tests lane, named.
pub const TESTS: Lane = Lane {
    key: "tests",
    medium: "*.prova.lua",
    latent: "promise",
    active: "proof",
};

/// The reminders lane, named.
pub const REMINDERS: Lane = Lane {
    key: "reminders",
    medium: "prova.remind",
    latent: "watching",
    active: "due",
};

/// The three lanes, in lifecycle order (spec layer, then proof layer, then the orthogonal attention
/// rail). The single source of truth for lane identity and state names.
pub const LANES: &[Lane] = &[SPECS, TESTS, REMINDERS];

impl Lane {
    /// Look a lane up by its key. Callers that name a literal key pair this with `expect` —
    /// a lane that vanished from the registry is a broken build, not a runtime condition.
    pub fn by_key(key: &str) -> Option<&'static Lane> {
        LANES.iter().find(|l| l.key == key)
    }

    /// Read a CLI argument as one of this lane's two state flags — `--<state>` or `--<state>s`
    /// (the plural-tolerant spelling family: `--claims`, `--promises`, `--backlog` all read
    /// naturally). Returns the canonical state name, so a filter can only ever name a state its
    /// lane actually has (alignment invariant 4, docs/plans/query-consolidation.md).
    pub fn state_flag(&self, arg: &str) -> Option<&'static str> {
        let name = arg.strip_prefix("--")?;
        [self.latent, self.active]
            .into_iter()
            .find(|state| name == *state || name.strip_suffix('s') == Some(state))
    }

    /// Fold a state flag into a report's ONE state slot — mutual exclusion structural, not
    /// hand-checked per verb: the second, different state is a taught error in the lane's own
    /// vocabulary. `Ok(false)` means the argument was not a state flag at all.
    pub fn fold_state_flag(
        &self,
        slot: &mut Option<&'static str>,
        arg: &str,
    ) -> Result<bool, String> {
        let Some(state) = self.state_flag(arg) else {
            return Ok(false);
        };
        match slot {
            Some(prev) if *prev != state => Err(format!(
                "--{} and --{} are mutually exclusive — a {} item is {} or {}, not both",
                self.latent, self.active, self.key, self.latent, self.active
            )),
            _ => {
                *slot = Some(state);
                Ok(true)
            }
        }
    }

    /// Read an MCP `state` value as one of this lane's states (same plural tolerance as the CLI
    /// flags), or a taught error naming both — the tool twin of [`Lane::state_flag`], so the two
    /// surfaces cannot drift.
    pub fn parse_state(&self, value: &str) -> Result<&'static str, String> {
        [self.latent, self.active]
            .into_iter()
            .find(|state| value == *state || value.strip_suffix('s') == Some(state))
            .ok_or_else(|| {
                format!(
                    "unknown {} state {value:?} — expected \"{}\" or \"{}\"",
                    self.key, self.latent, self.active
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Invariant 4 (docs/plans/query-consolidation.md), by construction: every state flag a lane
    /// accepts resolves to a state that lane actually has, in both spellings — and no lane
    /// answers for another lane's states.
    #[test]
    fn state_flags_derive_from_the_registry() {
        for lane in LANES {
            assert_eq!(lane.state_flag(&format!("--{}", lane.latent)), Some(lane.latent));
            assert_eq!(lane.state_flag(&format!("--{}s", lane.active)), Some(lane.active));
            assert_eq!(lane.state_flag("--bogus"), None);
            assert_eq!(lane.state_flag(lane.latent), None, "a state name is not a flag without --");
            assert_eq!(lane.parse_state(lane.active), Ok(lane.active));
            assert!(lane.parse_state("bogus").is_err());
        }
        let specs = Lane::by_key("specs").unwrap();
        assert_eq!(specs.state_flag("--promises"), None, "another lane's state does not read");
    }

    /// The one state slot: same flag twice is idempotent; the OTHER state is a taught error in
    /// the lane's own vocabulary.
    #[test]
    fn the_state_slot_is_structurally_exclusive() {
        let lane = Lane::by_key("specs").unwrap();
        let mut slot = None;
        assert_eq!(lane.fold_state_flag(&mut slot, "--claims"), Ok(true));
        assert_eq!(lane.fold_state_flag(&mut slot, "--claim"), Ok(true), "same state re-folds");
        assert_eq!(slot, Some("claim"));
        let err = lane.fold_state_flag(&mut slot, "--backlog").unwrap_err();
        assert!(err.contains("mutually exclusive") && err.contains("backlog"), "{err}");
        assert_eq!(lane.fold_state_flag(&mut slot, "--undated"), Ok(false), "not a state flag");
    }
}
