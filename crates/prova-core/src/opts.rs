//! Closed option tables — one gate for every surface that takes an `opts` table.
//!
//! **An option prova cannot honor is refused, never dropped.** A dropped option is worse than a
//! rejected one because it reads as *configured*: `tiemout = "10m"` means unbounded, and the suite
//! that believes it is bounded finds out from a hung CI job.
//!
//! The unit surface (`prova.test`'s opts) has been closed since
//! `agent-ergonomics.md#unknown-test-opts-silently-ignored`. The module surface — `shell.run`,
//! `docker.build`, `docker.run`, `http` — was not, and it is the same disease one layer over:
//! every one of them parses by key lookup, so a typo'd key reads as configured exactly as a unit
//! opt did (`agent-ergonomics.md#module-opts-silently-ignored`).
//!
//! Version skew is the wrinkle that makes the module case sharper than the unit case. A proof
//! written against a NEWER prova — `docker.build{ first_byte = "90s" }` — runs on an older binary
//! that has never heard of the option, drops it, and passes while proving nothing about the bound
//! it names. Refusing is what turns that into the loud, accurate "this proof needs a newer prova"
//! it always was.

use mlua::{Table, Value};

/// A key prova does not honor, paired with what the author actually wants. The key's own name is
/// the least useful thing to say about it: they asked for a *behavior*, and need where it lives.
///
/// Two kinds earn an entry, and they are the same failure from either end — a spelling prova
/// USED to accept (`spec`, whose removal turned tolerated open specs into hard failures), and one
/// it NEVER accepted but that a reasonable author will reach for anyway (`args` on `shell.spawn`,
/// which every other process API in the world takes). Nearest-spelling suggestion cannot rescue
/// either: there is nothing close to `args` in `{cwd, env}`, so without a teaching the message is
/// a true and useless "unknown option `args`".
pub(crate) type Teaching = (&'static str, &'static str);

/// What one option-taking surface accepts — closed by construction.
pub(crate) struct Closed<'a> {
    /// Every key the site honors.
    pub accepted: &'a [&'a str],
    /// Keys honored but deliberately not advertised — deprecated spellings on their way out, so
    /// the message never teaches a spelling it is trying to retire.
    pub hidden: &'a [&'a str],
    /// Keys prova refuses, each with where the behavior it names actually lives.
    pub teachings: &'a [Teaching],
    /// A concrete named-option example for the positional-entry hint, where one surface has a
    /// characteristic mistake worth naming. `None` gives the generic wording.
    pub example: Option<&'a str>,
}

impl<'a> Closed<'a> {
    /// The common case: everything advertised, no deprecation history.
    pub fn of(accepted: &'a [&'a str]) -> Self {
        Self {
            accepted,
            hidden: &[],
            teachings: &[],
            example: None,
        }
    }

    /// Refuse any key this surface cannot honor. `site` names the verb in the error, and should
    /// carry enough identity to make the fix one jump (`docker.run`, `prova.test("boots")`).
    ///
    /// Unknown keys are collected and sorted before reporting: Lua table order is unspecified, and
    /// a diagnostic that names a different key on each run is not a diagnostic.
    pub fn check(&self, t: &Table, site: &str) -> mlua::Result<()> {
        let mut unknown: Vec<String> = Vec::new();
        let mut positional = 0usize;
        for pair in t.clone().pairs::<Value, Value>() {
            let (k, _) = pair?;
            match k {
                Value::String(s) => {
                    let key = s.to_string_lossy();
                    if !self.accepted.contains(&key.as_ref()) {
                        unknown.push(key.to_string());
                    }
                }
                // A positional entry is the same silent drop wearing a different shape:
                // `{ "slow" }` looks like tags to the author and is nothing to prova.
                _ => positional += 1,
            }
        }
        if unknown.is_empty() && positional == 0 {
            return Ok(());
        }
        unknown.sort();
        let advertised: Vec<&str> = self
            .accepted
            .iter()
            .copied()
            .filter(|k| !self.hidden.contains(k))
            .collect();
        let mut parts: Vec<String> = unknown
            .iter()
            .map(|key| match self.teachings.iter().find(|(r, _)| r == key) {
                Some((_, teaching)) => format!("`{key}` {teaching}"),
                None => match crate::suggest::nearest(key, advertised.iter().copied()) {
                    Some(best) => format!("unknown option `{key}` — did you mean `{best}`?"),
                    None => format!("unknown option `{key}`"),
                },
            })
            .collect();
        if positional > 0 {
            let shape = match self.example {
                Some(example) => format!("(`{example}`, not `{{ … }}`)"),
                None => "(`key = value`)".to_string(),
            };
            parts.push(format!(
                "{positional} positional entr{} in the opts table — options are named {shape}",
                if positional == 1 { "y" } else { "ies" }
            ));
        }
        Err(mlua::Error::RuntimeError(format!(
            "{site}: {} (accepted: {}). An option prova cannot honor is refused, never dropped — \
             a dropped one reads as configured.",
            parts.join("; "),
            advertised.join(", ")
        )))
    }
}

