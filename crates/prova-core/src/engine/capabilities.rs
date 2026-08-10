//! Capabilities: the registered + built-in vocabulary (`requires = {...}`), probed
//! once at load, with version comparison for gated skips.

use super::*;

/// Capabilities the project registered in its `prova.lua` companion — name → its reported version
/// (`None` = available but versionless).
///
/// **Per run, not global.** This lives in [`RunConfig`], so two projects resolved in one process —
/// the warm MCP resolving one at startup, then `run { project }` — cannot see each other's
/// vocabulary. It was a process-global static once, and that leaked across projects
/// (`tests/capability_isolation.rs`).
///
/// **Answers, not closures, evaluated once at load.** Three reasons that are one: `must_run` is a
/// precondition checked before any suite exists (nothing to call back into); each suite gets its own
/// `Lua` and mlua handles are `!Send` (a stored closure could not cross states); and a capability
/// that answered differently for two suites in one run would be a coin flip, not a capability.
#[derive(Clone, Default, Debug)]
pub struct Capabilities(std::collections::BTreeMap<String, Option<semver::Version>>);

impl Capabilities {
    /// Record a registered capability. `version` is `None` for a bare `true` predicate (available,
    /// no version to compare).
    pub fn register(&mut self, name: &str, version: Option<semver::Version>) {
        self.0.insert(name.to_string(), version);
    }

    /// The names a project registered via `runtime.capability` — the `prova capabilities` report
    /// lists them beside what the manifest references, probed with the project's own predicates.
    pub fn registered_names(&self) -> impl Iterator<Item = &String> {
        self.0.keys()
    }

    /// Available = registered by the project, OR a built-in the host provides. Registered wins, but
    /// registering over a built-in is refused at load, so this cannot shadow `docker`.
    pub fn available(&self, name: &str) -> bool {
        self.0.contains_key(name) || builtin_available(name)
    }

    /// The version a constraint compares against: the project's reported version if registered, else
    /// a probed built-in version. `None` = no version to compare, which makes a constraint
    /// unsatisfiable — "cannot confirm" is not "satisfied".
    pub fn version(&self, name: &str) -> Option<semver::Version> {
        if let Some(v) = self.0.get(name) {
            return v.clone();
        }
        builtin_version(name)
    }

    /// Is this capability expression satisfied here, and if not, why?
    ///
    /// - `Ok(None)`         — satisfied.
    /// - `Ok(Some(reason))` — unmet: absent, or the wrong version (phrased for a human).
    /// - `Err(e)`           — the expression is malformed (a config error, not an environment one).
    ///
    /// The one function both halves of the contract call — `requires` (skip on unmet) and `must_run`
    /// (fail on unmet) — so they can never disagree about what a string means. Name before version,
    /// so an absent tool never reaches a probe and `windows >= 10` short-circuits on unix.
    pub fn expr_status(&self, expr: &str) -> Result<Option<String>, String> {
        let parsed = CapabilityExpr::parse(expr)?;
        if !self.available(parsed.name) {
            return Ok(Some(format!("{:?} is unavailable", parsed.name)));
        }
        let Some(req) = parsed.req else {
            return Ok(None);
        };
        match self.version(parsed.name) {
            Some(v) if req.matches(&v) => Ok(None),
            Some(v) => Ok(Some(format!("{} {v} does not satisfy {req}", parsed.name))),
            None => Ok(Some(format!(
                "{}'s version could not be determined, so {req} cannot be confirmed",
                parsed.name
            ))),
        }
    }

    /// Why `expr` is unmet, or `None` if satisfied — the skip-side phrasing over `expr_status`. A
    /// malformed expression is reported as the reason rather than folded into "absent": the author
    /// needs to see the typo, not hunt for a tool that was never named.
    pub(super) fn unmet_reason(&self, expr: &str) -> Option<String> {
        match self.expr_status(expr) {
            Ok(None) => None,
            Ok(Some(reason)) => Some(format!("requires {expr:?} ({reason})")),
            Err(e) => Some(e),
        }
    }
}

