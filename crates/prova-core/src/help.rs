//! `prova.help()` — the API surface, discoverable **from inside the environment being driven**.
//!
//! **One source, two sinks.** The LuaCATS stubs in `library/*.lua` already ship to a test author's
//! editor (prova-cli's `annotations.rs` embeds and syncs them). They are hand-written, rich, and
//! canonical — so this module makes them the source for a *second* sink: structured data an
//! **agent** can read at runtime, without opening prova's source.
//!
//! That framing matters. An IDE stub serves a human in an editor; it is invisible to an agent
//! driving `prova eval`. Before this, learning `shell.run`'s return shape meant guessing field
//! names and probing with `for k in pairs(r)`. The stub already said `code`/`stdout`/`stderr`/
//! `duration` — it just could not be *asked*. See `docs/design/agent-ergonomics.md` §0.
//!
//! Deriving from the stub (rather than a parallel Rust registry) is deliberate: a registry would be
//! a second place to write the same summary, and the two would drift. The stub is what ships to the
//! editor, so it is the copy that cannot be allowed to rot.

/// The core LuaCATS stubs, embedded once and consumed twice: here (→ `prova.help`) and by
/// prova-cli's `annotations.rs` (→ the IDE annotation folder).
pub const CORE_STUBS: &[(&str, &str)] = &[
    ("prova.lua", include_str!("../../../library/prova.lua")),
    ("modules.lua", include_str!("../../../library/modules.lua")),
];

/// One documented thing: a function, a method, or a class (a value shape).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelpEntry {
    /// `shell.run`, `Context:manage`, `prova.ShellResult`.
    pub name: String,
    /// `(command: string, opts?: prova.ShellOpts) -> prova.ShellResult`, or for a class the field
    /// shape `{ code: integer, stdout: string, … }`.
    pub signature: String,
    /// The stub's prose, collapsed to one line.
    pub summary: String,
}

/// Split a LuaCATS type off its trailing `# note` comment.
fn split_note(s: &str) -> (String, Option<String>) {
    match s.split_once('#') {
        Some((ty, note)) => {
            let note = note.trim();
            (
                ty.trim().to_string(),
                (!note.is_empty()).then(|| note.to_string()),
            )
        }
        None => (s.trim().to_string(), None),
    }
}

