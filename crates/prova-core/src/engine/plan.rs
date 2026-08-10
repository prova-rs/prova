//! Planning: collected definition tree -> flat leaf plan, through the selection,
//! falsify, switch and specs filters, with dependency expansion and cycle checks.

use super::*;

// ---------------------------------------------------------------------------------------------
// Plan (definition → plan → execute)
// ---------------------------------------------------------------------------------------------

pub(super) struct PlanItem {
    pub(super) path: String,
    pub(super) body: Function,
    /// Applied before the body under `prova falsify`, never on the ordinary path.
    pub(super) falsifier: Option<Function>,
    pub(super) timeout: Option<Duration>,
    pub(super) case: Option<Value>,
    /// Source file index — selects this item's `Scope.File` instance.
    pub(super) file: usize,
    /// Declaration line in that file (captured at registration), for reported source locations.
    pub(super) line: Option<u32>,
}

/// A scheduling atom. Independent units may run concurrently (`buffer_unordered`); a flow's steps
/// are serial *within* the unit but the flow parallelizes with its siblings like any other unit.
pub(super) enum PlanUnit {
    Test(PlanItem),
    Flow { steps: Vec<PlanItem> },
}

impl PlanUnit {
    /// Every reported leaf path in this unit (a test is one; a flow is one per step).
    pub(super) fn leaf_paths(&self) -> Vec<&str> {
        match self {
            PlanUnit::Test(item) => vec![item.path.as_str()],
            PlanUnit::Flow { steps } => steps.iter().map(|s| s.path.as_str()).collect(),
        }
    }

    /// Every reported leaf item in this unit — `leaf_paths` with the whole item, for callers that
    /// also need the source location.
    pub(super) fn items(&self) -> Vec<&PlanItem> {
        match self {
            PlanUnit::Test(item) => vec![item],
            PlanUnit::Flow { steps } => steps.iter().collect(),
        }
    }
}

// The expect is the design: register_test constructs every Test/Step node WITH a body, so a
// bodyless one here is a collector bug — and panicking on it beats the alternative, because
// silently dropping the node would run the suite minus one proof and report green (the vacuous
// pass this whole system exists to prevent).
#[allow(clippy::expect_used)]
pub(super) fn plan_item(node: &Node, ancestors: &[String]) -> PlanItem {
    let mut path = ancestors.to_vec();
    path.push(format!("{}{}", node.name, node.params.suffix()));
    PlanItem {
        path: path.join(" › "),
        body: node.body.clone().expect("test/step node has a body"),
        falsifier: node.falsifier.clone(),
        timeout: node.opts.timeout,
        case: node.case.clone(),
        file: node.file,
        line: node.line,
    }
}

/// One schedulable unit: a top-level `test` or a `flow` (a group is not a leaf — it expands to the
/// leaves under it). `deps` are the leaf ids this leaf must wait for; a leaf is skipped if any of
/// them failed or was skipped. `deps` and `reqs` already fold in **inherited** group-level options.
pub(super) struct Leaf {
    pub(super) unit: PlanUnit,
    /// Node-level dependencies (own `depends_on` + inherited from ancestor groups), pre-expansion.
    pub(super) raw_deps: Vec<NodeIx>,
    /// Expanded leaf-id dependencies (filled by `expand_deps`).
    pub(super) deps: Vec<usize>,
    /// Resources this leaf holds while running (own + inherited; plus the injected global for
    /// `serial`). The scheduler will not co-schedule two leaves whose reqs conflict.
    pub(super) reqs: Vec<ResourceReq>,
    /// Process-wide exclusive (never concurrent with anything).
    pub(super) serial: bool,
    /// Capabilities this leaf needs (own + inherited); resolved into `precondition_skip`.
    pub(super) requires: Vec<String>,
    /// If set, this leaf is skipped before it ever runs (an unmet `requires`), with this reason.
    pub(super) precondition_skip: Option<String>,
    /// Effective tags: the unit's own plus every enclosing group's (selection matches on these).
    pub(super) tags: Vec<String>,
    /// Effective opt-in classes (own `switch` + every enclosing scope's). Off unless every one is
    /// thrown; empty means always eligible.
    pub(super) switches: Vec<String>,
    /// Obligation addresses this leaf discharges (`covers`).
    pub(super) covers: Vec<String>,
    /// Whether this leaf declares a `falsified_by` mutation — the selector for `prova falsify`.
    pub(super) falsifiable: bool,
    /// `Some(reason)` when this leaf carries its own `spec` flag (always a non-empty reason)
    /// — test-level only, never inherited. Drives the outcome inversion: red body →
    /// `Outcome::Promised`, green body → a failure demanding the flag's removal.
    pub(super) promises: Option<String>,
}

