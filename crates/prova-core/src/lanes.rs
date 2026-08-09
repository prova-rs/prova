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

/// The three lanes, in lifecycle order (spec layer, then proof layer, then the orthogonal attention
/// rail). The single source of truth for lane identity and state names.
pub const LANES: &[Lane] = &[
    Lane {
        key: "specs",
        medium: "prose ([specs] docs)",
        latent: "backlog",
        active: "claim",
    },
    Lane {
        key: "tests",
        medium: "*.prova.lua",
        latent: "promise",
        active: "proof",
    },
    Lane {
        key: "reminders",
        medium: "prova.remind",
        latent: "watching",
        active: "due",
    },
];
