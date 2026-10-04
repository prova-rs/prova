//! Run-scoped dependency overrides: `prova --dep <name>=<tag|rev|path>` and `PROVA_DEP_<NAME>`.
//!
//! Verifying a consumer against a NEW version of one of its dependencies used to mean editing the
//! consumer's `prova.toml` — someone else's file — and remembering to put it back. An override
//! changes one direct dependency's pin for ONE run, touches no file, and is written into the run
//! record so the evidence says which version was judged (gap 01a103f0: the integration train's
//! release gate diffs a consumer's verdict under the old tag and the new one).
//!
//! - `name=v1.4.0` / `name=tag:v1.4.0` — a git dependency, pinned to that tag (its URL and module
//!   kept).
//! - `name=3f2c9a1` / `name=rev:3f2c9a1…` — a git dependency, pinned to that commit.
//! - `name=../standards` / `name=path:…` — any dependency, served from a local path instead.
//!
//! A bare spec is a path when it looks like one (`/`, `./`, `../`, `~`) or names an existing
//! directory; a 7–40 hex string is a rev; anything else is a tag. Prefixes settle any doubt.
//! Only a DIRECT dependency can be overridden: the consumer's own `[dependencies]` is what the
//! precedence rule lets it own, and a name it does not declare is refused with the names it does
//! (and `-P` for adding an ad-hoc package).

use std::collections::BTreeMap;

use crate::manifest::{PackageDetail, PackageSource};

/// What the dependency is pinned to for this run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Pin {
    Path(String),
    Tag(String),
    Rev(String),
}

/// One requested override, and where it was asked for (`--dep` or `PROVA_DEP_<NAME>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DepOverride {
    pub(crate) name: String,
    pub(crate) pin: Pin,
    pub(crate) origin: String,
}

/// The env var prefix of an override: `PROVA_DEP_<NAME>`, NAME uppercased with `-` as `_`.
pub(crate) const ENV_PREFIX: &str = "PROVA_DEP_";

/// Parse `name=spec` (as `--dep` gives it).
pub(crate) fn parse(entry: &str, origin: &str) -> Result<DepOverride, String> {
    match entry.split_once('=') {
        Some((name, spec)) if !name.is_empty() && !spec.is_empty() => Ok(DepOverride {
            name: name.to_string(),
            pin: parse_pin(spec)?,
            origin: origin.to_string(),
        }),
        _ => Err(format!(
            "{origin} expects name=<tag|rev|path> (e.g. --dep standards=v1.4.0), got {entry:?}"
        )),
    }
}

fn parse_pin(spec: &str) -> Result<Pin, String> {
    let non_empty = |kind: &str, v: &str| {
        if v.is_empty() { Err(format!("{kind}: needs a value")) } else { Ok(v.to_string()) }
    };
    if let Some(v) = spec.strip_prefix("tag:") {
        return non_empty("tag", v).map(Pin::Tag);
    }
    if let Some(v) = spec.strip_prefix("rev:") {
        return non_empty("rev", v).map(Pin::Rev);
    }
    if let Some(v) = spec.strip_prefix("path:") {
        return non_empty("path", v).map(Pin::Path);
    }
    let looks_like_path = spec.starts_with('/')
        || spec.starts_with("./")
        || spec.starts_with("../")
        || spec.starts_with('~')
        || std::path::Path::new(spec).is_dir();
    if looks_like_path {
        return Ok(Pin::Path(spec.to_string()));
    }
    if (7..=40).contains(&spec.len()) && spec.chars().all(|c| c.is_ascii_hexdigit()) {
        return Ok(Pin::Rev(spec.to_string()));
    }
    Ok(Pin::Tag(spec.to_string()))
}

/// The `PROVA_DEP_<NAME>` overrides among `vars`. NAME is matched against the dependency names
/// later (uppercased, `-` as `_`), so the override keeps the env spelling until then.
pub(crate) fn from_env(vars: impl Iterator<Item = (String, String)>) -> Result<Vec<DepOverride>, String> {
    let mut out = Vec::new();
    for (key, value) in vars {
        let Some(name) = key.strip_prefix(ENV_PREFIX) else { continue };
        if name.is_empty() || value.is_empty() {
            return Err(format!("{key} needs a dependency name and a value (e.g. {ENV_PREFIX}STANDARDS=v1.4.0)"));
        }
        out.push(DepOverride { name: name.to_string(), pin: parse_pin(&value)?, origin: key });
    }
    Ok(out)
}

