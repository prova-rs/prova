//! First-party capability modules injected as globals alongside `prova`.
//!
//! These are what make prova useful beyond testing itself — bring a system into existence and poke
//! it. Two kinds live here, and the split is deliberate:
//!
//! - **Primitives + substrate** (always the foundation): `shell.run`/`shell.spawn`, `fs`, `net`,
//!   `docker` (the typed **bollard** daemon client). `shell`/`docker` are async (child processes /
//!   docker calls never block the worker); `fs` is synchronous. All take context explicitly (no
//!   ambient cwd), preserving the isolation the design promises.
//! - **Network-drive clients** — `http`, `grpc`, `graphql` — how you *drive the app under test*
//!   (there is no CLI-in-image for arbitrary gRPC), plus `yaml` (a parse util) and `sqlite` (the one
//!   embedded, no-docker database). Each is behind a default-on feature so a build can opt out.
//!
//! **Resource clients are NOT here.** Databases, caches, brokers, object stores, streams — every
//! *containerized* resource — are **external docker-exec plugins** (`prova-rs/prova-<name>`, authored
//! through `prova.containerized` + `container:run`), fetched via `prova.toml`, not compiled in. That
//! keeps the binary lean and privileges no technology. Modules that need docker declare
//! `requires = { "docker" }` to skip gracefully where the daemon is absent.

use std::path::Path;

use mlua::{Lua, ObjectLike, Table, Value};

use crate::progress::{self, Kind, Progress};
use std::sync::Arc;

/// One naming rule for every document-format module: **`decode` reads text into a Lua value, `encode`
/// writes a Lua value back to text.** `json`, `yaml`, `toml` and `csv` all obey it, and a `_all`
/// suffix marks the multi-document variants where the format actually has them (YAML `---` streams).
///
/// This used to be per-format folklore — `json.decode` but `yaml.parse`, `toml.encode` but
/// `yaml.dump` — each name borrowed from its own ecosystem (cjson, PyYAML, the Rust toml crate). That
/// reads fine one module at a time and badly in a proof that touches two: the four share one encode
/// half and one set of fidelity sentinels (`json.null`, `json.array`), so they are one system, and a
/// system wants one vocabulary. Lua has no compile-time check, so a wrong-but-plausible name is a
/// runtime `attempt to call a nil value` at the moment that line runs — or worse, swallowed by a
/// `pcall` and reported as something else entirely.
///
/// `proofs/spec/formats/naming_test.lua` holds the rule executably — including the reverse direction
/// (no format may expose a read/write verb outside it, `parse`/`dump` among them), so the fifth format
/// cannot drift.
///
/// The previous spellings are **gone, with no aliases** — the same clean cut api-freeze §1 made when it
/// removed `prova.parse.json`. Pre-announcement there is nobody to carry, and a deprecation shim on a
/// surface with no consumers is just a second name to keep working and a second thing to explain.
mod format_names {
    pub const DECODE: &str = "decode";
    pub const ENCODE: &str = "encode";
    pub const DECODE_ALL: &str = "decode_all";
    pub const ENCODE_ALL: &str = "encode_all";
}

mod cassette;
mod date;
mod ingest;
mod junit;
mod measure;
mod sarif;
mod shellproxy;
mod socket;
mod terminal;
mod websocket;
mod wiretap;

/// What `client_opts` yields: the URL, the header pairs, and the optional timeout.
#[cfg(any(feature = "http", feature = "graphql"))]
type ClientOpts = (String, Vec<(String, String)>, Option<std::time::Duration>);

/// Parse the shared options every HTTP-flavored client constructor takes: the required URL
/// (under the given key — `http.client` says `base_url`, `graphql.client` says `url`), optional
/// `headers`, optional `timeout`. `who` names the verb in the teaching error.
#[cfg(any(feature = "http", feature = "graphql"))]
pub(super) fn client_opts(opts: &Table, who: &str, url_key: &str) -> mlua::Result<ClientOpts> {
    let url = opts
        .get::<Option<String>>(url_key)?
        .ok_or_else(|| mlua::Error::RuntimeError(format!("{who} requires a `{url_key}`")))?;
    let mut headers = Vec::new();
    if let Some(hdrs) = opts.get::<Option<Table>>("headers")? {
        for pair in hdrs.pairs::<String, String>() {
            let (k, v) = pair?;
            headers.push((k, v));
        }
    }
    let timeout = opts
        .get::<Option<String>>("timeout")?
        .and_then(|s| crate::model::parse_duration(&s));
    Ok((url, headers, timeout))
}

