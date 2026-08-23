//! Capabilities: the declared vocabulary of host facts (`requires = {...}`, `must_run = [...]`),
//! resolved against this machine.
//!
//! See docs/design/capabilities.md. A capability is declared in the manifest's `[capabilities]`
//! table as a name plus a factory, in one of three kinds:
//!
//! - a **package predicate** — Lua, exported from a package, resolved EAGERLY at load;
//! - a **command probe** — declarative TOML (`command`/`probe`/`version`/`pattern`), resolved
//!   LAZILY on first reference and memoized;
//! - an **intrinsic** — one of prova's own built-in checkers, named out loud.
//!
//! Undeclared names fall through per the `"*"` policy: probed as a binary on `PATH` (the default),
//! probed with a teaching warning, or refused as a config error.

use super::*;

/// The project's capability vocabulary: what each name means here, and the answers probed for it.
///
/// **Per run, not global.** This lives in [`RunConfig`], so two projects resolved in one process —
/// the warm MCP resolving one at startup, then `run { project }` — cannot see each other's
/// vocabulary. It was a process-global static once, and that leaked across projects
/// (`tests/capability_isolation.rs`).
///
/// **Lua answers, not Lua closures.** A package predicate's verdict is captured at load and only
/// the answer survives. Three reasons that are one: `must_run` is a precondition checked before any
/// suite exists (nothing to call back into); each suite gets its own `Lua` and mlua handles are
/// `!Send` (a stored closure could not cross states); and a capability that answered differently for
/// two suites in one run would be a coin flip, not a capability.
///
/// **Command and intrinsic answers are lazy and memoized.** Those kinds are pure data with no state
/// to die, so they are probed on first reference and remembered for the rest of the run. That is
/// worth the machinery: under `"*" = "error"` a serious package declares every tool it touches, and
/// probing twenty of them eagerly would add twenty process spawns to every invocation — including
/// `prova --list` — to answer questions nothing asked. It also means `requires = { "docker" }` on
/// fifty tests probes the daemon once rather than fifty times.
#[derive(Clone, Default, Debug)]
pub struct Capabilities {
    /// Package-predicate verdicts, captured at load. An entry is present whether the predicate said
    /// yes or no — see [`Capabilities::declared_answer`] for why a declared "no" must be recorded
    /// rather than left absent.
    resolved: std::collections::BTreeMap<String, Option<semver::Version>>,
    /// Names whose package predicate answered NO. Kept beside `resolved` rather than as an
    /// `Option<Option<Version>>` so `registered_names` and the report can still speak of "what this
    /// project declares" without unwrapping two layers of optionality.
    refused: std::collections::BTreeSet<String>,
    /// Declarative command probes (`command = "..."`), by capability name.
    probes: std::collections::BTreeMap<String, CommandProbe>,
    /// Names wired to one of prova's built-in checkers (`intrinsic = "docker"`): name → preset. The
    /// preset may differ from the name, which is how a built-in is aliased
    /// (`dockerd = { intrinsic = "docker" }`).
    intrinsics: std::collections::BTreeMap<String, String>,
    /// What an undeclared, non-built-in name means here (the `"*"` entry).
    undeclared: UndeclaredPolicy,
    /// Lazy answers, memoized for the run. `Arc` so the worker-pool clones of `RunConfig`
    /// (`suite.rs`) share one memo instead of each re-probing the host.
    memo: std::sync::Arc<std::sync::Mutex<Memo>>,
    /// Under [`UndeclaredPolicy::Warn`], the undeclared names actually reached — collected here and
    /// reported by the CLI as one grouped teaching block after the run.
    ///
    /// Collected rather than printed from here because prova-core is a library: an embedder decides
    /// whether and where configuration advice appears, and a grouped block at the end reads better
    /// than lines interleaved with test output.
    fell_through: std::sync::Arc<std::sync::Mutex<std::collections::BTreeSet<String>>>,
}

/// Memoized answers for the lazily-probed kinds. Availability and version are cached separately
/// because they cost different subprocesses: a bare `requires = { "docker" }` needs only the
/// liveness probe, and folding both into one entry would make it pay for a version query nothing
/// asked for.
#[derive(Default, Debug)]
struct Memo {
    available: std::collections::BTreeMap<String, bool>,
    version: std::collections::BTreeMap<String, Option<semver::Version>>,
}

/// What an undeclared, non-built-in capability name means in this package (the `"*"` entry in
/// `[capabilities]`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UndeclaredPolicy {
    /// Probe it as a binary on `PATH` — the default, and the behavior of every manifest already in
    /// the wild. `requires = { "kubectl" }` works with no ceremony.
    #[default]
    Probe,
    /// Probe it, and teach the missing declaration. The migration rung: run warm, collect the
    /// lines, declare what they name, then close the door.
    Warn,
    /// Refuse it. An undeclared name is a config error, not an unavailable capability — the
    /// vocabulary is closed.
    Error,
}

