//! Which Docker daemon prova talks to: the one the `docker` CLI talks to.

use bollard::Docker;

/// The client for THE daemon the `docker` CLI talks to — never a different one.
///
/// prova speaks to Docker two ways: bollard (containers, images, most networks) and the `docker`
/// CLI (a topology's synchronous `ctx.network`, the `Drop` backstops). Unset `DOCKER_HOST`, the CLI
/// follows its CURRENT CONTEXT while bollard's local defaults dial `/var/run/docker.sock` — and on a
/// machine with more than one daemon those are different daemons. Witnessed 2026-10-04 on a Mac
/// with Docker Desktop (the current context) beside OrbStack (owner of `/var/run/docker.sock`): the
/// CLI created `prova-net-<pid>-0` on one, bollard started the container on the other, and every
/// topology test failed `network prova-net-… not found`. So bollard follows the CLI: a set
/// `DOCKER_HOST` already binds both; otherwise the current context's endpoint is read once
/// ([`context_endpoint`]) and dialled directly.
pub(super) fn local_client() -> Result<Docker, bollard::errors::Error> {
    match context_endpoint() {
        Some(host) => Docker::connect_with_local(host, DOCKER_TIMEOUT_SECS, bollard::API_DEFAULT_VERSION),
        None => Docker::connect_with_local_defaults(),
    }
}

/// bollard's own default request timeout.
const DOCKER_TIMEOUT_SECS: u64 = 120;

/// The current docker context's endpoint, when `DOCKER_HOST` is unset and the context names a local
/// socket or pipe; asked once per process (`docker context inspect` honours `DOCKER_CONTEXT` and the
/// configured current context exactly as every other CLI call does). `None` keeps bollard's defaults:
/// no CLI, no context, or a remote endpoint (tcp/ssh) that the defaults already handle via `DOCKER_HOST`.
fn context_endpoint() -> Option<&'static str> {
    static ENDPOINT: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    ENDPOINT
        .get_or_init(|| {
            if std::env::var_os("DOCKER_HOST").is_some() {
                return None;
            }
            let out = std::process::Command::new("docker")
                .args(["context", "inspect", "--format", "{{.Endpoints.docker.Host}}"])
                .output()
                .ok()?;
            if !out.status.success() {
                return None;
            }
            endpoint_from_context(&String::from_utf8_lossy(&out.stdout))
        })
        .as_deref()
}

/// Pure: the endpoint a `docker context inspect` answer names, when it is one prova dials locally —
/// a unix socket, or a Windows named pipe.
fn endpoint_from_context(raw: &str) -> Option<String> {
    let host = raw.trim();
    (host.starts_with("unix://") || host.starts_with("npipe://")).then(|| host.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// bollard follows the CLI's current context only to a LOCAL endpoint it can dial; anything
    /// else (tcp, ssh, empty, junk) keeps bollard's defaults.
    #[test]
    fn a_context_endpoint_is_dialled_only_when_local() {
        assert_eq!(
            endpoint_from_context("unix:///Users/me/.docker/run/docker.sock\n").as_deref(),
            Some("unix:///Users/me/.docker/run/docker.sock")
        );
        assert_eq!(
            endpoint_from_context("npipe:////./pipe/dockerDesktopLinuxEngine").as_deref(),
            Some("npipe:////./pipe/dockerDesktopLinuxEngine")
        );
        assert_eq!(endpoint_from_context("tcp://10.0.0.5:2376"), None);
        assert_eq!(endpoint_from_context("ssh://me@box"), None);
        assert_eq!(endpoint_from_context(""), None);
    }
}
