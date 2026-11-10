//! `[isolation]`: what every process a proof spawns sees in place of the host's own environment
//! (docs/design/manifest.md#isolation). Set once per run, before any proof runs, and applied at the
//! one place `shell.run` and `shell.spawn` both pass through, so a deputed `cargo nextest run`, a
//! service and a one-off command alike start against the run's own roots: a product binary a proof
//! starts can never reach the host's live services by default (2026-11-10: an agora test binary
//! reached a host's live swarm and tried to re-write three real leases; the swarm's guard refused it).
//! A proof's own `env = { … }` overlay still wins, so a proof that means the host says so by name.

use std::collections::BTreeMap;
use std::sync::Mutex;

/// `[isolation]` in `prova.toml`: `env` maps a variable to a value in which `{run}` names this run's own
/// scratch directory; `unless` names a switch that, thrown, turns it off for the run (a live lane).
#[derive(Debug, serde::Deserialize, Default, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Isolation {
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub unless: Option<String>,
}

static APPLIED: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

/// This run's isolation: each `(variable, value)` every spawned process gets. Empty turns it off.
pub fn set(pairs: Vec<(String, String)>) {
    *APPLIED.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = pairs;
}

/// What a spawned process gets now.
pub fn applied() -> Vec<(String, String)> {
    APPLIED.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
}