impl UndeclaredPolicy {
    /// Parse the `"*"` entry's value. An unrecognized policy is REFUSED rather than defaulted
    /// (docs/design/agent-ergonomics.md#unhonorable-option-is-refused): silently reading
    /// `"*" = "strict"` as `probe` would hand back the permissive behavior under a key that says
    /// otherwise, which is the exact failure the strict mode exists to prevent.
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "probe" => Ok(Self::Probe),
            "warn" => Ok(Self::Warn),
            "error" => Ok(Self::Error),
            other => Err(format!(
                "[capabilities] \"*\" = {other:?} is not a fall-through policy — say \"probe\" \
                 (the default: an undeclared name is probed on PATH), \"warn\" (probe, and teach \
                 the missing declaration), or \"error\" (refuse an undeclared name)"
            )),
        }
    }
}

/// Which stream a tool talks on. `java -version` writes to **stderr**, which the old
/// `--version`-and-take-the-first-number heuristic could not see at all — the concrete gap that
/// made a declarative probe worth having.
///
/// Governs both the `expect` comparison and version parsing: one knob meaning "where this tool
/// talks", rather than two that can disagree.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Stream {
    #[default]
    Stdout,
    Stderr,
    Both,
}

impl Stream {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "stdout" => Ok(Self::Stdout),
            "stderr" => Ok(Self::Stderr),
            "both" => Ok(Self::Both),
            other => Err(format!(
                "stream = {other:?} is not a stream — say \"stdout\" (the default), \"stderr\" \
                 (where tools like `java -version` report), or \"both\""
            )),
        }
    }

    /// The text this stream selects. `Both` concatenates so a pattern can match either half without
    /// the author having to know which one the tool chose.
    fn pick(self, stdout: &str, stderr: &str) -> String {
        match self {
            Self::Stdout => stdout.to_string(),
            Self::Stderr => stderr.to_string(),
            Self::Both => format!("{stdout}\n{stderr}"),
        }
    }
}

/// How a command probe answers "which version?".
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum VersionQuery {
    /// No `version` key: run `--version` and take the first version-shaped token — exactly what an
    /// undeclared name on `PATH` already did, which is what makes `{ command = "kind" }` a faithful
    /// written-down form of the implicit probe rather than a new behavior.
    #[default]
    Heuristic,
    /// `version = false` — this capability has no version concept.
    None,
    /// `version = [...]` — these args produce the version.
    Args(Vec<String>),
}

/// A declarative command probe: the `command` selector's whole configuration.
///
/// Two questions, kept apart, because conflating them is what made the old built-in vocabulary
/// unfixable from outside: **is it here** (`probe`/`expect`, or PATH presence) and **which version**
/// (`version`/`stream`/`pattern`). Docker is the case that proves the split is real — the daemon
/// must answer *and* run Linux containers, which is an availability question with an expected
/// answer, while its version comes from an entirely different invocation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommandProbe {
    /// The executable. Also the PATH-presence check when `probe` is absent.
    pub command: String,
    /// Args for the availability check; exit 0 means available. Absent → PATH presence only.
    pub probe: Option<Vec<String>>,
    /// Require the probe's output (on `stream`) to equal this, trimmed, case-insensitively.
    pub expect: Option<String>,
    /// How the version is obtained.
    pub version: VersionQuery,
    /// Which stream carries the output, for both `expect` and version parsing.
    pub stream: Stream,
    /// A regex over that output; the first capture group if there is one, else the whole match. The
    /// matched text still goes through [`parse_first_version`], so a pattern narrows and the parser
    /// normalizes — `v1.30` and `1.30.2-rc1` both land as semver without the author padding them.
    pub pattern: Option<String>,
    /// Retry the availability probe this many times, with backoff. For a daemon that can hiccup
    /// transiently under load rather than one that is absent.
    pub retries: u32,
}

impl CommandProbe {
    /// Validate what cannot be checked by the type system: that the pattern compiles.
    ///
    /// Called at declaration time so a typo'd regex is a config error naming the entry, not a
    /// version that silently never parses — which would read as "cannot confirm" and skip forever,
    /// the vacuous green this whole contract exists to remove.
    pub fn validate(&self, name: &str) -> Result<(), String> {
        if let Some(p) = &self.pattern {
            regex::Regex::new(p).map_err(|e| {
                format!("[capabilities] {name}: pattern {p:?} is not a valid regex: {e}")
            })?;
        }
        if self.expect.is_some() && self.probe.is_none() {
            return Err(format!(
                "[capabilities] {name}: `expect` needs a `probe` to compare against — give the \
                 args that produce the output (e.g. probe = [\"info\", \"--format\", \
                 \"{{{{.OSType}}}}\"])"
            ));
        }
        Ok(())
    }

    /// The built-in `docker` checker, written in the declarative vocabulary. Not used to *implement*
    /// the intrinsic — it exists so a unit test can assert the two agree, which is what keeps
    /// `intrinsic` a named preset rather than a privileged escape hatch
    /// (docs/design/capabilities.md#intrinsics-are-expressible).
    pub fn docker_equivalent() -> Self {
        Self {
            command: "docker".to_string(),
            probe: Some(vec![
                "info".to_string(),
                "--format".to_string(),
                "{{.OSType}}".to_string(),
            ]),
            expect: Some("linux".to_string()),
            version: VersionQuery::Args(vec![
                "version".to_string(),
                "--format".to_string(),
                "{{.Server.Version}}".to_string(),
            ]),
            stream: Stream::Stdout,
            pattern: None,
            retries: 8,
        }
    }

