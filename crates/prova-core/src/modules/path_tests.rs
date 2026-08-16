//! The path algebra and the option/filter boundaries of the parent module.
//!
//! Chosen for what a silent bug COSTS rather than for uncovered-line count. These are the places
//! where a wrong answer does not raise: a path that crosses to Lua mis-normalized corrupts every
//! assertion written against it, and a journal filter that keeps the wrong entries lets a proof
//! assert confidently about a message the SUT never sent. Both fail as *plausible data*, which is
//! the failure mode a black-box suite is worst at catching.

use super::*;

// ---------------------------------------------------------------------------------------------
// separator normalization — one convention for every path prova emits
// ---------------------------------------------------------------------------------------------

/// Windows renders `\`, and canonicalization grows a `\\?\` verbatim prefix. Both leak into
/// TOML-embedding, shell-quoting and pattern-matching in proofs, so every emitted path is
/// `/`-normalized — which means this function is load-bearing for cross-platform assertions.
#[test]
fn normalization_strips_the_verbatim_prefix_and_speaks_one_separator() {
    assert_eq!(path_norm_seps(r"C:\a\b"), "C:/a/b");
    assert_eq!(path_norm_seps(r"\\?\C:\a\b"), "C:/a/b", "the verbatim prefix is stripped first");
    assert_eq!(path_norm_seps("/already/slashed"), "/already/slashed");
    assert_eq!(path_norm_seps(r"mixed/sep\arators"), "mixed/sep/arators");
    assert_eq!(path_norm_seps(""), "");
}

/// The prefix is stripped only where it IS a prefix. A path that merely contains the sequence
/// later on keeps it — otherwise normalization would edit the middle of a name.
#[test]
fn the_verbatim_prefix_is_a_prefix_not_a_substring() {
    // `\\?\` appearing after the first component is part of the name, and only its separators
    // normalize. Nothing is deleted.
    assert_eq!(path_norm_seps(r"a\\?\b"), "a//?/b");
}

/// `emit_path` is where an OS path becomes a Lua string, and its contract is platform-split for a
/// reason: a unix filename may legally CONTAIN a backslash, so blanket replacement there would
/// corrupt a legitimate name — silently, in the value the proof then asserts on.
#[test]
fn emitting_a_path_normalizes_on_windows_and_is_byte_exact_everywhere_else() {
    use std::path::Path;
    assert_eq!(emit_path(Path::new("/tmp/plain")), "/tmp/plain");

    #[cfg(not(windows))]
    {
        // The load-bearing case: `we\ird` is a valid unix filename, not a separator mistake.
        assert_eq!(
            emit_path(Path::new(r"/tmp/we\ird")),
            r"/tmp/we\ird",
            "a backslash in a unix filename is content and must survive emission"
        );
    }
    #[cfg(windows)]
    {
        assert_eq!(emit_path(Path::new(r"C:\tmp\x")), "C:/tmp/x");
        assert_eq!(emit_path(Path::new(r"\\?\C:\tmp\x")), "C:/tmp/x");
    }
}

// ---------------------------------------------------------------------------------------------
// roots — what a path is anchored to decides what may be trimmed off it
// ---------------------------------------------------------------------------------------------

/// Every lexical verb (`dirname`, `basename`, trimming) splits at the root first, so a misread
/// root does not error — it returns a *different path*. The UNC case is the sharp one: the double
/// slash is load-bearing, and reading it as a plain unix root turns `//server/share` into a path
/// under `/`, pointing filesystem work at the wrong machine.
#[test]
fn a_root_is_read_as_unc_unix_drive_or_relative() {
    assert_eq!(path_root("//server/share/x"), "//", "UNC: the double slash is the root");
    assert_eq!(path_root("/a/b"), "/");
    assert_eq!(path_root("C:/a"), "C:/");
    assert_eq!(path_root("c:/a"), "c:/", "drive letters are case-insensitive");
    assert_eq!(path_root("a/b"), "", "relative paths have no root");
    assert_eq!(path_root(""), "");
}

