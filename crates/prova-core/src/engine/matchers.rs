//! Matchers and value helpers: the `t:expect` chain, `:eventually`, snapshots, and
//! the structural comparison/display primitives they share.

use super::*;

// ---------------------------------------------------------------------------------------------
// Matchers
// ---------------------------------------------------------------------------------------------

/// One `:eventually` poll observation, deposited by a probe-mode `Matcher`: `(passed, message)`.
pub(super) type ProbeState = Rc<RefCell<Option<(bool, String)>>>;

pub(super) struct Matcher {
    pub(super) subject: Value,
    pub(super) label: Option<String>,
    pub(super) negated: bool,
    pub(super) run: Rc<RefCell<TestRun>>,
    /// `:eventually` probe mode: when set, `record` deposits `(passed, message)` here instead of
    /// counting an assertion or raising — one poll iteration, observed by the retry loop.
    pub(super) probe: Option<ProbeState>,
}

impl Matcher {
    fn record(&self, raw_pass: bool, detail: impl FnOnce() -> String) -> mlua::Result<()> {
        let passed = raw_pass ^ self.negated;
        if let Some(probe) = &self.probe {
            let msg = if passed {
                String::new()
            } else {
                let prefix = self
                    .label
                    .as_ref()
                    .map(|l| format!("{l}: "))
                    .unwrap_or_default();
                let neg = if self.negated { "not: " } else { "" };
                format!("{prefix}{neg}{}", detail())
            };
            *probe.borrow_mut() = Some((passed, msg));
            return Ok(());
        }
        let mut r = self.run.borrow_mut();
        r.assertions += 1;
        if passed {
            return Ok(());
        }
        let prefix = self
            .label
            .as_ref()
            .map(|l| format!("{l}: "))
            .unwrap_or_default();
        let neg = if self.negated { "not: " } else { "" };
        let msg = format!("{prefix}{neg}{}", detail());
        if r.soft {
            // Inside `expect_all`: collect and keep going.
            r.soft_failures.push(msg);
            Ok(())
        } else {
            r.failure = Some(msg.clone());
            Err(mlua::Error::RuntimeError(msg))
        }
    }
}

/// The `:eventually` handle (docs/plans/api-freeze.md §4): returned by
/// `t:expect(fn):eventually(opts?)`, it dispatches ANY terminal matcher — `__index` hands back an
/// async closure that re-evaluates the function subject and re-runs that matcher (via a
/// probe-mode `Matcher`) until it passes or the deadline lapses. Sugar over the same
/// poll-until-truthy idea as `prova.retry`, which stays the public primitive.
#[derive(Clone)]
pub(super) struct Eventually {
    pub(super) func: mlua::Function,
    pub(super) label: Option<String>,
    pub(super) negated: bool,
    pub(super) run: Rc<RefCell<TestRun>>,
    pub(super) timeout: Duration,
    pub(super) every: Duration,
}

impl UserData for Eventually {
    fn add_methods<M: mlua::UserDataMethods<Self>>(methods: &mut M) {
        methods.add_meta_method(mlua::MetaMethod::Index, |lua, this, name: String| {
            let ev = this.clone();
            lua.create_async_function(move |lua, args: mlua::MultiValue| {
                let ev = ev.clone();
                let name = name.clone();
                async move {
                    // `ev:gte(3)` sugars to `f(ev, 3)`: drop the handle, keep the matcher args.
                    let rest: mlua::MultiValue = args.into_iter().skip(1).collect();
                    // Lua-semantics dispatch onto a probe matcher: `m[name](m, ...)`.
                    let dispatch: mlua::Function = lua
                        .load("return function(m, name, ...) return m[name](m, ...) end")
                        .eval()?;
                    let deadline = Instant::now() + ev.timeout;
                    let mut last = format!("the probe was never evaluated (timeout {:?})", ev.timeout);
                    loop {
                        // Re-evaluate the subject; a raise means "not yet", exactly like prova.retry.
                        match ev.func.call_async::<Value>(()).await {
                            Ok(value) => {
                                let state = Rc::new(RefCell::new(None));
                                let probe = lua.create_userdata(Matcher {
                                    subject: value,
                                    label: ev.label.clone(),
                                    negated: ev.negated,
                                    run: ev.run.clone(),
                                    probe: Some(state.clone()),
                                })?;
                                // A raise from the matcher itself (bad arguments) is a programming
                                // error — propagate, never retry.
                                let mut call_args = mlua::MultiValue::new();
                                call_args.push_back(Value::UserData(probe));
                                call_args.push_back(Value::String(lua.create_string(&name)?));
                                for v in rest.clone() {
                                    call_args.push_back(v);
                                }
                                dispatch.call_async::<()>(call_args).await?;
                                let observed = state.borrow_mut().take();
                                match observed {
                                    Some((true, _)) => {
                                        // Honored: one real assertion for the whole poll.
                                        let real = Matcher {
                                            subject: Value::Nil,
                                            label: None,
                                            negated: false,
                                            run: ev.run.clone(),
                                            probe: None,
                                        };
                                        return real.record(true, String::new);
                                    }
                                    Some((false, msg)) => last = msg,
                                    // The dispatched method never recorded (e.g. `never`, which
                                    // returns a new matcher): not a terminal matcher — refuse.
                                    None => {
                                        return Err(mlua::Error::RuntimeError(format!(
                                            "eventually:{name} is not a terminal matcher — apply modifiers before :eventually()",
                                        )));
                                    }
                                }
                            }
                            Err(err) => last = err.to_string(),
                        }
                        if Instant::now() >= deadline {
                            let real = Matcher {
                                subject: Value::Nil,
                                label: None,
                                negated: false,
                                run: ev.run.clone(),
                                probe: None,
                            };
                            let timeout = ev.timeout;
                            return real.record(false, move || {
                                format!("eventually timed out after {timeout:?} — last: {last}")
                            });
                        }
                        tokio::time::sleep(ev.every).await;
                    }
                }
            })
        });
    }
}