/// Is `name` a capability this build defines itself? Registering over one is refused: `docker` means
/// something specific (a daemon that answers AND runs linux containers), and letting a project
/// redefine it would make `requires = { "docker" }` mean different things in different repos —
/// silently, which is the worst kind.
pub fn is_builtin_capability(name: &str) -> bool {
    matches!(
        name,
        "docker" | "github" | "network" | "internet" | "unix" | "windows"
    ) || native_capability_compiled(name).is_some()
}

/// The built-in capability vocabulary prova probes by name — the enumerable core behind
/// `prova capabilities`. Beyond these, any executable on `PATH` is a capability, and a project
/// registers more via `runtime.capability`. Every entry satisfies `is_builtin_capability`
/// (guarded by a unit test) — this list is the single place the host report enumerates from.
pub fn builtin_capability_names() -> &'static [&'static str] {
    &[
        "docker", "github", "network", "internet", "unix", "windows", // named host probes
        "http", "sqlite", "grpc", "graphql", "yaml", // compiled-in native clients
    ]
}

/// A capability expression: a name, optionally with a semver constraint — `"docker"`,
/// `"dotnet >= 9"`, `"node ^20"`, `"git >= 1.0, < 3.0"`.
///
/// It is a **string**, and that is load-bearing rather than lazy. `must_run` lives in `prova.toml`,
/// which is TOML and holds no functions, so a predicate expressible only in Lua would split the
/// contract into two vocabularies — one for what a test needs, another for what a context
/// guarantees. One string parses for both.
pub struct CapabilityExpr<'a> {
    pub name: &'a str,
    pub req: Option<semver::VersionReq>,
}

impl<'a> CapabilityExpr<'a> {
    /// Parse `"<name>"` or `"<name> <constraint>"`. An unparseable constraint is an **error**, never
    /// a quiet "unavailable": a typo'd constraint that silently never matched would skip forever and
    /// read as green — the vacuous green this whole contract exists to remove.
    pub fn parse(expr: &'a str) -> Result<Self, String> {
        let expr = expr.trim();
        // The name runs until whitespace or the first constraint character, so `git>=1.0` and
        // `git >= 1.0` are the same expression — whitespace is not meaning.
        let split = expr
            .find(|c: char| c.is_whitespace() || "<>=^~".contains(c))
            .unwrap_or(expr.len());
        let (name, rest) = expr.split_at(split);
        let name = name.trim();
        let rest = rest.trim();
        if name.is_empty() {
            return Err(format!("invalid capability expression {expr:?}: no name"));
        }
        if rest.is_empty() {
            return Ok(Self { name, req: None });
        }
        match semver::VersionReq::parse(rest) {
            Ok(req) => Ok(Self {
                name,
                req: Some(req),
            }),
            Err(e) => Err(format!(
                "invalid capability expression {expr:?}: {e} \
                 (expected a semver constraint like \">= 9\", \"^20\", or \">= 1.0, < 3.0\")"
            )),
        }
    }
}

/// What version of `name` is installed, if the question is meaningful and answerable.
///
/// `None` means "no version to compare" — either the capability has no version concept, or its probe
/// could not answer. A constraint against `None` is **unsatisfiable**, because the honest response to
/// "is this ≥ 9?" when the version is unknowable is "cannot confirm", and a gate that cannot confirm
/// must not wave the suite through.
pub(super) fn builtin_version(name: &str) -> Option<semver::Version> {
    let raw = match name {
        // Docker's SERVER version — the daemon is the thing a suite depends on, and it can differ
        // from the CLI talking to it. `docker --version` would report the client and quietly answer
        // a different question.
        "docker" => run_capture("docker", &["version", "--format", "{{.Server.Version}}"])?,
        // Platform predicates are booleans, not versions: `cfg!(unix)` has no number. A future
        // `windows >= 10` wants the OS build, which is a separate probe per platform; until that
        // exists, say so honestly (None ⇒ a constraint cannot be satisfied) rather than invent one.
        "unix" | "windows" => return None,
        // The general case: ask the tool. Every candidate answers `--version` on stdout —
        //   git    → "git version 2.54.0"
        //   dotnet → "8.0.421"
        //   sh     → "GNU bash, version 5.3.9(1)-release (…)"
        // so take the first version-shaped token rather than trying to know each tool's format.
        other => run_capture(other, &["--version"])?,
    };
    parse_first_version(&raw)
}