/// Tie a resource's life to the caller's scope via `ctx:manage`, exactly as containers do —
/// reaped by the same LIFO machinery, in the same order, as every other resource — including
/// under `prova up`, where the scope is held until a signal rather than ending with a test.
/// Shared by every transport's mock/proxy constructor; `what` names the verb in the teaching
/// error (`http.mock`, `socket.proxy`, ...).
pub(super) fn manage(what: &str, ctx: &Value, ud: &mlua::AnyUserData) -> mlua::Result<()> {
    match ctx {
        Value::UserData(c) => {
            let _: Value = c.call_method("manage", ud)?;
            Ok(())
        }
        Value::Nil => Err(mlua::Error::RuntimeError(format!(
            "{what}(ctx): pass the test or fixture context (`t` / `ctx`) so it is torn down with \
             the scope"
        ))),
        other => Err(mlua::Error::RuntimeError(format!(
            "{what}(ctx): expected the test or fixture context, got a {}",
            other.type_name()
        ))),
    }
}

/// The §6 journal-filter contract, shared by every mock's `received(filter?)`: `nil` keeps
/// everything, a **table** is the same structural-subset match as `:on`/`:matches` (fields the
/// filter names must match; everything else unconstrained — so `{ matched = false }` or
/// `{ path = "/x" }` both work), a **function** is an arbitrary predicate over the entry.
/// Filtering happens over the *exposed* entry table, so `seq`/`source`/`matched` are as
/// filterable as the transport-native fields.
fn journal_keep(lua: &Lua, filter: &Option<Value>, entry: &Table) -> mlua::Result<bool> {
    let _ = lua;
    match filter {
        None | Some(Value::Nil) => Ok(true),
        Some(Value::Table(shape)) => {
            Ok(crate::engine::subset_mismatch(shape, entry, &mut Vec::new()).is_none())
        }
        Some(Value::Function(f)) => {
            let r: Value = f.call(entry.clone())?;
            Ok(!matches!(r, Value::Nil | Value::Boolean(false)))
        }
        Some(other) => Err(mlua::Error::RuntimeError(format!(
            "received: filter must be a table (subset match) or a function (predicate), got {}",
            other.type_name()
        ))),
    }
}