/// Serialize a `matches_snapshot` subject to the string that gets stored/compared, honoring the
/// **level** dial. A string subject is its own content. A **filesystem subject** — any Lua table with
/// a `path` string field (the convention every prova path-handle follows: `archetect.render` output,
/// `out:file(...)`, `out:dir(...)`) — serializes at a level:
///
/// - `layout` — the sorted relative file paths (the render's *shape*; stable, low-rot). Default for a
///   directory subject.
/// - `content` — the paths plus each file's bytes, as `=== path ===` sections. Default for a *file*
///   subject (a single file has one content and no meaningful "layout").
///
/// The default-by-kind is the anti-rot guard: a broad directory snapshot defaults to the cheap shape,
/// and you *opt into* `content`.
pub(super) fn serialize_snapshot_subject(subject: &Value, level: Option<&str>) -> Result<String, String> {
    match subject {
        Value::String(s) => Ok(s.to_string_lossy().to_string()),
        Value::Table(t) => {
            let path: Option<String> = t.get("path").ok().flatten();
            let path = path.ok_or_else(|| {
                "matches_snapshot: table subject must be a path handle (a `path` field); \
                 got a table without one"
                    .to_string()
            })?;
            serialize_path(Path::new(&path), level)
        }
        // The snapshot protocol: a userdata exposing `snapshot_text()` snapshots as that text —
        // how a terminal `Screen` becomes a golden frame without the engine knowing its type.
        Value::UserData(ud) => {
            use mlua::ObjectLike;
            ud.call_method::<String>("snapshot_text", ()).map_err(|_| {
                "matches_snapshot: userdata subject must expose snapshot_text() \
                 (a terminal Screen does)"
                    .to_string()
            })
        }
        other => Err(format!(
            "matches_snapshot expects a string or a filesystem path-handle subject, got {}",
            other.type_name()
        )),
    }
}

/// Serialize a filesystem path at a snapshot level (see [`serialize_snapshot_subject`]).
pub(super) fn serialize_path(path: &Path, level: Option<&str>) -> Result<String, String> {
    let meta = std::fs::metadata(path)
        .map_err(|e| format!("matches_snapshot: cannot stat {}: {e}", path.display()))?;

    if meta.is_file() {
        // A single file: `content` is the only meaningful level.
        if matches!(level, Some("layout")) {
            return Err(format!(
                "matches_snapshot: level=\"layout\" needs a directory subject, but {} is a file",
                path.display()
            ));
        }
        return std::fs::read_to_string(path)
            .map_err(|e| format!("matches_snapshot: cannot read {}: {e}", path.display()));
    }

    // A directory: default to the low-rot `layout` (shape), opt into `content`.
    let rels = walk_files_relative(path)?;
    match level.unwrap_or("layout") {
        "layout" => Ok(rels.join("\n")),
        "content" => {
            let mut out = String::new();
            for rel in &rels {
                let full = path.join(rel);
                let body = std::fs::read_to_string(&full)
                    .unwrap_or_else(|_| "<binary or unreadable>".to_string());
                out.push_str(&format!("=== {rel} ===\n{body}\n"));
            }
            Ok(out.trim_end().to_string())
        }
        other => Err(format!(
            "matches_snapshot: unknown level {other:?} (expected \"layout\" or \"content\")"
        )),
    }
}

/// Every file under `root`, as `/`-separated relative paths, sorted — a deterministic layout listing.
pub(super) fn walk_files_relative(root: &Path) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir)
            .map_err(|e| format!("matches_snapshot: cannot read dir {}: {e}", dir.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("matches_snapshot: dir entry error: {e}"))?;
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else if let Ok(rel) = p.strip_prefix(root) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    out.sort();
    Ok(out)
}

/// A filesystem-safe slug of a node path (or a user-given snapshot name): alphanumerics kept,
/// everything else collapsed to single `-`, lowercased. `"orders › creates a row"` → `"orders-creates-a-row"`.
pub(super) fn slugify(s: &str) -> String {
    let mut out = String::new();
    let mut pending_dash = false;
    for c in s.chars() {
        if c.is_alphanumeric() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            out.extend(c.to_lowercase());
            pending_dash = false;
        } else {
            pending_dash = true;
        }
    }
    if out.is_empty() {
        "snapshot".to_string()
    } else {
        out
    }
}

/// The stored `.snap` document: a small header (for review context) then a `---` line, then the raw
/// body. The lone `---` delimiter is robust — a body starting with `#!/bin/sh` or containing later
/// `---` lines round-trips, since only the *first* `---` splits header from body.
pub(super) fn format_snapshot(source: &str, body: &str) -> String {
    format!("prova-snapshot v1\nsource: {source}\n---\n{body}")
}

/// Extract the body from a stored `.snap` document (everything after the first lone `---` line). A
/// document with no delimiter (hand-written / legacy) is treated as all-body.
pub(super) fn snapshot_body(doc: &str) -> &str {
    match doc.split_once("\n---\n") {
        Some((_header, body)) => body,
        None => doc,
    }
}

