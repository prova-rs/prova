//! The `archetect` plugin module for prova: render an archetype **in-process** via archetect-core
//! and expose it to the Lua DSL as `archetect.render{...}`.
//!
//! This is the justifying use case for prova — testing rendered archetypes without a subprocess, so
//! answers pass as data and failures surface as real diagnostics. It lives in its own crate (not
//! `prova-core`) to keep the core domain-agnostic: `prova_archetect::install` is a `prova_core`
//! plugin module the host wires in.
//!
//! Rendering runs on a dedicated OS thread (join-blocking) so it is fully isolated from prova's
//! per-worker Tokio runtime — archetect's render is synchronous, and a fresh thread guarantees no
//! nested-runtime surprise. It is headless (never prompts): supply `answers` for anything without a
//! default, or `defaults = true` to take every default.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use archetect_api::{ClientMessage, ContextValue, IoError, ScriptIoHandle, ScriptMessage};

/// Re-exported so callers of [`render_headless`] can build answer maps without a direct
/// archetect-api dependency.
pub use archetect_api::ContextMap;
use archetect_core::archetype::render_context::RenderContext;
use archetect_core::configuration::Configuration;
use archetect_core::errors::ArchetectError;
use archetect_core::system::XdgSystemLayout;
use archetect_core::Archetect;
use archetect_terminal_io::TerminalScriptIoHandle;
use camino::Utf8PathBuf;
use mlua::{Lua, Table, Value};

/// Install the `archetect` global into a Lua state. Pass to `RunConfig::with_module`.
pub fn install(lua: &Lua) -> mlua::Result<()> {
    let archetect = lua.create_table()?;
    archetect.set("render", lua.create_function(render)?)?;
    lua.globals().set("archetect", archetect)?;
    // `archetect.verify{...}` is authoring sugar composed from prova primitives + fs/shell/yaml, so
    // it lives in Lua rather than Rust. It defines the function; the globals it uses are resolved
    // when it is *called* (at collection), by which point they all exist.
    lua.load(VERIFY_LUA)
        .set_name("@prova-archetect/verify")
        .exec()?;
    Ok(())
}

