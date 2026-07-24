--- `csv` — header-aware, row shape mirroring prova.parse.table (a list of header-keyed maps).

prova.test("csv.parse is header-aware", function(t)
  local rows = csv.parse("name,port\nredis,6379\npostgres,5432\n")
  t:expect(rows):has_length(2)
  t:expect(rows[1].name):equals("redis")
  t:expect(rows[2].port):equals("5432")
end)

prova.test("csv.parse honors quoted fields with embedded commas", function(t)
  local rows = csv.parse('name,desc\nredis,"fast, in-memory"\n')
  t:expect(rows[1].desc):equals("fast, in-memory")
end)

prova.test("csv.encode emits headers + rows, round-tripping parse", function(t)
  local rows = { { name = "redis", port = "6379" }, { name = "postgres", port = "5432" } }
  local out = csv.parse(csv.encode(rows))
  t:expect(out):equals(rows)
end)