/// Write a snapshot document, creating the `snapshots/` dir if needed. Returns a message on failure.
pub(super) fn write_snapshot(path: &Path, doc: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create snapshot dir {}: {e}", dir.display()))?;
    }
    std::fs::write(path, doc).map_err(|e| format!("cannot write snapshot {}: {e}", path.display()))
}

/// A minimal LCS-based line diff (`  ` context, `- ` expected-only, `+ ` actual-only), for the
/// snapshot mismatch message. O(n·m) — fine for snapshot-sized inputs.
pub(super) fn line_diff(expected: &str, actual: &str) -> String {
    let a: Vec<&str> = expected.lines().collect();
    let b: Vec<&str> = actual.lines().collect();
    let (n, m) = (a.len(), b.len());
    // dp[i][j] = LCS length of a[i..] and b[j..].
    let mut dp = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i][j] = if a[i] == b[j] {
                dp[i + 1][j + 1] + 1
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }
    let mut out = String::new();
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if a[i] == b[j] {
            out.push_str(&format!("    {}\n", a[i]));
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            out.push_str(&format!("  - {}\n", a[i]));
            i += 1;
        } else {
            out.push_str(&format!("  + {}\n", b[j]));
            j += 1;
        }
    }
    for line in &a[i..] {
        out.push_str(&format!("  - {line}\n"));
    }
    for line in &b[j..] {
        out.push_str(&format!("  + {line}\n"));
    }
    out.trim_end().to_string()
}

/// Chain modes: `:never()` (negation) and `:eventually{...}` (the poll matcher).
fn add_mode_methods<M: UserDataMethods<Matcher>>(methods: &mut M) {
    methods.add_method("never", |lua, this, ()| {
        lua.create_userdata(Matcher {
            subject: this.subject.clone(),
            label: this.label.clone(),
            negated: !this.negated,
            run: this.run.clone(),
            probe: this.probe.clone(),
        })
    });

    // `:eventually(opts?)` — poll-until-matches (docs/plans/api-freeze.md §4). Legal only on
    // a FUNCTION subject: the returned handle re-evaluates it (and the terminal matcher that
    // follows) until pass or timeout. `opts = { timeout, every }`, defaults matching
    // `prova.retry` — which remains the public primitive this sugars over.
    methods.add_method("eventually", |lua, this, opts: Option<Table>| {
        let Value::Function(func) = &this.subject else {
            return Err(mlua::Error::RuntimeError(
                "eventually requires a function subject — wrap the probe: t:expect(function() return ... end):eventually():matches{...}"
                    .into(),
            ));
        };
        let get = |key: &str, default: Duration| -> mlua::Result<Duration> {
            match &opts {
                Some(t) => match t.get::<Option<String>>(key)? {
                    Some(s) => parse_duration(&s).ok_or_else(|| {
                        mlua::Error::RuntimeError(format!(
                            "eventually: cannot parse {key} {s:?} (try \"30s\", \"500ms\")"
                        ))
                    }),
                    None => Ok(default),
                },
                None => Ok(default),
            }
        };
        let timeout = get("timeout", Duration::from_secs(30))?;
        let every = get("every", Duration::from_millis(500))?;
        lua.create_userdata(Eventually {
            func: func.clone(),
            label: this.label.clone(),
            negated: this.negated,
            run: this.run.clone(),
            timeout,
            every,
        })
    });

}

/// Structural equality: `:equals` and its alias `:eq`.
fn add_equality_methods<M: UserDataMethods<Matcher>>(methods: &mut M) {
    methods.add_method("equals", |_, this, other: Value| {
        let pass = values_equal(&this.subject, &other);
        this.record(pass, || {
            format!(
                "expected {}, got {}",
                display(&other),
                display(&this.subject)
            )
        })
    });
    methods.add_method("eq", |_, this, other: Value| {
        let pass = values_equal(&this.subject, &other);
        this.record(pass, || {
            format!(
                "expected {}, got {}",
                display(&other),
                display(&this.subject)
            )
        })
    });

    // Compare the subject against a stored `.snap` file colocated with the test
    // (`<dir>/snapshots/<file-stem>__<key>.snap`). `--update-snapshots` (re)writes it and passes;
    // otherwise a mismatch fails with a line diff and a missing snapshot fails after writing a
    // reviewable `.snap.new`. `arg` is nil, a name string, or an options table `{ name, level }`
    // (Phase A takes only a string subject + name; `level`/tree subjects come with the tree phase).
}