/// Install the built-in module globals (`shell`, `fs`, `docker`, and — with the `http` feature —
/// `http`) into `lua`.
pub(crate) fn install(
    lua: &Lua,
    progress: &Arc<dyn Progress>,
    deputed: Option<crate::model::DeputedRegistry>,
    measurements: Option<crate::model::MeasurementRegistry>,
) -> mlua::Result<()> {
    lua.globals().set("shell", shell::make_shell(lua, progress)?)?;
    lua.globals().set("fs", make_fs(lua)?)?;
    lua.globals().set("path", make_path(lua)?)?;
    lua.globals().set("str", make_str(lua)?)?;
    lua.globals().set("net", make_net(lua)?)?;
    lua.globals().set("socket", socket::make(lua)?)?;
    lua.globals().set("terminal", terminal::make(lua)?)?;
    lua.globals().set("websocket", websocket::make(lua)?)?;
    // `prova.parse.*` — the exec-CLI output-parsing toolkit (lines / rows / table), added to
    // the `prova` global built earlier in build_lua. Broadly useful, so it lives at the root.
    {
        let prova: Table = lua.globals().get("prova")?;
        prova.set("parse", formats::make_parse(lua)?)?;
    }
    // Tech-first format modules (api-freeze §1): encode AND decode together, one namespace per
    // technology. Always compiled — light, pure-Rust, and every underlying dep is already in the
    // binary. `yaml` predates the freeze and keeps its feature gate below.
    lua.globals().set("json", formats::make_json(lua)?)?;
    lua.globals().set("toml", formats::make_toml(lua)?)?;
    lua.globals().set("csv", formats::make_csv(lua)?)?;
    // The utility belt (api-freeze §1): separate from formats, same grammar, all reserved names.
    // The verdict-ingestion seam (docs/design/verifiers.md): junit.load parses the lingua franca
    // of test results; junit.verify (recipe, below) conducts a deputy and adopts its verdict.
    lua.globals()
        .set("junit", junit::make(lua, deputed.clone())?)?;
    // The findings seam (docs/design/verifiers.md): sarif.load parses SARIF — the de facto linter/
    // static-analysis interchange — and sarif.verify (recipe, below) adopts a linter's findings.
    // Shares the deputed account with junit, hence the clone above.
    lua.globals().set("sarif", sarif::make(lua, deputed)?)?;
    // The measurements seam (docs/design/verifiers.md): measure.record files a named scalar into
    // the run's measurement account; measure.ratchet (recipe, below) compares it to the committed
    // baseline (.prova/baselines/) and asserts no regression — the quality ratchet.
    lua.globals()
        .set("measure", measure::make(lua, measurements)?)?;
    // The `date` convenience over os.time/os.date — ergonomic time qualifiers for reminder `when`
    // conditions (date.past/days_since/…). A utility, not a scheduling mechanism.
    lua.globals().set("date", date::make(lua)?)?;
    lua.globals().set("base64", formats::make_base64(lua)?)?;
    lua.globals().set("hash", formats::make_hash(lua)?)?;
    lua.globals().set("uuid", formats::make_uuid(lua)?)?;
    lua.globals().set("url", formats::make_url(lua)?)?;
    #[cfg(feature = "docker")]
    lua.globals().set("docker", docker::make(lua, progress)?)?;
    #[cfg(feature = "http")]
    lua.globals().set("http", http::make(lua)?)?;
    #[cfg(feature = "sqlite")]
    lua.globals()
        .set("sqlite", sql::make(lua, sql::Engine::Sqlite)?)?;
    #[cfg(feature = "grpc")]
    lua.globals().set("grpc", grpc::make(lua)?)?;
    #[cfg(feature = "graphql")]
    lua.globals().set("graphql", graphql::make(lua)?)?;
    #[cfg(feature = "yaml")]
    lua.globals().set("yaml", yaml::make(lua)?)?;
    // Absent-namespace stubs: in a lean distribution a native namespace's feature may be off. Install
    // a stub so `kafka.client(...)` raises a clear "not compiled into this build" error instead of a
    // bare `attempt to index a nil value` — the call-side companion to the `requires` skip. In the
    // default build every feature is on, so none of these arms compile.
    #[cfg(not(feature = "docker"))]
    lua.globals().set("docker", absent_stub(lua, "docker")?)?;
    #[cfg(not(feature = "http"))]
    lua.globals().set("http", absent_stub(lua, "http")?)?;
    #[cfg(not(feature = "sqlite"))]
    lua.globals().set("sqlite", absent_stub(lua, "sqlite")?)?;
    #[cfg(not(feature = "grpc"))]
    lua.globals().set("grpc", absent_stub(lua, "grpc")?)?;
    #[cfg(not(feature = "graphql"))]
    lua.globals().set("graphql", absent_stub(lua, "graphql")?)?;
    #[cfg(not(feature = "yaml"))]
    lua.globals().set("yaml", absent_stub(lua, "yaml")?)?;
    // The `prova.containerized` scaffolding helper — the ergonomic keystone every containerized
    // resource (first-party recipe or third-party plugin) is authored through. Always available;
    // the globals it composes (`docker`, `prova.retry`) resolve when a generated `container` is
    // *called*. Loaded before the recipes so they can be expressed in terms of it.
    lua.load(shell::CONTAINERIZED_LUA)
        .set_name("@prova/containerized")
        .exec()?;
    junit::install_recipe(lua)?;
    sarif::install_recipe(lua)?;
    measure::install_recipe(lua)?;
    date::install_recipe(lua)?;
    // Resource recipes — Lua sugar over docker.run + prova.retry + a client + ctx:manage. Loaded
    // after the modules exist; the globals they touch resolve when a recipe is *called*.
    Ok(())
}

mod formats;

/// A stand-in for a native namespace whose feature was not compiled into this build: any field
/// access raises a clear, actionable error instead of a bare `attempt to index a nil value`. A test
/// that wants to *skip* rather than error should gate with `requires = { "<name>" }`.
///
/// `#[allow(dead_code)]`: only referenced by the `#[cfg(not(feature = …))]` install arms, so in a
/// default (all-features) build it compiles but is never called.
#[allow(dead_code)]
fn absent_stub(lua: &Lua, name: &'static str) -> mlua::Result<Table> {
    let tbl = lua.create_table()?;
    let mt = lua.create_table()?;
    let index = lua.create_function(move |_, (_t, key): (Table, mlua::String)| {
        let key = key.to_string_lossy();
        Err::<mlua::Value, _>(mlua::Error::RuntimeError(format!(
            "`{name}.{key}` is unavailable: the `{name}` capability is not compiled into this build \
             (use a distribution that includes it, or gate the test with requires = {{ \"{name}\" }} \
             to skip instead)"
        )))
    })?;
    mt.set("__index", index)?;
    tbl.set_metatable(Some(mt))?;
    Ok(tbl)
}

