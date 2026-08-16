--- `csv` — header-aware, row shape mirroring prova.parse.table (a list of header-keyed maps).

prova.test("csv.decode is header-aware", function(t)
  local rows = csv.decode("name,port\nredis,6379\npostgres,5432\n")
  t:expect(rows):has_length(2)
  t:expect(rows[1].name):equals("redis")
  t:expect(rows[2].port):equals("5432")
end)

prova.test("csv.decode honors quoted fields with embedded commas", function(t)
  local rows = csv.decode('name,desc\nredis,"fast, in-memory"\n')
  t:expect(rows[1].desc):equals("fast, in-memory")
end)

prova.test("csv.encode emits headers + rows, round-tripping parse", function(t)
  local rows = { { name = "redis", port = "6379" }, { name = "postgres", port = "5432" } }
  local out = csv.decode(csv.encode(rows))
  t:expect(out):equals(rows)
end)

prova.test("a verb that returns rows returns a LIST, even when there are none", {
  proves = "an empty result used to be a bare table, indistinguishable from an empty map, so `json.encode(csv.decode(header_only))` emitted `{}` where the author wrote something that returns rows — the same shape-loss `json.decode` had for `[]`, and it reaches every list-returning verb",
}, function(t)
  t:expect(json.encode(csv.decode("a,b\n")), "zero rows still encodes as a list"):equals("[]")
  t:expect(json.encode(csv.decode("a,b\n1,2\n")), "…and rows are unaffected")
    :equals('[{"a":"1","b":"2"}]')
  -- The marker must stay invisible to everything except re-encoding.
  local rows = csv.decode("a,b\n1,2\n")
  t:expect(#rows, "length is unchanged"):equals(1)
  t:expect(rows[1].a, "indexing is unchanged"):equals("1")
end)

prova.test("duplicate headers are refused, not silently collapsed", {
  proves = "rows are header-keyed maps, so two columns of one name cannot both survive — the second overwrote the first and the dropped column was never mentioned, which is data loss wearing a successful return. The contract is unsatisfiable for that input, so the honest answer is to say so",
}, function(t)
  local ok, err = pcall(function() return csv.decode("a,a\n1,2\n") end)
  t:expect(ok, "refused"):is_false()
  t:expect(tostring(err), "…naming the offending header"):contains("duplicate header")
  t:expect(tostring(err), "…and what to do instead"):contains("Rename one")
end)

prova.test("a row carrying a column the header row lacks is refused, not silently dropped", {
  proves = "headers are taken from the FIRST row, so a later row's extra field vanished with a successful return — the column existed in the data and not in the output, and which row happened to be first decided what was lost. Declaring `headers` is different: naming the columns IS choosing them, so a projection stays legal",
}, function(t)
  local ok, err = pcall(function()
    return csv.encode({ { a = "1" }, { a = "2", b = "lost" } })
  end)
  t:expect(ok, "refused"):is_false()
  t:expect(tostring(err), "…naming the row"):contains("row 2")
  t:expect(tostring(err), "…and the field"):contains("`b`")
  t:expect(tostring(err), "…and the way to say what you meant"):contains("headers")

  -- A row MISSING a column is not data loss — it is an empty cell, and stays legal.
  t:expect(csv.encode({ { a = "1", b = "2" }, { a = "3" } }), "sparse rows still encode")
    :equals("a,b\n1,2\n3,\n")
  -- Declared headers project deliberately.
  t:expect(csv.encode({ { a = "1", b = "2" } }, { headers = { "b" } }), "a projection is honored")
    :equals("b\n2\n")
end)

prova.test("encoding no rows produces no file, not a file with one nameless column", {
  proves = "it emitted `\"\"` — a header line declaring a single column whose name is the empty string — which decodes back to a row shape nobody asked for. Nothing to describe means nothing to write",
}, function(t)
  t:expect(csv.encode({}), "no rows, no output"):equals("")
end)
