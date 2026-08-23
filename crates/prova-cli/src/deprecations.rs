//! Deprecation bridges: the spellings and mechanisms prova still accepts, and the teaching that
//! makes them migrate rather than linger.
//!
//! Every warning here is a **dated obligation** in `docs/design/deprecations.md` — a bridge that
//! never comes down is just debt, so each one has a removal date the `backlog-drawdown` reminder
//! draws down. Extracted from `manifest.rs` when the capability companion's bridge arrived: the
//! concern is self-contained, and it is the natural home for a warning that teaches a whole
//! mechanism's replacement rather than a renamed key.

/// One warning per key per process, on stderr.
///
/// Deduped because these fire from manifest resolution, which runs more than once per invocation
/// (the project's own manifest, then a dependency's) — and a migration hint repeated five times
/// reads as five problems.
pub(crate) fn warn_once(key: &'static str, msg: &str) {
    use std::collections::BTreeSet;
    use std::sync::{Mutex, OnceLock};
    static WARNED: OnceLock<Mutex<BTreeSet<&'static str>>> = OnceLock::new();
    // Recover a poisoned lock: the set is plain data, and a lost dedup only repeats a warning.
    let mut set = WARNED
        .get_or_init(|| Mutex::new(BTreeSet::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if set.insert(key) {
        eprintln!("prova: {msg}");
    }
}

/// The `prova.lua` companion is deprecated in favor of `[capabilities]`
/// (docs/design/capabilities.md). Teach the replacement for THIS registration, naming the file it
/// came from and the exact TOML that replaces it.
///
/// One warning per capability name, not per file: the author's unit of work is "migrate this
/// capability", and each needs its own two lines of TOML.
///
/// The teaching names a package rather than offering an inline predicate form, because there
/// deliberately is no inline form — a predicate lives in a package so a proof can call it, which is
/// the whole reason the companion is going away.
pub(crate) fn warn_companion_capability(name: &str, companion: &std::path::Path) {
    // The dedup key must be 'static, and the name is not. Leaking one short string per distinct
    // migrated capability, once per process, is the honest trade for a set that outlives the call —
    // and a project has a handful of these, not thousands.
    let key: &'static str = Box::leak(format!("companion-capability:{name}").into_boxed_str());
    warn_once(
        key,
        &format!(
            "`runtime.capability({name:?}, …)` in {} is deprecated — declare it in prova.toml \
             instead:\n\n    [capabilities]\n    {name} = {{ package = \"<your-package>\", \
             capability = {name:?} }}\n\n       Move the predicate into that package as an exported \
             function, where a proof can call it directly. (`prova learn capabilities`)",
            companion.display()
        ),
    );
}

/// A capability declared in BOTH `[capabilities]` and the companion. The manifest wins; say so.
///
/// Silent precedence between a deprecated and a current mechanism is how a migration produces a
/// mystery: the author edits the companion, nothing changes, and nothing said why.
pub(crate) fn warn_companion_shadowed(name: &str) {
    let key: &'static str = Box::leak(format!("companion-shadowed:{name}").into_boxed_str());
    warn_once(
        key,
        &format!(
            "capability {name:?} is declared in both [capabilities] and the prova.lua companion — \
             the manifest declaration wins. Delete the `runtime.capability({name:?}, …)` \
             registration."
        ),
    );
}

/// One warning per deprecated spelling per process, on stderr. The serde `alias` attributes keep
/// the old spellings PARSING for one release; this is what keeps them TEACHING. Everything here
/// retires together at 1.0 with the other pre-1.0 spellings.
///
/// Runs against the generic TOML (same trick as the version gate), because serde aliases are
/// silent by design — after a successful parse there is no way to know which spelling was used.
/// Only the project's OWN manifest warns; a dependency's manifest is not the consumer's to fix.
pub(crate) fn warn_deprecated_spellings(text: &str) {
    let Ok(value) = text.parse::<toml::Value>() else {
        return; // unparseable text already failed phase two with a real diagnostic
    };
    if value.get("plugins").is_some() {
        warn_once(
            "plugins",
            "`[plugins]` is deprecated — rename the table to `[dependencies]` (every dependency \
             is a package; retires at 1.0)",
        );
    }
    if value.get("claims").is_some() {
        warn_once(
            "claims",
            "`[claims]` is deprecated — rename the table to `[specs]` (the section declares the \
             prose that holds claims AND backlog items; retires at 1.0)",
        );
    }
    if value.get("plugin").is_some() {
        warn_once(
            "plugin",
            "`[plugin]` is deprecated — rename the table to `[package]` (a package declares \
             itself; retires at 1.0)",
        );
    }
    let mut overlays: Vec<&toml::Value> = Vec::new();
    overlays.extend(value.get("run"));
    if let Some(table) = value.get("profiles").and_then(|p| p.as_table()) {
        overlays.extend(table.values());
    }
    for overlay in &overlays {
        if overlay.get("plugin_root").is_some() {
            warn_once(
                "plugin_root",
                "`plugin_root` is deprecated — rename the key to `packages` (the directory of \
                 this package's own packages; retires at 1.0)",
            );
        }
        // The companion key itself, not just what it registers: a project can be told to stop
        // pointing at a companion even before it has migrated the registrations inside it.
        if overlay.get("config").is_some() {
            warn_once(
                "run-config",
                "`config` (the prova.lua companion) is deprecated — capabilities are declared in \
                 `[capabilities]` now (`prova learn capabilities`); the key, `--config`, and \
                 `PROVA_CONFIG` retire with the companion",
            );
        }
    }
    if overlays
        .iter()
        .skip(usize::from(value.get("run").is_some())) // profile tables only
        .any(|p| p.get("plugins").is_some())
    {
        warn_once(
            "profile-plugins",
            "`[profiles.<name>.dependencies]` is deprecated — rename to \
             `[profiles.<name>.dependencies]` (retires at 1.0)",
        );
    }
    if let Some(topologies) = value.get("topologies").and_then(|t| t.as_table()) {
        if topologies.values().any(|decl| decl.get("plugin").is_some()) {
            warn_once(
                "topology-plugin",
                "`plugin =` in a [topologies] entry is deprecated — rename the key to `package =` \
                 (retires at 1.0)",
            );
        }
    }
}
