--- `prova learn` is a WIKI, not a pile of pages: every topic is reachable, and every pointer lands.
---
--- The teaching surface is how an agent discovers what prova can do. A page that names a topic
--- which does not exist sends it to `prova learn <nothing>`; a topic nothing points at is
--- discoverable only by reading the whole catalog, which is the thing progressive disclosure
--- exists to avoid. Neither failure is visible to the author of the page that caused it — the
--- link works when you write it and rots when someone else renames or removes a topic.
---
--- So the wiki's shape is asserted rather than maintained by care. Measured 2026-08-14, before
--- this proof existed: `verifiers` was a complete isolate (nothing linked to it, it linked to
--- nothing) and five topics were dead ends.
---
--- Everything here reads the SHIPPED surface through `prova.bin` — the topics are embedded in the
--- binary, so asking the binary is both the honest question and the only one that survives the
--- files moving. It is also the conductor-vs-subject rule: asking THIS tree's binary, never
--- whichever prova is conducting the suite.

local function learn(t, topic)
  local argv = { prova.bin, "learn" }
  if topic then argv[#argv + 1] = topic end
  local r = shell.run(argv, { merge_stderr = true, timeout = "30s" })
  t:expect(r.code, "`prova learn " .. (topic or "") .. "` exits clean: " .. r.stdout):equals(0)
  return r.stdout
end

--- The catalog's own list of topic names — the wiki's node set, as the binary reports it.
local function catalog(t)
  local names = {}
  for line in learn(t, nil):gmatch("[^\n]+") do
    -- Catalog rows are `  <name>   <summary>`; prose lines and the header are not indented pairs.
    local name = line:match("^  ([a-z][a-z%-]*)%s%s+%S")
    if name then names[#names + 1] = name end
  end
  return names
end

--- Every `prova learn <topic>` a page points at, excluding its own name.
local function outbound(body, self_name)
  local seen, list = {}, {}
  for ref in body:gmatch("prova learn ([a-z][a-z%-]*)") do
    if ref ~= self_name and not seen[ref] then
      seen[ref] = true
      list[#list + 1] = ref
    end
  end
  return list
end

local wiki = prova.fixture("learn-wiki", Scope.File, function(ctx)
  local t = ctx
  local names = catalog(t)
  local bodies, links = {}, {}
  for _, name in ipairs(names) do
    bodies[name] = learn(t, name)
    links[name] = outbound(bodies[name], name)
  end
  return { names = names, bodies = bodies, links = links }
end)

prova.test("the catalog is the whole node set — every listed topic renders", {
  proves = "a topic in the catalog that does not render is the worst kind of dead link, because the catalog is exactly where an agent goes when it does not know what to ask for",
}, function(t)
  local w = t:use(wiki)
  t:expect(#w.names, "the catalog lists a substantial surface"):gt(15)
  for _, name in ipairs(w.names) do
    t:expect(#w.bodies[name], name .. " renders something"):gt(200)
  end
end)

prova.test("every pointer lands: no topic names a topic that does not exist", {
  proves = "the link works when it is written and rots when someone else renames a topic, so the author who breaks it is never the author who sees it — only a standing assertion catches that",
}, function(t)
  local w = t:use(wiki)
  local known = {}
  for _, n in ipairs(w.names) do known[n] = true end

  local dangling = {}
  for _, name in ipairs(w.names) do
    for _, ref in ipairs(w.links[name]) do
      if not known[ref] then dangling[#dangling + 1] = name .. " -> " .. ref end
    end
  end
  t:expect(table.concat(dangling, ", "), "every `prova learn X` names a real topic"):equals("")
end)

prova.test("no topic is a dead end, and none is an orphan", {
  proves = "reachability is the whole difference between a wiki and a pile of pages: a dead end strands the agent that arrived, and an orphan can only be found by reading the entire catalog — which is what progressive disclosure exists to avoid (`verifiers` was both, until this proof)",
}, function(t)
  local w = t:use(wiki)

  local dead = {}
  for _, name in ipairs(w.names) do
    if #w.links[name] == 0 then dead[#dead + 1] = name end
  end
  t:expect(table.concat(dead, ", "), "every topic points somewhere"):equals("")

  local inbound = {}
  for _, name in ipairs(w.names) do inbound[name] = 0 end
  for _, name in ipairs(w.names) do
    for _, ref in ipairs(w.links[name]) do
      if inbound[ref] then inbound[ref] = inbound[ref] + 1 end
    end
  end
  local orphans = {}
  for _, name in ipairs(w.names) do
    if inbound[name] == 0 then orphans[#orphans + 1] = name end
  end
  t:expect(table.concat(orphans, ", "), "every topic is pointed AT by another"):equals("")
end)

prova.test("every topic ends with a See also, so the next hop is always in the same place", {
  proves = "an agent that must scan prose for the exit takes a different path on every page; a fixed trailing section is what makes traversal mechanical rather than a reading exercise",
}, function(t)
  local w = t:use(wiki)
  local missing = {}
  for _, name in ipairs(w.names) do
    if not w.bodies[name]:find("\nSee also:") then missing[#missing + 1] = name end
  end
  t:expect(table.concat(missing, ", "), "every topic carries a See also"):equals("")
end)

prova.test("the skill enumerates every matcher that is not an alias", {
  proves = "the matcher bullet claims to be a LIST, so a matcher missing from it is invisible to the agent that reads the skill and nowhere else — `is_false` and `is_truthy` were both absent while `is_true` and `is_falsy` were taught, which left the strict/loose pair half-told and pointed anyone wanting strict-false at `is_falsy`, where `nil` silently passes",
}, function(t)
  -- Both surfaces from the SUBJECT: `prova.help` inside `prova.bin eval`, and `prova.bin skill`.
  local listed = shell.run({ prova.bin, "eval", [[
local out = {}
for _, e in ipairs(prova.help("Matcher")) do
  local name = e.name:match("^Matcher:([A-Za-z_]+)$")
  -- An alias documents itself as one ("Alias for `equals`"), so the exemption is read from the
  -- surface rather than kept as a list here that would need its own maintenance.
  if name and not e.summary:match("^Alias for") then out[#out + 1] = name end
end
table.sort(out)
return table.concat(out, " ")
]] }, { merge_stderr = true, timeout = "30s" })
  t:expect(listed.code, "the subject enumerated its matchers: " .. listed.stdout):equals(0)

  local skill = shell.run({ prova.bin, "skill" }, { merge_stderr = true, timeout = "30s" })
  t:expect(skill.code, "the skill renders"):equals(0)

  local untaught = {}
  for name in listed.stdout:gmatch("[A-Za-z_]+") do
    -- A frontier pattern, because the skill lists names in runs inside ONE backtick span
    -- (`equals is is_nil …`) — and because `is_true` must not be satisfied by `is_truthy`
    -- appearing somewhere in the text, which a plain substring search would allow.
    if not skill.stdout:find("%f[%w_]" .. name .. "%f[^%w_]") then
      untaught[#untaught + 1] = name
    end
  end
  t:expect(table.concat(untaught, ", "), "every non-alias matcher is named in the skill"):equals("")
end)

prova.test("the skill's own pointers land too — it is the entry page", {
  proves = "`prova skill` is what an agent reads first and the densest set of pointers in the system, so a rotted link there costs more than one anywhere else",
}, function(t)
  local w = t:use(wiki)
  local known = {}
  for _, n in ipairs(w.names) do known[n] = true end

  local skill = shell.run({ prova.bin, "skill" }, { merge_stderr = true, timeout = "30s" })
  t:expect(skill.code, "the skill renders: " .. skill.stdout):equals(0)

  local dangling = {}
  for _, ref in ipairs(outbound(skill.stdout, nil)) do
    if not known[ref] then dangling[#dangling + 1] = ref end
  end
  t:expect(table.concat(dangling, ", "), "every topic the skill names exists"):equals("")
end)
