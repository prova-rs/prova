//! Discovery and listing: proof-file discovery, `--list` nodes, per-suite proof
//! obligations, and the package report behind `prova packages`.

use super::*;

/// An empty labeling `Group` node (a file-group). `file`/parent are set by `add`.
pub(super) fn group_node(name: String) -> Node {
    Node {
        name,
        kind: NodeKind::Group,
        params: Params::default(),
        opts: UnitOpts::default(),
        children: vec![],
        body: None,
        falsifier: None,
        case: None,
        file: 0,
        line: None,
    }
}

/// Tear down every per-file `Scope.File` instance (a suite may have several).
/// Tear down every file scope, returning any failures as reported leaves.
///
/// Keyed by file *index* rather than a path because that is the identity the scope map carries;
/// where a real path is known it names the node, so a report says which file leaked.
pub(super) async fn teardown_file_scopes(state: &RunState) -> Vec<NodeResult> {
    let scopes: Vec<(usize, Rc<RefCell<ScopeState>>)> = state
        .files
        .borrow()
        .iter()
        .map(|(i, s)| (*i, s.clone()))
        .collect();
    let mut out = Vec::new();
    for (idx, scope) in scopes {
        let errors = teardown_scope(&scope).await;
        if errors.is_empty() {
            continue;
        }
        let file = state.file_path_str(idx);
        let label = file.clone().unwrap_or_else(|| format!("file {idx}"));
        out.extend(teardown_results(&label, errors, file.as_deref(), None));
    }
    out
}

/// Discovery: collect the test tree without executing tests (basis for a GUI/IDE model view).
pub fn discover_path(path: &Path) -> mlua::Result<Vec<String>> {
    discover_path_with(path, &RunConfig::new(1))
}

/// Discovery with plugin modules installed. Collection runs the file's top level, so any plugin
/// global used there (e.g. `archetect.verify` registering tests) must exist during discovery too —
/// pass the same `RunConfig` you would run with.
pub fn discover_path_with(path: &Path, config: &RunConfig) -> mlua::Result<Vec<String>> {
    Ok(list_path_nodes(path, config)?.into_iter().map(|n| n.path).collect())
}

/// The state-carrying discovery for one file — `discover_path_with` drops the state for its
/// `Vec<String>` callers; the suite path keeps it. Shared so the single-file fast path in
/// `discover_suite_files` and `discover_path_with` cannot diverge.
pub(super) fn list_path_nodes(path: &Path, config: &RunConfig) -> mlua::Result<Vec<ListNode>> {
    let (_lua, col) = read_and_collect(path, config)?;
    let col = col.borrow();
    list_plan(&col, config)
}

/// Discover node paths for a whole **suite** — the setup (`suite.lua`) loads first, exactly as in
/// `run_suite_files`. The list view must see the same collection a run would: a per-file discover
/// skips the setup, so suite-level opts (a `spec` flag, `requires`, the suite name) silently
/// vanish — and a member file's `spec = false` marker reads as an orphan and errors. Caught by
/// the spec suites' own `--specs --list` (dogfooding).
pub(crate) fn discover_suite_files(
    name: &str,
    setup: Option<&Path>,
    files: &[PathBuf],
    config: &RunConfig,
) -> mlua::Result<Vec<ListNode>> {
    if setup.is_none() && files.len() == 1 {
        return list_path_nodes(&files[0], config);
    }
    let (_lua, col) = load_collection(name, setup, files, config)?;
    let col = col.borrow();
    list_plan(&col, config)
}

/// What a proof owes or discharges — collected WITHOUT running anything, because reconciling
/// prose against proofs is a static question and must not require a green suite (or a docker
/// daemon, or a broker) to answer.
#[derive(Debug, Clone)]
pub struct ProofObligation {
    pub path: String,
    /// The `promises` reason, when this proof is still open.
    pub promises: Option<String>,
    /// Obligation addresses this proof claims to discharge.
    pub covers: Vec<String>,
}

/// Collect every proof's obligations for a suite, without executing them.
pub fn obligations_for_suite(
    setup: Option<&Path>,
    files: &[PathBuf],
    config: &RunConfig,
) -> mlua::Result<Vec<ProofObligation>> {
    let (_lua, col) = load_collection("obligations", setup, files, config)?;
    let col = col.borrow();
    let plan = build_plan(&col, &config.capabilities)?;
    Ok(plan
        .leaves
        .iter()
        .filter(|leaf| leaf.promises.is_some() || !leaf.covers.is_empty())
        .map(|leaf| ProofObligation {
            path: leaf.unit.leaf_paths().first().map(|s| s.to_string()).unwrap_or_default(),
            promises: leaf.promises.clone(),
            covers: leaf.covers.clone(),
        })
        .collect())
}