    /// Is this capability available? `probe` decides when given, else PATH presence.
    fn available(&self) -> bool {
        let Some(args) = &self.probe else {
            return binary_on_path(&self.command);
        };
        let attempts = self.retries.max(1);
        for attempt in 0..attempts {
            if let Some((out, err)) = run_streams(&self.command, args) {
                let Some(expected) = &self.expect else {
                    return true;
                };
                if self
                    .stream
                    .pick(&out, &err)
                    .trim()
                    .eq_ignore_ascii_case(expected.trim())
                {
                    return true;
                }
                // The command answered and said something else. That is a definitive NO — retrying
                // would not change a Windows-container daemon's mind — so stop rather than burn the
                // whole backoff budget on a settled answer.
                return false;
            }
            if attempt + 1 < attempts {
                std::thread::sleep(Duration::from_millis(300));
            }
        }
        false
    }

    /// Which version, if the question is meaningful and answerable here.
    fn version(&self) -> Option<semver::Version> {
        let args: Vec<String> = match &self.version {
            VersionQuery::None => return None,
            VersionQuery::Heuristic => vec!["--version".to_string()],
            VersionQuery::Args(args) => args.clone(),
        };
        let (out, err) = run_streams(&self.command, &args)?;
        extract_version(&self.stream.pick(&out, &err), self.pattern.as_deref())
    }
}

/// A manifest `[capabilities]` entry, resolved: a name and the factory behind it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapabilityRegistration {
    pub name: String,
    pub factory: CapabilityFactory,
}

/// Which registry a declaration's factory comes from — the `package` / `command` / `intrinsic`
/// selector, resolved. Exactly one, because an entry naming two would be a guess about which the
/// author meant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CapabilityFactory {
    /// `package = "env", capability = "gpu"` (or `factory = "capabilities.gpu"`) — a Lua predicate,
    /// invoked once at load. `options` is a pre-serialized Lua value expression handed to it as its
    /// only argument, or `None` to call it bare.
    Package {
        package: String,
        factory: String,
        options: Option<String>,
    },
    /// `command = "..."` and friends — the declarative probe.
    Command(CommandProbe),
    /// `intrinsic = "docker"` — one of prova's own checkers, possibly under a different name.
    Intrinsic(String),
}

/// Is `s` a legal capability name? `[A-Za-z0-9_-]+`, which is also what makes `"*"` safe as the
/// fall-through policy key: it cannot collide with a name.
pub(super) fn is_capability_name(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Is `s` a dotted path of Lua identifiers? Manifest-supplied package and factory names are spliced
/// into generated Lua source, so they are validated against a conservative shape first — an
/// out-of-shape value is a clear error, never a silent hole or an injection.
pub(super) fn is_ident_path(s: &str) -> bool {
    !s.is_empty()
        && s.split('.').all(|seg| {
            let mut c = seg.chars();
            c.next()
                .is_some_and(|f| f.is_ascii_alphabetic() || f == '_')
                && c.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        })
}

/// Which kind of declaration a name carries — for the report, which must be able to say what a name
/// means here rather than just whether it holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeclKind {
    /// A Lua predicate from a package, already resolved.
    Package,
    /// A declarative command probe.
    Command,
    /// One of prova's built-in checkers, named out loud.
    Intrinsic,
}

impl DeclKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Package => "package predicate",
            Self::Command => "command probe",
            Self::Intrinsic => "intrinsic",
        }
    }
}

impl Capabilities {
    /// Record a package predicate that said YES. `version` is `None` for a bare `true` (available,
    /// no version to compare).
    pub fn register(&mut self, name: &str, version: Option<semver::Version>) {
        self.refused.remove(name);
        self.resolved.insert(name.to_string(), version);
    }

    /// Record a package predicate that said NO.
    ///
    /// Recording the negative is load-bearing. Leaving it absent — which is what the companion did —
    /// means a declared capability whose predicate answered "no" falls through to the PATH probe and
    /// can come back **available** because a binary of that name happens to exist. Declaring what a
    /// name means and then having prova quietly ask a different question is the worst of both
    /// mechanisms (docs/design/capabilities.md#a-declared-no-is-final).
    pub fn register_absent(&mut self, name: &str) {
        self.resolved.remove(name);
        self.refused.insert(name.to_string());
    }

    /// Declare a command probe for `name`.
    pub fn declare_command(&mut self, name: &str, probe: CommandProbe) {
        self.probes.insert(name.to_string(), probe);
    }

    /// Declare that `name` resolves to the built-in checker `preset`.
    pub fn declare_intrinsic(&mut self, name: &str, preset: &str) {
        self.intrinsics
            .insert(name.to_string(), preset.to_string());
    }