fn env_spelling(name: &str) -> String {
    name.to_ascii_uppercase().replace('-', "_")
}

/// One override as it was applied: the dependency, its manifest pin, the pin this run used, and
/// who asked. Rendered into the run record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Applied {
    pub(crate) name: String,
    pub(crate) from: String,
    pub(crate) to: String,
    pub(crate) origin: String,
}

impl Applied {
    /// The record's line: `standards: tag v1.3.0 -> tag v1.4.0 (--dep)`.
    pub(crate) fn line(&self) -> String {
        format!("{}: {} -> {} ({})", self.name, self.from, self.to, self.origin)
    }
}

fn describe(source: &PackageSource) -> String {
    match source {
        PackageSource::Path(p) => format!("path {p}"),
        PackageSource::Detailed(d) => match (&d.path, &d.git, &d.tag, &d.rev, &d.branch) {
            (Some(p), ..) => format!("path {p}"),
            (None, Some(_), Some(t), ..) => format!("tag {t}"),
            (None, Some(_), None, Some(r), _) => format!("rev {r}"),
            (None, Some(_), None, None, Some(b)) => format!("branch {b}"),
            (None, Some(_), None, None, None) => "git default branch".to_string(),
            (None, None, ..) => "an empty source".to_string(),
        },
    }
}

/// Apply `overrides` to the consumer's direct `deps`, returning what each changed. A CLI override
/// beats an env one for the same dependency (the later entry in `overrides` wins; callers pass env
/// first). Refuses, naming the fix: an unknown name, a tag or rev on a dependency that is not a git
/// source.
pub(crate) fn apply(
    deps: &mut BTreeMap<String, PackageSource>,
    overrides: &[DepOverride],
) -> Result<Vec<Applied>, String> {
    let mut applied: BTreeMap<String, Applied> = BTreeMap::new();
    for o in overrides {
        let name = if o.origin.starts_with(ENV_PREFIX) {
            deps.keys().find(|k| env_spelling(k) == o.name).cloned()
        } else {
            deps.contains_key(&o.name).then(|| o.name.clone())
        };
        let Some(name) = name else {
            let known: Vec<&str> = deps.keys().map(String::as_str).collect();
            return Err(format!(
                "{}: no direct dependency named {:?}; this package declares [{}]. An override changes a declared dependency's pin for one run; to add a package that is not declared, use -P name=path",
                o.origin,
                o.name,
                known.join(", ")
            ));
        };
        let current = deps.get(&name).cloned().unwrap_or(PackageSource::Path(String::new()));
        let from = applied.get(&name).map_or_else(|| describe(&current), |a| a.from.clone());
        let next = match (&o.pin, &current) {
            (Pin::Path(p), _) => PackageSource::Path(p.clone()),
            (Pin::Tag(_) | Pin::Rev(_), PackageSource::Detailed(d)) if d.git.is_some() => {
                let mut d: PackageDetail = d.clone();
                d.tag = None;
                d.rev = None;
                d.branch = None;
                match &o.pin {
                    Pin::Tag(t) => d.tag = Some(t.clone()),
                    Pin::Rev(r) => d.rev = Some(r.clone()),
                    Pin::Path(_) => {}
                }
                PackageSource::Detailed(d)
            }
            (Pin::Tag(_) | Pin::Rev(_), _) => {
                return Err(format!(
                    "{}: {name} is {} in the manifest, not a git source, so it has no tag or rev to change; override it with a path (--dep {name}=path:<dir>)",
                    o.origin,
                    describe(&current)
                ));
            }
        };
        let to = describe(&next);
        deps.insert(name.clone(), next);
        applied.insert(name.clone(), Applied { name, from, to, origin: o.origin.clone() });
    }
    Ok(applied.into_values().collect())
}

#[cfg(test)]
#[path = "dep_override_tests.rs"]
mod tests;