/// Group-level options that flow down to every contained leaf: `depends_on`, `resources`, `serial`,
/// `requires`.
#[derive(Clone, Default)]
pub(super) struct Inherited {
    pub(super) deps: Vec<NodeIx>,
    pub(super) resources: Vec<ResourceReq>,
    pub(super) serial: bool,
    pub(super) requires: Vec<String>,
    pub(super) tags: Vec<String>,
    /// Opt-in classes from enclosing scopes (`switch = "…"` on a group or `suite.config`). A leaf
    /// runs only when EVERY switch on its path is thrown — nested classes intersect.
    pub(super) switches: Vec<String>,
}

/// The executable plan: a flat list of leaves plus the leaf-level dependency DAG.
pub(super) struct Plan {
    pub(super) leaves: Vec<Leaf>,
}

/// Walk the tree, emitting a `Leaf` per test/flow and recording, for every node, which leaves live
/// under it (so a `depends_on`/`resources` on a group can expand to that group's leaves).
/// `inherited` carries ancestor groups' options down so a group-level declaration applies to each
/// contained leaf.
pub(super) fn collect_leaves(
    col: &Collector,
    ix: NodeIx,
    ancestors: &mut Vec<String>,
    inherited: &Inherited,
    leaves: &mut Vec<Leaf>,
    node_leaves: &mut HashMap<NodeIx, Vec<usize>>,
) -> Vec<usize> {
    let node = &col.nodes[ix];
    let my_leaves = match node.kind {
        NodeKind::Group => {
            let named = ix != 0 && !node.name.is_empty();
            if named {
                ancestors.push(format!("{}{}", node.name, node.params.suffix()));
            }
            let mut child_inherited = inherited.clone();
            child_inherited
                .deps
                .extend(node.opts.depends_on.iter().copied());
            child_inherited
                .resources
                .extend(node.opts.resources.iter().cloned());
            child_inherited.serial |= node.opts.serial;
            child_inherited
                .requires
                .extend(node.opts.requires.iter().cloned());
            child_inherited.tags.extend(node.opts.tags.iter().cloned());
            child_inherited.switches.extend(node.opts.switch.iter().cloned());
            let mut ids = Vec::new();
            for &child in &node.children {
                ids.extend(collect_leaves(
                    col,
                    child,
                    ancestors,
                    &child_inherited,
                    leaves,
                    node_leaves,
                ));
            }
            if named {
                ancestors.pop();
            }
            ids
        }
        NodeKind::Flow => {
            ancestors.push(format!("{}{}", node.name, node.params.suffix()));
            let steps = node
                .children
                .iter()
                .map(|&c| plan_item(&col.nodes[c], ancestors))
                .collect();
            ancestors.pop();
            let id = push_leaf(leaves, PlanUnit::Flow { steps }, node, inherited);
            vec![id]
        }
        NodeKind::Test => {
            let id = push_leaf(
                leaves,
                PlanUnit::Test(plan_item(node, ancestors)),
                node,
                inherited,
            );
            vec![id]
        }
    };
    node_leaves.insert(ix, my_leaves.clone());
    my_leaves
}

pub(super) fn push_leaf(leaves: &mut Vec<Leaf>, unit: PlanUnit, node: &Node, inherited: &Inherited) -> usize {
    let mut raw_deps = inherited.deps.clone();
    raw_deps.extend(node.opts.depends_on.iter().copied());
    let mut reqs = inherited.resources.clone();
    reqs.extend(node.opts.resources.iter().cloned());
    let mut requires = inherited.requires.clone();
    requires.extend(node.opts.requires.iter().cloned());
    let mut tags = inherited.tags.clone();
    tags.extend(node.opts.tags.iter().cloned());
    let mut switches = inherited.switches.clone();
    switches.extend(node.opts.switch.iter().cloned());
    let id = leaves.len();
    leaves.push(Leaf {
        unit,
        raw_deps,
        deps: Vec::new(),
        reqs,
        serial: inherited.serial || node.opts.serial,
        requires,
        precondition_skip: None,
        tags,
        switches,
        // Test-level only, by design: the leaf's own flag, never an ancestor's.
        falsifiable: node.falsifier.is_some(),
        covers: node.opts.covers.clone(),
        promises: node.opts.promises.clone(),
    });
    id
}

