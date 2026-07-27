--- THE NAMING RULE for the document-format modules, held executably.
---
--- One rule: **`decode` reads text into a Lua value, `encode` writes a Lua value back to text** — for
--- every document format, with a `_all` suffix only where the format genuinely has multi-document
--- streams (YAML `---`). api-freeze §1 stated this as "encode + decode together" and the shipped
--- implementations drifted anyway: `yaml` took `parse`/`dump` from PyYAML, `toml` and `csv` took
--- `parse` from their own ecosystems, and `json` kept `decode` from cjson. That left **two** outliers
--- pointing in opposite directions — `json.decode` against three `parse`s on the read side, `yaml.dump`
--- against three `encode`s on the write side — so neither half was consistent.
---
--- Why this needs a proof rather than a convention note: the modules are ONE system (they share a
--- single encode half and the `json.null` / `json.array` fidelity sentinels), they are used side by
--- side in a single proof file, and Lua has no compile-time check. A wrong-but-plausible name is a
--- runtime `attempt to call a nil value` at the moment that line executes — and if the call sits
--- inside a `pcall`, it is swallowed and reported as something else entirely. That is exactly how a
--- removed `prova.parse.json` survived unnoticed in an archetype's proof suite: the failure surfaced
--- as "no run_finished event in prova output", blaming the program under test rather than the caller.
---
--- The forward direction (every listed name is callable) is what the parity proof in
--- `proofs/introspection/` already does per-module. What is unique here is the REVERSE: no document
--- format may expose a read/write verb OUTSIDE the rule — the former `parse`/`dump` spellings among
--- them. That is the assertion a fifth format cannot silently violate, and the reason the old names
--- were removed outright rather than aliased: with no consumers to carry, a shim would only be a
--- second name to keep working, a second thing to document, and a way for the drift to grow back.

-- The document formats and the multi-document variants each one legitimately has. `base64` and `url`
-- are deliberately absent: those are blob and component transforms, not document (de)serialization
-- (`url.encode` percent-encodes one component), so they keep their own names.
local DOCUMENT_FORMATS = {
  { name = "json", mod = json, multi = false },
  { name = "yaml", mod = yaml, multi = true },
  { name = "toml", mod = toml, multi = false },
  { name = "csv", mod = csv, multi = false },
}

-- Verbs that would mean "read a document" or "write a document" under some other ecosystem's naming.
-- Any of these on a format module is the drift this proof exists to catch.
--
-- `parse`, `parse_all`, `dump` and `dump_all` lead the list because they are not hypothetical: they
-- are prova's OWN former spellings, removed with no alias (pre-announcement, nobody to carry — the
-- same clean cut api-freeze §1 made for `prova.parse.json`). Listing them here is what makes that
-- removal permanent rather than a thing that quietly grows back the next time someone adds a format
-- by copying an ecosystem's conventions.
local FOREIGN_VERBS = {
  "parse", "parse_all", "dump", "dump_all",
  "load", "loads", "dumps", "stringify", "serialize", "deserialize",
  "from_string", "to_string", "read", "write", "unmarshal", "marshal",
}

-- ── the rule ─────────────────────────────────────────────────────────────────────────────────

prova.test("every document format reads with decode and writes with encode", function(t)
  for _, fmt in ipairs(DOCUMENT_FORMATS) do
    t:expect(type(fmt.mod.decode), fmt.name .. ".decode must be callable"):equals("function")
    t:expect(type(fmt.mod.encode), fmt.name .. ".encode must be callable"):equals("function")
  end
end)

prova.test("the _all variants exist exactly where the format has multi-document streams", function(t)
  for _, fmt in ipairs(DOCUMENT_FORMATS) do
    local want = fmt.multi and "function" or "nil"
    t:expect(type(fmt.mod.decode_all), fmt.name .. ".decode_all presence"):equals(want)
    t:expect(type(fmt.mod.encode_all), fmt.name .. ".encode_all presence"):equals(want)
  end
end)

-- The reverse direction, and the whole point of the file: a format may not carry a read/write verb
-- borrowed from somewhere else. Adding `yaml.load` or `csv.read` fails HERE, at authoring time,
-- rather than becoming folklore a caller has to memorize.
prova.test("no document format exposes a read/write verb outside the rule", function(t)
  for _, fmt in ipairs(DOCUMENT_FORMATS) do
    for _, verb in ipairs(FOREIGN_VERBS) do
      t:expect(fmt.mod[verb], fmt.name .. "." .. verb .. " must not exist — use decode/encode")
        :is_nil()
    end
  end
end)

-- ── round-tripping, through the new names only ───────────────────────────────────────────────

prova.test("decode and encode are true inverses for every document format", function(t)
  -- One shape per format, chosen to survive that format's own type model: TOML has no null and wants
  -- a table root; CSV is untyped text in header-keyed rows.
  t:expect(json.decode(json.encode({ a = 1, b = "two" }))):equals({ a = 1, b = "two" })
  t:expect(yaml.decode(yaml.encode({ a = 1, b = "two" }))):equals({ a = 1, b = "two" })
  t:expect(toml.decode(toml.encode({ tbl = { a = 1 } }))):equals({ tbl = { a = 1 } })
  t:expect(csv.decode(csv.encode({ { h = "v" } }))):equals({ { h = "v" } })
end)

prova.test("yaml's _all pair round-trips a multi-document stream", function(t)
  local docs = { { a = 1 }, { b = 2 } }
  local stream = yaml.encode_all(docs)
  t:expect(stream):contains("---")
  t:expect(yaml.decode_all(stream)):equals(docs)
end)

-- ── the clean cut ────────────────────────────────────────────────────────────────────────────

-- The former spellings are GONE, not deprecated. `FOREIGN_VERBS` above already asserts they carry no
-- value; this asserts the *shape of the failure* a stale call site gets, because that is what someone
-- upgrading actually experiences. An indexing error naming the missing field beats a wrapper that
-- works-but-warns: the call site has to be fixed either way, and only one of the two makes that
-- unmissable.
prova.test("a former spelling raises rather than silently working", function(t)
  local ok, err = pcall(function()
    return yaml.parse("a: 1")
  end)
  t:expect(ok, "yaml.parse must not resolve to anything callable"):is_false()
  t:expect(tostring(err)):contains("nil value")
end)