mod shell;

// ---------------------------------------------------------------------------------------------
// fs
// ---------------------------------------------------------------------------------------------

fn make_fs(lua: &Lua) -> mlua::Result<Table> {
    let fs = lua.create_table()?;

    fs.set(
        "exists",
        lua.create_function(|_, path: String| Ok(Path::new(&path).exists()))?,
    )?;

    fs.set(
        "read",
        lua.create_function(|_, path: String| {
            std::fs::read_to_string(&path)
                .map_err(|e| mlua::Error::RuntimeError(format!("fs.read {path:?}: {e}")))
        })?,
    )?;

    fs.set(
        "write",
        lua.create_function(|_, (path, contents): (String, String)| {
            if let Some(parent) = Path::new(&path).parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| mlua::Error::RuntimeError(format!("fs.write {path:?}: {e}")))?;
            }
            std::fs::write(&path, contents)
                .map_err(|e| mlua::Error::RuntimeError(format!("fs.write {path:?}: {e}")))
        })?,
    )?;

    fs.set(
        "remove_all",
        lua.create_function(|_, path: String| {
            let p = Path::new(&path);
            let result = if p.is_dir() {
                std::fs::remove_dir_all(p)
            } else {
                std::fs::remove_file(p)
            };
            match result {
                Ok(()) => Ok(()),
                // Removing something already gone is a no-op, not an error.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(mlua::Error::RuntimeError(format!(
                    "fs.remove_all {path:?}: {e}"
                ))),
            }
        })?,
    )?;

    fs.set(
        "tempdir",
        lua.create_function(|_, ()| {
            crate::engine::make_tempdir()
                .map(|p| emit_path(&p))
                .map_err(|e| mlua::Error::RuntimeError(format!("fs.tempdir: {e}")))
        })?,
    )?;

    // fs.mkdir(path) — create the directory and every missing parent (like `mkdir -p`); idempotent
    // (no error if it already exists). The platform-agnostic replacement for
    // `shell.run("mkdir -p " .. path)`, which routes through `cmd /C` on Windows where `-p` is not a
    // flag and `/` is not a separator.
    fs.set(
        "mkdir",
        lua.create_function(|_, path: String| {
            std::fs::create_dir_all(&path)
                .map_err(|e| mlua::Error::RuntimeError(format!("fs.mkdir {path:?}: {e}")))
        })?,
    )?;

    // fs.glob(root, "**/*.rs") → sorted list of matching paths (as strings).
    fs.set(
        "glob",
        lua.create_function(|lua, (root, pattern): (String, String)| {
            let joined = Path::new(&root).join(&pattern);
            let pattern = joined.to_string_lossy();
            let paths = glob::glob(&pattern)
                .map_err(|e| mlua::Error::RuntimeError(format!("fs.glob {pattern:?}: {e}")))?;
            let mut out: Vec<String> = paths
                .filter_map(|r| r.ok())
                .map(|p| emit_path(&p))
                .collect();
            out.sort();
            lua.create_sequence_from(out)
        })?,
    )?;

    Ok(fs)
}

// ---------------------------------------------------------------------------------------------
// str — string utilities + the archetect casing vocabulary (canonical `prova.str`)
// ---------------------------------------------------------------------------------------------

