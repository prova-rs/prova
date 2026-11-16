//! What prova's exit code says (E m/14201): whose fault a non-green run is, so a caller like Herald can tell "this
//! change is broken" from "this host lacks something". Every exit that was a bare 2 now names one of these.
//!
//! - `0`: green.
//! - `1`: red, a proof failed. The change's.
//! - [`EXIT_UNMET`] (2): a guarantee this host can't keep: a `must_run` capability it lacks, a tool it should provide. The
//!   host's; a caller paces it rather than blaming the change.
//! - [`EXIT_CONFIG`] (78, sysexits `EX_CONFIG`): prova's own usage or configuration error: no `prova.toml` where one is
//!   needed, a manifest that doesn't read, an unknown profile, suite or module, a bad argument. The change's (or its
//!   caller's); nothing on the host fixes it.

use std::process::ExitCode;

/// A guarantee this host can't keep.
pub(crate) const EXIT_UNMET: u8 = 2;
/// prova's own usage or configuration error (sysexits `EX_CONFIG`).
pub(crate) const EXIT_CONFIG: u8 = 78;
/// A failure neither the change's configuration nor a guarantee the host can't keep, as far as its site shows: an
/// I/O or runtime failure, or one path reached from both. It exits 2 as before, until each is ruled (E m/14255).
pub(crate) const EXIT_UNRULED: u8 = 2;

/// [`EXIT_CONFIG`] as an exit code.
pub(crate) fn config() -> ExitCode {
    ExitCode::from(EXIT_CONFIG)
}

/// [`EXIT_UNMET`] as an exit code.
pub(crate) fn unmet() -> ExitCode {
    ExitCode::from(EXIT_UNMET)
}
