//! What prova's exit code says (E m/14201, m/14323): whose fault a non-green run is, so a caller like Herald can tell
//! "this change is broken" from "this host lacks something". Every exit that was a bare 2 now names one of these.
//!
//! - `0`: green.
//! - [`EXIT_RED`] (1): red, a proof failed, a proof file that can't load included. The change's; bisectable.
//! - [`EXIT_HOST`] (2): the host or the runtime, not the change: a `must_run` capability this host lacks, an I/O or
//!   runtime failure (a report that can't be written, a child that won't start, a fetch that failed, home dirs that
//!   can't be found). A caller paces it rather than blaming the change.
//! - [`EXIT_INTERNAL`] (70, sysexits `EX_SOFTWARE`): prova's own bug. Paced like 2, and reported to prova's owner,
//!   never blamed on the change.
//! - [`EXIT_CONFIG`] (78, sysexits `EX_CONFIG`): prova's own usage or configuration error: no `prova.toml` where one
//!   is needed, a manifest that doesn't read, an unknown profile, suite or module, a bad argument. The change's (or its
//!   caller's); nothing on the host fixes it.

use std::process::ExitCode;

/// A proof failed, or a proof file couldn't load.
pub(crate) const EXIT_RED: u8 = 1;
/// The host or the runtime, not the change.
pub(crate) const EXIT_HOST: u8 = 2;
/// prova's own bug (sysexits `EX_SOFTWARE`).
pub(crate) const EXIT_INTERNAL: u8 = 70;
/// prova's own usage or configuration error (sysexits `EX_CONFIG`).
pub(crate) const EXIT_CONFIG: u8 = 78;

/// [`EXIT_CONFIG`] as an exit code.
pub(crate) fn config() -> ExitCode {
    ExitCode::from(EXIT_CONFIG)
}

/// [`EXIT_HOST`] as an exit code.
pub(crate) fn host() -> ExitCode {
    ExitCode::from(EXIT_HOST)
}

/// [`EXIT_RED`] as an exit code.
pub(crate) fn red() -> ExitCode {
    ExitCode::from(EXIT_RED)
}

/// An uncaught panic in `main` is prova's own bug (E m/14379, m/14392): Rust's default hook prints it as ever, and
/// `main`'s body runs under `catch_unwind`, so one that unwinds out of it exits [`EXIT_INTERNAL`] (70) rather than
/// Rust's 101, which no caller reads. A panic someone joins or catches on purpose (a topology holder's thread in MCP,
/// archetect's render thread, a Rust callback mlua turns into a Lua error) stays theirs: nothing here preempts it.
pub(crate) fn panics_in_main_are_internal(body: fn() -> ExitCode) -> ExitCode {
    match std::panic::catch_unwind(body) {
        Ok(code) => code,
        Err(_) => {
            eprintln!("prova: internal error (a panic, prova's own bug): exit {EXIT_INTERNAL}; report it to prova's owner");
            ExitCode::from(EXIT_INTERNAL)
        }
    }
}

/// A debug build's planted panics, so a proof plants them through the real binary: `PROVA_TEST_PANIC=main` panics in
/// `main`'s body; `=joined` panics on a thread `main` joins and says so, as a caught panic's site does.
pub(crate) fn planted_panic() {
    if !cfg!(debug_assertions) {
        return;
    }
    match std::env::var("PROVA_TEST_PANIC").as_deref() {
        Ok("main") => panic!("a planted panic in main (PROVA_TEST_PANIC)"),
        Ok("joined") => {
            let joined = std::thread::spawn(|| panic!("a planted panic on a joined thread (PROVA_TEST_PANIC)")).join();
            if joined.is_err() {
                eprintln!("prova: the planted thread panicked; its site says so and goes on");
            }
        }
        _ => {}
    }
}