/// The first `N.N[.N]` in `text`, padded to three components.
///
/// Tools are inconsistent (`2.54`, `8.0.421`, `5.3.9(1)-release`) and an author should not have to
/// care, so normalize rather than demand strict semver from arbitrary CLIs.
pub(super) fn parse_first_version(text: &str) -> Option<semver::Version> {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
            i += 1;
        }
        let tok = text[start..i].trim_end_matches('.');
        let mut parts: Vec<&str> = tok.split('.').filter(|p| !p.is_empty()).collect();
        if parts.len() >= 2 {
            parts.truncate(3);
            while parts.len() < 3 {
                parts.push("0");
            }
            if let Ok(v) = semver::Version::parse(&parts.join(".")) {
                return Some(v);
            }
        }
    }
    None
}

/// Run `program args…` and capture stdout, or `None` if it cannot be run.
pub(super) fn run_capture(program: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new(program)
        .args(args)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Is this capability expression satisfied here, and if not, why?
///
/// - `Ok(None)`         — satisfied.
/// - `Ok(Some(reason))` — unmet, phrased for a human: absent, or the wrong version.
/// - `Err(e)`           — the expression itself is malformed (a config error, not an environment one).
///
/// The three are kept apart because they call for different actions: install the tool, upgrade it,
/// or fix the typo. Name is checked before version, so an absent tool never reaches a version probe
/// and `windows >= 10` short-circuits on unix without asking Windows what build it is.
///
/// This is the one function both halves of the contract call — `requires` (skip on unmet) and
/// `must_run` (fail on unmet). They must never disagree about what a string means.
/// Is `name` a built-in capability this host provides? The base layer under [`Capabilities`]: a
/// project's registered names are consulted first (in `Capabilities::available`), then this answers
/// for `docker`, the platform predicates, compiled-in native clients, and finally any tool-of-that-
/// name on PATH (so `requires = { "kubectl" }` just works). A missing capability never fails a test
/// — it skips it, visibly.
pub(super) fn builtin_available(name: &str) -> bool {
    match name {
        // The docker daemon must be reachable *and* the feature compiled in. Retry a few times: a
        // single `docker info` can transiently fail when the daemon is momentarily busy (heavy
        // container churn — e.g. many container tests tearing down at once), which would otherwise
        // skip a whole test spuriously. This resolves once per run (memoized), so the cost is bounded;
        // a genuinely-absent daemon fails fast (connection-refused is instant), so the retry budget is
        // paid mostly as backoff sleeps only when the daemon is present-but-busy.
        "docker" => cfg!(feature = "docker") && docker_runs_linux_containers(),
        "github" => std::env::var_os("GITHUB_TOKEN").is_some(),
        // Platform predicates. `shell.run("…")` routes a STRING through the platform's shell — `sh -c`
        // on unix, `cmd /C` on Windows — so a test asserting POSIX syntax (`$VAR`, `;`, `1>&2`,
        // `sleep`) genuinely *cannot run* off unix. That is a capability question, not a bug: the
        // honest answer is to skip, the way an absent docker daemon skips. (The argv form
        // `shell.run{"prog", "arg"}` needs no shell and stays portable — prefer it.)
        "unix" => cfg!(unix),
        "windows" => cfg!(windows),
        // No cheap, reliable synchronous probe; assume present (a real offline mode is future work).
        "network" | "internet" => true,
        // A native-client capability (`kafka`, `postgres`, …) is available iff its feature was
        // compiled into this build — so `requires = { "kafka" }` skips gracefully in a build that
        // lacks it, exactly as `docker` skips without a daemon. This is the unified gate: there is no
        // separate `requires_native`, just a capability with a compiled-in detector. Anything not a
        // native capability falls through to a tool-on-PATH probe (`requires = { "kubectl" }`).
        other => match native_capability_compiled(other) {
            Some(compiled) => compiled,
            None => binary_on_path(other),
        },
    }
}

/// Whether `name` is a native-client capability and, if so, whether *this* build compiled it in.
/// `Some(true)`/`Some(false)` for a known native capability; `None` if `name` is not one (so the
/// caller falls back to a binary-on-PATH probe). The name set is fixed (independent of features);
/// only the `cfg!` results vary per build, which is what makes a lean distribution skip cleanly.
pub(super) fn native_capability_compiled(name: &str) -> Option<bool> {
    let compiled = match name {
        "http" => cfg!(feature = "http"),
        "sqlite" => cfg!(feature = "sqlite"),
        "grpc" => cfg!(feature = "grpc"),
        "graphql" => cfg!(feature = "graphql"),
        "yaml" => cfg!(feature = "yaml"),
        _ => return None,
    };
    Some(compiled)
}

/// Run `program args...`, discarding output; true iff it exits 0. Used for daemon-liveness checks.
pub(super) fn command_succeeds(program: &str, args: &[&str]) -> bool {
    std::process::Command::new(program)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// `command_succeeds`, retried up to `attempts` times with a short backoff — for a daemon-liveness
/// probe that can hiccup transiently. Succeeds on the first passing attempt (so when the daemon is
/// healthy there is no delay); only a genuinely-absent daemon pays the full backoff.
/// Can this daemon run the **Linux** containers prova's resources are?
///
/// Answering `docker info` is not enough, and the gap is not hypothetical: Docker on Windows in
/// *Windows-container* mode answers `info` perfectly happily and then cannot pull
/// `postgres:16-alpine`. A suite that says `requires = { "docker" }` means "I am about to run a
/// linux image", so that is what the capability has to check — otherwise the gate waves the suite
/// through and it dies later on an obscure "Docker stream error" instead of skipping. Ask the daemon
/// what OS its containers are.
pub fn docker_runs_linux_containers() -> bool {
    if !command_succeeds_retry("docker", &["info"], 8) {
        return false;
    }
    std::process::Command::new("docker")
        .args(["info", "--format", "{{.OSType}}"])
        .output()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .trim()
                .eq_ignore_ascii_case("linux")
        })
        .unwrap_or(false)
}

pub(super) fn command_succeeds_retry(program: &str, args: &[&str], attempts: u32) -> bool {
    for attempt in 0..attempts {
        if command_succeeds(program, args) {
            return true;
        }
        if attempt + 1 < attempts {
            std::thread::sleep(Duration::from_millis(300));
        }
    }
    false
}

/// Is an executable named `name` on `PATH`?
///
/// On Windows an executable on `PATH` carries an extension (`cargo.exe`), so probing the bare name
/// finds nothing and *every* `requires` gate would skip. Try each `PATHEXT` suffix as well —
/// `requires = { "cargo" }` names the tool, not the file.
pub(super) fn binary_on_path(name: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };

    let mut candidates = vec![name.to_string()];
    if cfg!(windows) {
        let pathext =
            std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string());
        candidates.extend(
            pathext
                .split(';')
                .filter(|ext| !ext.is_empty())
                .map(|ext| format!("{name}{ext}")),
        );
    }

    std::env::split_paths(&path).any(|dir| candidates.iter().any(|file| dir.join(file).is_file()))
}