/// The casing half CALLS archetect's own inflections and MIRRORS archetect's filter names
/// (`constant_case` is archetect's name for screaming-snake, so it is prova's too). One
/// implementation, one vocabulary: a proof asserting on an archetype's rendered output uses the
/// same function the archetype's templates did, so the two cannot drift.
fn make_str(lua: &Lua) -> mlua::Result<Table> {
    let s = lua.create_table()?;

    macro_rules! string_fn {
        ($name:literal, $f:expr) => {
            s.set(
                $name,
                lua.create_function(|_, v: String| {
                    #[allow(clippy::redundant_closure_call)]
                    Ok(($f)(v.as_str()))
                })?,
            )?;
        };
    }

    // General utilities.
    string_fn!("trim", |v: &str| v.trim().to_string());

    // str.split(s, sep) — plain (non-pattern) separator split, KEEPING empty fields: a split is
    // data extraction, and "a,,c" has three fields.
    s.set(
        "split",
        lua.create_function(|lua, (v, sep): (String, String)| {
            if sep.is_empty() {
                return Err(mlua::Error::RuntimeError(
                    "str.split: separator must be non-empty".into(),
                ));
            }
            lua.create_sequence_from(v.split(&sep as &str).map(str::to_string))
        })?,
    )?;

    // str.lines(s) — the portable line reader: splits on `\n`, absorbing a preceding `\r`, and a
    // trailing newline yields no phantom empty line. The same result whether the text came from a
    // unix or a Windows program — which is the whole reason to reach for it over a hand split.
    s.set(
        "lines",
        lua.create_function(|lua, v: String| {
            lua.create_sequence_from(v.lines().map(str::to_string))
        })?,
    )?;

    // Casing converters — archetect's filter table, name for name.
    string_fn!("camel_case", archetect_inflections::to_camel_case);
    string_fn!("class_case", archetect_inflections::to_class_case);
    string_fn!("cobol_case", archetect_inflections::to_cobol_case);
    string_fn!("constant_case", archetect_inflections::to_screaming_snake_case);
    string_fn!("directory_case", archetect_inflections::to_directory_case);
    string_fn!("kebab_case", archetect_inflections::to_kebab_case);
    string_fn!("package_case", archetect_inflections::to_package_case);
    string_fn!("pascal_case", archetect_inflections::to_pascal_case);
    string_fn!("sentence_case", archetect_inflections::to_sentence_case);
    string_fn!("snake_case", archetect_inflections::to_snake_case);
    string_fn!("title_case", archetect_inflections::to_title_case);
    string_fn!("train_case", archetect_inflections::to_train_case);

    // Casing predicates.
    string_fn!("is_camel_case", archetect_inflections::is_camel_case);
    string_fn!("is_class_case", archetect_inflections::is_class_case);
    string_fn!("is_cobol_case", archetect_inflections::is_cobol_case);
    string_fn!("is_constant_case", archetect_inflections::is_screaming_snake_case);
    string_fn!("is_directory_case", archetect_inflections::is_directory_case);
    string_fn!("is_kebab_case", archetect_inflections::is_kebab_case);
    string_fn!("is_package_case", archetect_inflections::is_package_case);
    string_fn!("is_pascal_case", archetect_inflections::is_pascal_case);
    string_fn!("is_sentence_case", archetect_inflections::is_sentence_case);
    string_fn!("is_snake_case", archetect_inflections::is_snake_case);
    string_fn!("is_title_case", archetect_inflections::is_title_case);
    string_fn!("is_train_case", archetect_inflections::is_train_case);

    // Plurals and ordinals.
    string_fn!("pluralize", archetect_inflections::to_plural);
    string_fn!("singularize", archetect_inflections::to_singular);
    string_fn!("ordinalize", archetect_inflections::ordinalize);
    string_fn!("deordinalize", archetect_inflections::deordinalize);

    Ok(s)
}

// ---------------------------------------------------------------------------------------------
// path — pure, platform-agnostic path algebra (canonical `prova.path`; ambient only if injected)
// ---------------------------------------------------------------------------------------------

