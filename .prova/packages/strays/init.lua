-- Proofs that find their own children by command line (`pgrep -f`) and reap strays (`pkill -f`).
--
-- A fixed token (`sleep 77.221`, `prova up raced`) is shared by every prova on the host running the
-- same proof. Two gates at once each saw the other's process as a survivor, and one's cleanup killed
-- the other's mid-release (prova red at 0325818f and 0f2d1687; D m/11882). Every token here is this
-- run's own: a name gains a uuid suffix, and a duration gains per-run digits, so `sleep 77.221` runs
-- as `sleep 77.221048213`, still about 77 s.
--
-- The probe brackets the token's last character (`77.22104821[3]`): it matches the process's
-- command line, never the probe's own. On Linux, `pgrep -f` sees the wrapping shell, whose argv
-- carries the pattern.
local M = {}

local function digits()
  return string.format("%09d", tonumber(uuid.v4():gsub("-", ""):sub(1, 7), 16) % 1000000000)
end

--- A name of this run's own: `base-<12 hex>`, for a topology or anything named on a command line.
function M.name(base)
  return base .. "-" .. uuid.v4():gsub("-", ""):sub(1, 12)
end

--- A `sleep` duration of this run's own: `base` (it must carry a `.`) with per-run digits.
function M.duration(base)
  assert(base:find("%."), "a duration token carries a decimal point: " .. base)
  return base .. digits()
end

local function probe(word, token)
  -- pgrep reads a regex: a token's `.` is escaped as `[.]`, so it matches only a dot (D m/11887).
  local head = token:sub(1, -2):gsub("%.", "[.]")
  return "'" .. word .. " " .. head .. "[" .. token:sub(-1) .. "]'"
end

--- Is a process whose command line carries `word token` (`sleep 77.221…`, `prova up raced-…`) alive?
function M.alive(word, token)
  return shell.run("pgrep -f " .. probe(word, token)).code == 0
end

--- Reap any stray carrying `word token`, this run's only.
function M.reap(word, token)
  shell.run("pkill -f " .. probe(word, token) .. " 2>/dev/null; true")
end

return M