/// The declarative archetype check — prova's answer to the pytest harness's `manifest.yaml`, matched
/// field-for-field but as real Lua you can extend. Two calling forms over one core:
///
///   archetect.verify{ source = ..., <checks> }        -- one-shot: renders, then registers checks
///   archetect.verify(render_fixture, { <checks> })    -- compositional: checks an existing render
///
/// The compositional form is what makes render → verify → black-box a single pipeline: declare the
/// render fixture yourself (your name, scope, destination, computed answers), point `verify` at it,
/// and hang boot/probe fixtures off the same handle. The one-shot is sugar that creates the fixture
/// (`Scope.File` unless `spec.scope` says otherwise) and delegates. Both return the render fixture.
const VERIFY_LUA: &str = r#"
function archetect.verify(a, b)
  local rendered, spec
  if b ~= nil then
    rendered, spec = a, b
    assert(type(spec) == "table", "archetect.verify(fixture, checks) expects a checks table")
    assert(spec.source == nil,
      "archetect.verify(fixture, checks): `source` belongs to the one-shot form — the fixture already owns the render")
  else
    spec = a
    assert(type(spec) == "table", "archetect.verify expects a table")
    assert(spec.source, "archetect.verify requires a `source` (or pass a render fixture as the first argument)")
  end
  local label = spec.name or "archetype"
  local timeout = spec.timeout or "600s"

  -- One-shot form: render once (headless by default); every check below shares this output.
  if rendered == nil then
    rendered = prova.fixture(label .. ":render", spec.scope or Scope.File, function(ctx)
      return archetect.render{
        source = spec.source,
        answers = spec.answers,
        switches = spec.switches,
        defaults = spec.defaults ~= false,
        destination = ctx:tempdir(),
      }
    end)
  end

  -- The project root, optionally a subdirectory the render produces (like the manifest's project_dir).
  local function project(t)
    local tree = t:use(rendered)
    if spec.project_dir then return tree:dir(spec.project_dir) end
    return tree
  end

  prova.describe(label, function()
    if (spec.expected_files and #spec.expected_files > 0)
       or (spec.absent_files and #spec.absent_files > 0) then
      prova.test("layout", function(t)
        local p = project(t)
        t:expect_all(function()
          for _, f in ipairs(spec.expected_files or {}) do
            t:expect(p:file(f), f):exists()
          end
          for _, f in ipairs(spec.absent_files or {}) do
            t:expect(p:file(f), f):never():exists()
          end
        end)
      end)
    end

    if spec.fully_rendered ~= false then
      prova.test("fully rendered", function(t)
        t:expect(project(t)):is_fully_rendered()
      end)
    end

    if spec.yaml_globs and #spec.yaml_globs > 0 then
      prova.test("yaml manifests parse", function(t)
        local root = project(t).path
        for _, g in ipairs(spec.yaml_globs) do
          local matches = fs.glob(root, g)
          t:expect(#matches, "glob '" .. g .. "' matched no files"):never():equals(0)
          for _, path in ipairs(matches) do
            yaml.parse_all(fs.read(path))  -- raises (fails the test) on invalid YAML
          end
        end
      end)
    end

    if spec.build_steps and #spec.build_steps > 0 then
      prova.test("build", { requires = spec.requires or {}, tags = { "build" }, timeout = timeout }, function(t)
        local p = project(t)
        for _, step in ipairs(spec.build_steps) do
          local cmd = type(step) == "table" and table.concat(step, " ") or step
          local r = shell.run(cmd, { cwd = p.path, env = spec.env, timeout = timeout })
          t:expect(r.code, cmd):equals(0)
        end
      end)
    end
  end)

  return rendered
end
"#;

/// `archetect.render{ source, answers?, switches?, defaults?, destination? }` → a tree handle
/// rooted at the destination (`out.path`, `out:file(rel)`, `out:read()`, `out.writes`).
fn render(lua: &Lua, opts: Table) -> mlua::Result<Table> {
    let source: String = opts
        .get::<Option<String>>("source")?
        .ok_or_else(|| mlua::Error::RuntimeError("archetect.render requires a `source`".into()))?;
    let switches: Vec<String> = opts
        .get::<Option<Vec<String>>>("switches")?
        .unwrap_or_default();
    let use_defaults = opts.get::<Option<bool>>("defaults")?.unwrap_or(false);
    let answers = parse_answers(opts.get::<Option<Table>>("answers")?)?;
    let destination = match opts.get::<Option<String>>("destination")? {
        Some(d) => PathBuf::from(d),
        None => temp_destination()
            .map_err(|e| mlua::Error::RuntimeError(format!("archetect.render tempdir: {e}")))?,
    };

    let writes = render_on_thread(source, destination.clone(), answers, switches, use_defaults)
        .map_err(mlua::Error::RuntimeError)?;

    let handle = path_handle(lua, destination.to_string_lossy().into_owned())?;
    handle.set("writes", lua.create_sequence_from(writes)?)?;
    Ok(handle)
}

/// Convert a Lua `answers` table into a `ContextMap`. Supports string / integer / number / boolean
/// values and string arrays — the common archetype answer shapes.
fn parse_answers(answers: Option<Table>) -> mlua::Result<ContextMap> {
    let mut map = ContextMap::new();
    if let Some(table) = answers {
        for pair in table.pairs::<String, Value>() {
            let (key, value) = pair?;
            map.insert(key, lua_to_context_value(value)?);
        }
    }
    Ok(map)
}

fn lua_to_context_value(value: Value) -> mlua::Result<ContextValue> {
    Ok(match value {
        Value::String(s) => ContextValue::String(s.to_str()?.to_string()),
        Value::Integer(i) => ContextValue::Integer(i),
        Value::Number(n) => ContextValue::Float(n),
        Value::Boolean(b) => ContextValue::Boolean(b),
        Value::Table(t) => {
            // Treat a sequence table as an array of values.
            let mut items = Vec::new();
            for item in t.sequence_values::<Value>() {
                items.push(lua_to_context_value(item?)?);
            }
            ContextValue::Array(items)
        }
        other => {
            return Err(mlua::Error::RuntimeError(format!(
                "archetect.render: unsupported answer value of type {}",
                other.type_name()
            )))
        }
    })
}

/// Run the render on a dedicated OS thread and join — isolates it from prova's Tokio runtime, and
/// keeps `ArchetectError` (not `Send`-guaranteed) from crossing the boundary by stringifying first.
fn render_on_thread(
    source: String,
    destination: PathBuf,
    answers: ContextMap,
    switches: Vec<String>,
    use_defaults: bool,
) -> Result<Vec<String>, String> {
    std::thread::spawn(move || {
        render_headless(&source, &destination, answers, switches, use_defaults)
            .map_err(|e| e.to_string())
    })
    .join()
    .map_err(|_| "archetect render thread panicked".to_string())?
}

/// One archetect system layout (cache/config/data) per process. archetect-core tracks fetched
/// sources in a process-global set (`source::cached_paths`), so the layout must be process-global
/// too: with a per-render temp layout, the second render in a process believes its (empty) cache
/// is already warm and aborts resolving its first catalog library. Sharing the layout also means
/// catalog libraries are cloned once per run instead of once per render.
fn shared_layout_root() -> Result<&'static Path, String> {
    static ROOT: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    ROOT.get_or_init(|| {
        let mut path = std::env::temp_dir();
        path.push(format!("prova-archetect-{}", std::process::id()));
        std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
        Ok(path)
    })
    .as_deref()
    .map_err(|e| e.clone())
}

/// archetect-core's fetched-source tracking is not safe for two concurrent fetches into one
/// cache, so renders serialize per process. Subsequent renders are mostly cache hits.
fn render_mutex() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Render an archetype headlessly, returning the paths written (in order). No terminal, no prompts:
/// headless resolves every prompt from its default or a supplied answer, or errors if neither.
pub fn render_headless(
    source: &str,
    destination: &Path,
    answers: ContextMap,
    switches: Vec<String>,
    use_defaults: bool,
) -> Result<Vec<String>, ArchetectError> {
    let dest = Utf8PathBuf::from_path_buf(destination.to_path_buf())
        .map_err(|_| ArchetectError::GeneralError("non-UTF-8 destination".into()))?;

    let _serialized = render_mutex()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let writes = Arc::new(Mutex::new(Vec::new()));
    let handle = CapturingIoHandle {
        writes: writes.clone(),
        pending: Mutex::new(None),
    };

    let configuration = Configuration::default().with_headless(true);

    let layout_root = shared_layout_root().map_err(ArchetectError::GeneralError)?;
    let layout = archetect_core::system::RootedSystemLayout::new(
        Utf8PathBuf::from_path_buf(layout_root.to_path_buf())
            .map_err(|_| ArchetectError::GeneralError("non-UTF-8 temp dir".into()))?,
    )?;

    let archetect = Archetect::builder()
        .with_driver(handle)
        .with_configuration(configuration)
        .with_layout(layout) // process-shared cache/config/data — NOT the render destination
        .build()?;

    let archetype = archetect.new_archetype(source)?;

    let mut ctx = RenderContext::new(dest, answers);
    for switch in switches {
        ctx = ctx.with_switch(switch);
    }
    if use_defaults {
        ctx = ctx.with_use_defaults_all(true);
    }

    archetype.render(ctx)?; // ArchetypeError -> ArchetectError via #[from]

    let writes = Arc::try_unwrap(writes)
        .map(|m| m.into_inner().unwrap_or_default())
        .unwrap_or_else(|arc| arc.lock().unwrap().clone());
    Ok(writes)
}

/// Build an archetect answer map from plain string key/values — what `prova init` collects from
/// `config.toml` baked answers and `--answer key=value`. String is the only answer shape init needs;
/// richer types stay behind Lua's `archetect.render`. Keeps archetect-api types out of the CLI.
pub fn string_answers(pairs: impl IntoIterator<Item = (String, String)>) -> ContextMap {
    let mut map = ContextMap::new();
    for (key, value) in pairs {
        map.insert(key, ContextValue::String(value));
    }
    map
}

/// Render an archetype for `prova init` — the CLI scaffolding path (as opposed to the in-test
/// [`render_headless`]). Uses archetect's real **XDG** system layout, so the source and its catalog
/// libraries fetch and cache where a normal archetect install keeps them (not a throwaway temp dir).
///
/// Two modes over one core:
///   - `headless == false`: interactive. The terminal driver ([`TerminalScriptIoHandle`]) prompts
///     (via inquire) for any answer the supplied `answers` / `switches` / `defaults` don't cover.
///   - `headless == true`: no prompts. Every prompt resolves from an answer or its default; a prompt
///     with neither is a hard error (never a hang) — the mode CI and `prova init --headless` use.
///
/// `defaults` maps to archetect's *use-defaults-all*: take a prompt's default without asking (still
/// interactive for prompts that have no default, unless `headless`). Returns the paths written in the
/// headless case; interactive rendering returns an empty list (the terminal driver owns its own
/// progress output).
pub fn render_interactive(
    source: &str,
    destination: &Path,
    answers: ContextMap,
    switches: Vec<String>,
    defaults: bool,
    headless: bool,
) -> Result<Vec<String>, ArchetectError> {
    let dest = Utf8PathBuf::from_path_buf(destination.to_path_buf())
        .map_err(|_| ArchetectError::GeneralError("non-UTF-8 destination".into()))?;

    // Serialize per process against archetect-core's process-global source tracking (same reason
    // `render_headless` does). `init` renders once, so this is virtually always uncontended.
    let _serialized = render_mutex()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let configuration = Configuration::default().with_headless(headless);
    let layout = XdgSystemLayout::new()
        .map_err(|e| ArchetectError::GeneralError(format!("archetect system layout: {e}")))?;

    // The two drivers are different types, so the build+render is inlined per branch rather than
    // routed through one generic helper.
    if headless {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let handle = CapturingIoHandle {
            writes: writes.clone(),
            pending: Mutex::new(None),
        };
        let archetect = Archetect::builder()
            .with_driver(handle)
            .with_configuration(configuration)
            .with_layout(layout)
            .build()?;
        let archetype = archetect.new_archetype(source)?;
        let mut ctx = RenderContext::new(dest, answers);
        for switch in switches {
            ctx = ctx.with_switch(switch);
        }
        if defaults {
            ctx = ctx.with_use_defaults_all(true);
        }
        archetype.render(ctx)?;
        let writes = Arc::try_unwrap(writes)
            .map(|m| m.into_inner().unwrap_or_default())
            .unwrap_or_else(|arc| arc.lock().unwrap().clone());
        Ok(writes)
    } else {
        let archetect = Archetect::builder()
            .with_driver(TerminalScriptIoHandle::default())
            .with_configuration(configuration)
            .with_layout(layout)
            .build()?;
        let archetype = archetect.new_archetype(source)?;
        let mut ctx = RenderContext::new(dest, answers);
        for switch in switches {
            ctx = ctx.with_switch(switch);
        }
        if defaults {
            ctx = ctx.with_use_defaults_all(true);
        }
        archetype.render(ctx)?;
        Ok(Vec::new())
    }
}

/// A headless client handle: writes files/dirs to disk, records file paths, Acks each write, and
/// logs everything else. Single-threaded lockstep — the engine calls `receive` immediately after
/// each `send`. Headless renders never emit `PromptFor*`, so this never blocks on a prompt.
#[derive(Debug)]
struct CapturingIoHandle {
    writes: Arc<Mutex<Vec<String>>>,
    pending: Mutex<Option<ClientMessage>>,
}

impl ScriptIoHandle for CapturingIoHandle {
    fn send(&self, message: ScriptMessage) -> Result<(), IoError> {
        match message {
            ScriptMessage::WriteDirectory(info) => {
                let _ = std::fs::create_dir_all(&info.path);
                *self.pending.lock().unwrap() = Some(ClientMessage::Ack);
            }
            ScriptMessage::WriteFile(info) => {
                let path = PathBuf::from(&info.destination);
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let reply = match std::fs::write(&path, &info.contents) {
                    Ok(()) => {
                        self.writes.lock().unwrap().push(info.destination);
                        ClientMessage::Ack
                    }
                    Err(e) => ClientMessage::Error(e.to_string()),
                };
                *self.pending.lock().unwrap() = Some(reply);
            }
            // Diagnostics: to stderr, keeping stdout clean for prova's JSON protocol.
            ScriptMessage::LogWarn(m)
            | ScriptMessage::LogError(m)
            | ScriptMessage::CompleteError(m)
            | ScriptMessage::Display(m) => eprintln!("{m}"),
            _ => {} // Log{Trace,Debug,Info}, Print, CompleteSuccess, PromptFor* (never in headless)
        }
        Ok(())
    }

    fn receive(&self) -> Result<ClientMessage, IoError> {
        self.pending
            .lock()
            .unwrap()
            .take()
            .ok_or(IoError::ClientDisconnected)
    }
}

/// Build a path-handle table: `{ path = "...", file(self, rel), dir(self, rel), read(self) }`. The
/// `path` field is what prova's filesystem matchers read, so `t:expect(out:file("x")):exists()`
/// works. `file`/`dir` return child handles; nesting composes.
fn path_handle(lua: &Lua, path: String) -> mlua::Result<Table> {
    let handle = lua.create_table()?;
    handle.set("path", path)?;
    handle.set(
        "file",
        lua.create_function(|lua, (this, rel): (Table, String)| {
            let base: String = this.get("path")?;
            path_handle(lua, join(&base, &rel))
        })?,
    )?;
    handle.set(
        "dir",
        lua.create_function(|lua, (this, rel): (Table, String)| {
            let base: String = this.get("path")?;
            path_handle(lua, join(&base, &rel))
        })?,
    )?;
    handle.set(
        "read",
        lua.create_function(|_, this: Table| {
            let path: String = this.get("path")?;
            std::fs::read_to_string(&path)
                .map_err(|e| mlua::Error::RuntimeError(format!("read {path:?}: {e}")))
        })?,
    )?;
    Ok(handle)
}

fn join(base: &str, rel: &str) -> String {
    Path::new(base).join(rel).to_string_lossy().into_owned()
}

fn temp_destination() -> std::io::Result<PathBuf> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut path = std::env::temp_dir();
    path.push(format!(
        "prova-render-{}-{}-{}",
        std::process::id(),
        nanos,
        n
    ));
    std::fs::create_dir_all(&path)?;
    Ok(path)
}
