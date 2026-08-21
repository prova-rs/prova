//! Run-wide topologies: one instance per RUN, shared by every file that declares it
//! (docs/design/topologies.md#run-wide-topology-is-provisioned-once).
//!
//! A `prova.topology(...)` in a proof file is a FIXTURE — local to the files that declare it — so a
//! package whose proofs span several files provisions the same environment once per file in a cold
//! run. Held topologies already dedupe (every file attaches to the one live instance), which made
//! the field workaround `prova start <name> && prova`: hold it yourself, and the suite is fast.
//! That inverts the promise of the cold path — CI and a fresh checkout pay N× for the suite the
//! author runs warm — so the capability belongs here, in the engine.
//!
//! **Why a holder thread.** The instance must cross Lua states: suites are the unit of one state
//! (`crate::suite`), and a `Lua` — with the teardown closures a factory parks on its scope — is
//! pinned to its thread and dies with its suite. So the provisioning cannot happen in the asking
//! worker: whoever provisions must outlive every suite, which means its own thread, its own state,
//! and its own runtime. That is exactly what a HELD topology already is (`hold_topology`), so this
//! pool holds one per run-wide name and reaps them when the run ends — the holder is the one true
//! reaper here as everywhere else.
//!
//! **Why the definition comes from `[topologies]`.** The holder rebuilds the factory in a fresh
//! state, and the only definition a fresh state can rebuild is the registration
//! (`require("<package>").<factory>`) — a closure in a proof file cannot travel. So interning is
//! keyed by the REGISTERED name, and a file's declaration of that name is a demand trigger rather
//! than the definition, exactly as it is when a run attaches to a detached holder
//! (docs/design/topologies.md#attach-binds-by-name).
//!
//! **What crosses is data.** The value each declaring file sees is the JSON projection of the
//! factory's returned value, rehydrated per state — the same projection attach uses, and for the
//! same reason: closures and userdata cannot cross, and the resource grammar's standing answer is
//! that clients attach by `url`.

use super::*;

use std::collections::{BTreeSet, HashMap as StdHashMap};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

/// One run-wide topology's slot, the same three states a `Scope.Run` conduct occupies
/// (`ConductSlot`): claimed before the provision runs, settled to its outcome after. `Provisioning`
/// is what a second worker waits on; both settled states memoize for the rest of the run.
enum Slot {
    Provisioning,
    Ready(serde_json::Value),
    Poisoned(String),
}

/// A request to the holder thread: provision this registered topology and settle its slot.
enum Order {
    Provision(String),
    Shutdown,
}

/// The handle every worker holds (cloned into each suite's `RunState` via the `RunConfig`): the set
/// of run-wide names, the settled slots, and the line to the holder thread.
#[derive(Clone)]
pub(crate) struct InternedTopologies(Arc<Shared>);

struct Shared {
    /// The names declared run-wide (`[topologies] <name> = { …, scope = "run" }`). A topology
    /// outside this set is untouched — file-local, exactly as before.
    names: BTreeSet<String>,
    slots: Mutex<StdHashMap<String, Slot>>,
    /// `Sender` is `Send` but not `Sync`, and this handle is shared across worker threads.
    orders: Mutex<Sender<Order>>,
}

