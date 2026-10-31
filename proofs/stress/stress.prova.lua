--- `prova stress`, black-box: N isolated copies of each nextest case, beside burners killed by PID.
---
--- The contract, in one line: **every selected case runs as its own process, N copies, each with
--- its own short TMPDIR and process group (reaped when it ends); burners start and die by PID —
--- also on SIGTERM; a case whose copies disagree is FLAKY in the ledger; any failed copy exits 1;
--- and a stress run writes no `last-run.json`.**
---
--- No cargo here: a saved listing (`--listing`) names a shell script as the "test binary", which
--- reads `--exact <name>` the way a libtest binary does.

local FAKE = [[#!/bin/sh
# A libtest-shaped binary: argv is `--exact <name> --nocapture --test-threads=1`.
name="$2"
case "$name" in
  tests::ok) exit 0 ;;
  tests::red)
    echo "thread 'tests::red' panicked at src/lib.rs:1:1:"
    echo "always red"
    exit 101 ;;
  tests::flaky)
    # The first copy to claim the marker fails; every other copy passes.
    if mkdir "$STRESS_STATE/flaky.claimed" 2>/dev/null; then
      echo "thread 'tests::flaky' panicked at src/lib.rs:2:2:"
      echo "lost the race"
      exit 101
    fi
    exit 0 ;;
  tests::tmp)
    printf '%s\n' "$TMPDIR" >> "$STRESS_STATE/tmps"
    exit 0 ;;
  tests::daemon)
    # Leaves a child behind in its process group — stress must reap it.
    sleep 300 &
    echo $! >> "$STRESS_STATE/daemons"
    exit 0 ;;
  tests::slow)
    echo $$ >> "$STRESS_STATE/slow"
    exec sleep 300 ;;
esac
exit 3
]]