/// The boundaries around the UNC test, each of which the naive predicate gets wrong.
#[test]
fn root_detection_holds_at_its_edges() {
    assert_eq!(path_root("//"), "/", "a bare double slash has no server/share under it");
    assert_eq!(path_root("///a"), "/", "three or more slashes is not a UNC share");
    assert_eq!(path_root("1:/a"), "", "a drive letter must be alphabetic");
    assert_eq!(path_root("C:a"), "", "drive-relative is not rooted — no separator after the colon");
    assert_eq!(path_root("C:"), "", "too short to carry a separator");
}

/// Absoluteness is asked of paths in either convention, so it normalizes before it reads.
#[test]
fn absoluteness_is_asked_in_either_convention() {
    assert!(path_is_absolute(r"C:\x"), "a windows path is absolute before normalization too");
    assert!(path_is_absolute("/x"));
    assert!(path_is_absolute("//server/share"));
    assert!(!path_is_absolute("x/y"));
    assert!(!path_is_absolute(""));
}

/// Trailing separators are noise except when they ARE the root: trimming `/` to `""` would turn
/// an absolute path into a relative one, which is the silent kind of wrong.
#[test]
fn trailing_separators_are_trimmed_but_a_root_is_never_eaten() {
    assert_eq!(path_trim_trailing("a/b/"), "a/b");
    assert_eq!(path_trim_trailing("a//"), "a");
    assert_eq!(path_trim_trailing("/"), "/", "the unix root survives");
    assert_eq!(path_trim_trailing("C:/"), "C:/", "…so does a drive root");
    assert_eq!(path_trim_trailing("//server/share/"), "//server/share");
    assert_eq!(path_trim_trailing("a/b"), "a/b", "nothing to trim");
}

// ---------------------------------------------------------------------------------------------
// journal filtering — which recorded interactions an assertion gets to see
// ---------------------------------------------------------------------------------------------

/// A filter that keeps the wrong entries does not fail: it hands the proof a journal that looks
/// right and lets it assert confidently about traffic the SUT never produced. Absent filter and
/// explicit `nil` must both mean "everything" — a filter argument that goes missing must not
/// silently become "nothing".
#[test]
fn an_absent_filter_keeps_every_entry() {
    let lua = Lua::new();
    let entry = lua.create_table().unwrap();
    entry.set("method", "GET").unwrap();

    assert!(journal_keep(&lua, &None, &entry).unwrap());
    assert!(journal_keep(&lua, &Some(Value::Nil), &entry).unwrap());
}

/// A table filter is a SUBSET match, matching `:matches` — extra keys on the entry are
/// unconstrained, and every key the author wrote must hold.
#[test]
fn a_table_filter_matches_a_subset_of_the_entry() {
    let lua = Lua::new();
    let entry = lua.create_table().unwrap();
    entry.set("method", "POST").unwrap();
    entry.set("path", "/orders").unwrap();
    entry.set("status", 201).unwrap();

    let keep = |pairs: &[(&str, &str)]| {
        let shape = lua.create_table().unwrap();
        for (k, v) in pairs {
            shape.set(*k, *v).unwrap();
        }
        journal_keep(&lua, &Some(Value::Table(shape)), &entry).unwrap()
    };

    assert!(keep(&[("method", "POST")]), "one matching key is enough — extras are unconstrained");
    assert!(keep(&[("method", "POST"), ("path", "/orders")]));
    assert!(!keep(&[("method", "GET")]), "a mismatched key drops the entry");
    assert!(!keep(&[("method", "POST"), ("path", "/other")]), "every stated key must hold");
    assert!(!keep(&[("absent", "x")]), "a key the entry lacks is a mismatch, not a pass");
}