/// The run-wide topology pool: **owns** the holder thread, so dropping it reaps every instance the
/// run provisioned. Created by the host (the CLI) before the run and shut down after it; the
/// `handle()` goes into the `RunConfig` the suites share.
pub struct TopologyPool {
    shared: Arc<Shared>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl TopologyPool {
    /// Start a pool for `names`, provisioning from `config`'s `[topologies]` registrations. The
    /// thread is started here but provisions NOTHING: a run-wide topology is still demand-driven —
    /// a selection that reaches no test using it pays nothing, which is the property that lets an
    /// expensive environment be declared run-wide without taxing every `-k` run.
    pub fn start(names: impl IntoIterator<Item = String>, config: &RunConfig) -> TopologyPool {
        let (tx, rx) = channel::<Order>();
        let shared = Arc::new(Shared {
            names: names.into_iter().collect(),
            slots: Mutex::new(StdHashMap::new()),
            orders: Mutex::new(tx),
        });
        let holder_shared = Arc::clone(&shared);
        let config = config.clone();
        let thread = match std::thread::Builder::new()
            .name("prova-topologies".to_string())
            .spawn(move || hold(&holder_shared, &config, rx))
        {
            Ok(handle) => Some(handle),
            // Said here rather than only at first demand: a run whose topologies cannot be held
            // is about to behave differently from the one that was asked for, and the reason
            // (the OS refused a thread) is visible only at this point.
            Err(e) => {
                eprintln!("prova: cannot hold run-wide topologies: {e}");
                None
            }
        };
        TopologyPool { shared, thread }
    }

    /// The worker-side handle, for `RunConfig::with_interned_topologies`.
    pub fn handle(&self) -> InternedHandle {
        InternedHandle(InternedTopologies(Arc::clone(&self.shared)))
    }

    /// Reap every instance this run provisioned, and wait for the teardowns to finish. Idempotent:
    /// `Drop` calls it too, so an early return or a panic mid-run still reaps.
    pub fn shutdown(&mut self) {
        let Some(thread) = self.thread.take() else {
            return;
        };
        // A closed channel would also end the loop, but the handle in the run's config keeps a
        // sender alive — so say it explicitly rather than depending on who still holds a clone.
        let _ = self
            .shared
            .orders
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .send(Order::Shutdown);
        let _ = thread.join();
    }

    /// The names this run actually stood up, so the caller narrates a teardown that is really
    /// about to happen — and says nothing when demand never reached the pool.
    pub fn provisioned(&self) -> Vec<String> {
        let slots = self
            .shared
            .slots
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        slots
            .iter()
            .filter(|(_, s)| matches!(s, Slot::Ready(_)))
            .map(|(n, _)| n.clone())
            .collect()
    }
}

impl Drop for TopologyPool {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// An opaque, `Send` wrapper so the host can install the handle without seeing the crate internals.
pub struct InternedHandle(pub(crate) InternedTopologies);

/// The holder thread: provision on order, hold everything until the run ends, then reap.
///
/// Each instance is a `HeldTopology` — the same object `prova up` and the MCP warm path hold — so
/// there is one provisioning path and one teardown path, not a third of each.
fn hold(shared: &Arc<Shared>, config: &RunConfig, orders: Receiver<Order>) {
    let mut held: Vec<HeldTopology> = Vec::new();
    while let Ok(order) = orders.recv() {
        let name = match order {
            Order::Shutdown => break,
            Order::Provision(name) => name,
        };
        // Settle the slot whatever happens — including an unwind inside the factory. A slot left
        // `Provisioning` would spin every waiting worker for the rest of the run.
        let mut settle = SettleOnDrop {
            shared: Arc::clone(shared),
            name: name.clone(),
            done: false,
        };
        let activity = crate::progress::start(
            config.progress(),
            crate::progress::Kind::Build,
            format!("topology {name:?} — one instance for this whole run"),
        );
        let outcome = hold_topology(&[], &name, config);
        activity.done();
        match outcome {
            Ok(instance) => {
                let snapshot = instance.snapshot();
                held.push(instance);
                settle.settle(Slot::Ready(snapshot));
            }
            Err(e) => settle.settle(Slot::Poisoned(e.to_string())),
        }
    }
    // The run is over: reap in reverse order of provisioning, so a topology built on another's
    // resources goes first. Failures print to stderr (`teardown` warns) — a swallowed one is a
    // container still running after prova said it was done.
    for instance in held.into_iter().rev() {
        instance.teardown();
    }
}

/// Poisons its slot if dropped unsettled — the holder-side twin of the conduct store's `Settle`.
struct SettleOnDrop {
    shared: Arc<Shared>,
    name: String,
    done: bool,
}

impl SettleOnDrop {
    fn settle(&mut self, slot: Slot) {
        self.shared
            .slots
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(self.name.clone(), slot);
        self.done = true;
    }
}

impl Drop for SettleOnDrop {
    fn drop(&mut self) {
        if !self.done {
            self.settle(Slot::Poisoned(
                "the run-wide provisioning was abandoned before settling (cancelled or panicked)"
                    .into(),
            ));
        }
    }
}

impl InternedTopologies {
    /// Is this topology name run-wide in this run?
    pub(crate) fn covers(&self, name: &str) -> bool {
        self.0.names.contains(name)
    }

