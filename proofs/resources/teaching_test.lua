-- The teaching surface of `locks` (docs/design/verifiers.md#exclusive-conduct-resources): the
-- mechanics are architecture.md's claims; THIS suite pins that the cure for tool contention is
-- named at every door a stuck operator actually tries. The field report that filed it: three
-- concurrent cargo conducts starved on one target lock, and the operator — with the fix already
-- shipped — diagnosed via `ps` and dialed `--jobs 1`, because nothing they read named locks.

prova.test("the --jobs door teaches locks, not a smaller dial", {
  covers = "docs/design/verifiers.md#exclusive-conduct-resources",
  proves = "the operator who hits contention reaches for the parallelism dial first — if the dial's own help does not name the scalpel, the workaround (global -j 1) becomes the fix and files the feature as missing",
}, function(t)
  local dir = t:tempdir()
  local r = shell.run(prova.bin .. " --help", { cwd = dir, merge_stderr = true })
  t:expect(r.code):equals(0)
  -- The pointer sits ON the -j/--jobs entry: the cure named where the workaround lives.
  local jobs_entry = r.stdout:match("%-j, %-%-jobs.-\n%s*%-")
  t:expect(jobs_entry, "the jobs entry exists in --help"):is_truthy()
  t:expect(jobs_entry, "…and it names locks as the contention cure"):contains("locks")
  t:expect(jobs_entry, "…teaching where to learn them"):contains("prova learn locks")
end)

prova.test("the binary teaches locks: catalog, topic, and the skill's vocabulary", {
  covers = "docs/design/verifiers.md#exclusive-conduct-resources",
  proves = "a capability an agent cannot discover does not exist — the catalog must name the topic, the topic must teach the grammar (writes/reads/port), serial's run-scoped distinction, and the cross-instance hold, and the skill must speak the same vocabulary",
}, function(t)
  local dir = t:tempdir()

  local catalog = shell.run(prova.bin .. " learn", { cwd = dir, merge_stderr = true })
  t:expect(catalog.stdout, "the catalog names the topic"):contains("locks")

  local topic = shell.run(prova.bin .. " learn locks", { cwd = dir, merge_stderr = true })
  t:expect(topic.code):equals(0)
  t:expect(topic.stdout, "the writer hold"):contains("prova.writes(")
  t:expect(topic.stdout, "the concurrent hold"):contains("prova.reads(")
  t:expect(topic.stdout, "the canonical house rule"):contains('writes("cargo")')
  t:expect(topic.stdout, "serial's run-scoped distinction"):contains("serial = true")
  t:expect(topic.stdout, "the cross-instance reach"):contains("across prova instances")

  local skill = shell.run(prova.bin .. " skill", { cwd = dir, merge_stderr = true })
  t:expect(skill.stdout, "the skill speaks the grammar"):contains('prova.writes(')
  t:expect(skill.stdout, "…and routes to the topic"):contains("learn locks")
end)