/// A function filter is a Lua predicate, so it answers by LUA truthiness — where `0` and `""` are
/// true and only `nil`/`false` are not. Reading it with Rust's intuition would drop every entry a
/// predicate scored as `0`, which is exactly the kind of counting predicate people write.
#[test]
fn a_function_filter_answers_by_lua_truthiness() {
    let lua = Lua::new();
    let entry = lua.create_table().unwrap();
    entry.set("status", 200).unwrap();

    let keep = |src: &str| {
        let f: mlua::Function = lua.load(src).eval().unwrap();
        journal_keep(&lua, &Some(Value::Function(f)), &entry).unwrap()
    };

    assert!(keep("function(e) return e.status == 200 end"));
    assert!(!keep("function(e) return e.status == 404 end"));
    assert!(!keep("function(e) return nil end"), "nil drops the entry");
    assert!(keep("function(e) return 0 end"), "0 is TRUE in Lua — the entry is kept");
    assert!(keep("function(e) return '' end"), "so is the empty string");
}

/// Anything else is refused by name rather than silently keeping or dropping everything. A filter
/// the runtime cannot honor must not look like it worked.
#[test]
fn a_filter_that_is_neither_table_nor_function_is_refused_by_name() {
    let lua = Lua::new();
    let entry = lua.create_table().unwrap();
    let bogus = Value::String(lua.create_string("method == GET").unwrap());

    let err = journal_keep(&lua, &Some(bogus), &entry).unwrap_err().to_string();
    assert!(err.contains("filter must be a table"), "got: {err}");
    assert!(err.contains("string"), "the message names what it actually received; got: {err}");
}

// ---------------------------------------------------------------------------------------------
// client options — the shared constructor boundary for the HTTP-flavored clients
// ---------------------------------------------------------------------------------------------

#[cfg(any(feature = "http", feature = "graphql"))]
mod client {
    use super::*;

    fn opts(lua: &Lua, build: impl FnOnce(&Table)) -> Table {
        let t = lua.create_table().unwrap();
        build(&t);
        t
    }

    /// The URL key differs per constructor (`http.client` says `base_url`, `graphql.client` says
    /// `url`), so the closed set is built per call — and the refusal must name the key the caller
    /// was actually supposed to use, not a generic one.
    #[test]
    fn the_url_is_required_under_the_constructors_own_key() {
        let lua = Lua::new();
        let err = client_opts(&opts(&lua, |_| {}), "http.client", "base_url")
            .unwrap_err()
            .to_string();
        assert!(err.contains("base_url"), "got: {err}");
        assert!(err.contains("http.client"), "the site is named too; got: {err}");

        let (url, headers, timeout) =
            client_opts(&opts(&lua, |t| t.set("url", "http://x").unwrap()), "graphql.client", "url")
                .unwrap();
        assert_eq!(url, "http://x");
        assert!(headers.is_empty());
        assert!(timeout.is_none());
    }

    /// The options table is CLOSED: a typo'd key used to be dropped silently, leaving a client
    /// configured differently than it reads (the 0.24.0 upgrade hazard).
    #[test]
    fn an_unknown_key_is_refused_rather_than_dropped() {
        let lua = Lua::new();
        let t = opts(&lua, |t| {
            t.set("base_url", "http://x").unwrap();
            t.set("timeut", "5s").unwrap(); // the typo that used to cost a timeout
        });
        let err = client_opts(&t, "http.client", "base_url").unwrap_err().to_string();
        assert!(err.contains("timeut"), "the refusal names the offending key; got: {err}");
    }

    /// Headers cross as pairs, and the timeout parses through the same duration grammar as every
    /// other wait in the tree.
    #[test]
    fn headers_and_timeout_cross_intact() {
        let lua = Lua::new();
        let t = opts(&lua, |t| {
            t.set("base_url", "http://x").unwrap();
            t.set("timeout", "1500ms").unwrap();
            let h = lua.create_table().unwrap();
            h.set("authorization", "Bearer tok").unwrap();
            t.set("headers", h).unwrap();
        });

        let (_, headers, timeout) = client_opts(&t, "http.client", "base_url").unwrap();
        assert_eq!(headers, vec![("authorization".to_string(), "Bearer tok".to_string())]);
        assert_eq!(timeout, Some(std::time::Duration::from_millis(1500)));
    }
}
