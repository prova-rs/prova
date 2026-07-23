# Plan: init catalog — LANDED (2026-07)

**Folded into [`docs/design/ide-and-layout.md`](../design/ide-and-layout.md) §prova init.**
Shipped: `prova init` renders a catalog archetype (built-ins `project` + `plugin`, pinned
`#v1`; `~/.config/prova/config.toml` `[init.*]` overlays), interactive picker on a TTY /
hard error headless, `--list`, `-a/--answer`, `-s/--switch`, `--defaults`, `--headless`,
`--no-ide`, never-clobber unless `in_package = "allow"`, and package-state injection
(`prova_package_root`/`prova_plugin_root`/`prova:in-package`) for in-package renders. This
stub remains as the historical pointer.