    /// Resolve `name` to this state's own copy of the run-wide instance: claim the slot and order
    /// the provision, or wait for whoever claimed it first, then rehydrate the settled projection.
    ///
    /// Waiting is async (never a thread block): a same-state waiter must not wedge the thread
    /// driving the tests it shares a suite with. The wait is narrated for the same reason a queued
    /// conduct is (docs/design/agent-ergonomics.md#narrate-lock-waits) — it lands inside the
    /// waiting test's own duration, so without a word it reads as a slow test.
    pub(crate) async fn resolve(
        &self,
        lua: &Lua,
        name: &str,
        progress: &Arc<dyn crate::progress::Progress>,
    ) -> mlua::Result<Value> {
        let mut waiting: Option<crate::progress::Activity> = None;
        loop {
            {
                let mut slots = self
                    .0
                    .slots
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                match slots.get(name) {
                    None => {
                        slots.insert(name.to_string(), Slot::Provisioning);
                        drop(slots);
                        // The order can only fail if the holder thread is gone, which is a defect
                        // in this process rather than a fixture failure — settle the slot so every
                        // other worker gets the same verdict instead of spinning.
                        let sent = self
                            .0
                            .orders
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .send(Order::Provision(name.to_string()));
                        if sent.is_err() {
                            let msg = format!(
                                "topology {name:?} is run-wide, but its holder thread is gone — \
                                 nothing can provision it in this run"
                            );
                            self.0
                                .slots
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .insert(name.to_string(), Slot::Poisoned(msg.clone()));
                            return Err(mlua::Error::RuntimeError(msg));
                        }
                    }
                    Some(Slot::Ready(v)) => {
                        let v = v.clone();
                        drop(slots);
                        return json_to_lua(lua, &v);
                    }
                    Some(Slot::Poisoned(err)) => {
                        return Err(mlua::Error::RuntimeError(format!(
                            "run-wide topology {name:?} already failed in this run — memoized, \
                             not re-provisioned: {err}"
                        )));
                    }
                    Some(Slot::Provisioning) => {}
                }
            }
            if waiting.is_none() {
                waiting = Some(crate::progress::start(
                    progress,
                    crate::progress::Kind::Waiting,
                    format!("topology {name:?} — provisioned once for this run, in progress"),
                ));
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A handle with no holder thread behind it, plus the receiver the caller must keep alive: a
    /// DROPPED receiver makes every order fail, which is its own case (proved separately below).
    fn shared(names: &[&str]) -> (InternedTopologies, Receiver<Order>) {
        let (tx, rx) = channel::<Order>();
        let handle = InternedTopologies(Arc::new(Shared {
            names: names.iter().map(|s| s.to_string()).collect(),
            slots: Mutex::new(StdHashMap::new()),
            orders: Mutex::new(tx),
        }));
        (handle, rx)
    }

    /// Only the declared names are run-wide: everything else keeps file-local semantics, which is
    /// what makes this opt-in rather than a change to how every topology behaves.
    #[test]
    fn only_declared_names_are_covered() {
        let (h, _rx) = shared(&["kind"]);
        assert!(h.covers("kind"));
        assert!(!h.covers("docker"));
    }

    /// A settled slot memoizes: a second reader rehydrates the projection instead of ordering a
    /// second provision — the whole point of interning.
    #[test]
    fn a_ready_slot_is_rehydrated_not_reprovisioned() {
        let (h, _rx) = shared(&["kind"]);
        h.0.slots.lock().unwrap().insert(
            "kind".to_string(),
            Slot::Ready(serde_json::json!({ "api": { "url": "https://127.0.0.1:6443" } })),
        );
        let lua = Lua::new();
        let progress: Arc<dyn crate::progress::Progress> =
            Arc::new(crate::progress::NullProgress);
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let v = rt
            .block_on(h.resolve(&lua, "kind", &progress))
            .expect("a ready slot resolves");
        let Value::Table(t) = v else { panic!("expected a table") };
        let api: Table = t.get("api").unwrap();
        assert_eq!(api.get::<String>("url").unwrap(), "https://127.0.0.1:6443");
    }

    /// A failure memoizes exactly as success does (docs/design/lifecycle.md#fixture-failure-memoization):
    /// the second consumer replays the recorded error, named as a replay, instead of re-paying a
    /// provision that just failed.
    #[test]
    fn a_poisoned_slot_replays_the_error() {
        let (h, _rx) = shared(&["kind"]);
        h.0.slots
            .lock()
            .unwrap()
            .insert("kind".to_string(), Slot::Poisoned("no cluster".into()));
        let lua = Lua::new();
        let progress: Arc<dyn crate::progress::Progress> =
            Arc::new(crate::progress::NullProgress);
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let err = rt
            .block_on(h.resolve(&lua, "kind", &progress))
            .expect_err("a poisoned slot replays");
        let text = err.to_string();
        assert!(text.contains("memoized"), "{text}");
        assert!(text.contains("no cluster"), "{text}");
    }

    /// No holder thread, no run-wide topology: the claimer says so rather than spinning forever on
    /// a slot nothing will ever settle.
    #[test]
    fn a_dead_holder_is_reported_not_awaited() {
        let (tx, rx) = channel::<Order>();
        drop(rx);
        let h = InternedTopologies(Arc::new(Shared {
            names: ["kind".to_string()].into_iter().collect(),
            slots: Mutex::new(StdHashMap::new()),
            orders: Mutex::new(tx),
        }));
        let lua = Lua::new();
        let progress: Arc<dyn crate::progress::Progress> =
            Arc::new(crate::progress::NullProgress);
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let err = rt
            .block_on(h.resolve(&lua, "kind", &progress))
            .expect_err("a dead holder cannot provision");
        assert!(err.to_string().contains("holder thread is gone"), "{err}");
    }
}
