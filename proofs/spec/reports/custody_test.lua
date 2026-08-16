--- Reports: a conduct's ARTIFACT, taken into custody and made addressable
--- (docs/design/verifiers.md#reports-are-custody-not-visualization).
---
--- A deputed conduct hands back three things. Cases go to the ledger, measurements go to the
--- ratchets, and the deputy's own report — llvm-cov's HTML, a junit file — used to be dropped: it
--- landed under `target/`, which the sweep deletes, and nothing named it. So a coverage floor could
--- refuse a regression at 73.46% and be unable to show which lines moved, having had that answer in
--- hand. Measured cost of that gap: days.
---
--- Prova does not RENDER these — the boundary in verifiers.md stands, and this is the other side of
--- it. The deputy rendered the artifact; prova preserves it, summarizes it in one line, and hands
--- out paths. Custody, not visualization.
---
--- Black-box through `prova.bin`: custody is a property of a RUN (the file is copied while the run
--- is live, and the record is written at its end), so it can only be observed from outside one.

local PACKAGE = [[
  prova.test("publishes what it produced", function(t)
    local made = prova.root .. "/artifacts"
    fs.mkdir(made)
    fs.write(made .. "/cov.json", '{"lines":42}')
    fs.write(made .. "/index.html", "<html>coverage</html>")
    report.publish{
      name = "coverage",
      summary = "unit 73.47% · merged 86.37%",
      explains = "rust.coverage.unit",
      forms = { json = made .. "/cov.json", html = made .. "/index.html" },
    }
    t:expect(true):is_true()
  end)
]]

local function package(t)
  local dir = t:tempdir("reports-pkg")
  fs.write(dir .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')
  fs.mkdir(dir .. "/proofs")
  fs.write(dir .. "/proofs/publish_test.lua", PACKAGE)
  return dir
end

local function run(dir, ...)
  return shell.run({ prova.bin, ... }, { cwd = dir, merge_stderr = true })
end

prova.test("a published artifact outlives the conduct that produced it", {
  covers = "docs/design/verifiers.md#reports-are-custody-not-visualization",
  proves = "the artifact used to be left where the deputy wrote it — under target/, which the sweep deletes — so the evidence behind a number was routinely gone by the time anyone wanted it. A recorded path that rots is worse than no report, because it reads as available",
}, function(t)
  local dir = package(t)
  local r = run(dir)
  t:expect(r.code, r.stdout):equals(0)

  -- Custody is under the package's own generated state, not wherever the conduct happened to write.
  local filed = dir .. "/.prova/var/reports/coverage/cov.json"
  t:expect(filed, "the artifact was copied into custody"):exists()
  t:expect(fs.read(filed)):equals('{"lines":42}')

  -- The proof of custody rather than reference: destroy what produced it, and the report stands.
  fs.remove_all(dir .. "/artifacts")
  t:expect(filed, "…and survives the conduct's own output being swept"):exists()
  -- Compared by tail: the custody root is CANONICAL (macOS resolves /var → /private/var), which is
  -- correct and not what this proof is about.
  local addressed = run(dir, "reports", "coverage", "--kind", "json").stdout:gsub("%s+$", "")
  t:expect(addressed, "the verb addresses the filed copy"):contains("/.prova/var/reports/coverage/cov.json")
  t:expect(addressed, "…and it is a real file"):exists()
end)

prova.test("the run announces its reports, and the record carries them", {
  covers = "docs/design/verifiers.md#reports-are-custody-not-visualization",
  proves = "an artifact nobody knows exists is one nobody reads — the recap line is the same argument as `switched off:`. The record is the agent's half of it: same facts, no console parsing",
}, function(t)
  local dir = package(t)
  local r = run(dir)
  t:expect(r.stdout, "the recap names what was published"):contains("reports: coverage")

  local record = json.decode(fs.read(dir .. "/.prova/var/last-run.json"))
  t:expect(#record.reports, "one report on the record"):equals(1)
  local row = record.reports[1]
  t:expect(row.name):equals("coverage")
  t:expect(row.summary, "the one line prova itself renders"):contains("unit 73.47%")
  t:expect(row.explains, "the measurements this artifact explains"):equals({ "rust.coverage.unit" })
  -- Both forms, so each reader takes the one that suits it. This is the property that makes the
  -- surface equally useful to a person and to an agent — one publish, two audiences.
  t:expect(row.forms.json):contains("/.prova/var/reports/coverage/cov.json")
  t:expect(row.forms.html):contains("/.prova/var/reports/coverage/index.html")
end)

prova.test("reports LIST for discovery and ADDRESS for use", {
  covers = "docs/design/verifiers.md#reports-are-custody-not-visualization",
  proves = "discovery and addressing are different needs: a reader who does not know what exists needs the index, and a reader who does needs a path that composes. `--kind` prints the path ALONE so `open $(prova reports coverage --kind html)` is the whole viewing story — no platform-specific opener belongs in prova",
}, function(t)
  local dir = package(t)
  run(dir)

  -- List: the discovery mode. Names, the one-line summary, and which forms exist.
  local listed = run(dir, "reports")
  t:expect(listed.code):equals(0)
  t:expect(listed.stdout):contains("coverage")
  t:expect(listed.stdout, "the gist without opening anything"):contains("unit 73.47%")
  t:expect(listed.stdout, "…and what it can be read as"):contains("html, json")

  -- Address: one report, every form with its path.
  local one = run(dir, "reports", "coverage")
  t:expect(one.code):equals(0)
  t:expect(one.stdout):contains("evidence for: rust.coverage.unit")
  t:expect(one.stdout):contains("json")
  t:expect(one.stdout):contains("html")

  -- A form: the path and nothing else, so it composes into a shell substitution.
  local path = run(dir, "reports", "coverage", "--kind", "html")
  t:expect(path.code):equals(0)
  local only = path.stdout:gsub("%s+$", "")
  t:expect(only:find("\n") == nil, "stdout is ONE line — nothing to strip before using it"):is_true()
  t:expect(only, "…and that line is the artifact's path"):contains("/.prova/var/reports/coverage/index.html")
  t:expect(only, "which resolves"):exists()
end)

prova.test("asking for what is not there says what is", {
  covers = "docs/design/verifiers.md#reports-are-custody-not-visualization",
  proves = "the commonest reason to be here is a half-remembered name, so the refusal that lists what DOES exist is the difference between one command and a hunt",
}, function(t)
  local dir = package(t)
  run(dir)

  local missing = run(dir, "reports", "covrage")
  t:expect(missing.code):never():equals(0)
  t:expect(missing.stdout, "names what is actually published"):contains("coverage")

  local wrong_kind = run(dir, "reports", "coverage", "--kind", "pdf")
  t:expect(wrong_kind.code):never():equals(0)
  t:expect(wrong_kind.stdout, "names the forms it does come in"):contains("html, json")

  -- A form is a form OF something: without a name the answer would be ambiguous the moment a
  -- second report exists, so it is refused rather than guessed.
  local dangling = run(dir, "reports", "--kind", "json")
  t:expect(dangling.code):never():equals(0)
  t:expect(dangling.stdout):contains("name it")
end)

prova.test("a run that publishes nothing says nothing", {
  covers = "docs/design/verifiers.md#reports-are-custody-not-visualization",
  proves = "a recap line reading `reports: none` on every ordinary run is noise that trains the eye to skip the line — and the line only earns its place by being rare",
}, function(t)
  local dir = t:tempdir("quiet-pkg")
  fs.write(dir .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')
  fs.mkdir(dir .. "/proofs")
  fs.write(dir .. "/proofs/quiet_test.lua",
    'prova.test("nothing published", function(t) t:expect(1):equals(1) end)\n')

  local r = run(dir)
  t:expect(r.code):equals(0)
  t:expect(r.stdout, "silent when there is nothing to announce"):never():contains("reports:")
  t:expect(run(dir, "reports").stdout, "and the verb says so plainly"):contains("no reports")
end)
