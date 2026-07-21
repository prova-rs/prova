--- THE PROOF FOR `prova learn` — written before the implementation existed (Proof-Driven
--- Development, applied to Prova itself; docs/plans/autodidact.md M1). Black-box: invoke the real
--- binary and hold the progressive-disclosure catalog to its contract:
---
---   * `prova learn` lists the topic catalog: `topic  one-line hook` rows, exit 0
---   * `prova learn <topic>` prints that topic, exit 0
---   * aliases resolve (`mocks` → `doubles`) — an intuitive name never bounces off our taxonomy
---   * an unknown topic is a usage error: exit 2, and the listing is shown so the next call works
---   * topics with dynamic slots render THIS project's facts when run inside one, and degrade to
---     an imperative pointer (`prova init`) when there is no manifest
---
--- The launcher (tests/selftest.rs) sets PROVA_BIN and PROVA_FIXTURES.

local prova_bin = assert(os.getenv("PROVA_BIN"), "PROVA_BIN not set")
local fixtures = assert(os.getenv("PROVA_FIXTURES"), "PROVA_FIXTURES not set")

local function learn(args, opts)
  return shell.run(prova_bin .. " learn " .. (args or ""), opts)
end

prova.group("prova learn", function(g)
  g:test("no args lists the topic catalog with one-line hooks", function(t)
    local r = learn("")
    t:expect(r.code):equals(0)
    -- The seed topics, each present as a listing row.
    t:expect(r.stdout):contains("pdd")
    t:expect(r.stdout):contains("project")
    t:expect(r.stdout):contains("init")
    t:expect(r.stdout):contains("doubles")
  end)

  g:test("a topic prints doctrine in the agent register", function(t)
    local r = learn("pdd")
    t:expect(r.code):equals(0)
    -- The one-line thesis, and the iteration verb an agent must know.
    t:expect(r.stdout):contains("proof")
    t:expect(r.stdout):contains("--last-failed")
  end)

  g:test("aliases resolve: `mocks` and `containers` are the doubles topic", function(t)
    local canonical = learn("doubles")
    t:expect(canonical.code):equals(0)
    for _, alias in ipairs({ "mocks", "containers" }) do
      local r = learn(alias)
      t:expect(r.code, alias .. " resolves"):equals(0)
      t:expect(r.stdout, alias .. " prints the doubles topic"):equals(canonical.stdout)
    end
  end)

  g:test("an unknown topic is a usage error that shows the catalog", function(t)
    local r = learn("definitely-not-a-topic")
    t:expect(r.code):equals(2)
    t:expect(r.stderr):contains("definitely-not-a-topic")
    -- The listing rides along so the agent's NEXT call is right.
    t:expect(r.stderr):contains("pdd")
  end)

  g:test("doubles teaches the shipped mocking surface", function(t)
    local r = learn("doubles")
    t:expect(r.code):equals(0)
    t:expect(r.stdout):contains("http.mock")
  end)

  g:test("the full taxonomy is served, one screen each", function(t)
    local listing = learn("")
    for _, topic in ipairs({ "pdd", "project", "init", "authoring", "fixtures", "doubles",
                             "proxies", "drivers", "topologies", "plugins", "plugin-authoring",
                             "running", "mcp" }) do
      t:expect(listing.stdout, "listed: " .. topic):contains(topic)
      local r = learn(topic)
      t:expect(r.code, topic .. " renders"):equals(0)
      t:expect(#prova.parse.lines(r.stdout), topic .. " stays one-screen-ish"):lte(90)
    end
  end)

  g:test("unshipped surface is taught as unshipped", function(t)
    -- The proxies topic exists to PREVENT reaching for vapor — its first lines must say so.
    local r = learn("proxies")
    t:expect(r.code):equals(0)
    t:expect(r.stdout):contains("not a shipped surface")
  end)

  g:test("the three-postures cross-references hold", function(t)
    t:expect(learn("drivers").stdout):contains("doubles")
    t:expect(learn("topologies").stdout):contains("ctx.network")
    t:expect(learn("mcp").stdout):contains("up")
  end)

  g:test("init renders the archetype catalog dynamically", function(t)
    local r = learn("init")
    t:expect(r.code):equals(0)
    -- The built-in catalog entries surface in the topic — computed, not hand-written.
    t:expect(r.stdout):contains("default")
    t:expect(r.stdout):contains("plugin")
  end)

  g:test("project renders THIS package's facts inside a package", function(t)
    -- fixtures/mcp-project declares `proofs = ["tests"]` — the rendered topic must say so.
    local r = learn("project", { cwd = fixtures .. "/mcp-project" })
    t:expect(r.code):equals(0)
    t:expect(r.stdout):contains("tests")
    t:expect(r.stdout):contains("prova.toml")
  end)

  g:test("project degrades imperatively with no manifest in reach", function(t)
    local r = learn("project", { cwd = fs.tempdir() })
    t:expect(r.code):equals(0)
    t:expect(r.stdout):contains("prova init")
  end)
end)

prova.group("prova skill routes to learn", function(g)
  g:test("the skill teaches the discovery moves", function(t)
    local r = shell.run(prova_bin .. " skill")
    t:expect(r.code):equals(0)
    -- The entry point must route: depth lives behind `prova learn`, and the skill says so.
    t:expect(r.stdout):contains("prova learn")
  end)

  g:test("the skill's manifest facts are current", function(t)
    local r = shell.run(prova_bin .. " skill")
    -- Regression pin: the skill once said `[run] paths`; the manifest key is `proofs`.
    t:expect(r.stdout):contains("[run] proofs")
    t:expect(r.stdout):never():contains("[run] paths")
  end)
end)