/// One separator convention for every path prova emits: `/`. These are STRING functions on
/// purpose — `std::path` renders `\` on Windows, which is exactly the class of output that broke
/// TOML-embedding, shell-quoting, and pattern-matching in proofs. Input accepts either separator
/// (and the Windows verbatim `\\?\` prefix); output is always `/`-normalized, so the same
/// assertions hold on every OS.
fn path_norm_seps(p: &str) -> String {
    p.strip_prefix(r"\\?\").unwrap_or(p).replace('\\', "/")
}

/// Render an OS path for emission to Lua: `/`-normalized on Windows (where the OS renders `\` and
/// canonicalization grows a `\\?\` prefix), byte-exact everywhere else — a unix filename may
/// legally CONTAIN `\`, so blanket replacement would corrupt it. Every path-PRODUCING API
/// (`fs.tempdir`, `fs.glob`, `ctx:tempdir`) must emit through this.
pub(crate) fn emit_path(p: &std::path::Path) -> String {
    let s = p.to_string_lossy();
    if cfg!(windows) {
        path_norm_seps(&s)
    } else {
        s.into_owned()
    }
}

/// The root prefix of an already `/`-normalized path: `"//"` (UNC — the double slash is
/// load-bearing, the server/share are plain components under it), `"/"` (unix), `"X:/"` (drive),
/// or `""` (relative). What follows the prefix is plain components.
fn path_root(p: &str) -> &str {
    if p.starts_with("//") && !p[2..].starts_with('/') && p.len() > 2 {
        "//"
    } else if p.starts_with('/') {
        "/"
    } else if p.len() >= 3
        && p.as_bytes()[1] == b':'
        && p.as_bytes()[2] == b'/'
        && p.as_bytes()[0].is_ascii_alphabetic()
    {
        &p[..3]
    } else {
        ""
    }
}

fn path_is_absolute(p: &str) -> bool {
    !path_root(&path_norm_seps(p)).is_empty()
}

/// Strip trailing separators without eating a root ("a/b/" → "a/b", but "/" stays "/").
fn path_trim_trailing(p: &str) -> &str {
    let root = path_root(p);
    let mut end = p.len();
    while end > root.len() && p.as_bytes()[end - 1] == b'/' {
        end -= 1;
    }
    &p[..end]
}

/// The last-component verbs (`dirname`/`basename`/`ext`/`stem`) — all lexical, all sharing the
/// normalize-then-split-at-the-last-separator shape.
fn add_path_component_fns(lua: &Lua, path: &Table) -> mlua::Result<()> {
    // path.dirname(p) — everything before the last component; "." for a bare name, the root for a
    // first-level entry ("/a" → "/", "C:/a" → "C:/").
    path.set(
        "dirname",
        lua.create_function(|_, p: String| {
            let s = path_norm_seps(&p);
            let s = path_trim_trailing(&s);
            let root = path_root(s);
            if s == root {
                return Ok(if root.is_empty() { ".".into() } else { root.to_string() });
            }
            match s.rfind('/') {
                None => Ok(".".to_string()),
                Some(i) if i < root.len() => Ok(root.to_string()),
                Some(i) => Ok(s[..i.max(root.len())].to_string()),
            }
        })?,
    )?;

    // path.basename(p) — the last component ("" for a bare root, which has none).
    path.set(
        "basename",
        lua.create_function(|_, p: String| {
            let s = path_norm_seps(&p);
            let s = path_trim_trailing(&s);
            let root = path_root(s);
            if s == root {
                return Ok(String::new());
            }
            Ok(match s.rfind('/') {
                None => s[root.len()..].to_string(),
                Some(i) => s[i + 1..].to_string(),
            })
        })?,
    )?;

    // path.ext(p) — the extension of the last component, WITHOUT the dot ("txt", not ".txt");
    // "" when there is none. A dotfile (".gitignore") is all stem, no extension.
    path.set(
        "ext",
        lua.create_function(|_, p: String| {
            let s = path_norm_seps(&p);
            let s = path_trim_trailing(&s);
            let base = s.rfind('/').map_or(s, |i| &s[i + 1..]);
            Ok(match base.rfind('.') {
                Some(i) if i > 0 => base[i + 1..].to_string(),
                _ => String::new(),
            })
        })?,
    )?;

    // path.stem(p) — the last component minus its extension ("b.tar.gz" → "b.tar").
    path.set(
        "stem",
        lua.create_function(|_, p: String| {
            let s = path_norm_seps(&p);
            let s = path_trim_trailing(&s);
            let base = s.rfind('/').map_or(s, |i| &s[i + 1..]);
            Ok(match base.rfind('.') {
                Some(i) if i > 0 => base[..i].to_string(),
                _ => base.to_string(),
            })
        })?,
    )?;

    Ok(())
}

fn make_path(lua: &Lua) -> mlua::Result<Table> {
    let path = lua.create_table()?;

    // path.join(a, b, …) — segments joined with `/`; empty segments contribute nothing, and an
    // absolute later segment resets the join (the std::path law: predictable, not surprising).
    path.set(
        "join",
        lua.create_function(|_, segments: mlua::Variadic<String>| {
            let mut out = String::new();
            for seg in segments.iter() {
                let seg = path_norm_seps(seg);
                if seg.is_empty() {
                    continue;
                }
                if out.is_empty() || path_is_absolute(&seg) {
                    out = seg;
                } else {
                    while out.ends_with('/') {
                        out.pop();
                    }
                    out.push('/');
                    out.push_str(seg.trim_start_matches('/'));
                }
            }
            Ok(out)
        })?,
    )?;

    add_path_component_fns(lua, &path)?;

    // path.normalize(p) — collapse `.`/`..`/duplicate separators, strip trailing slash, emit `/`.
    // Purely lexical (no filesystem): leading `..` in a relative path survives; `..` cannot climb
    // above a root. "" and a fully-collapsed relative path normalize to ".".
    path.set(
        "normalize",
        lua.create_function(|_, p: String| {
            let s = path_norm_seps(&p);
            let root = path_root(&s).to_string();
            let mut stack: Vec<&str> = Vec::new();
            for comp in s[root.len()..].split('/') {
                match comp {
                    "" | "." => {}
                    ".." => match stack.last() {
                        Some(&last) if last != ".." => {
                            stack.pop();
                        }
                        _ if !root.is_empty() => {} // cannot climb above a root
                        _ => stack.push(".."),
                    },
                    c => stack.push(c),
                }
            }
            let joined = stack.join("/");
            Ok(if root.is_empty() {
                if joined.is_empty() { ".".to_string() } else { joined }
            } else if joined.is_empty() {
                root
            } else {
                root + &joined
            })
        })?,
    )?;

    // path.is_absolute(p) — unix ("/…"), drive ("C:/…" or "C:\…"), and UNC ("//server/…") roots.
    path.set(
        "is_absolute",
        lua.create_function(|_, p: String| Ok(path_is_absolute(&p)))?,
    )?;

    Ok(path)
}

// ---------------------------------------------------------------------------------------------
// net
// ---------------------------------------------------------------------------------------------

fn make_net(lua: &Lua) -> mlua::Result<Table> {
    let net = lua.create_table()?;

    // net.free_port() → an OS-assigned free TCP port on 127.0.0.1. Bind to :0, read the assigned
    // port, and release it. The classic use is a dynamic port for a locally `shell.spawn`ed app (a
    // container gets its random host port from `docker.run` instead). There is an inherent race —
    // the port is free *now*, not guaranteed still free when the app binds — but in practice the
    // window is tiny and this is the standard approach.
    net.set(
        "free_port",
        lua.create_function(|_, ()| {
            let listener = std::net::TcpListener::bind(("127.0.0.1", 0))
                .map_err(|e| mlua::Error::RuntimeError(format!("net.free_port: {e}")))?;
            let port = listener
                .local_addr()
                .map_err(|e| mlua::Error::RuntimeError(format!("net.free_port: {e}")))?
                .port();
            Ok(port)
        })?,
    )?;

    Ok(net)
}

// ---------------------------------------------------------------------------------------------
// http (async; HTTP-only in v1 — https lands behind a later `tls` feature)
// ---------------------------------------------------------------------------------------------

#[cfg(feature = "http")]
mod http;

// ---------------------------------------------------------------------------------------------
// mock — the `mock` facet: an in-process stub/record server (`http.mock`)
// ---------------------------------------------------------------------------------------------

/// `http.mock` — the fourth facet, alongside `client` (attach to a real one), `container`
/// (provision a real one), and `wait_for` (probe one). It provisions a *fake* one: a real HTTP
/// server, in this process, that you stub, drive, and then assert on.
///
/// **It is not for the dependency you can run.** Prova's whole containerized-topology arc exists so
/// a test can drive the real thing; a mock earns its place on the boundary you cannot own (a partner
/// API), the behavior the real thing will not produce on demand (a 5xx, a timeout), and — the one
/// with no substitute — the *interaction itself*: a real dependency answers "did it work", never
/// "did we call it exactly once with the right idempotency key". See `docs/plans/mocks.md`.
///
/// **Handlers are Lua, and that is the point.** A stub's reply may be a table (terse) or a function
/// (general). The function runs on this very Lua state while the test coroutine that drove the SUT
/// is suspended — which is only possible because the engine is async to the ground (`engine.rs`:
/// bodies are `call_async`'d futures in a `FuturesUnordered`) and because `block_on_local` polls a
/// `LocalSet` alongside them. That is why there is no response-templating mini-language here: the
/// thing WireMock invented Handlebars to approximate is just a Lua closure.
///
/// **Readiness is a contract, as with `docker.run`'s `wait`.** The listener is bound *synchronously*
/// before `http.mock` returns, so the first request cannot race the bind and no caller needs a
/// `prova.retry`. In-process is what buys that — there is no daemon in the middle to lie about it.
#[cfg(feature = "mock")]
mod mock;

// ---------------------------------------------------------------------------------------------
// docker (testcontainers-style ephemeral dependencies, via the typed bollard daemon client)
// ---------------------------------------------------------------------------------------------

#[cfg(feature = "docker")]
pub(crate) mod docker;

// ---------------------------------------------------------------------------------------------
// sql (postgres/mysql/sqlite namespaces over one generic Connection via sqlx's `Any` driver)
// ---------------------------------------------------------------------------------------------

#[cfg(feature = "sqlite")]
mod sql;

// ---------------------------------------------------------------------------------------------
// grpc (async; native — no `grpcurl` binary. Plaintext-only in v1, like http.)
// ---------------------------------------------------------------------------------------------

// A *dynamic* gRPC client: it learns the server's schema at runtime via gRPC Server Reflection
// (so tests need no `.proto` files and no codegen), builds request messages from Lua tables against
// the fetched descriptors, invokes with a generic tonic codec over `DynamicMessage`, and decodes the
// reply back to a Lua table. This keeps prova a single self-contained binary — the whole point of
// going native instead of shelling out to `grpcurl`. The server must have reflection enabled; if it
// doesn't, `grpc.client` fails with a clear message (a proto-file path mode can layer on later).
#[cfg(feature = "grpc")]
mod grpc;

// ---------------------------------------------------------------------------------------------
// grpc_mock — the `mock` facet on the grpc namespace (`grpc.mock`)
// ---------------------------------------------------------------------------------------------

/// `grpc.mock` — a real gRPC server, in this process, that you stub and then assert on.
///
/// **The client's central trick does not invert, and that is the whole design problem.**
/// `grpc.client` needs no `.proto` because it learns the schema *from the server* over reflection. A
/// mock **is** the server: there is nobody to learn from, so it must be told. `proto` compiles a
/// `.proto` at runtime via `protox` — pure Rust, no `protoc` on PATH, which keeps the module's
/// promise ("no codegen") intact on the server side too. (A `FileDescriptorSet` and harvesting from
/// a live service are the other two sources; see `docs/plans/mocks.md` §6.)
///
/// **The mock serves reflection itself**, from the real `tonic-reflection` server. That is what lets
/// the *unmodified* `grpc.client` drive it with no special case — and it is the honest bar: if the
/// real client cannot tell the mock from a server, it is a server.
///
/// **Lua handlers survive the trip to HTTP/2**, which was not obvious. Two properties make it work,
/// and both are load-bearing: `tonic::server::UnaryService::Future` carries **no `Send` bound** (only
/// the request body must be Send, and hyper's `Incoming` is), and hyper's http2 delegates spawning to
/// a generic `E: Executor` that is likewise unbounded — so a `LocalExec` built on `spawn_local` keeps
/// the whole connection on the Lua thread. Reflection, which never touches Lua, is free to keep its
/// `Send` boxed future right next to it.
#[cfg(feature = "grpc-mock")]
mod grpc_mock;

// ---------------------------------------------------------------------------------------------
// yaml (sync — decode YAML text to Lua values; the counterpart to http's `:json()`)
// ---------------------------------------------------------------------------------------------

// A general capability for a cloud-oriented, polyglot world: k8s manifests, CI configs, and compose
// files are all YAML. `yaml.decode` handles a single document; `yaml.decode_all` handles a
// multi-document stream (`---`-separated), which is exactly what Kubernetes manifests use.
//
// The `_all` pair is the one place a format module carries more than decode/encode, and it earns it:
// `---` streams are a real YAML feature with no analogue in json/toml/csv. The suffix — rather than a
// separate verb — is what keeps the extra capability inside the one naming rule (see `format_names`).
#[cfg(feature = "yaml")]
mod yaml;

// ---------------------------------------------------------------------------------------------
// graphql (async; POST { query, variables } → { data, errors } over HTTP — the third transport)
// ---------------------------------------------------------------------------------------------

// GraphQL is one endpoint spoken over HTTP POST, so this is a thin, consistent layer: a client bound
// to a URL + headers, with `:query` (the happy path — returns `data`, raises if the response carries
// `errors`) and `:execute` (the full `{ data, errors, status }` envelope, for asserting on errors) —
// mirroring the grpc module's `call` / `call_status`. Queries and mutations share the transport.
#[cfg(feature = "graphql")]
mod graphql;
