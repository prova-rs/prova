# reports — the artifact a conduct produced, kept

A deputed conduct hands back three things: cases (to the ledger), measurements (to the
ratchets), and its own artifact — which used to be dropped under `target/`, so a coverage
floor could refuse a regression and be unable to show which lines moved.

```lua
report.publish{
  name = "coverage",                       -- the address: `prova reports coverage`
  summary = "unit 73.5% · merged 86.4%",   -- the gist, no viewer needed
  explains = "rust.coverage.unit",         -- the measurement this is evidence FOR
  forms = { json = json_path, html = html_dir .. "/index.html" },
}
```

COPIED into `.prova/var/reports/<name>/`, so it outlives the conduct. `forms` are
renderings of one fact — an agent takes the `json`, a person opens the `html`. Three ways
to read them back, one per reader:

    prova reports                       what exists, each summary and its forms
    prova reports coverage --kind json  that path ALONE — composes into $(…)
    prova reports -c                    pick from a menu and open it

`-c`/`--choose` is the human lane: it offers only what a person would open, and opens it.
It needs a terminal — **anywhere else it lists instead of prompting**, so a menu can never
hang a pipeline that inherited the flag from a doc. Prova never renders an artifact; the
deputy did. Custody, not a dashboard.

Two consumers ship with prova: the layered coverage conduct (json + html, naming the three
floors it explains) and the nextest deputy (its junit, whose per-case detail also used to
vanish with `target/`).

See also: `prova learn verifiers` (the seam that produces them) · `prova learn record`
(what a run banks) · `prova learn running` (lanes and selection)
