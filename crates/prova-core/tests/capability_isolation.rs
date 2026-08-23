use std::path::{Path, PathBuf};

use prova_core::{load_project_config, RunConfig};

/// Two project companions, loaded in ONE process — exactly what the warm MCP does when it resolves
/// project A at startup and then `run { project = "B" }`. Each project's capabilities must be its
/// own: B must never inherit what only A's `prova.lua` registered.
///
/// This is the bug the first `prova.capability` cut introduced: a process-global registry, populated
/// per resolve and never cleared, so the second project saw the first's capabilities. The fix makes
/// registration a per-load value ([`prova_core::Capabilities`]) carried in `RunConfig`, so there is
/// no shared state to leak through.
fn companion(dir: &Path, name: &str, body: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, body).unwrap();
    p
}

#[test]
fn companions_do_not_leak_across_resolves() {
    let cfg = RunConfig::new(1);
    let dir = std::env::temp_dir().join(format!("prova-caps-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let a = companion(
        &dir,
        "a.lua",
        r#"runtime.capability("iso_a", function() return true end)"#,
    );
    let b = companion(
        &dir,
        "b.lua",
        r#"runtime.capability("iso_b", function() return true end)"#,
    );

    // Each resolve returns its OWN capability set. Same process, back to back.
    let caps_a = load_project_config(&a, &cfg).expect("load A");
    let caps_b = load_project_config(&b, &cfg).expect("load B");

    // A saw its own, B saw its own — the sanity legs.
    assert!(
        caps_a.available("iso_a"),
        "A's own capability present in A's set"
    );
    assert!(
        caps_b.available("iso_b"),
        "B's own capability present in B's set"
    );

    // THE PROOF: B's set must not contain A's capability. B is a different project; it cannot inherit
    // A's vocabulary. With the process-global registry it did — this is the isolation the fix buys.
    assert!(
        !caps_b.available("iso_a"),
        "project B inherited project A's capability — capabilities are not per-resolve"
    );

    // …and built-ins still work through any set (registered names are consulted first, then these).
    assert!(caps_a.available("unix") == cfg!(unix));

    std::fs::remove_dir_all(&dir).ok();
}

/// A capability predicate must be able to make an async-backed call.
///
/// `runtime.capability`'s own documentation offers "a GPU, a licence file, a kind cluster" as the
/// motivating cases, and two of those three want to make a call — probing a real dependency is the
/// whole point of a custom capability. The companion used to load from plain sync `main()`, so any
/// async-backed API panicked with "there is no reactor running"; supplying a reactor alone still left
/// "attempt to yield from outside a coroutine", because neither the chunk nor the predicate was
/// invoked asynchronously.
///
/// The failure mode is nastier than a crash: a plugin that wrapped its probe in `pcall` read the
/// panic as "the dependency is unreachable", silently degraded to a presence-only check, and ran its
/// suite against a credential the registry rejects.
///
/// Asserts the call REACHED the network stack — a refused connection to a closed port proves the
/// reactor was there to refuse it. Deliberately NOT "some particular error is absent": an earlier
/// version of this test asserted `not find("no reactor running")` and duly passed once the error
/// merely changed to the coroutine one, while the async call remained impossible.
#[test]
fn a_capability_predicate_can_make_an_async_call() {
    let cfg = RunConfig::new(1);
    let dir = std::env::temp_dir().join(format!("prova-caps-async-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let errfile = dir.join("probe_err.txt");
    let c = companion(
        &dir,
        "async.lua",
        &format!(
            r#"
        -- Port 1 is reserved and never listening, so the connect gets a socket-level outcome.
        local ok, err = pcall(function()
          return http.get("http://127.0.0.1:1/", {{ timeout = "2s" }})
        end)
        _G.__probe_err = tostring(err)
        local f = io.open([[{}]], "w"); f:write(_G.__probe_err); f:close()
        -- What that outcome LOOKS like varies: unix refuses ("Connection refused"), Windows
        -- refuses in its own words ("actively refused", os error 10061) or — on runners whose
        -- firewall silently drops loopback port 1 — times out. Every spelling is a TRANSPORT
        -- outcome, which is the point: the call reached the network stack. A missing reactor
        -- ("no reactor running") or coroutine error contains none of these tokens.
        runtime.capability("async_probe_ran", function()
          local e = tostring(_G.__probe_err):lower()
          for _, token in ipairs({{ "refused", "timed out", "timeout", "unreachable", "reset" }}) do
            if e:find(token, 1, true) then return true end
          end
          return false
        end)
        "#,
            errfile.display()
        ),
    );

    let caps = load_project_config(&c, &cfg).expect("companion loads");
    assert!(
        caps.available("async_probe_ran"),
        "a predicate's async call must reach the network stack (refused/timed out/unreachable), \
         got: {:?}",
        std::fs::read_to_string(&errfile).unwrap_or_else(|_| "<probe never ran>".into())
    );
}

/// The same isolation, on the CURRENT mechanism. The test above guards the deprecated companion
/// path; `[capabilities]` is what the warm MCP will resolve going forward, and the property has to
/// hold there or the bug simply moves.
///
/// Two shapes of leak are possible now that were not before, because a declaration carries more than
/// a verdict:
///
///   1. the DECLARATIONS themselves — B must not see a name only A declared;
///   2. the memoized ANSWERS — the lazily-probed kinds cache per `Capabilities`, and a shared cache
///      would let A's "absent" answer for B. Each resolve mints its own memo (a fresh `Arc`), and
///      only clones of ONE set share it — which is what makes a run's worker threads probe once
///      while two projects stay independent.
#[test]
fn declared_capabilities_do_not_leak_across_resolves() {
    use prova_core::{
        resolve_capabilities, CapabilityFactory, CapabilityRegistration, CommandProbe,
        UndeclaredPolicy, VersionQuery,
    };

    // A command probe, so this exercises the LAZY kind — the one with a cache to leak through.
    // `sh` on unix / `cmd` on windows: present either way, so the answer is a real MET, not a
    // vacuous agreement between two absent tools.
    let present = if cfg!(windows) { "cmd" } else { "sh" };
    let decl = |name: &str, command: &str| CapabilityRegistration {
        name: name.to_string(),
        factory: CapabilityFactory::Command(CommandProbe {
            command: command.to_string(),
            version: VersionQuery::None,
            ..Default::default()
        }),
    };

    let cfg = RunConfig::new(1);
    let caps_a = resolve_capabilities(
        &[decl("iso_decl_a", present)],
        UndeclaredPolicy::Error,
        &cfg,
    )
    .expect("resolve A");
    let caps_b = resolve_capabilities(
        &[decl("iso_decl_b", present)],
        UndeclaredPolicy::Error,
        &cfg,
    )
    .expect("resolve B");

    // Sanity legs: each set answers for its own declaration, and answers MET (so the proof below is
    // not two absent tools agreeing).
    assert!(caps_a.available("iso_decl_a"), "A answers for its own");
    assert!(caps_b.available("iso_decl_b"), "B answers for its own");

    // THE PROOF, part 1: the declaration did not travel.
    assert!(
        caps_b.declaration("iso_decl_a").is_none(),
        "project B sees a capability only project A declared"
    );
    // …and under a CLOSED vocabulary an undeclared name is a config error, which is the sharpest
    // form of the same statement: B does not merely answer "unavailable", it refuses the name.
    assert!(
        caps_b.expr_status("iso_decl_a").is_err(),
        "B must refuse a name it never declared, not silently answer for it"
    );

    // THE PROOF, part 2: the memo did not travel either. A has now probed `iso_decl_a` (the
    // assertions above forced it), so a shared cache would surface it here under B.
    assert!(
        !caps_b.available("iso_decl_a"),
        "project B read project A's memoized answer — the probe cache is not per-resolve"
    );

    // The other half of the memo contract: CLONES of one set DO share it. That is what lets a run's
    // worker threads (each holding a cloned `RunConfig`) probe the host once instead of N times.
    let clone_a = caps_a.clone();
    assert!(
        clone_a.available("iso_decl_a"),
        "a clone answers from the shared memo"
    );
    assert!(
        clone_a.declaration("iso_decl_a").is_some(),
        "a clone carries the declarations"
    );
}
