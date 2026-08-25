//! The stop-signal flag a *supervising* verb waits on.
//!
//! Most of prova wants a signal's default disposition: an interrupted run dies, and the conduct
//! lease (`prova_core::lease`) sweeps what it spawned from OUTSIDE the dying process, which is
//! stronger than any handler could be. `prova start` is the one verb that cannot work that way.
//! Its child is deliberately in its own process group — that is what "detached" means, and it is
//! why the terminal's Ctrl-C never reaches the holder — and it deliberately holds no lease
//! (verifiers.md#detached-topologies-hold-no-lease), because outliving the invocation is the
//! verb's whole purpose. Both facts are correct, and together they mean nothing at all reaps a
//! holder whose supervisor dies mid-startup: `prova start` vanished, the `prova up` it spawned
//! kept provisioning, and the containers arrived with no one left to report them. Before the
//! topology registers there is not even a run-state record, so `prova down` answers "not running"
//! while docker fills up.
//!
//! So the supervisor — and only the supervisor — catches the signal and stops the holder the way
//! `prova down` would. The handler does exactly one async-signal-safe thing: bump a counter.
//! Everything else happens on the waiting thread, where it can print, wait, and escalate.
//!
//! Unix only. The graceful stop this exists to trigger is `runstate::terminate` (SIGTERM →ceded to
//! the holder's in-process teardown), which is itself unix-only — on Windows there is no signal to
//! hand a detached process that runs its teardown, so a handler there could only kill the holder
//! and strand its containers, which is not an improvement over today's orphan. Windows detached
//! teardown is one story, told once, when job objects land for the windows lane.

use std::sync::atomic::{AtomicUsize, Ordering};

/// How many stop signals have arrived since [`arm`]. A counter rather than a flag so a caller can
/// tell "the user asked me to stop" from "the user asked me AGAIN, stop waiting politely".
static COUNT: AtomicUsize = AtomicUsize::new(0);

/// Stop signals seen so far.
pub(crate) fn count() -> usize {
    COUNT.load(Ordering::SeqCst)
}

/// Has the user (or a supervisor) asked us to stop?
pub(crate) fn raised() -> bool {
    count() > 0
}

/// The handler. Async-signal-safe by construction: one atomic store, no allocation, no I/O, no
/// lock. Everything a human should see about the interrupt is printed by the thread that observes
/// the count, never from here.
#[cfg(unix)]
extern "C" fn on_signal(_sig: libc::c_int) {
    COUNT.fetch_add(1, Ordering::SeqCst);
}

/// Catch SIGINT (Ctrl-C), SIGTERM (a supervisor or CI cancelling us) and SIGHUP (the terminal went
/// away) for the rest of this process's life.
///
/// All three mean the same thing to a supervisor that has not finished starting: nobody is left to
/// hand the endpoints to, so a half-built environment must not be left standing. SIGHUP is included
/// deliberately — closing the terminal on a `prova start` that is still provisioning is the case
/// most likely to leave containers nobody remembers creating.
#[cfg(unix)]
pub(crate) fn arm() {
    COUNT.store(0, Ordering::SeqCst);
    for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
        // SAFETY: installing a handler that only bumps an atomic. `libc::signal` is the whole
        // interface here — no mask, no flags, nothing that outlives the process. The double cast
        // is what `sighandler_t` (an integer) requires of a function item, spelled the way the
        // `function_casts_as_integer` lint asks for.
        unsafe {
            libc::signal(sig, on_signal as *const () as libc::sighandler_t);
        }
    }
}

/// No-op off unix: see the module docs — there is nothing a handler could do here that beats the
/// default, because the graceful stop it would trigger does not exist on Windows.
#[cfg(not(unix))]
pub(crate) fn arm() {}

#[cfg(test)]
mod tests {
    use super::*;

    /// Arming is the reset point, and an unsignalled process reads as clear. The signalled path is
    /// proven black-box (`proofs/topologies/interrupt_test.lua`) — raising a real SIGINT inside the
    /// unit-test process would take the test binary with it on any thread that has not armed.
    #[test]
    fn arming_clears_the_count() {
        arm();
        assert_eq!(count(), 0);
        assert!(!raised());
    }
}