/// One discovered node: its path plus the two state axes a proof carries — which side of the
/// promise⇄proof duality it sits on, and whether a claim backs it. `prova tests` state-tags on
/// `promised`; `prova specs backfill` gates on `backed`; the plain path listing throws both away.
#[derive(Debug, Clone)]
pub struct ListNode {
    pub path: String,
    /// True when this leaf is an open promise (carries a `promises` reason), false when it is a
    /// settled proof.
    pub promised: bool,
    /// True when this leaf declares at least one `covers` binding — a claim backs it. False is the
    /// backfill red condition: a proof tied to no documented claim (a dangling `covers` still counts
    /// as backed here — the missing claim is `owed`'s concern, not backfill's).
    pub backed: bool,
}

/// The shared tail of discovery: build the plan (validations included), honor selection and the
/// `--specs`/`--proofs` state filter, and return the surviving leaves as `ListNode`s (path + state).
pub(super) fn list_plan(col: &Collector, config: &RunConfig) -> mlua::Result<Vec<ListNode>> {
    let (plan, _deselected, _dropped) =
        apply_selection(build_plan(col, &config.capabilities)?, &config.selection);
    let (plan, _falsify_deselected, _) = apply_falsify_filter(plan, config.falsify);
    let (plan, _spec_deselected, _) =
        apply_specs_filter(plan, config.promises_only, config.proofs_only);
    Ok(plan
        .leaves
        .iter()
        .flat_map(|leaf| {
            let promised = leaf.promises.is_some();
            let backed = !leaf.covers.is_empty();
            leaf.unit
                .leaf_paths()
                .into_iter()
                .map(move |p| ListNode { path: p.to_string(), promised, backed })
        })
        .collect())
}

/// A lint report for a plugin module: the grammar facets it exposes and any conformance issues.
/// What kind of namespace a plugin returned. A plugin is *any* Lua module that returns a table; the
/// resource shape (`client`/`container`/`wait_for`/`mock`) is one common kind, but a library of helpers is
/// equally valid — so lint classifies rather than requiring a fixed shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageShape {
    /// Exposes resource facets (`container`/`client`/`wait_for`/`mock`) — a provisioned, attachable, or
    /// virtualized resource.
    Resource,
    /// A table with no resource facets — a helper library (custom matchers, builders, DSLs, …).
    Library,
}

#[derive(Debug, Default)]
pub struct PackageReport {
    /// The plugin's shape, if it returned a table (`None` only when it returned a non-table).
    pub shape: Option<PackageShape>,
    /// Resource facet names found on the namespace (`client`/`container`/`wait_for`/`mock`). Empty for a
    /// library — which is fine, not an error.
    pub facets: Vec<String>,
    /// Conformance problems that make the plugin *invalid* — non-table return, or a malformed facet.
    /// An empty list means the plugin is well-formed (whatever its shape).
    pub issues: Vec<String>,
}

/// Load a plugin file (with the primitives + searcher installed, exactly as at run time), evaluate it
/// to its returned namespace, and check it against the plugin contract.
///
/// The *only* universal requirement is that a plugin `return`s a table. Beyond that, lint
/// **classifies** rather than prescribes: a namespace exposing resource facets
/// (`client`/`container`/`wait_for`) is a [`PackageShape::Resource`]; a plain table of helpers with no
/// such facets is a [`PackageShape::Library`] — equally valid. It therefore flags only what is wrong
/// for *any* plugin: a non-table return, or a resource facet that is present but not a function.
/// (A `container` facet is expected to yield the `{ client?, url, container }` trio, which can't be
/// verified without provisioning, so that is left to tests.)
pub fn inspect_package(path: &Path, config: &RunConfig) -> mlua::Result<PackageReport> {
    let code = std::fs::read_to_string(path)
        .map_err(|e| mlua::Error::RuntimeError(format!("cannot read {}: {e}", path.display())))?;
    let (lua, _col) = build_lua("plugin".to_string(), config)?;
    let value: Value = lua.load(&code).set_name(file_chunk_name(path)).eval()?;

    let mut report = PackageReport::default();
    let Value::Table(ns) = value else {
        report.issues.push(format!(
            "the package must `return` a namespace table, but returned a {}",
            value.type_name()
        ));
        return Ok(report);
    };

    // Recognized resource facets, in grammar order. A present facet must be a function; a malformed
    // one is an issue. Absent facets are fine — that just means this isn't a resource plugin.
    for facet in ["client", "container", "wait_for", "mock"] {
        match ns.get::<Value>(facet)? {
            Value::Nil => {}
            Value::Function(_) => report.facets.push(facet.to_string()),
            other => report.issues.push(format!(
                "`{facet}` should be a function, but is a {}",
                other.type_name()
            )),
        }
    }

    report.shape = Some(if report.facets.is_empty() {
        PackageShape::Library
    } else {
        PackageShape::Resource
    });
    Ok(report)
}