    /// Set the `"*"` fall-through policy.
    pub fn set_undeclared_policy(&mut self, policy: UndeclaredPolicy) {
        self.undeclared = policy;
    }

    pub fn undeclared_policy(&self) -> UndeclaredPolicy {
        self.undeclared
    }

    /// Take everything else's declarations into this one, letting `other` win. Used to layer the
    /// manifest's `[capabilities]` over the deprecated companion's registrations — the manifest is
    /// the current mechanism, so it is the one that wins.
    pub fn absorb(&mut self, other: Capabilities) {
        for (name, version) in other.resolved {
            self.register(&name, version);
        }
        for name in other.refused {
            self.register_absent(&name);
        }
        self.probes.extend(other.probes);
        self.intrinsics.extend(other.intrinsics);
        if other.undeclared != UndeclaredPolicy::default() {
            self.undeclared = other.undeclared;
        }
    }

    /// The names a package predicate registered — the report's "declared by a predicate" rows, and
    /// (during the deprecation bridge) what the companion contributed.
    pub fn registered_names(&self) -> impl Iterator<Item = &String> {
        self.resolved.keys().chain(self.refused.iter())
    }

    /// Every declared name with its kind, for the report.
    pub fn declared_names(&self) -> Vec<(String, DeclKind)> {
        let mut out: Vec<(String, DeclKind)> = Vec::new();
        for name in self.resolved.keys().chain(self.refused.iter()) {
            out.push((name.clone(), DeclKind::Package));
        }
        out.extend(self.probes.keys().map(|n| (n.clone(), DeclKind::Command)));
        out.extend(
            self.intrinsics
                .keys()
                .map(|n| (n.clone(), DeclKind::Intrinsic)),
        );
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out.dedup_by(|a, b| a.0 == b.0);
        out
    }

    /// What kind of declaration `name` carries here, if any.
    pub fn declaration(&self, name: &str) -> Option<DeclKind> {
        if self.resolved.contains_key(name) || self.refused.contains(name) {
            return Some(DeclKind::Package);
        }
        if self.probes.contains_key(name) {
            return Some(DeclKind::Command);
        }
        if self.intrinsics.contains_key(name) {
            return Some(DeclKind::Intrinsic);
        }
        None
    }

    /// Does this declaration override a built-in of the same name? The report says so explicitly,
    /// which is what makes overriding safe: the old blanket refusal was protecting against a
    /// *silent* redefinition, and a declaration the report names is not silent
    /// (docs/design/capabilities.md#overriding-a-builtin-is-declared).
    pub fn overrides_builtin(&self, name: &str) -> bool {
        !matches!(self.declaration(name), None | Some(DeclKind::Intrinsic))
            && is_builtin_capability(name)
    }

    /// The undeclared names this run fell through to under [`UndeclaredPolicy::Warn`]. Drained by
    /// the CLI after the run to teach the missing declarations in one block.
    pub fn take_fell_through(&self) -> Vec<String> {
        let mut set = self
            .fell_through
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::mem::take(&mut *set).into_iter().collect()
    }

    /// A declared name's answer, or `None` if `name` carries no declaration.
    fn declared_answer(&self, name: &str) -> Option<bool> {
        if self.resolved.contains_key(name) {
            return Some(true);
        }
        if self.refused.contains(name) {
            return Some(false);
        }
        if let Some(probe) = self.probes.get(name) {
            return Some(self.memo_available(name, || probe.available()));
        }
        if let Some(preset) = self.intrinsics.get(name) {
            let preset = preset.clone();
            return Some(self.memo_available(name, || builtin_available(&preset)));
        }
        None
    }

    /// Available = its declaration says so, else a built-in of that name says so, else the
    /// fall-through policy's PATH probe.
    ///
    /// A declared name never consults the layers below it. That is the point of declaring: the
    /// vocabulary says what the word means here, and prova asks exactly that question.
    pub fn available(&self, name: &str) -> bool {
        if let Some(answer) = self.declared_answer(name) {
            return answer;
        }
        if is_builtin_capability(name) {
            let owned = name.to_string();
            return self.memo_available(name, || builtin_available(&owned));
        }
        if self.undeclared == UndeclaredPolicy::Warn {
            self.fell_through
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(name.to_string());
        }
        let owned = name.to_string();
        self.memo_available(name, || binary_on_path(&owned))
    }

    /// The version a constraint compares against. `None` = no version to compare, which makes a
    /// constraint unsatisfiable — "cannot confirm" is not "satisfied".
    pub fn version(&self, name: &str) -> Option<semver::Version> {
        if let Some(v) = self.resolved.get(name) {
            return v.clone();
        }
        if self.refused.contains(name) {
            return None;
        }
        if let Some(probe) = self.probes.get(name) {
            return self.memo_version(name, || probe.version());
        }
        if let Some(preset) = self.intrinsics.get(name) {
            let preset = preset.clone();
            return self.memo_version(name, || builtin_version(&preset));
        }
        let owned = name.to_string();
        self.memo_version(name, || builtin_version(&owned))
    }