/// `:matches_snapshot` — golden-file comparison with `--update-snapshots` banking.
fn add_snapshot_method<M: UserDataMethods<Matcher>>(methods: &mut M) {
    methods.add_method("matches_snapshot", |_, this, arg: Value| {
        if this.negated {
            return Err(mlua::Error::RuntimeError(
                "matches_snapshot cannot be negated".into(),
            ));
        }
        // `arg` is nil | a name string | an options table `{ name?, level? }`.
        let (name, level): (Option<String>, Option<String>) = match arg {
            Value::Nil => (None, None),
            Value::String(s) => (Some(s.to_string_lossy().to_string()), None),
            Value::Table(t) => (t.get::<Option<String>>("name")?, t.get::<Option<String>>("level")?),
            other => {
                return Err(mlua::Error::RuntimeError(format!(
                    "matches_snapshot(name?) expects a string name or an options table, got {}",
                    other.type_name()
                )))
            }
        };
        let actual = serialize_snapshot_subject(&this.subject, level.as_deref())
            .map_err(mlua::Error::RuntimeError)?;

        // Resolve the `.snap`/`.snap.new` paths + update flag + a header source label from the
        // per-test snapshot context (advancing the auto-name counter for an unnamed snapshot).
        let (snap, snap_new, update, source, registry) = {
            let mut r = this.run.borrow_mut();
            let ctx = r.snapshot.as_mut().ok_or_else(|| {
                mlua::Error::RuntimeError(
                    "matches_snapshot needs a test-file context (no source path recorded for this run)"
                        .into(),
                )
            })?;
            let key = match &name {
                Some(n) => slugify(n),
                None => {
                    ctx.counter += 1;
                    format!("{}-{}", ctx.key_base, ctx.counter)
                }
            };
            let base = format!("{}__{}", ctx.stem, key);
            (
                ctx.dir.join(format!("{base}.snap")),
                ctx.dir.join(format!("{base}.snap.new")),
                ctx.update,
                format!("{} / {}", ctx.key_base, key),
                ctx.registry.clone(),
            )
        };

        // Record this `.snap` as referenced (whatever the outcome), so an unreferenced-snapshot
        // reconcile can tell orphaned files from ones a test still points at.
        if let Some(reg) = &registry {
            if let Ok(mut set) = reg.lock() {
                set.insert(snap.clone());
            }
        }

        let stored_doc = format_snapshot(&source, &actual);

        if update {
            if let Err(e) = write_snapshot(&snap, &stored_doc) {
                return Err(mlua::Error::RuntimeError(e));
            }
            let _ = std::fs::remove_file(&snap_new); // accepted → drop any pending .new
            return this.record(true, String::new);
        }

        match std::fs::read_to_string(&snap) {
            Ok(doc) => {
                let expected = snapshot_body(&doc);
                if expected == actual {
                    let _ = std::fs::remove_file(&snap_new);
                    this.record(true, String::new)
                } else {
                    let _ = write_snapshot(&snap_new, &stored_doc);
                    let diff = line_diff(expected, &actual);
                    let path = snap.display().to_string();
                    this.record(false, move || {
                        format!(
                            "snapshot mismatch ({path})\n{diff}\n  \
                             run `prova --update-snapshots` to accept, or see the .snap.new"
                        )
                    })
                }
            }
            Err(_) => {
                let _ = write_snapshot(&snap_new, &stored_doc);
                let path = snap.display().to_string();
                this.record(false, move || {
                    format!(
                        "no snapshot at {path} — wrote {path}.new; \
                         run `prova --update-snapshots` to accept it"
                    )
                })
            }
        }
    });
    // Identity, not structure: the *same* table/function/userdata (reference), or an equal
    // primitive (`rawequal` semantics). Complements the **deep** `equals` — use `is` to assert
    // "this is that same object", including tables that hold function fields `equals` can't compare.
}

/// Identity and truthiness: `:is`, `:is_true` / `:is_false` / `:is_nil` / `:is_truthy`.
fn add_identity_methods<M: UserDataMethods<Matcher>>(methods: &mut M) {
    methods.add_method("is", |_, this, other: Value| {
        let pass = this.subject == other;
        this.record(pass, || {
            format!(
                "expected {} to be (identity) {}",
                display(&this.subject),
                display(&other)
            )
        })
    });
    methods.add_method("is_true", |_, this, ()| {
        let pass = matches!(this.subject, Value::Boolean(true));
        this.record(pass, || {
            format!("expected true, got {}", display(&this.subject))
        })
    });
    methods.add_method("is_false", |_, this, ()| {
        let pass = matches!(this.subject, Value::Boolean(false));
        this.record(pass, || {
            format!("expected false, got {}", display(&this.subject))
        })
    });
    methods.add_method("is_nil", |_, this, ()| {
        let pass = matches!(this.subject, Value::Nil);
        this.record(pass, || {
            format!("expected nil, got {}", display(&this.subject))
        })
    });
    methods.add_method("is_truthy", |_, this, ()| {
        let pass = truthy(&this.subject);
        this.record(pass, || {
            format!("expected a truthy value, got {}", display(&this.subject))
        })
    });
}