/// Narrow a plan to the selection, pulling in the dependencies of every selected leaf (an outcome
/// gate can't be evaluated against a node that never ran) and remapping leaf-id edges. Returns the
/// surviving plan and how many leaves were deselected. Flows are atomic: a flow is selected if ANY
/// of its step paths match.
pub(super) fn apply_selection(plan: Plan, sel: &Selection) -> (Plan, usize, Vec<(String, usize)>) {
    if sel.is_empty() {
        return (plan, 0, Vec::new());
    }
    let mut keep = vec![false; plan.leaves.len()];
    for (i, leaf) in plan.leaves.iter().enumerate() {
        if sel.selects(&leaf.unit.leaf_paths(), &leaf.tags) {
            keep[i] = true;
        }
    }
    narrow_plan(plan, keep)
}

/// Narrow a plan to the leaves carrying an effective spec flag (`--specs`): the burndown
/// selector. Graduated leaves are ordinary tests again and count as deselected, exactly like an
/// unmatched `-k` — the spec surface is precisely what is still open (or wrongly green).
/// `prova falsify` selects exactly the leaves that declare a mutation. A proof without one is not
/// a failure — most proofs will never declare one — it is simply not what this pass is about.
pub(super) fn apply_falsify_filter(plan: Plan, enabled: bool) -> (Plan, usize, Vec<(String, usize)>) {
    if !enabled {
        return (plan, 0, Vec::new());
    }
    let keep = plan.leaves.iter().map(|l| l.falsifiable).collect();
    narrow_plan(plan, keep)
}

/// Hold back the opt-in classes nobody asked for: a leaf carrying switches runs only when every
/// one is thrown (docs/design/manifest.md#switches-not-env-capabilities). Two escapes, by design:
/// a thrown switch (`-s`, or a profile's/`[run]`'s `switches`), and an exact `--node` naming the
/// leaf — deselecting a test the caller named precisely would be the swallowed-selector
/// dishonesty; fuzzy `-k`/`--tags` matches never imply a throw. Returns the per-class held-back
/// counts beside the usual narrowing facts, for the one-line summary.
/// Per-class held-back counts: switch class → how many leaves it kept out of this run.
pub(super) type SwitchedOff = std::collections::BTreeMap<String, usize>;

pub(super) fn apply_switch_filter(
    plan: Plan,
    thrown: &std::collections::BTreeSet<String>,
    sel: &Selection,
) -> (Plan, usize, Vec<(String, usize)>, SwitchedOff) {
    let mut off: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    let keep: Vec<bool> = plan
        .leaves
        .iter()
        .map(|l| {
            if l.switches.iter().all(|s| thrown.contains(s)) {
                return true;
            }
            if l.unit
                .leaf_paths()
                .iter()
                .any(|p| sel.nodes.iter().any(|n| n == p))
            {
                return true;
            }
            for s in l.switches.iter().filter(|s| !thrown.contains(*s)) {
                *off.entry(s.clone()).or_insert(0) += 1;
            }
            false
        })
        .collect();
    let (plan, deselected, dropped) = narrow_plan(plan, keep);
    (plan, deselected, dropped, off)
}

/// Narrow to one side of the promise⇄proof state duality: `promises_only` keeps the open promises,
/// `proofs_only` keeps the settled proofs (its mirror). At most one is set — the CLI rejects both —
/// and neither set means no filter.
pub(super) fn apply_specs_filter(
    plan: Plan,
    promises_only: bool,
    proofs_only: bool,
) -> (Plan, usize, Vec<(String, usize)>) {
    if !promises_only && !proofs_only {
        return (plan, 0, Vec::new());
    }
    let keep = plan
        .leaves
        .iter()
        .map(|l| if proofs_only { l.promises.is_none() } else { l.promises.is_some() })
        .collect();
    narrow_plan(plan, keep)
}

