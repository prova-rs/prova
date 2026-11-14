-- prova.cargo — the Cargo module adapter (docs/plans/module-proofs.md).
--
-- A change runs exactly the tests it can reach: each workspace member is a module, its key is the
-- digest of its build CLOSURE (its own directory and those of the local crates it depends on, the
-- files rustc's dep-info says it embeds, the test-only reads `[modules] reads` names, the lock file
-- and toolchain), and its account is CACHED by that key once a conduct passes every one of its
-- cases. The first `verify`/`account` of a run conducts every stale module in ONE nextest over one
-- workspace build; every other module's account is served from the cache, each case marked with
-- the run that executed it. Proofs always execute: only the conduct is cached.
--
--   local cargo = require("prova.cargo")
--   cargo.configure{ profile = "prova", args = { "--features", "x/y" } }   -- once, optional
--   prova.test("scm's unit cases pass", { switch = "ut" }, function(t) cargo.verify(t) end)
--
-- A leaf that may conduct holds the token the conduct contends on: `locks = { prova.writes("cargo") }`.

local cargo = {}

local settings = {
  profile = "default", -- the nextest profile; its junit must be enabled ([profile.<p>.junit] path)
  junit = "junit.xml", -- that profile's junit file name, under <target>/nextest/<profile>/
  args = {},           -- extra `cargo nextest run` arguments (features, --test-threads, …)
  run = {},            -- extra `shell.run` options for the conduct: `env`, `sample`, `idle_timeout`
}

--- Set the conduct's nextest profile, junit file name and extra arguments. Call it where the
--- package's proofs share setup (a suite file or a local package): every state that may conduct
--- must agree, since whichever state asks first conducts for the run.
function cargo.configure(opts)
  for k, v in pairs(opts or {}) do
    if settings[k] == nil then
      error("prova.cargo.configure: unknown option " .. tostring(k) .. " (profile, junit, args, run)", 2)
    end
    settings[k] = v
  end
end

local function run(argv)
  return shell.run(argv, { cwd = prova.root, merge_stderr = false, idle_timeout = "600s" })
end

