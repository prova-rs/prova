//! Scopes & fixtures: the Scope vocabulary, fixture definitions/handles, per-scope
//! state and the teardown machinery.

use super::*;

// ---------------------------------------------------------------------------------------------
// Scopes & fixtures
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ScopeKind {
    Test,
    Flow,
    File,
    Suite,
}

impl ScopeKind {
    pub(super) fn rank(self) -> u8 {
        match self {
            ScopeKind::Test => 0,
            ScopeKind::Flow => 1,
            ScopeKind::File => 2,
            ScopeKind::Suite => 3,
        }
    }
    pub(super) fn label(self) -> &'static str {
        match self {
            ScopeKind::Test => "test",
            ScopeKind::Flow => "flow",
            ScopeKind::File => "file",
            ScopeKind::Suite => "suite",
        }
    }
}

/// A typed fixture-scope value — the members of the `Scope` global (`Scope.Test`/`Scope.Flow`/
/// `Scope.File`/`Scope.Suite`). This is the only way to name a scope; discoverable and typo-safe.
#[derive(Clone, Copy)]
pub(super) struct ScopeRef {
    pub(super) kind: ScopeKind,
}

impl UserData for ScopeRef {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("scope", |_, this| Ok(this.kind.label()));
    }
}

/// Build the `Scope` global — the typed scope constants.
pub(super) fn make_scope_global(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set(
        "Test",
        ScopeRef {
            kind: ScopeKind::Test,
        },
    )?;
    t.set(
        "Flow",
        ScopeRef {
            kind: ScopeKind::Flow,
        },
    )?;
    t.set(
        "File",
        ScopeRef {
            kind: ScopeKind::File,
        },
    )?;
    t.set(
        "Suite",
        ScopeRef {
            kind: ScopeKind::Suite,
        },
    )?;
    Ok(t)
}

pub(super) fn parse_scope(v: Value) -> mlua::Result<ScopeKind> {
    match v {
        Value::UserData(ud) => ud.borrow::<ScopeRef>().map(|r| r.kind).map_err(|_| {
            mlua::Error::RuntimeError(
                "fixture scope must be a Scope value: Scope.Test / Scope.Flow / Scope.File / Scope.Suite"
                    .into(),
            )
        }),
        _ => Err(mlua::Error::RuntimeError(
            "fixture scope must be a Scope value: Scope.Test / Scope.Flow / Scope.File / Scope.Suite"
                .into(),
        )),
    }
}

#[derive(Clone)]
pub(super) struct FixtureDef {
    pub(super) name: String,
    pub(super) scope: ScopeKind,
    pub(super) factory: Function,
    /// True when this fixture was declared via `prova.topology` (rather than `prova.fixture`). A
    /// topology's factory context is "topology-capable": it exposes an ambient managed network on
    /// `ctx.network`. Ordinary fixtures leave it `false`, so `ctx.network` is nil for them.
    pub(super) is_topology: bool,
}

/// Opaque handle returned by `prova.fixture`; carries the registry id `ctx:use` resolves.
pub(super) struct FixtureHandle {
    pub(super) id: usize,
}
impl UserData for FixtureHandle {}

/// Opaque handle returned by `prova.test`/`flow`/`group` (and the builder variants); carries the
/// unit's arena index so `depends_on = { handle }` can resolve the edge. Treat as opaque.
#[derive(Clone, Copy)]
pub(super) struct UnitHandle {
    pub(super) ix: NodeIx,
}
impl UserData for UnitHandle {}

/// A typed resource reference from `prova.port`/`writes`/`reads`. Preferred over magic-format
/// strings (`"port:8080"`) — a constructor validates and can't be typo'd into a wrong-but-valid
/// token. A bare string in a `resources` list is accepted too and is exclusive by default.
#[derive(Clone)]
pub(super) struct ResourceRef {
    pub(super) token: String,
    pub(super) shared: bool,
}
impl UserData for ResourceRef {}

/// Live state for one scope instance: cached fixture values, LIFO teardowns, temp dirs.
#[derive(Default)]
pub(super) struct ScopeState {
    pub(super) cache: HashMap<usize, Value>,
    pub(super) teardowns: Vec<Function>,
    pub(super) tempdirs: Vec<PathBuf>,
    /// The topology's ambient managed network (a `docker.network` handle), created lazily on the
    /// first `ctx.network` access inside a topology factory and cached here on the topology's own
    /// scope instance so repeated reads return the same handle. Its teardown is registered on this
    /// same scope right after creation, so LIFO order reaps it *after* the containers joined to it.
    pub(super) network: Option<Value>,
}

/// Shared across the whole suite run: the fixture registry, the one suite-scope instance, and a lazy
/// **per-file** scope instance (a suite may load several files into one state, and each gets its own
/// `Scope.File`). A single file just has one entry (index 0).
pub(super) struct RunState {
    pub(super) defs: Vec<FixtureDef>,
    pub(super) suite: Rc<RefCell<ScopeState>>,
    pub(super) files: RefCell<HashMap<usize, Rc<RefCell<ScopeState>>>>,
    /// Source path per file index (from the collector), so a test's snapshot assertion can place its
    /// `.snap` beside the file it ran from. Empty for the topology (`up`/`watch`) paths.
    pub(super) file_paths: Vec<PathBuf>,
    /// When set, `matches_snapshot` writes/overwrites snapshots instead of comparing (`--update-snapshots`).
    pub(super) update_snapshots: bool,
    /// Shared registry of referenced `.snap` files, for unreferenced-snapshot reconciliation.
    pub(super) snapshot_registry: Option<SnapshotRegistry>,
    /// The falsification pass is active: apply each leaf's declared mutation before its body and
    /// invert the verdict.
    pub(super) falsify: bool,
}