/// Refuse any key `accepted` does not name — the module-surface spelling, where every accepted key
/// is advertised and there is no deprecation history to carry.
pub(crate) fn reject_unknown(t: &Table, accepted: &[&str], site: &str) -> mlua::Result<()> {
    Closed::of(accepted).check(t, site)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(lua: &mlua::Lua, keys: &[(&str, &str)]) -> Table {
        let t = lua.create_table().unwrap();
        for (k, v) in keys {
            t.set(*k, *v).unwrap();
        }
        t
    }

    /// The accepted set passes untouched — the property every refusal below is only safe because
    /// of, since a gate that refused everything would satisfy them all.
    #[test]
    fn an_accepted_key_passes() {
        let lua = mlua::Lua::new();
        let t = table(&lua, &[("timeout", "30s"), ("cwd", ".")]);
        assert!(reject_unknown(&t, &["cwd", "timeout"], "site").is_ok());
    }

    #[test]
    fn an_unknown_key_is_named_with_the_accepted_set() {
        let lua = mlua::Lua::new();
        let t = table(&lua, &[("bogus", "1")]);
        let err = reject_unknown(&t, &["cwd", "timeout"], "shell.run").unwrap_err().to_string();
        assert!(err.contains("shell.run"), "the site is named: {err}");
        assert!(err.contains("bogus"), "the key is named: {err}");
        assert!(err.contains("cwd, timeout"), "the accepted set is listed: {err}");
    }

    /// Proximity earns a suggestion; distance does not. A confident wrong suggestion is worse than
    /// none, which is why the far case must stay silent rather than reach for the nearest string.
    #[test]
    fn a_near_miss_is_suggested_and_a_far_one_is_not() {
        let lua = mlua::Lua::new();
        let near = reject_unknown(&table(&lua, &[("tiemout", "1")]), &["timeout"], "s")
            .unwrap_err()
            .to_string();
        assert!(near.contains("did you mean `timeout`"), "{near}");

        let far = reject_unknown(&table(&lua, &[("parallelism", "1")]), &["timeout"], "s")
            .unwrap_err()
            .to_string();
        assert!(!far.contains("did you mean"), "no guess when nothing is close: {far}");
    }

    /// Lua table order is unspecified, so an unsorted report would name a different key on each
    /// run — a diagnostic that changes under you is not a diagnostic. Hard to see from a black-box
    /// proof, which observes one ordering at a time.
    #[test]
    fn several_unknown_keys_are_reported_in_a_stable_order() {
        let lua = mlua::Lua::new();
        let t = table(&lua, &[("zulu", "1"), ("alpha", "2"), ("mike", "3")]);
        let err = reject_unknown(&t, &["known"], "s").unwrap_err().to_string();
        let (a, m, z) = (
            err.find("alpha").unwrap(),
            err.find("mike").unwrap(),
            err.find("zulu").unwrap(),
        );
        assert!(a < m && m < z, "sorted, not table order: {err}");
    }

    /// A hidden key is honored but never advertised — the deprecated spelling on its way out, which
    /// the message must not teach to someone who has not already got it.
    #[test]
    fn a_hidden_key_is_accepted_but_unadvertised() {
        let lua = mlua::Lua::new();
        let closed = Closed {
            accepted: &["locks", "resources"],
            hidden: &["resources"],
            teachings: &[],
            example: None,
        };
        assert!(closed.check(&table(&lua, &[("resources", "x")]), "s").is_ok());

        let err = closed.check(&table(&lua, &[("bogus", "x")]), "s").unwrap_err().to_string();
        assert!(err.contains("locks"), "the live spelling is offered: {err}");
        assert!(!err.contains("resources"), "the retiring one is not: {err}");
    }

    /// A teaching replaces the bare denial for keys where nearest-spelling has nothing to offer —
    /// `args` has no neighbour in `{cwd, env}`, so without this the message is true and useless.
    #[test]
    fn a_teaching_replaces_the_bare_denial() {
        let lua = mlua::Lua::new();
        let closed = Closed {
            accepted: &["cwd", "env"],
            hidden: &[],
            teachings: &[("args", "is not an option — pass an argv table")],
            example: None,
        };
        let err = closed.check(&table(&lua, &[("args", "x")]), "shell.spawn").unwrap_err().to_string();
        assert!(err.contains("pass an argv table"), "{err}");
        assert!(!err.contains("unknown option"), "the teaching replaces it: {err}");
    }

    /// A positional entry is the same silent drop wearing a different shape: `{ "slow" }` reads as
    /// tags to the author and is nothing to prova.
    #[test]
    fn positional_entries_are_counted_and_pluralized() {
        let lua = mlua::Lua::new();
        let one = lua.create_table().unwrap();
        one.set(1, "slow").unwrap();
        let err = reject_unknown(&one, &["tags"], "s").unwrap_err().to_string();
        assert!(err.contains("1 positional entry"), "singular: {err}");

        let two = lua.create_table().unwrap();
        two.set(1, "a").unwrap();
        two.set(2, "b").unwrap();
        let err = reject_unknown(&two, &["tags"], "s").unwrap_err().to_string();
        assert!(err.contains("2 positional entries"), "plural: {err}");
    }
}