/// Containment and filesystem probes: `:contains`, `:exists`, `:is_file` / `:is_dir`.
fn add_content_methods<M: UserDataMethods<Matcher>>(methods: &mut M) {
    methods.add_method("contains", |_, this, needle: Value| {
        let pass = contains(&this.subject, &needle);
        this.record(pass, || {
            let shown = match (&this.subject, &needle) {
                (Value::String(s), Value::String(n)) => display_windowed(
                    &s.to_string_lossy(),
                    &n.to_string_lossy(),
                ),
                _ => display(&this.subject),
            };
            format!("expected {} to contain {}", shown, display(&needle))
        })
    });

    // Filesystem matchers: the subject is a path string (e.g. `t:expect(dir.."/Cargo.toml")`).
    //
    // `exists` means exists for whatever the subject IS — the same resolution `is_empty` below
    // already makes, and for the same reason. Sitting next to `is_nil` in every matcher listing,
    // `exists` reads as its opposite, so `expect(some_table):exists()` is the natural way to
    // write a presence check. It used to FAIL, reporting `expected path <table> to exist` about
    // a value that was never a path — a message no one can act on. The inconsistency was the
    // bug, not the expectation.
    //
    // Strings stay filesystem-checked: asserting a file is there is this matcher's load-bearing
    // use, and `expect(dir.."/f"):exists()` must keep failing when the file is missing. For a
    // string's presence, `never():is_nil()` is the matcher.
    methods.add_method("exists", |_, this, ()| {
        match subject_path(&this.subject) {
            Some(p) => {
                let pass = p.exists();
                let subject = display(&this.subject);
                // A separator-less string that is not on disk is far more likely a value someone
                // meant to null-check than a path they expected to find, so name the other
                // matcher rather than leaving them to guess.
                let looks_like_a_value = matches!(&this.subject, Value::String(_))
                    && !subject.contains(std::path::MAIN_SEPARATOR)
                    && !subject.contains('/');
                this.record(pass, move || {
                    if looks_like_a_value {
                        format!(
                            "expected path {subject} to exist — `exists` is a filesystem \
                             matcher; for a presence check use `never():is_nil()`"
                        )
                    } else {
                        format!("expected path {subject} to exist")
                    }
                })
            }
            // Not path-shaped at all (a table without `path`, a number, a boolean): the only
            // coherent reading is "this value is present".
            None => {
                let pass = !matches!(this.subject, Value::Nil);
                this.record(pass, || "expected a value to be present, got nil".to_string())
            }
        }
    });
    methods.add_method("is_file", |_, this, ()| {
        let pass = subject_path(&this.subject).is_some_and(|p| p.is_file());
        this.record(pass, || {
            format!("expected {} to be a file", display(&this.subject))
        })
    });
    methods.add_method("is_dir", |_, this, ()| {
        let pass = subject_path(&this.subject).is_some_and(|p| p.is_dir());
        this.record(pass, || {
            format!("expected {} to be a directory", display(&this.subject))
        })
    });
    // Empty means empty for whatever the subject IS: a string with no bytes, a table with no
    // entries, or a path with no children. It read as filesystem-only, so `expect(""):is_empty()`
    // and `expect({}):is_empty()` both FAILED — reporting `expected "" to be empty` about an
    // empty string, which no one can act on. `has_length(0)` already worked on both, so the
    // inconsistency was the bug, not the expectation.
}

/// Emptiness and render checks: `:is_empty`, `:is_fully_rendered`, `:is_falsy`.
fn add_emptiness_methods<M: UserDataMethods<Matcher>>(methods: &mut M) {
    methods.add_method("is_empty", |_, this, ()| {
        // A string is ambiguous — it may be a path OR a literal. Resolve it by what is on
        // disk: an existing path is a filesystem check (the long-standing behaviour), anything
        // else is its byte length. So `expect(dir):is_empty()` still asks the filesystem, and
        // `expect(""):is_empty()` finally answers about the string.
        let pass = match &this.subject {
            Value::Table(t) => t.clone().pairs::<Value, Value>().next().is_none(),
            Value::String(s) if subject_path(&this.subject).is_none_or(|p| !p.exists()) => {
                s.as_bytes().is_empty()
            }
            other => path_is_empty(other),
        };
        this.record(pass, || {
            format!("expected {} to be empty", display(&this.subject))
        })
    });

    // The signature archetype check: every file under a rendered tree (a path string, or a
    // tree/dir handle with a `path`) must be free of leftover template markers — no `{{`, `{%`,
    // or `{#` in file *contents* or *path segments*. GitHub Actions `${{ … }}` expressions are
    // legitimately present in rendered workflows, so they are excluded. Tedious to hand-roll
    // (glob every file, read, scan); one call here.
    methods.add_method("is_fully_rendered", |_, this, ()| {
        let offenders = match subject_path(&this.subject) {
            Some(p) => unrendered_markers(&p),
            None => vec!["subject is not a path or tree handle".to_string()],
        };
        let pass = offenders.is_empty();
        this.record(pass, || {
            let shown = offenders
                .iter()
                .take(10)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n    ");
            let more = if offenders.len() > 10 {
                format!("\n    … and {} more", offenders.len() - 10)
            } else {
                String::new()
            };
            format!(
                "expected {} to be fully rendered, but found unrendered template markers:\n    {shown}{more}",
                display(&this.subject)
            )
        })
    });

    methods.add_method("is_falsy", |_, this, ()| {
        let pass = !truthy(&this.subject);
        this.record(pass, || {
            format!("expected a falsy value, got {}", display(&this.subject))
        })
    });

    // Polymorphic on the argument (the `contains` precedent — docs/plans/api-freeze.md §3):
    // a STRING is a Lua pattern match on a string subject (delegates to `string.find`); a
    // TABLE is a recursive structural SUBSET — every key in the shape must exist in the
    // subject and recursively match, extra subject keys ignored, arrays same-index. One
    // semantics for every surface that matches shapes; spec: proofs/spec/matching/.
}

