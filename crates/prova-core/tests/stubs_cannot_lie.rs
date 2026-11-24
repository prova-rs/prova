//! The stubs cannot lie — the API surface an editor and `prova introspect` teach is the API the
//! runtime serves, checked in both directions.
//!
//! The LuaCATS stubs (`library/*.lua`) are hand-written and canonical (help.rs: one source, two
//! sinks). Nothing tied them to the runtime, and they drifted both ways: `sarif`, `measure` and
//! `date` shipped with no stub at all, while the stubs taught `str.*` and `path.*` as globals the
//! runtime only serves under `prova.*`. This walk builds the real environment, lists every function
//! it serves, and holds the stub entries to that list.
//!
//! Exceptions live in `WAIVERS`, each with its reason, and the list only shrinks: a waiver whose
//! name now agrees FAILS as stale, so a fixed drift cannot leave its excuse behind.

use std::collections::BTreeSet;

use prova_core::{eval_snippet, help, RunConfig};

/// Every function the runtime serves, by its canonical path. `prova.*` is the first-party surface
/// (bundled modules join it whether or not `[globals] inject` makes them ambient); the bundled
/// `require` modules are walked under their module names.
const WALK: &str = r#"
local out = {}
local function walk(prefix, t)
  for k, v in pairs(t) do
    if type(k) == "string" then
      if type(v) == "function" then
        out[#out + 1] = prefix .. k
      elseif type(v) == "table" and k:match("^[a-z_][a-z0-9_]*$") and prefix == "prova." then
        for k2, v2 in pairs(v) do
          if type(k2) == "string" and type(v2) == "function" then
            out[#out + 1] = prefix .. k .. "." .. k2
          end
        end
      end
    end
  end
end
walk("prova.", prova)
-- Bare globals that are not bundled modules (`report`, `suite`, a host's `archetect`): served
-- where they stand, so they are walked under their own names.
local std = { _G = 1, coroutine = 1, io = 1, math = 1, os = 1, package = 1, string = 1,
  table = 1, utf8 = 1, debug = 1 }
for k, v in pairs(_G) do
  if type(k) == "string" and type(v) == "table" and not std[k] then
    for k2, v2 in pairs(v) do
      if type(k2) == "string" and type(v2) == "function" then out[#out + 1] = k .. "." .. k2 end
    end
  end
end
for _, m in ipairs({ "prova.workspace", "prova.double", "prova.cargo" }) do
  local ok, mod = pcall(require, m)
  if ok and type(mod) == "table" then walk(m .. ".", mod) end
end
table.sort(out)
return out
"#;

fn live() -> BTreeSet<String> {
    let v = eval_snippet(WALK, &RunConfig::new(1)).expect("the walk runs in the full environment");
    v.as_array()
        .expect("the walk returns a list")
        .iter()
        .map(|s| s.as_str().expect("names are strings").to_string())
        .collect()
}

/// A stub entry's canonical path, or `None` for what this walk does not cover (classes and
/// methods, which need an instance to reach).
fn canonical(name: &str) -> Option<String> {
    if name.contains(':') {
        return None; // a method: checked where an instance can be built, not here
    }
    let (owner, _) = name.split_once('.')?;
    if owner.chars().next().is_some_and(char::is_uppercase) {
        // A class's constructor-style function (`Double.new`) lives on its bundled module.
        return match owner {
            "Double" => Some(format!("prova.double.{}", &name[owner.len() + 1..])),
            _ => None,
        };
    }
    // A bundled module's canonical home is `prova.<name>`; anything else is a bare global.
    if owner != "prova" && prova_core::RESERVED_NAMESPACES.contains(&owner) {
        return Some(format!("prova.{name}"));
    }
    Some(name.to_string())
}

fn stubbed() -> BTreeSet<String> {
    // Deprecated entries count: a name the runtime still serves is stubbed when an editor strikes
    // it through, which is exactly what `---@deprecated` does.
    help::CORE_STUBS
        .iter()
        .flat_map(|(_, src)| help::parse_stub_with(src, true))
        .collect::<Vec<_>>()
        .iter()
        // `prova.Foo` entries are classes (value shapes), not functions.
        .filter(|e| e.signature.starts_with('('))
        .filter_map(|e| canonical(&e.name))
        .collect()
}

/// Known disagreements, each with why it is allowed today. Only shrinks.
const WAIVERS: &[(&str, &str)] = &[
    (
        "archetect.render",
        "served by the prova binary's host (prova-cli wires prova-archetect); prova-core alone does not",
    ),
    (
        "archetect.verify",
        "served by the prova binary's host (prova-cli wires prova-archetect); prova-core alone does not",
    ),
    (
        "runtime.capability",
        "served only inside the deprecated prova.lua companion; retires with it at 1.0",
    ),
    ("prova.conduct_cache.file", "the bundled prova.cargo adapter's internal seam, not an authoring API"),
    ("prova.conduct_cache.key", "the bundled prova.cargo adapter's internal seam, not an authoring API"),
    ("prova.conduct_cache.lookup", "the bundled prova.cargo adapter's internal seam, not an authoring API"),
];

/// The disagreements between what the runtime serves and what the stubs teach, after waivers:
/// (served but not stubbed, stubbed but not served, stale waivers).
fn disagreements<'a>(
    live: &'a BTreeSet<String>,
    stubbed: &'a BTreeSet<String>,
    waivers: &[(&'a str, &str)],
) -> (Vec<&'a String>, Vec<&'a String>, Vec<&'a str>) {
    let waived = |n: &str| waivers.iter().any(|(w, _)| *w == n);
    let unstubbed = live.difference(stubbed).filter(|n| !waived(n)).collect();
    let unserved = stubbed.difference(live).filter(|n| !waived(n)).collect();
    let stale = waivers
        .iter()
        .map(|(w, _)| *w)
        .filter(|w| live.contains(*w) == stubbed.contains(*w))
        .collect();
    (unstubbed, unserved, stale)
}

#[test]
fn every_function_the_runtime_serves_is_stubbed_and_every_stub_is_served() {
    let live = live();
    let stubbed = stubbed();
    assert!(live.len() > 100, "the walk found too little to be the real surface: {}", live.len());
    let (unstubbed, unserved, stale) = disagreements(&live, &stubbed, WAIVERS);

    assert!(
        unstubbed.is_empty() && unserved.is_empty() && stale.is_empty(),
        "the stubs must teach exactly the API the runtime serves.\n\n\
         SERVED BUT NOT STUBBED — the editor and `prova introspect` cannot see these; add a stub in \
         library/*.lua:\n  {}\n\n\
         STUBBED BUT NOT SERVED — the editor offers what the runtime lacks (or names the wrong \
         path); fix the stub:\n  {}\n\n\
         STALE WAIVERS — these agree now; delete the entry (the list only shrinks):\n  {}",
        join(&unstubbed),
        join(&unserved),
        stale.join("\n  ")
    );
}

fn join(names: &[&String]) -> String {
    names.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n  ")
}

/// The check has teeth: doctor each side once and demand the lie is named. A checker that passed
/// these would pass a drifted tree too.
#[test]
fn a_doctored_surface_is_named() {
    let live = live();
    let mut stubbed = stubbed();

    // A stub for a function the runtime does not serve (the `retries` / `str.*` class of lie).
    stubbed.insert("prova.fs.teleport".to_string());
    // A served function whose stub went missing (the `sarif` / `date` class).
    assert!(stubbed.remove("prova.fs.read"), "fs.read is stubbed, so removing it doctors the stubs");

    let (unstubbed, unserved, stale) = disagreements(&live, &stubbed, WAIVERS);
    assert_eq!(unstubbed, vec!["prova.fs.read"], "a missing stub is named");
    assert_eq!(unserved, vec!["prova.fs.teleport"], "a stub for nothing is named");
    assert!(stale.is_empty());

    // A waiver for a name that agrees is stale, so the list cannot keep an excuse it no longer needs.
    let (_, _, stale) = disagreements(&live, &stubbed, &[("prova.fs.write", "no reason")]);
    assert_eq!(stale, vec!["prova.fs.write"], "a waiver over an agreeing name is stale");
}

/// Every closed option table takes its accepted set from the stubs (`crate::stub_opts`, generated
/// by build.rs from `library/*.lua`), never from a hand-written list. A hand list is a second
/// source, and a second source is how `TestOpts` came to teach `spec` and `retries` while the
/// runtime refused both. This census keeps a new gate from quietly starting one.
#[test]
fn no_option_gate_keeps_a_hand_written_list() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = Vec::new();
    let mut stack = vec![src.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("src/ reads") {
            let path = entry.expect("an entry").path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            // opts.rs is the gate itself; its own tests exercise it with literal sets.
            if path.extension().is_none_or(|e| e != "rs") || path.ends_with("opts.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("a source file reads");
            let lines: Vec<&str> = text.lines().collect();
            for (i, line) in lines.iter().enumerate() {
                let window = lines[i..lines.len().min(i + 3)].join(" ");
                let hand_const = line.contains("OPTS: &[&str] = &[");
                let hand_call = (line.contains("reject_unknown") && window.contains("&[\""))
                    || line.contains("accepted: &[\"");
                if hand_const || hand_call {
                    let rel = path.strip_prefix(&src).unwrap_or(&path).display().to_string();
                    offenders.push(format!("{rel}:{}: {}", i + 1, line.trim()));
                }
            }
        }
    }
    offenders.sort();
    assert!(
        offenders.is_empty(),
        "an option gate keeps its own list — declare the options as a `---@class prova.<Name>` in \
         library/*.lua and use `crate::stub_opts::<NAME>` (build.rs generates it):\n  {}",
        offenders.join("\n  ")
    );
}