--- A package, the fake binary, and a listing that offers `names` (plus an ignored and a
--- quarantined case, which a default stress must not run).
local function sandbox(t, names)
  local root = t:tempdir()
  local pkg, state = root .. "/pkg", root .. "/state"
  fs.mkdir(pkg)
  fs.mkdir(state)
  fs.write(pkg .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')
  local bin = root .. "/fake-test"
  fs.write(bin, FAKE)
  shell.run({ "chmod", "+x", bin }, { check = true })
  local tcs = {
    ["tests::ignored"] = { ignored = true, ["filter-match"] = { status = "matches" } },
    ["tests::quarantined"] = { ignored = false, ["filter-match"] = { status = "mismatch", reason = "default-filter" } },
  }
  for _, n in ipairs(names) do
    tcs[n] = { ignored = false, ["filter-match"] = { status = "matches" } }
  end
  fs.write(root .. "/listing.json", json.encode({
    ["rust-suites"] = { ["fake::bin"] = { ["binary-path"] = bin, cwd = root, testcases = tcs } },
  }))
  return { pkg = pkg, state = state, listing = root .. "/listing.json" }
end

local function stress(sb, args)
  return shell.run(prova.bin .. " stress --listing " .. sb.listing .. " " .. args, {
    cwd = sb.pkg,
    env = { PROVA_VAR_DIR = "", STRESS_STATE = sb.state },
    merge_stderr = true,
    timeout = "120s",
  })
end

local function lines(path)
  local out = {}
  if fs.exists(path) then
    for l in fs.read(path):gmatch("[^\n]+") do
      out[#out + 1] = l
    end
  end
  return out
end

local function alive(pid)
  return shell.run("kill -0 " .. pid .. " 2>/dev/null").code == 0
end

--- The newest stress summary under the package's var/.
local function summary(sb)
  local runs = {}
  local ls = shell.run("ls -1 " .. sb.pkg .. "/.prova/var/stress", { check = true })
  for l in ls.stdout:gmatch("[^\n]+") do
    runs[#runs + 1] = l
  end
  table.sort(runs)
  return json.decode(fs.read(sb.pkg .. "/.prova/var/stress/" .. runs[#runs] .. "/summary.json"))
end

prova.test("stress runs N copies of each case, and a case whose copies disagree is FLAKY", {
  requires = { "unix" },
  covers = { "docs/design/flakes.md#stress-mode", "docs/design/flakes.md#stress-artifacts" },
}, function(t)
  local sb = sandbox(t, { "tests::ok", "tests::red", "tests::flaky" })
  local r = stress(sb, "--copies 4 --par 4")
  t:expect(r.code, "a failed copy fails the stress:\n" .. r.stdout):equals(1)
  t:expect(r.stdout):contains("3 case(s) x 4 copies = 12 runs")
  -- A wrong tally names its copies: each failed copy's own log, which the elided stdout cannot show.
  local copies = ""
  for _, log in ipairs(summary(sb).cases["fake::bin tests::flaky"].logs or {}) do
    copies = copies .. "\n--- " .. log .. "\n" .. (fs.exists(log) and fs.read(log) or "(no log)")
  end
  t:expect(r.stdout, "the flake is named; the flaky case's failed copies:" .. copies)
    :contains("FLAKY  1/4  fake::bin tests::flaky")
  t:expect(r.stdout, "with its panic"):contains("lost the race")
  t:expect(r.stdout, "an always-red case is red, not flaky"):contains("RED  4/4  fake::bin tests::red")

  local s = summary(sb)
  t:expect(s.copies):equals(4)
  t:expect(s.par):equals(4)
  t:expect(s.cases["fake::bin tests::ok"].pass):equals(4)
  t:expect(s.cases["fake::bin tests::flaky"].pass):equals(3)
  t:expect(s.cases["fake::bin tests::flaky"].fail):equals(1)
  t:expect(s.cases["fake::bin tests::red"].fail):equals(4)
  t:expect(#s.cases["fake::bin tests::red"].logs, "a log per failed copy"):equals(4)
  t:expect(fs.read(s.cases["fake::bin tests::red"].logs[1]), "holding the copy's output"):contains("always red")
  t:expect(s.cases["fake::bin tests::quarantined"], "the profile's quarantine stays out"):equals(nil)
  t:expect(s.cases["fake::bin tests::ignored"], "ignored never runs"):equals(nil)

  local ledger = lines(sb.pkg .. "/.prova/var/flakes.jsonl")
  t:expect(#ledger, "one flake event — the red case is not a flake"):equals(1)
  local e = json.decode(ledger[1])
  t:expect(e.source):equals("stress")
  t:expect(e.test):equals("fake::bin#tests::flaky")
  t:expect(e.deputed.suite):equals("fake::bin")
  t:expect(e.first_failure):contains("lost the race")
  t:expect(fs.exists(sb.pkg .. "/.prova/var/last-run.json"), "a probe, not a gate run"):is_false()
end)

prova.test("each copy has its own SHORT TMPDIR, and its process group is reaped", {
  requires = { "unix" },
  covers = "docs/design/flakes.md#stress-mode",
}, function(t)
  local sb = sandbox(t, { "tests::tmp", "tests::daemon" })
  local r = stress(sb, "--copies 3 --par 3 --include-excluded -k tmp -k daemon")
  t:expect(r.code, r.stdout):equals(0)
  local tmps = lines(sb.state .. "/tmps")
  t:expect(#tmps):equals(3)
  local seen = {}
  for _, d in ipairs(tmps) do
    seen[d] = true
    -- A Unix socket path is capped at 104 bytes on macOS: a copy's scratch must leave room.
    t:expect(#d <= 64, "short enough for a socket path: " .. d):is_true()
  end
  t:expect(seen[tmps[1]] and seen[tmps[2]] and seen[tmps[3]] and tmps[1] ~= tmps[2] and tmps[2] ~= tmps[3] and tmps[1] ~= tmps[3],
    "every copy its own"):is_true()
  for _, pid in ipairs(lines(sb.state .. "/daemons")) do
    t:expect(alive(pid), "the daemon " .. pid .. " a copy left behind died with its group"):is_false()
  end
  t:expect(#lines(sb.state .. "/daemons")):equals(3)
end)

prova.test("SIGTERM kills every burner and every live copy, by PID", {
  requires = { "unix" },
  covers = "docs/design/flakes.md#stress-mode",
}, function(t)
  local sb = sandbox(t, { "tests::slow" })
  local out = sb.state .. "/out"
  -- Start stress in the background, wait (by condition, never by clock) until its burners and both
  -- copies are up, then SIGTERM it and report its exit.
  local script = string.format([[
%s stress --listing %s --copies 2 --par 2 --burners 3 > %s 2>&1 &
p=$!
i=0
while [ $i -lt 600 ]; do
  if grep -q "burner pids:" %s 2>/dev/null && [ "$(wc -l < %s/slow 2>/dev/null || echo 0)" -ge 2 ]; then break; fi
  sleep 0.1; i=$((i+1))
done
kill -TERM $p
wait $p
echo "exit=$?"
]], prova.bin, sb.listing, out, out, sb.state)
  local r = shell.run({ "sh", "-c", script }, {
    cwd = sb.pkg,
    env = { PROVA_VAR_DIR = "", STRESS_STATE = sb.state },
    timeout = "90s",
  })
  t:expect(r.stdout, fs.read(out)):contains("exit=130")
  local text = fs.read(out)
  t:expect(text):contains("stopped by a signal")
  local pids = text:match("burner pids: ([%d ]+)")
  t:expect(pids ~= nil, text):is_true()
  local n = 0
  for pid in pids:gmatch("%d+") do
    n = n + 1
    t:expect(alive(pid), "burner " .. pid .. " is gone"):is_false()
  end
  t:expect(n, "three burners were started"):equals(3)
  local slow = lines(sb.state .. "/slow")
  t:expect(#slow):equals(2)
  for _, pid in ipairs(slow) do
    t:expect(alive(pid), "copy " .. pid .. " is gone"):is_false()
  end
end)

prova.test("a stress of nothing is refused, never green", {
  requires = { "unix" },
  covers = "docs/design/flakes.md#stress-mode",
}, function(t)
  local sb = sandbox(t, { "tests::ok" })
  local r = stress(sb, "-k nomatch")
  t:expect(r.code, r.stdout):equals(2)
  t:expect(r.stdout):contains("matched no case")
  local bad = stress(sb, "--copies 0")
  t:expect(bad.code):equals(2)
end)