/// Shape and ordering: `:matches`, `:has_length`, `:is_one_of`, `:gt` / `:gte` / `:lt` / `:lte`.
fn add_shape_methods<M: UserDataMethods<Matcher>>(methods: &mut M) {
    methods.add_method("matches", |lua, this, arg: Value| match arg {
        Value::String(pattern) => {
            let pattern = pattern.to_str()?.to_string();
            let (pass, subject) = match &this.subject {
                Value::String(s) => {
                    let subject = s.to_str()?.to_string();
                    let find: mlua::Function =
                        lua.globals().get::<Table>("string")?.get("find")?;
                    let found: Value = find.call((subject.clone(), pattern.clone()))?;
                    (!matches!(found, Value::Nil), subject)
                }
                other => (false, display(other)),
            };
            this.record(pass, || {
                format!("expected {subject:?} to match pattern {pattern:?}")
            })
        }
        Value::Table(shape) => {
            let mismatch = match &this.subject {
                Value::Table(subject) => subset_mismatch(&shape, subject, &mut Vec::new()),
                other => Some(format!("expected a table, got {}", display(other))),
            };
            let pass = mismatch.is_none();
            this.record(pass, || match mismatch {
                Some(detail) => format!("does not match shape — {detail}"),
                None => "matches the shape".to_string(),
            })
        }
        _ => Err(mlua::Error::RuntimeError(
            "matches takes a Lua pattern (string) or a shape (table)".into(),
        )),
    });

    methods.add_method("has_length", |_, this, n: i64| {
        let len = value_length(&this.subject);
        this.record(len == Some(n), || match len {
            Some(l) => format!("expected length {n}, got {l}"),
            None => format!(
                "expected a string/table of length {n}, got {}",
                display(&this.subject)
            ),
        })
    });

    methods.add_method("is_one_of", |_, this, options: Table| {
        let mut pass = false;
        for item in options.sequence_values::<Value>() {
            if values_equal(&this.subject, &item?) {
                pass = true;
                break;
            }
        }
        this.record(pass, || {
            format!(
                "expected {} to be one of the given options",
                display(&this.subject)
            )
        })
    });

    methods.add_method("gt", |_, this, n: f64| {
        let pass = as_number(&this.subject).is_some_and(|x| x > n);
        this.record(pass, || {
            format!("expected {} > {n}", display(&this.subject))
        })
    });
    methods.add_method("gte", |_, this, n: f64| {
        let pass = as_number(&this.subject).is_some_and(|x| x >= n);
        this.record(pass, || {
            format!("expected {} >= {n}", display(&this.subject))
        })
    });
    methods.add_method("lt", |_, this, n: f64| {
        let pass = as_number(&this.subject).is_some_and(|x| x < n);
        this.record(pass, || {
            format!("expected {} < {n}", display(&this.subject))
        })
    });
    methods.add_method("lte", |_, this, n: f64| {
        let pass = as_number(&this.subject).is_some_and(|x| x <= n);
        this.record(pass, || {
            format!("expected {} <= {n}", display(&this.subject))
        })
    });
}

impl UserData for Matcher {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        add_mode_methods(methods);
        add_equality_methods(methods);
        add_snapshot_method(methods);
        add_identity_methods(methods);
        add_content_methods(methods);
        add_emptiness_methods(methods);
        add_shape_methods(methods);
    }
}

/// A `Value` interpreted as a filesystem path: a string, or a handle table with a `path` field
/// (as returned by `archetect.render(...)` — `t:expect(out:file("Cargo.toml")):exists()`).
pub(super) fn subject_path(v: &Value) -> Option<PathBuf> {
    match v {
        Value::String(s) => s.to_str().ok().map(|bs| PathBuf::from(&*bs)),
        Value::Table(t) => t
            .get::<Option<String>>("path")
            .ok()
            .flatten()
            .map(PathBuf::from),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------------
// Value helpers
// ---------------------------------------------------------------------------------------------

pub(super) fn truthy(v: &Value) -> bool {
    !matches!(v, Value::Nil | Value::Boolean(false))
}

pub(super) fn values_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Nil, Value::Nil) => true,
        (Value::Boolean(x), Value::Boolean(y)) => x == y,
        (Value::Integer(x), Value::Integer(y)) => x == y,
        (Value::Number(x), Value::Number(y)) => x == y,
        (Value::Integer(x), Value::Number(y)) | (Value::Number(y), Value::Integer(x)) => {
            (*x as f64) == *y
        }
        (Value::String(x), Value::String(y)) => x.to_string_lossy() == y.to_string_lossy(),
        (Value::Table(x), Value::Table(y)) => tables_equal(x, y),
        // Sentinels (json.null) and other lightuserdata compare by identity — what makes
        // `t:expect({ x = json.null }):matches{ x = json.null }` hold (api-freeze §3).
        (Value::LightUserData(x), Value::LightUserData(y)) => x == y,
        _ => false,
    }
}

/// The structural-subset walk behind `:matches(shape)`: every key present in `shape` must exist
/// in `subject` and recursively match; extra subject keys are ignored; an array is just integer
/// keys, so elements match same-index (a shape array shorter than the subject's passes, longer
/// fails on the missing index). Scalar leaves compare with `values_equal` (int↔float coercion).
/// Returns the FIRST mismatch as a `path: expected X, got Y` line — the table-aware diff that
/// pinpoints `status.readyReplicas: expected 3, got 1` instead of `<table> != <table>`.
pub(crate) fn subset_mismatch(shape: &Table, subject: &Table, path: &mut Vec<String>) -> Option<String> {
    for pair in shape.clone().pairs::<Value, Value>() {
        let Ok((key, expected)) = pair else {
            return Some(format!("{}: unreadable shape entry", path_str(path)));
        };
        path.push(key_segment(&key));
        let actual: Value = subject.get::<Value>(key).unwrap_or(Value::Nil);
        let mismatch = match (&expected, &actual) {
            (Value::Table(es), Value::Table(actual_t)) => subset_mismatch(es, actual_t, path),
            _ if values_equal(&expected, &actual) => None,
            (_, Value::Nil) => Some(format!(
                "{}: expected {}, got nothing",
                path_str(path),
                display(&expected)
            )),
            _ => Some(format!(
                "{}: expected {}, got {}",
                path_str(path),
                display(&expected),
                display(&actual)
            )),
        };
        path.pop();
        if mismatch.is_some() {
            return mismatch;
        }
    }
    None
}

/// One path segment for the subset diff: array indices render as `[i]`, string keys as-is.
pub(super) fn key_segment(key: &Value) -> String {
    match key {
        Value::Integer(i) => format!("[{i}]"),
        Value::String(s) => s.to_string_lossy().to_string(),
        other => format!("[{}]", display(other)),
    }
}

