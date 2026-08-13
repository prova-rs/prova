--- The collect/runtime boundary (docs/design/agent-ergonomics.md#collect-time-shell-panics-raw):
--- a runtime-only surface reached from a proof file's TOP LEVEL teaches, and what it teaches is
--- where the work belongs plus what collect time does have.
---
--- Two failure shapes made this worth a claim, both observed: unwrapped, the tokio panic aborted
--- the run mid-collect (no report, no plan, an internal-error trace); wrapped in `pcall`, the same
--- panic was CAUGHT and the run reported green with a reactor panic printed above it.
---
--- Every case drives the SUBJECT — the sandbox is run by `prova.bin`.

local scaffold = require("scaffold")

local function ran(t, top_level)
  local proj = scaffold.package(t, {
    proofs = {
      ["boundary_test.lua"] = top_level .. '\nprova.test("t", function(t) t:expect(true):is_true() end)\n',
    },
  })
  return shell.run(prova.bin, { cwd = proj, merge_stderr = true, timeout = "60s" })
end

prova.test("collect-time shell.run is refused with the boundary named, not a reactor panic", {
  covers = "docs/design/agent-ergonomics.md#collect-time-shell-panics-raw",
  proves = "the raw panic ('there is no reactor running, must be called from the context of a Tokio 1.x runtime') reads as a prova bug and sends the author to prova's source, not to the two-line fix in their own file",
}, function(t)
  local r = ran(t, 'local out = shell.run("echo hi")')
  t:expect(r.code, "the run refuses"):never():equals(0)
  t:expect(r.stdout, "no tokio internals leak"):never():contains("no reactor running")
  t:expect(r.stdout, "the surface is named"):contains("shell.run")
  t:expect(r.stdout, "…the phase is named"):contains("COLLECT time")
  t:expect(r.stdout, "…and where the work belongs"):contains("fixture")
end)

prova.test("the error names what collect time DOES have — a boundary without an alternative is a wall", {
  covers = "docs/design/agent-ergonomics.md#collect-time-shell-panics-raw",
  proves = "the author's actual goal is discovering parameterization inputs at plan time, which fs/toml serve; 'not here' alone leaves them believing prova cannot do it at all",
}, function(t)
  local r = ran(t, 'local out = shell.run("echo hi")')
  t:expect(r.stdout, "pure reads are named as the collect-time toolkit"):contains("fs")
end)

prova.test("every runtime-only surface holds the same line — one boundary, not one per module", {
  covers = "docs/design/agent-ergonomics.md#collect-time-shell-panics-raw",
  proves = "the reactor is what is missing, so EVERY async surface panics identically — shell was merely the one an author reached first; a per-module fix leaves the next author the same trap with a different name",
}, function(t)
  local surfaces = {
    { code = 'local p = shell.spawn("sleep 1")', names = "shell.spawn" },
    { code = 'local r = http.get("http://127.0.0.1:1/")', names = "http.get" },
  }
  for _, s in ipairs(surfaces) do
    local r = ran(t, s.code)
    t:expect(r.code, s.names .. " is refused"):never():equals(0)
    t:expect(r.stdout, s.names .. " leaks no tokio internals"):never():contains("no reactor running")
    t:expect(r.stdout, s.names .. " names itself"):contains(s.names)
  end
end)

prova.test("a pcall'd boundary error cannot pass for a green run", {
  covers = "docs/design/agent-ergonomics.md#collect-time-shell-panics-raw",
  proves = "the shape that shipped: mlua converted the panic into a catchable error, so a file that wrapped its discovery in pcall printed a reactor panic to stderr and still reported 0 — the worst outcome, because the operator has a green suite and a panic in the log",
}, function(t)
  local r = ran(t, 'local ok, err = pcall(function() return shell.run("echo hi") end)\nprint("caught=" .. tostring(err))')
  -- A pcall'd refusal is the author's own choice to continue, so the run may pass — what must NOT
  -- survive is the panic text, and the error they caught must be the teaching one.
  t:expect(r.stdout, "the caught error is the teaching one"):contains("runtime-only")
  t:expect(r.stdout, "…and carries no tokio internals"):never():contains("no reactor running")
end)

prova.test("every async module entry carries the boundary guard — the next module inherits it", {
  covers = "docs/design/agent-ergonomics.md#collect-time-shell-panics-raw",
  proves = "the guard is per-entry, so the gap a new module leaves is invisible until an author finds it the way this one was found: by reading a tokio panic. A presence fact over the module sources fails the suite instead — asserted as presence, never as a count, because counts rot within hours (§11)",
}, function(t)
  local unguarded = {}
  for _, file in ipairs(fs.glob(prova.root .. "/crates/prova-core/src/modules", "*.rs")) do
    local lines = {}
    for line in (fs.read(file) .. "\n"):gmatch("([^\n]*)\n") do
      lines[#lines + 1] = line
    end
    for i, line in ipairs(lines) do
      if line:find("create_async_function", 1, true) then
        -- The guard is the first thing inside the future (a few lines of argument parsing may
        -- precede it in the surrounding sync closure).
        local guarded = false
        for j = i, math.min(i + 12, #lines) do
          if lines[j]:find("runtime_only(", 1, true) then guarded = true; break end
        end
        if not guarded then
          unguarded[#unguarded + 1] = file:match("([^/\\]+)$") .. ":" .. i
        end
      end
    end
  end
  t:expect(table.concat(unguarded, ", "), "every async entry states the collect/runtime boundary"):equals("")
end)

prova.test("the runtime surfaces still work where they belong — in a body and in a fixture", {
  covers = "docs/design/agent-ergonomics.md#collect-time-shell-panics-raw",
  proves = "a phase gate is one typo away from refusing the legitimate 99% of calls: the negative control that keeps the boundary a boundary instead of a ban",
}, function(t)
  local proj = scaffold.package(t, {
    proofs = {
      ["works_test.lua"] = [[
local tool = prova.fixture("tool", Scope.File, function(ctx)
  return shell.run("echo from-a-fixture").stdout
end)

prova.test("a body may conduct", function(t)
  t:expect(shell.run("echo from-a-body").stdout):contains("from-a-body")
end)

prova.test("a fixture may conduct", function(t)
  t:expect(t:use(tool)):contains("from-a-fixture")
end)
]],
    },
  })
  local r = shell.run(prova.bin, { cwd = proj, merge_stderr = true, timeout = "60s" })
  t:expect(r.code, r.stdout):equals(0)
  t:expect(r.stdout):contains("2 passed")
end)