    /// Look up (or compute and remember) an availability answer.
    ///
    /// The lock is released while `probe` runs. A duplicate probe is possible when two workers race
    /// on the same cold name — harmless, and far better than the alternative: holding the mutex
    /// across a subprocess spawn would let one worker's absent-docker backoff (8 attempts, 300ms
    /// apart) block every other worker asking about `unix`.
    fn memo_available(&self, name: &str, probe: impl FnOnce() -> bool) -> bool {
        if let Some(hit) = self
            .memo
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .available
            .get(name)
        {
            return *hit;
        }
        let answer = probe();
        self.memo
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .available
            .insert(name.to_string(), answer);
        answer
    }

    fn memo_version(
        &self,
        name: &str,
        probe: impl FnOnce() -> Option<semver::Version>,
    ) -> Option<semver::Version> {
        if let Some(hit) = self
            .memo
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .version
            .get(name)
        {
            return hit.clone();
        }
        let answer = probe();
        self.memo
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .version
            .insert(name.to_string(), answer.clone());
        answer
    }

    /// Is this capability expression satisfied here, and if not, why?
    ///
    /// - `Ok(None)`         — satisfied.
    /// - `Ok(Some(reason))` — unmet: absent, or the wrong version (phrased for a human).
    /// - `Err(e)`           — a config error: the expression is malformed, or the name is undeclared
    ///   under a closed vocabulary.
    ///
    /// The three are kept apart because they call for different actions: install the tool, upgrade
    /// it, or fix the config. This is the one function both halves of the contract call — `requires`
    /// (skip on unmet) and `must_run` (fail on unmet) — so they can never disagree about what a
    /// string means. Name before version, so an absent tool never reaches a probe and
    /// `windows >= 10` short-circuits on unix.
    pub fn expr_status(&self, expr: &str) -> Result<Option<String>, String> {
        let parsed = CapabilityExpr::parse(expr)?;
        self.check_declared(parsed.name)?;
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

    /// Under a closed vocabulary (`"*" = "error"`), an undeclared name is a **config error** rather
    /// than an unavailable capability — the two want different remedies, and reporting a typo as
    /// "unavailable" sends the reader to install a tool that was never named.
    ///
    /// Built-ins pass undeclared: prova defines them, so they are not names the project left
    /// unnailed-down (docs/design/capabilities.md#strict-governs-only-undefined-names).
    fn check_declared(&self, name: &str) -> Result<(), String> {
        if self.undeclared != UndeclaredPolicy::Error
            || self.declaration(name).is_some()
            || is_builtin_capability(name)
        {
            return Ok(());
        }
        Err(format!(
            "capability {name:?} is not declared, and this package's [capabilities] sets \
             \"*\" = \"error\" — declare it:\n\n    [capabilities]\n    {name} = {{ command = \
             {name:?} }}\n\nor relax the vocabulary with \"*\" = \"probe\". \
             (`prova learn capabilities`)"
        ))
    }

    /// Why `expr` is unmet, or `None` if satisfied — the skip-side phrasing over `expr_status`. A
    /// malformed expression or an undeclared name is reported as the reason rather than folded into
    /// "absent": the author needs to see the typo, not hunt for a tool that was never named.
    pub(super) fn unmet_reason(&self, expr: &str) -> Option<String> {
        match self.expr_status(expr) {
            Ok(None) => None,
            Ok(Some(reason)) => Some(format!("requires {expr:?} ({reason})")),
            Err(e) => Some(e),
        }
    }

    /// Everything known about one capability, for `prova capabilities <name>`.
    ///
    /// This closes a real diagnostic gap: an unmet capability used to say only `"foo" is
    /// unavailable`, with no way to see what prova ran to decide that. A wrong-version skip is the
    /// case that hurt — the version came from somewhere, and "somewhere" was unprintable.
    pub fn explain(&self, name: &str) -> CapabilityExplanation {
        let kind = match self.declaration(name) {
            Some(k) => k.label().to_string(),
            None if is_builtin_capability(name) => "built-in".to_string(),
            None => match self.undeclared {
                UndeclaredPolicy::Error => "UNDECLARED (refused: \"*\" = \"error\")".to_string(),
                _ => "undeclared (probed on PATH)".to_string(),
            },
        };
        let mut detail: Vec<(String, String)> = Vec::new();
        if let Some(probe) = self.probes.get(name) {
            detail.push(("command".into(), probe.command.clone()));
            match &probe.probe {
                Some(args) => {
                    detail.push((
                        "availability".into(),
                        format!("{} {}", probe.command, args.join(" ")),
                    ));
                    match run_streams(&probe.command, args) {
                        Some((out, err)) => detail.push((
                            "→ output".into(),
                            probe.stream.pick(&out, &err).trim().to_string(),
                        )),
                        None => detail.push((
                            "→ output".into(),
                            "(the command is absent or exited non-zero)".into(),
                        )),
                    }
                    if let Some(expected) = &probe.expect {
                        detail.push(("expect".into(), expected.clone()));
                    }
                }
                None => detail.push((
                    "availability".into(),
                    format!("`{}` on PATH (no `probe` declared)", probe.command),
                )),
            }
            match &probe.version {
                VersionQuery::None => detail.push((
                    "version via".into(),
                    "nothing — declared `version = false`".into(),
                )),
                VersionQuery::Heuristic => detail.push((
                    "version via".into(),
                    format!("{} --version (the default heuristic)", probe.command),
                )),
                VersionQuery::Args(args) => detail.push((
                    "version via".into(),
                    format!("{} {}", probe.command, args.join(" ")),
                )),
            }
            // The raw version output is the row that closes the diagnostic gap: a wrong-version
            // skip reported the numbers and never what produced them.
            if probe.version != VersionQuery::None {
                let args: Vec<String> = match &probe.version {
                    VersionQuery::Args(a) => a.clone(),
                    _ => vec!["--version".to_string()],
                };
                match run_streams(&probe.command, &args) {
                    Some((out, err)) => detail.push((
                        "→ output".into(),
                        probe.stream.pick(&out, &err).trim().to_string(),
                    )),
                    None => detail.push((
                        "→ output".into(),
                        "(the command is absent or exited non-zero)".into(),
                    )),
                }
            }
            if let Some(p) = &probe.pattern {
                detail.push(("pattern".into(), p.clone()));
            }
        }
        if let Some(preset) = self.intrinsics.get(name) {
            detail.push(("checker".into(), format!("prova's built-in {preset:?}")));
        }
        if self.overrides_builtin(name) {
            detail.push((
                "overrides".into(),
                format!("prova's built-in {name:?} — this package redefines it"),
            ));
        }
        CapabilityExplanation {
            name: name.to_string(),
            kind,
            detail,
            available: self.available(name),
            version: self.version(name),
        }
    }
}

/// The `prova capabilities <name>` answer: what this name means here, what was run to decide, and
/// what came back.
#[derive(Debug)]
pub struct CapabilityExplanation {
    pub name: String,
    /// Which kind of declaration governs — or that none does.
    pub kind: String,
    /// Label → value rows: the commands run and their raw output.
    pub detail: Vec<(String, String)>,
    pub available: bool,
    pub version: Option<semver::Version>,
}

/// Is `name` a capability this build defines itself?
///
/// Still the guard on the DEPRECATED companion registrar, which refuses to redefine one: the
/// companion is the silent path (a predicate in a file nobody reads), and silence is what made
/// overriding dangerous. A manifest `[capabilities]` entry may override a built-in, because the
/// manifest is the one file a reader consults to learn what a name means.
pub fn is_builtin_capability(name: &str) -> bool {
    matches!(
        name,
        "docker" | "github" | "network" | "internet" | "unix" | "windows"
    ) || native_capability_compiled(name).is_some()
}

/// The built-in capability vocabulary prova probes by name — the enumerable core behind
/// `prova capabilities`. Beyond these, any executable on `PATH` is a capability, and a project
/// declares more in `[capabilities]`. Every entry satisfies `is_builtin_capability` (guarded by a
/// unit test) — this list is the single place the host report enumerates from.
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
        // A tool that answers somewhere else (`java -version`, on stderr) is exactly what the
        // declarative `command` probe's `stream` key is for.
        other => run_capture(other, &["--version"])?,
    };
    parse_first_version(&raw)
}

