-- prova.parse.* — the exec-CLI output-parsing toolkit.

prova.test("lines: non-empty trimmed lines", function(t)
  local ls = prova.parse.lines("  a  \n\n b \nc\n")
  t:expect(#ls):equals(3)
  t:expect(ls[1]):equals("a")
  t:expect(ls[3]):equals("c")
end)

prova.test("rows: split each line on a separator (default tab)", function(t)
  local rows = prova.parse.rows("1|alpha\n2|beta", "|")
  t:expect(#rows):equals(2)
  t:expect(rows[1][1]):equals("1")
  t:expect(rows[2][2]):equals("beta")
end)

prova.test("table: header row keys the remaining rows", function(t)
  local rows = prova.parse.table("name\tsize\nfoo\t3\nbar\t5")
  t:expect(#rows):equals(2)
  t:expect(rows[1].name):equals("foo")
  t:expect(rows[1].size):equals("3")
  t:expect(rows[2].name):equals("bar")
end)

prova.test("json moved out: parse is format-agnostic, json.decode is the parser", function(t)
  -- The api-freeze §1 clean break: prova.parse.json is gone; the tech-first `json` module
  -- carries the same null-to-nil decode semantics.
  t:expect(prova.parse.json):is_nil() ---@diagnostic disable-line: undefined-field
  local v = json.decode('{"a": 1, "b": [true, "x"], "c": null}')
  t:expect(v.a):equals(1)
  t:expect(v.b[1]):is_true()
  t:expect(v.b[2]):equals("x")
  t:expect(v.c == nil):is_true()
  t:expect(json.decode("null") == nil):is_true()
end)