local function tail(text, n)
  local lines = {}
  for l in (text or ""):gmatch("[^\n]+") do lines[#lines + 1] = l end
  return table.concat(lines, "\n", math.max(1, #lines - (n or 40) + 1))
end

-- `a/b/../c` as `a/c`: rustc writes an embedded file as its crate wrote it
-- (`c/src/../../data/x.txt`), which read raw looks like a file inside `c`.
local function normalize(path)
  local out = {}
  for part in path:gmatch("[^/]+") do
    if part == ".." then
      out[#out] = nil
    elseif part ~= "." then
      out[#out + 1] = part
    end
  end
  return (path:sub(1, 1) == "/" and "/" or "") .. table.concat(out, "/")
end

local function rel(path)
  path = normalize(path)
  local root = prova.root .. "/"
  return path:sub(1, #root) == root and path:sub(#root + 1) or nil
end

local function dirname(path)
  return path:match("^(.*)/[^/]*$") or path
end

-- Every file a target's dep-info lists: what rustc read to build it, `include_str!`/
-- `include_bytes!` and build-script `rerun-if-changed` paths included. Both of cargo's layouts:
-- `<build>/debug/deps/<crate>-<hash>.d` and the newer `<build>/debug/build/<pkg>/<hash>/out/…d`.
-- A relative entry is relative to the workspace root, where cargo ran rustc.
local function dep_info(build_dir, workspace_root)
  local by_crate = {}
  local debug = build_dir .. "/debug"
  if not fs.exists(debug) then return by_crate end
  local files = {}
  for _, pat in ipairs({ "deps/*.d", "build/**/*.d" }) do
    for _, d in ipairs(fs.glob(debug, pat)) do files[#files + 1] = d end
  end
  for _, d in ipairs(files) do
    local crate = d:match("([^/]+)%-%x+%.d$")
    if crate then
      crate = crate:gsub("^lib", "")
      local text = fs.read(d)
      local first = text:match("^[^\n]*") or ""
      local list = first:match("^.-[^\\]:%s(.*)$") or ""
      list = list:gsub("\\ ", "\0")
      local entries = by_crate[crate] or {}
      for f in list:gmatch("%S+") do
        f = f:gsub("%z", " ")
        if f:sub(1, 1) ~= "/" then f = workspace_root .. "/" .. f end
        entries[#entries + 1] = f
      end
      by_crate[crate] = entries
    end
  end
  return by_crate
end

local GLOBAL_INPUTS = { "Cargo.lock", "Cargo.toml", "rust-toolchain", "rust-toolchain.toml",
  ".cargo/config.toml", ".config/nextest.toml" }

-- The graph: every workspace member, its directory, and its closure's paths. `nil, why` when the
-- graph cannot be read (docs/plans/module-proofs.md#honesty-rules-beside-phase-2s): the caller
-- then conducts every module, and says so.
local function graph()
  local meta = run({ "cargo", "metadata", "--format-version", "1", "--locked", "--offline" })
  if meta.code ~= 0 then
    -- cargo's own `error:` line first, so whoever keys on a failure's first line reads cargo's words.
    local said = meta.stderr or meta.stdout or ""
    local headline = said:match("[^\n]*error:[^\n]*") or said:match("[^\n]+") or "no output"
    return nil, "cargo metadata failed: " .. headline .. "\n" .. tail(said, 8)
  end
  local m = json.decode(meta.stdout)
  local pkgs, local_ids = {}, {}
  for _, p in ipairs(m.packages) do
    pkgs[p.id] = p
    if p.source == nil or p.source == json.null then local_ids[p.id] = true end
  end
  local nodes = {}
  for _, n in ipairs((m.resolve or {}).nodes or {}) do nodes[n.id] = n end
  local build_dir = (m.build_directory ~= json.null and m.build_directory) or m.target_directory
  local infos = dep_info(build_dir, m.workspace_root)
  local function crates_of(p, all_targets)
    local out = {}
    for _, t in ipairs(p.targets) do
      local kinds = table.concat(t.kind, ",")
      if all_targets or kinds:match("lib") or kinds:match("custom%-build") or kinds:match("proc%-macro") then
        out[#out + 1] = (t.name:gsub("%-", "_"))
      end
    end
    return out
  end
  local reads = prova.modules.reads or {}
  local modules = {}
  for _, id in ipairs(m.workspace_members) do
    local p = pkgs[id]
    -- The closure: the member with every edge kind (its tests compile its dev-dependencies), then
    -- the crates those reach through normal and build edges only.
    local seen, order, queue = { [id] = true }, { id }, { { id = id, root = true } }
    while #queue > 0 do
      local at = table.remove(queue, 1)
      for _, d in ipairs((nodes[at.id] or {}).deps or {}) do
        local follow = at.root
        for _, k in ipairs(d.dep_kinds or {}) do
          if k.kind == nil or k.kind == json.null or k.kind == "build" then follow = true end
        end
        if follow and not seen[d.pkg] and local_ids[d.pkg] then
          seen[d.pkg] = true
          order[#order + 1] = d.pkg
          queue[#queue + 1] = { id = d.pkg, root = false }
        end
      end
    end
    local paths, dirs = {}, {}
    for _, cid in ipairs(order) do
      local dir = rel(dirname(pkgs[cid].manifest_path))
      if dir then dirs[#dirs + 1] = dir; paths[#paths + 1] = dir end
    end
    local function inside_a_dir(r)
      for _, dir in ipairs(dirs) do
        if r == dir or r:sub(1, #dir + 1) == dir .. "/" then return true end
      end
      return false
    end
    local target_rel = rel(m.target_directory)
    for _, cid in ipairs(order) do
      for _, crate in ipairs(crates_of(pkgs[cid], cid == id)) do
        for _, f in ipairs(infos[crate] or {}) do
          local r = rel(f)
          if r and not inside_a_dir(r) and not (target_rel and r:sub(1, #target_rel + 1) == target_rel .. "/") then
            paths[#paths + 1] = r
          end
        end
      end
    end
    for _, r in ipairs(reads[p.name] or {}) do paths[#paths + 1] = r end
    for _, g in ipairs(GLOBAL_INPUTS) do paths[#paths + 1] = g end
    modules[p.name] = { dir = rel(dirname(p.manifest_path)) or ".", paths = paths }
  end
  return { modules = modules, target_dir = m.target_directory, workspace_root = m.workspace_root }
end

local function toolchain()
  local r = run({ "rustc", "-vV" })
  return r.code == 0 and r.stdout or "rustc unknown"
end

-- Where nextest writes its junit: its STORE, which is NOT the cargo target directory. nextest's
-- `store.dir` (default `target/nextest`) is relative to the WORKSPACE root, so a build under a
-- `CARGO_TARGET_DIR` elsewhere (Herald's gate) still writes its junit beside the workspace. A
-- `[store] dir` in .config/nextest.toml wins, absolute or workspace-relative.
local function nextest_store(workspace_root)
  local cfg = workspace_root .. "/.config/nextest.toml"
  local text = fs.exists(cfg) and fs.read(cfg) or ""
  local dir = ("\n" .. text):match("\n%[store%][^%[]-\n%s*dir%s*=%s*\"([^\"]+)\"")
  if not dir then return workspace_root .. "/target/nextest" end
  return dir:sub(1, 1) == "/" and dir or (workspace_root .. "/" .. dir)
end

local function nextest_filter(names)
  local parts = {}
  for _, n in ipairs(names) do parts[#parts + 1] = "package(=" .. n .. ")" end
  return table.concat(parts, " | ")
end

-- THE run's accounts: one per module, conducted or served. Run-scoped, so every suite on every
-- worker shares one graph read and at most one conduct. The value is plain data (paths).
cargo.accounts = prova.fixture("prova.cargo.accounts", Scope.Run, function()
  local g, off = graph()
  -- A run-scoped value outlives every Lua state, so its files live in the tree: under the build's
  -- own target directory, one directory per run (a lane and Herald never share a target).
  local scratch = (g and g.target_dir or (prova.root .. "/target")) .. "/prova-cargo"
  fs.remove_all(scratch)
  fs.mkdir(scratch)
  local tool = toolchain()
  local conduct_key = settings.profile .. "\0" .. table.concat(settings.args, "\0")
  local out = { modules = {}, by_dir = {}, off = off, conducted = {}, served = {} }
  local stale = {}
  -- The modules are the members with their own proofs/ (`prova.modules.of`): only they are this
  -- adapter's to conduct. A workspace that keeps a whole-workspace deputy beside a few modules
  -- must not have that workspace conducted twice.
  local has_proofs = {}
  for _, dir in pairs((prova.modules or {}).of or {}) do has_proofs[dir] = true end
  if g then
    for name, mod in pairs(g.modules) do
      out.by_dir[mod.dir] = name
    end
    for name, mod in pairs(g.modules) do
      if not has_proofs[mod.dir] then goto continue end
      local key = prova.conduct_cache.key({ "cargo", name, tool, conduct_key }, mod.paths)
      local filed = prova.conduct_cache.lookup(key)
      if filed then
        local stamped = scratch .. "/" .. name .. ".served.xml"
        junit.merge(filed.files.junit, nil, { reused_from = filed.run_id, out = stamped })
        out.modules[name] = { account = stamped, reused_from = filed.run_id }
        out.served[#out.served + 1] = name
      else
        stale[#stale + 1] = { name = name, key = key }
      end
      ::continue::
    end
  end
  table.sort(out.served)
  table.sort(stale, function(a, b) return a.name < b.name end)
  if off then
    io.stderr:write("prova.cargo: selection off, conducting every module: " .. off .. "\n")
  end
  if off or #stale > 0 then
    local argv = { "cargo", "nextest", "run", "--workspace", "--no-fail-fast", "--profile", settings.profile }
    if not off then
      local names = {}
      for _, s in ipairs(stale) do names[#names + 1] = s.name end
      argv[#argv + 1] = "-E"
      argv[#argv + 1] = nextest_filter(names)
    end
    for _, a in ipairs(settings.args) do argv[#argv + 1] = a end
    local artifact = nextest_store(g and g.workspace_root or prova.root) .. "/" .. settings.profile .. "/" .. settings.junit
    fs.remove_all(artifact)
    local opts = { cwd = prova.root, merge_stderr = true, idle_timeout = "600s" }
    for k, v in pairs(settings.run) do opts[k] = v end
    local ran = shell.run(argv, opts)
    if not fs.exists(artifact) then
      error(string.format("prova.cargo: nextest exited %s and wrote no junit at %s (is junit enabled "
        .. "for profile %q?); the last of its output:\n%s", tostring(ran.code), artifact,
        settings.profile, tail(ran.stdout, 60)), 0)
    end
    local whole = scratch .. "/conducted.xml"
    fs.write(whole, fs.read(artifact))
    out.whole = whole
    -- File under the key the NEXT run computes: after this build, whose dep-info now names what
    -- each target embeds (a fresh target directory had none before it).
    local after = graph()
    for _, s in ipairs(stale) do
      local mod = after and after.modules[s.name]
      if mod then s.key = prova.conduct_cache.key({ "cargo", s.name, tool, conduct_key }, mod.paths) end
    end
    for _, s in ipairs(stale) do
      local account = scratch .. "/" .. s.name .. ".xml"
      local n = junit.select(whole, { suites = { s.name }, out = account })
      out.modules[s.name] = { account = account }
      out.conducted[#out.conducted + 1] = s.name
      -- Filed only when every case ran and passed: a red, a skip or an empty account is never
      -- carried (an unfileable run says why on stderr, once, and files nothing).
      local report = junit.load(account)
      if n > 0 and report.passed == report.total then
        local ok, err = pcall(prova.conduct_cache.file, s.key, { junit = account })
        if not ok and not out.unfiled then
          out.unfiled = tostring(err)
          io.stderr:write("prova.cargo: " .. out.unfiled .. "\n")
        end
      end
    end
  end
  io.stderr:write(string.format("prova.cargo: %d module(s) conducted, %d served from the conduct cache\n",
    #out.conducted, #out.served))
  return out
end)

local function module_name(t, accounts)
  local m = t.module
  if not m then
    error("prova.cargo: this proof is in no module's proofs/ (it is in the root's): name one, "
      .. "cargo.verify(t, { module = \"<package>\" })", 3)
  end
  local name = accounts.by_dir[m.dir]
  if not name and accounts.off then
    -- The graph could not be read, so no directory is a member: say why, never blame the module.
    -- An infra cause, by value (prova.infra): the environment can't read the workspace, so the gate
    -- must not bisect it or call it flaky (E m/11669).
    prova.infra("prova.cargo: " .. accounts.off .. "\n(so no workspace member could be read, and " .. m.dir
      .. "'s account is unknown)")
  end
  if not name then
    error("prova.cargo: " .. m.dir .. " is no workspace member's directory", 3)
  end
  return name
end

--- The account of `name` (default: this proof's module) — a junit path, for `junit.load` readers
--- that bind claims to named cases.
function cargo.account(t, name)
  local accounts = t:use(cargo.accounts)
  name = name or module_name(t, accounts)
  local a = accounts.modules[name]
  if not a then
    error("prova.cargo: no account for " .. name .. (accounts.off and (" (" .. accounts.off .. ")") or ""), 2)
  end
  return a.account
end

--- Adopt `opts.module`'s (default: this proof's module's) account: every case ingested, red on any
--- failed one, red on an empty one. Returns the junit report.
function cargo.verify(t, opts)
  opts = opts or {}
  return junit.verify(t, { results = cargo.account(t, opts.module) })
end

return cargo