/// Keep exactly the marked leaves plus the dependency closure of every one of them (an outcome
/// gate can't be evaluated against a node that never ran), remapping leaf-id edges. Returns the
/// surviving plan and the reported path of every leaf that was dropped.
///
/// The dropped PATHS, not a count: a run that deselected everything and a run that deselected
/// nothing relevant report the same number, and the run record has to be able to tell a reader
/// which proofs produced no evidence. Naming them is the whole point (docs/plans/agent-reliability.md).
pub(super) fn narrow_plan(plan: Plan, mut keep: Vec<bool>) -> (Plan, usize, Vec<(String, usize)>) {
    // Dependency closure: selected leaves drag their upstream gates in, transitively.
    let mut work: Vec<usize> = keep
        .iter()
        .enumerate()
        .filter_map(|(i, &k)| k.then_some(i))
        .collect();
    while let Some(i) = work.pop() {
        for &d in &plan.leaves[i].deps {
            if !keep[d] {
                keep[d] = true;
                work.push(d);
            }
        }
    }
    // The COUNT is leaves, as it has always been — a flow is one schedulable unit however many
    // steps it has, and `deselected` is a scheduling fact. The PATHS are per leaf-path, because a
    // reader of the record needs the individual proofs, and a deselected leaf emits no event to
    // carry its file — this is the only chance to learn which file it came from.
    let deselected = keep.iter().filter(|&&k| !k).count();
    let dropped: Vec<(String, usize)> = plan
        .leaves
        .iter()
        .enumerate()
        .filter(|(i, _)| !keep[*i])
        .flat_map(|(_, leaf)| {
            leaf.unit
                .items()
                .into_iter()
                .map(|item| (item.path.clone(), item.file))
        })
        .collect();
    if deselected == 0 {
        return (plan, 0, dropped);
    }
    let mut remap = vec![usize::MAX; plan.leaves.len()];
    let mut kept = Vec::with_capacity(plan.leaves.len() - deselected);
    for (i, leaf) in plan.leaves.into_iter().enumerate() {
        if keep[i] {
            remap[i] = kept.len();
            kept.push(leaf);
        }
    }
    for leaf in &mut kept {
        leaf.deps = leaf.deps.iter().map(|&d| remap[d]).collect();
    }
    (Plan { leaves: kept }, deselected, dropped)
}

/// A leaf's reported path, prefixed with the stem of the file it was declared in.
///
/// The canonical address form for anything that has to name a leaf OUTSIDE a single run's console
/// output — the run record, `attest`. Two files may each declare a test called "it works", and an
/// address that cannot tell them apart lets one file's pass vouch for another file's skip.
///
/// Idempotent: the engine already prefixes the stem when a suite spans several files, so a path
/// that carries it is returned untouched.
pub fn qualify_leaf_path(path: &str, file: Option<&Path>) -> String {
    let Some(stem) = file.and_then(Path::file_stem).and_then(|s| s.to_str()) else {
        return path.to_string();
    };
    if path.split(" › ").next() == Some(stem) {
        path.to_string()
    } else {
        format!("{stem} › {path}")
    }
}

/// Qualify a batch of `(path, file index)` pairs against the collector's file table.
pub(super) fn qualify_all(dropped: Vec<(String, usize)>, file_paths: &[PathBuf]) -> Vec<String> {
    dropped
        .into_iter()
        .map(|(path, file)| qualify_leaf_path(&path, file_paths.get(file).map(PathBuf::as_path)))
        .collect()
}

/// Turn each leaf's node-level `raw_deps` (which may point at a group) into concrete leaf-id edges,
/// dropping any self-edge.
pub(super) fn expand_deps(leaves: &mut [Leaf], node_leaves: &HashMap<NodeIx, Vec<usize>>) {
    for (i, leaf) in leaves.iter_mut().enumerate() {
        let raw = std::mem::take(&mut leaf.raw_deps);
        let mut set = std::collections::BTreeSet::new();
        for dep_ix in raw {
            if let Some(dep_leaves) = node_leaves.get(&dep_ix) {
                for &dl in dep_leaves {
                    if dl != i {
                        set.insert(dl);
                    }
                }
            }
        }
        leaf.deps = set.into_iter().collect();
    }
}

/// Kahn-style reachability over the leaf DAG. Returns the names of leaves caught in a cycle, if any.
/// (Handle references are backward in definition order, so a cycle is practically unreachable from
/// valid Lua — this is a defensive collection-time guard the design mandates.)
pub(super) fn find_cycle(leaves: &[Leaf]) -> Option<Vec<String>> {
    let n = leaves.len();
    let mut resolved = vec![false; n];
    let mut remaining = n;
    while remaining > 0 {
        let mut progressed = false;
        for i in 0..n {
            if resolved[i] {
                continue;
            }
            if leaves[i].deps.iter().all(|&d| resolved[d]) {
                resolved[i] = true;
                remaining -= 1;
                progressed = true;
            }
        }
        if !progressed {
            return Some(
                (0..n)
                    .filter(|&i| !resolved[i])
                    .map(|i| unit_name(&leaves[i]).to_string())
                    .collect(),
            );
        }
    }
    None
}