/// Collapse accumulated `---` prose lines into one summary line.
fn collapse(prose: &[String]) -> String {
    prose
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Parse one `---@meta` LuaCATS stub into help entries.
///
/// Recognises the shapes the stubs actually use: `--- prose`, `---@param n ty`, `---@return ty`,
/// `---@class Name`, `---@field n ty`, and the `function name(args) end` / `function C:m() end`
/// declarations they document. Anything else is ignored — this reads documentation, it does not
/// type-check Lua.
pub fn parse_stub(src: &str) -> Vec<HelpEntry> {
    let mut out = Vec::new();
    let mut prose: Vec<String> = Vec::new();
    let mut params: Vec<(String, String)> = Vec::new();
    let mut ret: Option<String> = None;
    // A class stays open across its `---@field` lines and flushes when the block ends.
    let mut class: Option<(String, String, Vec<(String, String, Option<String>)>)> = None;

    let flush_class =
        |class: &mut Option<(String, String, Vec<(String, String, Option<String>)>)>,
         out: &mut Vec<HelpEntry>| {
            if let Some((name, summary, fields)) = class.take() {
                let body = fields
                    .iter()
                    .map(|(n, ty, note)| match note {
                        Some(note) => format!("{n}: {ty}  -- {note}"),
                        None => format!("{n}: {ty}"),
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                out.push(HelpEntry {
                    name,
                    signature: if body.is_empty() {
                        "{}".into()
                    } else {
                        format!("{{ {body} }}")
                    },
                    summary,
                });
            }
        };

    for line in src.lines() {
        let t = line.trim();

        if let Some(rest) = t.strip_prefix("---@class ") {
            flush_class(&mut class, &mut out);
            let name = rest.split_whitespace().next().unwrap_or("").to_string();
            class = Some((name, collapse(&prose), Vec::new()));
            prose.clear();
            continue;
        }
        if let Some(rest) = t.strip_prefix("---@field ") {
            if let Some((n, ty)) = rest.split_once(char::is_whitespace) {
                let (ty, note) = split_note(ty);
                if let Some((_, _, fields)) = class.as_mut() {
                    fields.push((n.trim().to_string(), ty, note));
                }
            }
            continue;
        }
        if let Some(rest) = t.strip_prefix("---@param ") {
            flush_class(&mut class, &mut out);
            if let Some((n, ty)) = rest.split_once(char::is_whitespace) {
                params.push((n.trim().to_string(), split_note(ty).0));
            }
            continue;
        }
        if let Some(rest) = t.strip_prefix("---@return ") {
            flush_class(&mut class, &mut out);
            ret = Some(split_note(rest).0);
            continue;
        }
        // Prose: `--- text` (but not another `---@tag` we don't model).
        if let Some(rest) = t.strip_prefix("---") {
            if !rest.starts_with('@') {
                let text = rest.trim();
                if !text.is_empty() {
                    prose.push(text.to_string());
                }
                continue;
            }
            continue; // an unmodelled tag — ignore, don't let it leak into prose
        }
        // A declaration closes the block: `function shell.run(command, opts) end`.
        if let Some(rest) = t.strip_prefix("function ") {
            flush_class(&mut class, &mut out);
            if let Some((name, _)) = rest.split_once('(') {
                let name = name.trim().to_string();
                let args = params
                    .iter()
                    .map(|(n, ty)| format!("{n}: {ty}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                let sig = match &ret {
                    Some(r) => format!("({args}) -> {r}"),
                    None => format!("({args})"),
                };
                if !name.is_empty() {
                    out.push(HelpEntry {
                        name,
                        signature: sig,
                        summary: collapse(&prose),
                    });
                }
            }
            prose.clear();
            params.clear();
            ret = None;
            continue;
        }
        // Any other line ends an open block (e.g. `local ShellResult = {}` after a class).
        if !t.is_empty() {
            flush_class(&mut class, &mut out);
        }
        if t.is_empty() {
            prose.clear();
            params.clear();
            ret = None;
        }
    }
    flush_class(&mut class, &mut out);
    out
}

/// Every entry from the embedded core stubs, sorted by name.
pub fn core_entries() -> Vec<HelpEntry> {
    let mut out: Vec<HelpEntry> = CORE_STUBS
        .iter()
        .flat_map(|(_, src)| parse_stub(src))
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out.dedup_by(|a, b| a.name == b.name);
    out
}

/// Case-insensitive substring match across name and summary — `help("shell")`, `help("retry")`.
pub fn filter(entries: &[HelpEntry], needle: &str) -> Vec<HelpEntry> {
    let n = needle.to_lowercase();
    entries
        .iter()
        .filter(|e| e.name.to_lowercase().contains(&n) || e.summary.to_lowercase().contains(&n))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_documented_function() {
        let entries = parse_stub(
            r#"
--- Start a long-running command in the background (a booted app, a mock server) and return a
--- handle. stdout/stderr are discarded.
---@param command string
---@param opts? prova.SpawnOpts
---@return prova.Process
function shell.spawn(command, opts) end
"#,
        );
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "shell.spawn");
        assert_eq!(
            entries[0].signature,
            "(command: string, opts?: prova.SpawnOpts) -> prova.Process"
        );
        assert!(entries[0]
            .summary
            .starts_with("Start a long-running command"));
        // Prose wraps across lines in the stubs — it must collapse to one line.
        assert!(!entries[0].summary.contains('\n'));
    }

    #[test]
    fn parses_a_class_into_its_field_shape() {
        // The exact stub whose shape an agent had to GUESS before this existed.
        let entries = parse_stub(
            r#"
--- The result of a finished command.
---@class prova.ShellResult
---@field code integer
---@field stdout string
---@field duration number   # seconds
local ShellResult = {}
---@return boolean          # code == 0
function ShellResult:ok() end
"#,
        );
        let sr = entries
            .iter()
            .find(|e| e.name == "prova.ShellResult")
            .expect("class entry");
        assert_eq!(
            sr.signature,
            "{ code: integer, stdout: string, duration: number  -- seconds }"
        );
        assert_eq!(sr.summary, "The result of a finished command.");
        // The method documented under the class is its own entry.
        let ok = entries
            .iter()
            .find(|e| e.name == "ShellResult:ok")
            .expect("method entry");
        assert_eq!(ok.signature, "() -> boolean");
    }

    /// Every documented FUNCTION carries a summary. This is prova's own stated requirement —
    /// "learnable without reading its source" — enforced rather than aspired to.
    ///
    /// Why functions and not classes: a help entry must **answer**. A function can only answer with
    /// prose, because a signature cannot say what it *does* or what it costs you to get wrong —
    /// `fs.glob(root, pattern) -> string[]` cannot tell you the paths come back ABSOLUTE, and that
    /// gap cost a real failed run. A class answers with its FIELDS: `ShellResult { code, stdout,
    /// stderr, duration }` needs no sentence. So the bar is "can an agent act on this entry alone?"
    ///
    /// This lives in Rust, next to the embedded stubs, rather than as a repo-level policy: it is an
    /// in-crate invariant over data the crate itself carries, so it belongs where `cargo test` will
    /// find it.
    #[test]
    fn every_documented_function_has_a_summary() {
        let entries = core_entries();
        let missing: Vec<&str> = entries
            .iter()
            // A function entry's signature starts with its parameter list; a class's is a field shape.
            .filter(|e| e.signature.starts_with('(') && e.summary.trim().is_empty())
            .map(|e| e.name.as_str())
            .collect();
        assert!(
            missing.is_empty(),
            "{} function(s) are in the stubs but say nothing about themselves, so `prova.help()` \
             cannot answer for them and an agent must read prova's source instead:\n  {}",
            missing.len(),
            missing.join("\n  ")
        );
    }

    #[test]
    fn the_real_stubs_cover_what_cost_probes() {
        let all = core_entries();
        assert!(
            all.len() > 20,
            "expected a substantial surface, got {}",
            all.len()
        );
        // Each of these was a wasted round-trip during the first dogfood (agent-ergonomics.md §0).
        for name in [
            "shell.run",
            "shell.spawn",
            "prova.ShellResult",
            "Context:tempdir",
        ] {
            assert!(
                all.iter().any(|e| e.name == name),
                "help() must cover `{name}`"
            );
        }
        // Filtering is how an agent narrows 100+ entries to the one it needs.
        assert!(!filter(&all, "shell").is_empty());
        assert!(filter(&all, "zzz-no-such-thing").is_empty());
    }
}