impl RunState {
    /// The directory a test's `.snap` files live in: `<source-file-dir>/snapshots`, or `None` if the
    /// file index has no recorded path (e.g. an ad-hoc topology run).
    /// The source path for a file index as a display string, or `None` when the index has no
    /// recorded path (an `eval`/topology run) — feeds the reported per-leaf source location.
    pub(super) fn file_path_str(&self, file: usize) -> Option<String> {
        self.file_paths
            .get(file)
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| p.to_string_lossy().into_owned())
    }

    pub(super) fn snapshot_dir(&self, file: usize) -> Option<PathBuf> {
        let p = self.file_paths.get(file)?;
        if p.as_os_str().is_empty() {
            return None;
        }
        Some(p.parent().unwrap_or(Path::new(".")).join("snapshots"))
    }

    /// The `Scope.File` instance for file `idx`, created on first use.
    pub(super) fn file_scope(&self, idx: usize) -> Rc<RefCell<ScopeState>> {
        self.files
            .borrow_mut()
            .entry(idx)
            .or_insert_with(|| Rc::new(RefCell::new(ScopeState::default())))
            .clone()
    }
}

/// Async so a `ctx:defer` callback can `await` (e.g. `proc:stop()` to kill a spawned process, or any
/// async resource cleanup). Sync callbacks just complete immediately under `call_async`.
///
/// Returns whatever raised, so the caller can report it. **Teardown errors used to be discarded**
/// (`let _ = …`), which mattered far more than "a TODO": `ctx:manage` teardown is what stops
/// containers, so a cleanup that raised was a *leaked container the run reported as green*. The
/// failure could not be seen, only noticed later as a stray container.
///
/// Every teardown still runs even if an earlier one raises. One bad `defer` must not strand the
/// cleanups registered around it, or a single mistake leaks every resource behind it.
#[must_use = "teardown errors must be reported, not dropped — that was the bug this returns to fix"]
pub(super) async fn teardown_scope(scope: &Rc<RefCell<ScopeState>>) -> Vec<String> {
    let (teardowns, tempdirs) = {
        let mut s = scope.borrow_mut();
        (
            std::mem::take(&mut s.teardowns),
            std::mem::take(&mut s.tempdirs),
        )
    };
    let mut errors = Vec::new();
    // LIFO: last registered runs first, so a fixture's cleanup runs before its dependencies'.
    for f in teardowns.into_iter().rev() {
        if let Err(e) = f.call_async::<()>(()).await {
            errors.push(e.to_string());
        }
    }
    for dir in tempdirs.into_iter().rev() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    errors
}

/// A scope's teardown failures, as their own reported leaf.
///
/// **Why separate leaves rather than blaming a test** (resolving `docs/design/api.md` §Open
/// questions #2). A `Scope.File` fixture tears down after *every* test in the file, so no single
/// test owns the failure — attributing it to one would blame whichever test happened to sort last.
/// And a teardown failure is not the test's fault: it happened *after* the body passed, so turning
/// that test red would report a defect in the wrong place. It is its own event, so it gets its own
/// node: `<scope> ⟶ teardown`, counted in `failed` like any other. That needs no new reporting
/// concept — `Event::NodeFinished` already carries a path, an outcome, and a message.
/// Tear every scope down and report failures to **stderr**, for the paths with no reporter: `eval`,
/// `up`, `watch`, `down`, and partial-provision cleanup.
///
/// They must go somewhere. These are exactly the paths that stop containers, so a swallowed teardown
/// error is a resource still running after prova said it was done — the operator's machine quietly
/// accumulating what a green run promised it had reaped.
pub(super) async fn teardown_all_and_warn(state: &RunState) {
    let mut late = teardown_file_scopes(state).await;
    late.extend(teardown_results(
        "suite",
        teardown_scope(&state.suite).await,
        None,
        None,
    ));
    for r in &late {
        eprintln!(
            "prova: {} failed: {}",
            r.path,
            r.message.as_deref().unwrap_or("(no message)")
        );
    }
    if !late.is_empty() {
        eprintln!(
            "prova: {} teardown failure(s) — resources may still be running; check `docker ps`",
            late.len()
        );
    }
}

pub(super) fn teardown_results(
    label: &str,
    errors: Vec<String>,
    file: Option<&str>,
    line: Option<u32>,
) -> Vec<NodeResult> {
    errors
        .into_iter()
        .map(|message| NodeResult {
            path: format!("{label} ⟶ teardown"),
            outcome: Outcome::Failed,
            duration: Duration::ZERO,
            assertions: 0,
            message: Some(message),
            file: file.map(str::to_string),
            line,
            teardown: true,
            promises: None,
        })
        .collect()
}

pub(crate) fn make_tempdir() -> std::io::Result<PathBuf> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut path = std::env::temp_dir();
    path.push(format!("prova-{}-{}-{}", std::process::id(), nanos, n));
    std::fs::create_dir_all(&path)?;
    Ok(path)
}