/// A leaf's display name for messages — its first reported path (the flow/test name with ancestry).
pub(super) fn unit_name(leaf: &Leaf) -> &str {
    leaf.unit.leaf_paths().first().copied().unwrap_or("<unit>")
}

/// The reserved token that makes `serial` work: a serial leaf takes it exclusively while every
/// other leaf takes it shared, so RW semantics alone enforce "never concurrent with anything".
pub(super) const SERIAL_TOKEN: &str = "__prova_serial__";

pub(super) fn build_plan(col: &Collector, caps: &Capabilities) -> mlua::Result<Plan> {
    // Spec flags and proves attributes are test-level only (api-freeze §5, revised): either on a
    // group would need the whole inheritance/graduation ceremony back. Refuse with the fix.
    for node in &col.nodes {
        if !matches!(node.kind, NodeKind::Group) {
            continue;
        }
        let name = if node.name.is_empty() {
            "the suite".to_string()
        } else {
            format!("group {:?}", node.name)
        };
        if node.opts.promises.is_some() {
            return Err(mlua::Error::RuntimeError(format!(
                "promises is test-level only — flag each open test, not {name}"
            )));
        }
        if node.opts.proves.is_some() {
            return Err(mlua::Error::RuntimeError(format!(
                "proves is test-level only — annotate each test, not {name}"
            )));
        }
    }
    let mut leaves = Vec::new();
    let mut node_leaves = HashMap::new();
    collect_leaves(
        col,
        0,
        &mut Vec::new(),
        &Inherited::default(),
        &mut leaves,
        &mut node_leaves,
    );
    expand_deps(&mut leaves, &node_leaves);
    if let Some(cycle) = find_cycle(&leaves) {
        return Err(mlua::Error::RuntimeError(format!(
            "dependency cycle detected among units: {}",
            cycle.join(", ")
        )));
    }
    // Only pay the global-token cost when someone actually asked for serial execution. A serial
    // leaf holds it exclusively; everyone else reads it, so a serial leaf waits for all others to
    // drain and blocks new starts — exactly "process-wide exclusive".
    if leaves.iter().any(|l| l.serial) {
        for leaf in &mut leaves {
            leaf.reqs.push(ResourceReq {
                token: SERIAL_TOKEN.to_string(),
                shared: !leaf.serial,
            });
        }
    }
    // Resolve `requires`: a leaf with an unavailable capability is pre-skipped (not failed).
    // Detect each distinct capability once — some detectors shell out (e.g. `docker info`).
    resolve_requires(&mut leaves, caps);
    Ok(Plan { leaves })
}

/// Set `precondition_skip` on any leaf whose `requires` are not satisfied.
///
/// A capability is an expression, not just a name: `"docker"` or `"dotnet >= 9"`. The skip reason
/// distinguishes the three ways it can go unmet, because they call for different actions — install
/// the tool, upgrade it, or fix the typo:
///
/// - **absent**    → "requires \"docker\" (unavailable)"
/// - **too old**   → "requires \"dotnet >= 9\" (dotnet 8.0.421 does not satisfy >= 9)"
/// - **malformed** → an error, not a skip: a constraint that can never parse would skip forever
///   and read as green, which is the vacuous green this contract exists to remove.
pub(super) fn resolve_requires(leaves: &mut [Leaf], caps: &Capabilities) {
    let mut cache: HashMap<String, Option<String>> = HashMap::new();
    for leaf in leaves.iter_mut() {
        for cap in &leaf.requires {
            // `None` = satisfied; `Some(reason)` = not, and why. Memoized: version probes shell out.
            let unmet = cache
                .entry(cap.clone())
                .or_insert_with(|| caps.unmet_reason(cap))
                .clone();
            if let Some(reason) = unmet {
                // An unmet `requires` wins over the spec flag — nothing was observed, so there is
                // no outcome to invert. But "not applicable on this machine" and "not built
                // anywhere" are different facts with different remedies, and collapsing them into
                // one bare `skipped:` hides a standing backlog from anyone reading the run. The
                // skip stays a skip; it just stops pretending the spec isn't there.
                leaf.precondition_skip = Some(match &leaf.promises {
                    Some(spec) => format!("skipped: {reason} — still promised: {spec}"),
                    None => format!("skipped: {reason}"),
                });
                break;
            }
        }
    }
}
