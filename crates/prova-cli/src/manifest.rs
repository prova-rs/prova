//! The suite manifest (`prova.toml`) — declare *what* to run and *how*, so `prova` with no
//! arguments runs the configured suite and CI is just `prova`. A general-purpose harness needs a
//! stable way to name suites and environments across local and CI; this is it.
//!
//! ```toml
//! [run]                         # the default profile
//! paths  = ["tests"]            # files/dirs to discover
//! jobs   = 4                    # concurrency (throughput only)
//! format = "console"           # "console" | "json"
//!
//! [run.env]                     # environment for the run
//! LOG = "info"
//!
//! [profiles.ci]                 # `prova --profile ci` overlays this on [run]
//! jobs = 8
//! [profiles.ci.env]
//! CI = "true"
//! ```

use std::collections::BTreeMap;

use serde::Deserialize;

/// A parsed `prova.toml`. The `[run]` table is the default profile; `[profiles.<name>]` tables are
/// overlays selected with `--profile <name>`.
#[derive(Debug, Deserialize, Default)]
pub struct Manifest {
    #[serde(default)]
    pub run: Profile,
    #[serde(default)]
    pub profiles: BTreeMap<String, Profile>,
}

/// One run profile. Every field is optional so a profile can override just what it needs.
#[derive(Debug, Deserialize, Default, Clone)]
pub struct Profile {
    #[serde(default)]
    pub paths: Vec<String>,
    pub jobs: Option<usize>,
    pub format: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// A fully-resolved run configuration (base `[run]` with an optional profile overlaid).
#[derive(Debug, PartialEq)]
pub struct Resolved {
    pub paths: Vec<String>,
    pub jobs: Option<usize>,
    pub format: Option<String>,
    pub env: BTreeMap<String, String>,
}

impl Manifest {
    pub fn parse(text: &str) -> Result<Manifest, String> {
        toml::from_str(text).map_err(|e| format!("invalid prova.toml: {e}"))
    }

    /// Overlay a profile on the base `[run]` profile. `None` uses the base as-is; `Some(name)` takes
    /// each field from the profile when present, otherwise from the base. Env is base-then-profile
    /// (profile wins). Errors if the named profile does not exist.
    pub fn resolve(&self, profile: Option<&str>) -> Result<Resolved, String> {
        let base = &self.run;
        let overlay = match profile {
            None => None,
            Some(name) => Some(
                self.profiles
                    .get(name)
                    .ok_or_else(|| format!("no such profile {name:?} in prova.toml"))?,
            ),
        };

        let paths = match overlay {
            Some(p) if !p.paths.is_empty() => p.paths.clone(),
            _ => base.paths.clone(),
        };
        let jobs = overlay.and_then(|p| p.jobs).or(base.jobs);
        let format = overlay
            .and_then(|p| p.format.clone())
            .or_else(|| base.format.clone());

        let mut env = base.env.clone();
        if let Some(p) = overlay {
            for (k, v) in &p.env {
                env.insert(k.clone(), v.clone());
            }
        }

        Ok(Resolved {
            paths,
            jobs,
            format,
            env,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
[run]
paths  = ["tests"]
jobs   = 4
format = "console"

[run.env]
LOG = "info"

[profiles.ci]
jobs = 8
[profiles.ci.env]
CI = "true"

[profiles.smoke]
paths = ["tests/smoke"]
"#;

    fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn default_profile_uses_run_table() {
        let m = Manifest::parse(SAMPLE).unwrap();
        let r = m.resolve(None).unwrap();
        assert_eq!(
            r,
            Resolved {
                paths: vec!["tests".into()],
                jobs: Some(4),
                format: Some("console".into()),
                env: env(&[("LOG", "info")]),
            }
        );
    }

    #[test]
    fn profile_overlays_base_and_merges_env() {
        let m = Manifest::parse(SAMPLE).unwrap();
        let r = m.resolve(Some("ci")).unwrap();
        // jobs overridden; paths + format inherited from [run]; env is base-then-profile.
        assert_eq!(r.jobs, Some(8));
        assert_eq!(r.paths, vec!["tests".to_string()]);
        assert_eq!(r.format.as_deref(), Some("console"));
        assert_eq!(r.env, env(&[("CI", "true"), ("LOG", "info")]));
    }

    #[test]
    fn profile_can_override_paths() {
        let m = Manifest::parse(SAMPLE).unwrap();
        let r = m.resolve(Some("smoke")).unwrap();
        assert_eq!(r.paths, vec!["tests/smoke".to_string()]);
        assert_eq!(r.jobs, Some(4)); // inherited
    }

    #[test]
    fn unknown_profile_is_an_error() {
        let m = Manifest::parse(SAMPLE).unwrap();
        assert!(m.resolve(Some("nope")).is_err());
    }

    #[test]
    fn empty_manifest_resolves_to_empty_defaults() {
        let m = Manifest::parse("").unwrap();
        let r = m.resolve(None).unwrap();
        assert!(r.paths.is_empty());
        assert_eq!(r.jobs, None);
    }
}
