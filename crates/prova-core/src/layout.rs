//! Filesystem layout — where Prova keeps config, cache, and data, abstracted so tests can point it
//! at a temp root. Mirrors archetect's `SystemLayout`: an XDG layout for production and a rooted
//! layout for tests.
//!
//! The three roots and what Prova puts under them:
//!
//! | Dir | Purpose |
//! |---|---|
//! | `config_dir` | global `prova.toml` defaults (future) |
//! | `cache_dir`  | git-fetched plugin checkouts (`cache_dir/plugins`) |
//! | `data_dir`   | globally-installed plugins the searcher consults (`data_dir/plugins`) |
//!
//! Like archetect v3, the XDG layout is used **on macOS too** (config at `~/.config/prova`, not
//! `~/Library/…`), so a developer's paths match across Unix-likes. `XDG_*` env vars are honored.

use std::path::PathBuf;

/// Where Prova reads and writes its own files. Implementors provide the three roots; the plugin
/// sub-dirs derive from them.
pub trait SystemLayout: Send + Sync {
    /// Global configuration (`~/.config/prova`).
    fn config_dir(&self) -> PathBuf;
    /// Ephemeral, reproducible-from-source cache (`~/.cache/prova`).
    fn cache_dir(&self) -> PathBuf;
    /// Durable application data (`~/.local/share/prova`).
    fn data_dir(&self) -> PathBuf;

    // NOTE: there is deliberately no `plugins_dir()` (a machine-global `data_dir/plugins` the
    // searcher would consult). A project may require only what is in version control — its own
    // `.prova/plugins/`, or a git source pinned in `prova.toml` — so a clean clone resolves what the
    // author's machine resolves. A per-user plugin dir would let a proof pass locally and fail in
    // CI with nothing in the repo to explain it. If you are about to add one back, that is why it
    // isn't here. (`plugin_cache_dir` below is different: it holds *pinned* git checkouts, which are
    // reproducible from the manifest.)

    /// Cache root for git-fetched plugin checkouts (`cache_dir/plugins`).
    fn plugin_cache_dir(&self) -> PathBuf {
        self.cache_dir().join("plugins")
    }

    /// Root for the LuaLS core annotation stubs (`data_dir/lua/annotations`), a **stable, unversioned**
    /// path shared by every project on the machine — a project's `.luarc.json` points straight at it,
    /// so the pointer is write-once and never churns on a prova upgrade. Freshness is kept by a
    /// `.version` stamp inside the dir (re-extract on mismatch), not by a version-keyed sub-path.
    ///
    /// `data`, not `cache`: an editor config points at this persistently, so a cache-wipe shouldn't
    /// silently kill completion. `lua/annotations` mirrors archetect's layout so the two tools' IDE
    /// setup is one convention (`~/.local/share/<tool>/lua/annotations/`), coexisting in one
    /// `.luarc.json` without either clobbering the other.
    fn annotations_dir(&self) -> PathBuf {
        self.data_dir().join("lua").join("annotations")
    }
}

/// Production layout: XDG base dirs (honoring `XDG_CONFIG_HOME` / `XDG_CACHE_HOME` /
/// `XDG_DATA_HOME`), else `~/.config`, `~/.cache`, `~/.local/share`, each with a `prova` leaf.
pub struct XdgSystemLayout {
    config: PathBuf,
    cache: PathBuf,
    data: PathBuf,
}

impl XdgSystemLayout {
    /// Build the layout from the environment. Errors only if the home directory cannot be found and
    /// no `XDG_*` override supplies the corresponding base.
    pub fn new() -> std::io::Result<Self> {
        let home = dirs::home_dir();
        let base = |env_key: &str, default: &str| -> std::io::Result<PathBuf> {
            match std::env::var_os(env_key) {
                Some(v) if !v.is_empty() => Ok(PathBuf::from(v)),
                _ => home.clone().map(|h| h.join(default)).ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        format!("cannot locate home directory (and {env_key} is unset)"),
                    )
                }),
            }
        };
        Ok(XdgSystemLayout {
            config: base("XDG_CONFIG_HOME", ".config")?.join("prova"),
            cache: base("XDG_CACHE_HOME", ".cache")?.join("prova"),
            data: base("XDG_DATA_HOME", ".local/share")?.join("prova"),
        })
    }
}

impl SystemLayout for XdgSystemLayout {
    fn config_dir(&self) -> PathBuf {
        self.config.clone()
    }
    fn cache_dir(&self) -> PathBuf {
        self.cache.clone()
    }
    fn data_dir(&self) -> PathBuf {
        self.data.clone()
    }
}

/// Test layout: every root under one temp directory (`root/config`, `root/cache`, `root/data`), so a
/// test can exercise plugin install/fetch against a throwaway home.
pub struct RootedSystemLayout {
    root: PathBuf,
}

impl RootedSystemLayout {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        RootedSystemLayout { root: root.into() }
    }
}

impl SystemLayout for RootedSystemLayout {
    fn config_dir(&self) -> PathBuf {
        self.root.join("config")
    }
    fn cache_dir(&self) -> PathBuf {
        self.root.join("cache")
    }
    fn data_dir(&self) -> PathBuf {
        self.root.join("data")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rooted_layout_derives_all_paths_under_one_root() {
        let layout = RootedSystemLayout::new("/tmp/prova-test");
        assert_eq!(layout.config_dir(), PathBuf::from("/tmp/prova-test/config"));
        assert_eq!(layout.cache_dir(), PathBuf::from("/tmp/prova-test/cache"));
        assert_eq!(layout.data_dir(), PathBuf::from("/tmp/prova-test/data"));
        // The git-checkout cache derives from cache. (There is no machine-global `plugins_dir` to
        // assert on — see the note on the trait.)
        assert_eq!(
            layout.plugin_cache_dir(),
            PathBuf::from("/tmp/prova-test/cache/plugins")
        );
    }

    #[test]
    fn xdg_layout_honors_env_overrides() {
        // Set all three so the test never touches the real home directory.
        std::env::set_var("XDG_CONFIG_HOME", "/xdg/config");
        std::env::set_var("XDG_CACHE_HOME", "/xdg/cache");
        std::env::set_var("XDG_DATA_HOME", "/xdg/data");
        let layout = XdgSystemLayout::new().expect("build layout");
        assert_eq!(layout.config_dir(), PathBuf::from("/xdg/config/prova"));
        assert_eq!(layout.cache_dir(), PathBuf::from("/xdg/cache/prova"));
        assert_eq!(layout.data_dir(), PathBuf::from("/xdg/data/prova"));
        std::env::remove_var("XDG_CONFIG_HOME");
        std::env::remove_var("XDG_CACHE_HOME");
        std::env::remove_var("XDG_DATA_HOME");
    }
}
