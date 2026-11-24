//! The `---@class` tables of the LuaCATS stubs, read as data — shared by `build.rs` (which
//! generates the accepted-option sets the runtime enforces) and the crate's own tests.
//!
//! This is what makes an option table unable to drift: the stub class an editor completes from IS
//! the set `opts::reject_unknown` accepts, because the accepted set is generated from it at build
//! time. Document an option in the stub and the runtime honors it; remove it and the runtime
//! refuses it. There is no second list to forget.
//!
//! Self-contained on purpose (std only, no crate items): `build.rs` compiles this file by path.

/// One `---@class prova.Name [: prova.Parent]` and the `---@field`s declared directly under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StubClass {
    pub name: String,
    pub parent: Option<String>,
    pub fields: Vec<String>,
}

/// Every class in one stub file, in declaration order. A class's fields are the `---@field` lines
/// that follow it up to the first line that is not a `---` comment.
pub fn parse(src: &str) -> Vec<StubClass> {
    let mut out: Vec<StubClass> = Vec::new();
    let mut open = false;
    for line in src.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("---@class ") {
            let (name, parent) = match rest.split_once(':') {
                Some((n, p)) => (n.trim(), p.split_whitespace().next().map(str::to_string)),
                None => (rest.split_whitespace().next().unwrap_or(""), None),
            };
            out.push(StubClass { name: name.to_string(), parent, fields: Vec::new() });
            open = true;
            continue;
        }
        if !open {
            continue;
        }
        if let Some(rest) = t.strip_prefix("---@field ") {
            if let Some(name) = rest.split_whitespace().next() {
                if let Some(class) = out.last_mut() {
                    class.fields.push(name.trim_end_matches('?').to_string());
                }
            }
            continue;
        }
        if !t.starts_with("---") {
            open = false;
        }
    }
    out
}

/// A class's fields including every ancestor's, sorted and deduplicated. `None` for an unknown
/// class; an unknown PARENT is a stub defect and is reported the same way.
pub fn fields_of(classes: &[StubClass], name: &str) -> Option<Vec<String>> {
    let class = classes.iter().find(|c| c.name == name)?;
    let mut fields = class.fields.clone();
    if let Some(parent) = &class.parent {
        fields.extend(fields_of(classes, parent)?);
    }
    fields.sort();
    fields.dedup();
    Some(fields)
}

/// `prova.DockerRunOpts` → `DOCKER_RUN_OPTS`: the generated constant's name.
pub fn const_name(class: &str) -> String {
    let base = class.strip_prefix("prova.").unwrap_or(class);
    let mut out = String::new();
    for (i, c) in base.chars().enumerate() {
        if c.is_ascii_uppercase() && i > 0 {
            out.push('_');
        }
        out.push(c.to_ascii_uppercase());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "\
---@class prova.UnitOpts
---@field tags? string[]       # selection tags
---@field serial? boolean

---@class prova.TestOpts : prova.UnitOpts
---@field timeout? string
local TestOpts = {}
---@field stray string
";

    #[test]
    fn a_class_carries_its_own_and_its_ancestors_fields() {
        let classes = parse(SRC);
        assert_eq!(classes.len(), 2);
        assert_eq!(classes[1].parent.as_deref(), Some("prova.UnitOpts"));
        assert_eq!(
            fields_of(&classes, "prova.TestOpts").unwrap(),
            vec!["serial", "tags", "timeout"],
            "inherited fields join, the optional marker is dropped, a field after the block is not the class's"
        );
        assert_eq!(fields_of(&classes, "prova.Nope"), None);
    }

    #[test]
    fn a_class_name_becomes_its_constant_name() {
        assert_eq!(const_name("prova.DockerRunOpts"), "DOCKER_RUN_OPTS");
        assert_eq!(const_name("prova.TestOpts"), "TEST_OPTS");
    }
}
