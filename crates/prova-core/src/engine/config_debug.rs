//! `RunConfig`'s Debug: the fields worth a line in a log, never the module closures it carries.

use super::RunConfig;

impl std::fmt::Debug for RunConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunConfig")
            .field("concurrency", &self.concurrency)
            .field("selection", &self.selection)
            .field("ports", &self.ports)
            .field("update_snapshots", &self.update_snapshots)
            .field("modules", &self.modules.len())
            .field("package_roots", &self.package_roots)
            .field("named_packages", &self.named_packages)
            .field("package_namespaces", &self.package_namespaces)
            .finish()
    }
}