/// The version in `text`, narrowed by `pattern` when one is given.
///
/// The pattern picks the region (first capture group, else the whole match) and
/// [`parse_first_version`] normalizes it — so `pattern` never has to produce strict semver, only to
/// point at the right number. That split is what keeps `GitVersion:"v1.30.2"` a one-line
/// declaration instead of a Lua predicate.
pub(super) fn extract_version(text: &str, pattern: Option<&str>) -> Option<semver::Version> {
    let Some(pattern) = pattern else {
        return parse_first_version(text);
    };
    let re = regex::Regex::new(pattern).ok()?;
    let caps = re.captures(text)?;
    let picked = caps.get(1).or_else(|| caps.get(0))?;
    parse_first_version(picked.as_str())
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

/// Run `program args…` and capture BOTH streams, or `None` if it cannot be run or exits non-zero.
///
/// Both, because a declarative probe's `stream` key decides which one matters and this is the layer
/// below that decision — and because `expect` and version parsing may want different halves of the
/// same invocation's output.
pub(super) fn run_streams(program: &str, args: &[String]) -> Option<(String, String)> {
    let out = std::process::Command::new(program)
        .args(args)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some((
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    ))
}

/// Is `name` a built-in capability this host provides? The base layer under [`Capabilities`]: a
/// project's declarations are consulted first, then this answers for `docker`, the platform
/// predicates, compiled-in native clients, and finally any tool-of-that-name on PATH (so
/// `requires = { "kubectl" }` just works). A missing capability never fails a test — it skips it,
/// visibly.
pub(super) fn builtin_available(name: &str) -> bool {
    match name {
        // The docker daemon must be reachable *and* the feature compiled in. Retry a few times: a
        // single `docker info` can transiently fail when the daemon is momentarily busy (heavy
        // container churn — e.g. many container tests tearing down at once), which would otherwise
        // skip a whole test spuriously. Memoized by `Capabilities`, so the cost is bounded; a
        // genuinely-absent daemon fails fast (connection-refused is instant), so the retry budget is
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

/// Can this daemon run the **Linux** containers prova's resources are?
///
/// Answering `docker info` is not enough, and the gap is not hypothetical: Docker on Windows in
/// *Windows-container* mode answers `info` perfectly happily and then cannot pull
/// `postgres:16-alpine`. A suite that says `requires = { "docker" }` means "I am about to run a
/// linux image", so that is what the capability has to check — otherwise the gate waves the suite
/// through and it dies later on an obscure "Docker stream error" instead of skipping. Ask the daemon
/// what OS its containers are.
///
/// Callable directly, and prova's own Rust unit tests do exactly that to self-gate: they run without
/// a manifest, so there is no `[capabilities]` declaration for them to respect. Everything that runs
/// *under* a manifest must go through [`Capabilities`] instead, so a declared override is honored
/// (docs/design/capabilities.md#one-resolution-point).
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The declarative vocabulary must be able to express prova's own `docker` checker, or
    /// `intrinsic` is a privileged escape hatch rather than a named preset — and every gap in the
    /// declarative form would be invisible from inside prova
    /// (docs/design/capabilities.md#intrinsics-are-expressible).
    ///
    /// Asserted as agreement on THIS host, whatever this host is: with a daemon running both say
    /// available and report the same server version; without one, both say unavailable. Either way
    /// the two spellings answer alike, which is the property.
    #[test]
    fn the_declarative_docker_agrees_with_the_intrinsic() {
        let declared = CommandProbe::docker_equivalent();
        assert_eq!(
            declared.available(),
            builtin_available("docker"),
            "the declarative docker probe disagrees with the built-in about availability"
        );
        // Only meaningful when the daemon is there; when it is not, both are None and this holds
        // trivially — which is correct, not vacuous: agreement is the claim.
        assert_eq!(
            declared.version(),
            builtin_version("docker"),
            "the declarative docker probe disagrees with the built-in about the version"
        );
    }

    /// `probe` + `expect` is the availability half: the command must answer AND say the expected
    /// thing. This is the shape that catches a Windows-container daemon, so it is worth pinning on
    /// something every unix box has.
    #[test]
    fn expect_must_match_for_a_probe_to_be_available() {
        if !cfg!(unix) {
            return;
        }
        let yes = CommandProbe {
            command: "sh".into(),
            probe: Some(vec!["-c".into(), "echo linux".into()]),
            expect: Some("linux".into()),
            version: VersionQuery::None,
            ..Default::default()
        };
        assert!(yes.available(), "matching output is available");
        let no = CommandProbe {
            expect: Some("windows".into()),
            ..yes.clone()
        };
        assert!(
            !no.available(),
            "a command that answers the WRONG thing is unavailable, not available"
        );
        // Exit code alone is not enough when `expect` is declared — that conflation is the bug the
        // docker checker exists to avoid.
        let fails = CommandProbe {
            probe: Some(vec!["-c".into(), "exit 3".into()]),
            ..yes.clone()
        };
        assert!(!fails.available(), "a non-zero exit is unavailable");
    }

    /// `stream = "stderr"` is the concrete gap the old `--version`-on-stdout heuristic could not
    /// see: `java -version` reports there, and nothing in the built-in vocabulary could reach it.
    #[test]
    fn a_version_can_be_read_from_stderr() {
        if !cfg!(unix) {
            return;
        }
        let probe = CommandProbe {
            command: "sh".into(),
            probe: None,
            expect: None,
            version: VersionQuery::Args(vec![
                "-c".into(),
                "echo 'tool version 4.5.6' 1>&2".into(),
            ]),
            stream: Stream::Stderr,
            pattern: None,
            retries: 1,
        };
        assert_eq!(probe.version(), Some(semver::Version::new(4, 5, 6)));
        // The same invocation read from stdout finds nothing — which is the whole point: the stream
        // is meaning, not decoration.
        let stdout_only = CommandProbe {
            stream: Stream::Stdout,
            ..probe.clone()
        };
        assert_eq!(stdout_only.version(), None);
    }

    /// `pattern` narrows and `parse_first_version` normalizes, so a pattern never has to produce
    /// strict semver — only to point at the right number. Without the pattern the heuristic would
    /// grab the wrong one, which is what makes this worth having.
    #[test]
    fn a_pattern_picks_the_right_number_out_of_structure() {
        let text = "Client Version: v1.30.2\nKustomize Version: v5.0.4\n";
        assert_eq!(
            extract_version(text, Some("Client Version: v([0-9.]+)")),
            Some(semver::Version::new(1, 30, 2))
        );
        // The whole match is used when the pattern has no capture group.
        assert_eq!(
            extract_version("build 9.9.9 ok", Some("[0-9]+\\.[0-9]+\\.[0-9]+")),
            Some(semver::Version::new(9, 9, 9))
        );
        // A pattern that matches nothing yields no version — "cannot confirm", which makes a
        // constraint unsatisfiable rather than quietly satisfied.
        assert_eq!(extract_version(text, Some("Server Version: v([0-9.]+)")), None);
        // A two-component version still normalizes, so the pattern author need not pad it.
        assert_eq!(
            extract_version("v2.7", Some("v([0-9.]+)")),
            Some(semver::Version::new(2, 7, 0))
        );
    }

    /// A malformed pattern is a config error at declaration time, not a version that silently never
    /// parses — which would skip forever and read as green.
    #[test]
    fn a_malformed_pattern_is_refused_at_declaration() {
        let probe = CommandProbe {
            command: "sh".into(),
            pattern: Some("([0-9".into()),
            ..Default::default()
        };
        let err = probe.validate("x").expect_err("refused");
        assert!(err.contains("not a valid regex"), "{err}");
        // `expect` without `probe` has nothing to compare against.
        let orphan = CommandProbe {
            command: "sh".into(),
            expect: Some("linux".into()),
            ..Default::default()
        };
        assert!(orphan
            .validate("x")
            .expect_err("refused")
            .contains("needs a `probe`"));
    }

    /// A declared "no" never falls through to the layers below it. `sh` is on PATH everywhere this
    /// runs, so a predicate that says no is the only thing that can make it unavailable — and that
    /// is the behavior the old companion got wrong.
    #[test]
    fn a_declared_no_is_final() {
        let mut caps = Capabilities::default();
        assert!(caps.available("sh"), "sh is on PATH, so it starts available");
        caps.register_absent("sh");
        assert!(
            !caps.available("sh"),
            "a declared no must not fall through to a PATH probe that says yes"
        );
    }

    /// The fall-through policy: `probe` answers, `error` refuses as a CONFIG error (an unmet
    /// capability and a misconfigured one want different remedies), and a built-in is exempt because
    /// prova defines it (docs/design/capabilities.md#strict-governs-only-undefined-names).
    #[test]
    fn the_undeclared_policy_governs_only_undefined_names() {
        let mut caps = Capabilities::default();
        assert!(caps.expr_status("some-tool-that-is-not-here").is_ok());

        caps.set_undeclared_policy(UndeclaredPolicy::Error);
        let err = caps
            .expr_status("some-tool-that-is-not-here")
            .expect_err("a closed vocabulary refuses an undeclared name");
        assert!(err.contains("is not declared"), "{err}");
        // A built-in passes undeclared: prova defines it, so it is not a name left unnailed-down.
        assert!(
            caps.expr_status("unix").is_ok(),
            "a built-in is usable without a declaration even under a closed vocabulary"
        );
        // And declaring it satisfies the policy.
        caps.declare_command(
            "some-tool-that-is-not-here",
            CommandProbe {
                command: "some-tool-that-is-not-here".into(),
                ..Default::default()
            },
        );
        assert!(caps.expr_status("some-tool-that-is-not-here").is_ok());
    }

    /// An unrecognized policy is refused rather than defaulted: reading `"strict"` as `probe` would
    /// hand back the permissive behavior under a key that says otherwise.
    #[test]
    fn an_unknown_policy_is_refused() {
        assert_eq!(UndeclaredPolicy::parse("warn"), Ok(UndeclaredPolicy::Warn));
        let err = UndeclaredPolicy::parse("strict").expect_err("refused");
        assert!(err.contains("not a fall-through policy"), "{err}");
        assert!(Stream::parse("stderr").is_ok());
        assert!(Stream::parse("stdio").is_err());
    }

    /// The manifest may override a built-in; the report has to be able to say so. An `intrinsic`
    /// declaration is NOT an override — it names the same checker out loud.
    #[test]
    fn an_override_is_distinguishable_from_an_intrinsic_declaration() {
        let mut caps = Capabilities::default();
        caps.declare_intrinsic("docker", "docker");
        assert!(!caps.overrides_builtin("docker"), "naming it is not overriding it");
        let mut caps = Capabilities::default();
        caps.declare_command(
            "docker",
            CommandProbe {
                command: "sh".into(),
                ..Default::default()
            },
        );
        assert!(caps.overrides_builtin("docker"));
        // A name prova does not define is never an "override", however it is declared.
        caps.declare_command(
            "kubectl",
            CommandProbe {
                command: "kubectl".into(),
                ..Default::default()
            },
        );
        assert!(!caps.overrides_builtin("kubectl"));
    }

    /// `absorb` layers the manifest over the deprecated companion — the current mechanism wins, or a
    /// migration produces a mystery (docs/design/capabilities.md#manifest-wins-over-the-companion).
    #[test]
    fn the_manifest_wins_over_the_companion() {
        let mut companion = Capabilities::default();
        companion.register("gpu", Some(semver::Version::new(1, 0, 0)));
        let mut manifest = Capabilities::default();
        manifest.register("gpu", Some(semver::Version::new(2, 4, 0)));
        companion.absorb(manifest);
        assert_eq!(companion.version("gpu"), Some(semver::Version::new(2, 4, 0)));
    }
}