/// Join diff path segments: dots between named keys, indices appended (`status.conditions[1].type`).
pub(super) fn path_str(path: &[String]) -> String {
    if path.is_empty() {
        return "(root)".to_string();
    }
    let mut out = String::new();
    for seg in path {
        if !seg.starts_with('[') && !out.is_empty() {
            out.push('.');
        }
        out.push_str(seg);
    }
    out
}

/// Deep table equality: same set of keys, values recursively equal. (Cyclic tables are not guarded
/// — test data is expected to be acyclic.)
pub(super) fn tables_equal(x: &Table, y: &Table) -> bool {
    let mut x_keys = 0;
    for pair in x.clone().pairs::<Value, Value>() {
        let Ok((key, xv)) = pair else { return false };
        x_keys += 1;
        match y.get::<Value>(key) {
            Ok(yv) if values_equal(&xv, &yv) => {}
            _ => return false,
        }
    }
    // Equal key counts (with every x-key matched in y) means no extra keys on either side.
    let y_keys = y.clone().pairs::<Value, Value>().count();
    x_keys == y_keys
}

pub(super) fn as_number(v: &Value) -> Option<f64> {
    match v {
        Value::Integer(i) => Some(*i as f64),
        Value::Number(n) => Some(*n),
        _ => None,
    }
}

/// Length of a string (bytes, matching Lua `#`) or a table (sequence length).
pub(super) fn value_length(v: &Value) -> Option<i64> {
    match v {
        Value::String(s) => Some(s.as_bytes().len() as i64),
        Value::Table(t) => Some(t.raw_len() as i64),
        _ => None,
    }
}

/// `is_empty` on a path subject: an empty directory, or a zero-byte file. A non-path (or missing
/// path) is not empty.
pub(super) fn path_is_empty(v: &Value) -> bool {
    let Some(path) = subject_path(v) else {
        return false;
    };
    if path.is_dir() {
        std::fs::read_dir(&path)
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(false)
    } else {
        std::fs::metadata(&path)
            .map(|m| m.len() == 0)
            .unwrap_or(false)
    }
}

