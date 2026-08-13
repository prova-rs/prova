--- One spelling, one meaning (docs/design/registry.md#update-flag-means-cached-assets): -U
--- refreshes the cached assets the invocation touches — git package sources on a run, the
--- registry cache on its verbs, the archetype checkout on `prova init` — and never the [runner]
--- provision, which carries its own distrust flag (proven in manifest/runner_provision_test).
--- The runtime force-update inside archetect is upstream's (v3.4.3, where with_force_update
--- shipped); prova's half is that every door accepts the flag and teaches the one meaning.

prova.test("every door accepts -U and teaches the one meaning", {
  covers = "docs/design/registry.md#update-flag-means-cached-assets",
  proves = "the flag rotted by accretion — three refreshes wearing one spelling, plus a subject rebuild that was never a cache — so the contract is taught at each door: the run's help scopes -U to cached assets and points the provision elsewhere; init's help names the archetype cache; and init ACCEPTS the flag instead of erroring on it",
}, function(t)
  local dir = t:tempdir()

  local help = shell.run(prova.bin .. " --help", { cwd = dir, merge_stderr = true })
  t:expect(help.stdout, "-U is scoped to cached assets"):contains("refresh cached assets")
  t:expect(help.stdout, "…and explicitly not the subject"):contains("never the [runner] provision")
  t:expect(help.stdout, "the provision's own flag exists"):contains("--reprovision")

  local init_help = shell.run(prova.bin .. " init --help", { cwd = dir, merge_stderr = true })
  t:expect(init_help.stdout, "init teaches its -U"):contains("-U/--update")
  t:expect(init_help.stdout, "…as archetype-cache distrust"):contains("re-probes the source")

  -- The flag parses on init (offline keeps this hermetic): the failure, if any, is about the
  -- catalog or the key — never about -U being unknown.
  local r = shell.run(prova.bin .. " init no-such-archetype -U --offline --headless",
    { cwd = dir, merge_stderr = true })
  t:expect(r.stdout, "the failure is about the key — -U parsed as a knob, not a stranger")
    :contains('unknown init key "no-such-archetype"')
  t:expect(r.stdout, "nothing blames the flag"):never():contains("-U")
end)
