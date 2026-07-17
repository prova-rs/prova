-- Teardown errors are reported, not swallowed.
--
-- Resolves docs/design/api.md §Open questions #2 — "do we surface teardown errors as separate
-- failures or attach them to the test?" — in favour of **separate**, and this file is the argument.
--
-- Attaching to a test cannot work in general: a Scope.File fixture tears down after *every* test in
-- its file, so no single test owns the failure, and picking one would blame whichever test happened
-- to sort last. It is also the wrong place: a teardown raises *after* the body passed, so the defect
-- is not in the test. So it gets its own leaf — `<scope> ⟶ teardown` — counted in `failed` like any
-- other. That needs no new reporting concept: Event::NodeFinished already carries path + outcome +
-- message.
--
-- The stakes, and why this is not cosmetic: `ctx:manage` teardown is what stops containers. Before
-- this, a cleanup that raised was discarded (`let _ = …`) — a **leaked container the run reported as
-- green**. Deleting the `let _` is the whole fix; the rest is deciding where it shows up.

--------------------------------------------------------------------------------------------
-- A raising teardown fails the run — and the test itself still passes, because it did.
--------------------------------------------------------------------------------------------
local bad = prova.fixture("bad_teardown", Scope.Test, function(ctx)
  ctx:defer(function() error("cleanup exploded") end)
  return "v"
end)

prova.test("the test passes; its teardown is what fails", function(t)
  t:expect(t:use(bad)):equals("v")
end)

--------------------------------------------------------------------------------------------
-- One raising cleanup must not strand the others. If it did, a single bad `defer` would leak
-- every resource registered before it — the exact failure this whole change exists to prevent.
--
-- A flow, because this needs *ordering*: a step's `test` scope tears down when that step ends,
-- so a later step can read what the earlier step's teardown actually did. A `group` makes no
-- order guarantee, so the same assertion there would be a coin flip.
--------------------------------------------------------------------------------------------
local log = prova.fixture("log", Scope.File, function() return {} end)

prova.flow("a raising teardown does not strand the cleanups around it", function(f)
  f:step("register three defers, the middle one raising", function(t)
    local l = t:use(log)
    t:defer(function() table.insert(l, "first-registered") end)
    t:defer(function() error("middle exploded") end)
    t:defer(function() table.insert(l, "last-registered") end)
  end)

  f:step("both survivors ran, in LIFO order, despite the raiser between them", function(t)
    -- LIFO: last registered runs first. Both are present, so the raiser in the middle neither
    -- stopped the run nor swallowed its neighbours.
    t:expect(t:use(log)):equals({ "last-registered", "first-registered" })
  end)
end)