/// Byte index of the first unrendered jinja marker (`{{`, `{%`, `{#`) in `s` that is *not* part of a
/// GitHub Actions `${{ … }}` expression (i.e. not immediately preceded by `$`). `None` if clean.
pub(super) fn first_marker(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let mut i = 0;
    while i + 1 < b.len() {
        if b[i] == b'{' && matches!(b[i + 1], b'{' | b'%' | b'#') {
            let preceded_by_dollar = i > 0 && b[i - 1] == b'$';
            if !preceded_by_dollar {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

/// Every leftover-template-marker offender under `root` — an unrendered `{{`/`{%`/`{#` in a file's
/// contents (reported as `relpath:line: snippet`) or in a path segment (`relpath (unrendered path
/// segment)`). Binary/unreadable files are skipped. A missing `root` is itself an offender.
pub(super) fn unrendered_markers(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    if !root.exists() {
        return vec![format!("{}: path does not exist", root.display())];
    }
    let scan_file = |path: &Path, rel: &Path, out: &mut Vec<String>| {
        if let Ok(contents) = std::fs::read_to_string(path) {
            if let Some(idx) = first_marker(&contents) {
                let line = contents[..idx].matches('\n').count() + 1;
                let snippet: String = contents[idx..]
                    .lines()
                    .next()
                    .unwrap_or("")
                    .trim()
                    .chars()
                    .take(60)
                    .collect();
                out.push(format!("{}:{line}: {snippet}", rel.display()));
            }
        }
    };
    if root.is_file() {
        scan_file(
            root,
            Path::new(root.file_name().unwrap_or_default()),
            &mut out,
        );
        return out;
    }
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let rel = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
            // An unrendered *name* (only the segment's own name, so a bad parent isn't re-reported
            // for every child).
            if first_marker(&entry.file_name().to_string_lossy()).is_some() {
                out.push(format!("{} (unrendered path segment)", rel.display()));
            }
            if path.is_dir() {
                stack.push(path);
            } else if path.is_file() {
                scan_file(&path, &rel, &mut out);
            }
        }
    }
    out.sort();
    out
}

pub(super) fn contains(subject: &Value, needle: &Value) -> bool {
    match subject {
        Value::String(s) => match needle {
            Value::String(n) => s.to_string_lossy().contains(&*n.to_string_lossy()),
            _ => false,
        },
        Value::Table(t) => {
            for (_, v) in t.clone().pairs::<Value, Value>().flatten() {
                if values_equal(&v, needle) {
                    return true;
                }
            }
            false
        }
        _ => false,
    }
}

/// Render a string subject for a `contains` diagnostic without dumping it whole. Small subjects
/// print verbatim (unchanged behavior); large ones show a window — around the first match when
/// there is one (the `never()` polarity: WHERE it matched is the actionable part), else the head
/// and tail (the plain polarity: the subject's edges, since no middle is more relevant than any
/// other). Field-reported: a `contains` against a captured CLI transcript dumped ~3KB into every
/// diagnostic line, burying the needle it was about.
pub(super) fn display_windowed(subject: &str, needle: &str) -> String {
    const LIMIT: usize = 600; // below this, verbatim
    const WINDOW: usize = 240; // bytes shown per side of a cut
    if subject.len() <= LIMIT {
        return format!("{subject:?}");
    }
    // Nearest char boundary at-or-before `i`, so a cut never lands mid-UTF-8.
    let clamp = |mut i: usize| -> usize {
        i = i.min(subject.len());
        while !subject.is_char_boundary(i) {
            i -= 1;
        }
        i
    };
    let elide = |n: usize| format!(" …[{n} bytes elided]… ");
    if let Some(at) = (!needle.is_empty())
        .then(|| subject.find(needle))
        .flatten()
    {
        let start = clamp(at.saturating_sub(WINDOW));
        let end = clamp((at + needle.len() + WINDOW).min(subject.len()));
        let mut out = String::new();
        if start > 0 {
            out.push_str(&elide(start));
        }
        out.push_str(&format!("{:?}", &subject[start..end]));
        if end < subject.len() {
            out.push_str(&elide(subject.len() - end));
        }
        out
    } else {
        let head = clamp(WINDOW);
        let tail = clamp(subject.len() - WINDOW);
        format!(
            "{:?}{}{:?}",
            &subject[..head],
            elide(tail - head),
            &subject[tail..]
        )
    }
}

pub(super) fn display(v: &Value) -> String {
    match v {
        Value::Nil => "nil".into(),
        Value::Boolean(b) => b.to_string(),
        Value::Integer(i) => i.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => format!("{:?}", s.to_string_lossy()),
        Value::Table(_) => "<table>".into(),
        Value::Function(_) => "<function>".into(),
        other => format!("<{}>", other.type_name()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lua_table(lua: &Lua, pairs: &[(&str, Value)]) -> Table {
        let t = lua.create_table().unwrap();
        for (k, v) in pairs {
            t.set(*k, v.clone()).unwrap();
        }
        t
    }

    /// The equality every matcher leaf stands on: int↔float coercion holds (a Lua `1` and a
    /// JSON-decoded `1.0` are the same fact), strings compare by content, and cross-type never
    /// silently coerces.
    #[test]
    fn values_equal_coerces_numbers_and_nothing_else() {
        let lua = Lua::new();
        assert!(values_equal(&Value::Integer(1), &Value::Number(1.0)));
        assert!(!values_equal(&Value::Integer(1), &Value::Number(1.5)));
        let a = lua.create_string("x").unwrap();
        let b = lua.create_string("x").unwrap();
        assert!(values_equal(&Value::String(a), &Value::String(b)));
        assert!(!values_equal(&Value::Integer(1), &Value::Boolean(true)));
        assert!(!values_equal(&Value::Nil, &Value::Boolean(false)));
    }

    /// The structural-subset walk behind `:matches` and the §6 journal filter: shape keys must
    /// match, extra subject keys are unconstrained, and the FIRST mismatch names its dotted path
    /// — the diff a failing assertion prints.
    #[test]
    fn subset_mismatch_ignores_extras_and_names_the_path() {
        let lua = Lua::new();
        let inner_shape = lua_table(&lua, &[("status", Value::Integer(200))]);
        let shape = lua_table(&lua, &[("reply", Value::Table(inner_shape))]);

        let inner_ok = lua_table(&lua, &[("status", Value::Integer(200)), ("extra", Value::Boolean(true))]);
        let subject = lua_table(&lua, &[("reply", Value::Table(inner_ok)), ("noise", Value::Integer(9))]);
        assert_eq!(subset_mismatch(&shape, &subject, &mut Vec::new()), None);

        let inner_bad = lua_table(&lua, &[("status", Value::Integer(500))]);
        let subject = lua_table(&lua, &[("reply", Value::Table(inner_bad))]);
        let msg = subset_mismatch(&shape, &subject, &mut Vec::new()).unwrap();
        assert!(msg.contains("reply.status"), "names the dotted path: {msg}");
        assert!(msg.contains("200") && msg.contains("500"), "shows both sides: {msg}");

        let subject = lua_table(&lua, &[]);
        let msg = subset_mismatch(&shape, &subject, &mut Vec::new()).unwrap();
        assert!(msg.contains("got nothing"), "absence reads as absence: {msg}");
    }

    /// `contains` is content for strings, membership for tables — and membership uses the same
    /// coercing equality as everything else.
    #[test]
    fn contains_is_substring_or_membership() {
        let lua = Lua::new();
        let s = Value::String(lua.create_string("hello world").unwrap());
        let needle = Value::String(lua.create_string("lo wo").unwrap());
        assert!(contains(&s, &needle));

        let arr = lua.create_table().unwrap();
        arr.push(1).unwrap();
        arr.push(2.0).unwrap();
        assert!(contains(&Value::Table(arr.clone()), &Value::Number(2.0)));
        assert!(contains(&Value::Table(arr.clone()), &Value::Integer(2)), "membership coerces int↔float");
        assert!(!contains(&Value::Table(arr), &Value::Integer(3)));
        assert!(!contains(&Value::Integer(5), &Value::Integer(5)), "a scalar contains nothing");
    }

    /// `has_length`'s substrate: strings measure in BYTES (what wire assertions need), tables by
    /// raw sequence length, and unmeasurable values answer None rather than 0.
    #[test]
    fn value_length_measures_bytes_and_rows() {
        let lua = Lua::new();
        let s = Value::String(lua.create_string("héllo").unwrap());
        assert_eq!(value_length(&s), Some(6), "é is two bytes — bytes, not chars");
        let arr = lua.create_table().unwrap();
        arr.push("a").unwrap();
        arr.push("b").unwrap();
        assert_eq!(value_length(&Value::Table(arr)), Some(2));
        assert_eq!(value_length(&Value::Integer(7)), None);
    }
}
