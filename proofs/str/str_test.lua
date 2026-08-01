--- prova.str — string utilities + the archetect casing vocabulary. The casing functions CALL
--- archetect's own inflections (archetect_inflections) and MIRROR archetect's filter names
--- (snake_case, constant_case, …), so a proof that asserts on an archetype's rendered output
--- speaks the same language the archetype was authored in — unity in functionality AND naming.
--- Canonical access is `prova.str`; like `path`, the bare name is too collision-prone to squat,
--- so it is ambient only when a package lists "str" in `[globals] inject`.

local s = prova.str

prova.test("str.trim strips surrounding whitespace",
  { proves = "increment-4: prova.str.trim" }, function(t)
  t:expect(s.trim("  hi \n")):equals("hi")
  t:expect(s.trim("hi")):equals("hi")
  t:expect(s.trim(" \t ")):equals("")
end)

prova.test("str.split splits on a plain separator, keeping empty fields",
  { proves = "increment-4: prova.str.split" }, function(t)
  local parts = s.split("a,b,,c", ",")
  t:expect(#parts):equals(4)
  t:expect(parts[1]):equals("a")
  t:expect(parts[3]):equals("")     -- empty fields are data (think CSV-ish rows), not noise
  t:expect(parts[4]):equals("c")
  t:expect(#s.split("no-sep-here", ",")):equals(1)
end)

prova.test("str.lines splits on newlines, absorbing \\r\\n — the portable line reader",
  { proves = "increment-4: prova.str.lines" }, function(t)
  local lines = s.lines("a\nb\r\nc\n")
  t:expect(#lines):equals(3)        -- no phantom empty line after the trailing newline
  t:expect(lines[2]):equals("b")    -- the \r is gone: same result for unix and Windows output
  t:expect(lines[3]):equals("c")
  local blanks = s.lines("a\n\nb")
  t:expect(#blanks):equals(3)       -- interior blank lines survive
  t:expect(blanks[2]):equals("")
end)

prova.test("str casing mirrors archetect's filters, name for name",
  { proves = "increment-4: prova.str casing converters" }, function(t)
  t:expect(s.snake_case("helloWorld")):equals("hello_world")
  t:expect(s.pascal_case("hello_world")):equals("HelloWorld")
  t:expect(s.camel_case("hello_world")):equals("helloWorld")
  t:expect(s.kebab_case("HelloWorld")):equals("hello-world")
  t:expect(s.constant_case("helloWorld")):equals("HELLO_WORLD")
  t:expect(s.cobol_case("helloWorld")):equals("HELLO-WORLD")
  t:expect(s.train_case("hello_world")):equals("Hello-World")
  t:expect(s.title_case("hello_world")):equals("Hello World")
  t:expect(s.sentence_case("helloWorld")):equals("Hello world")
  t:expect(s.class_case("foo_bars")):equals("FooBar")       -- class case singularizes
  t:expect(s.package_case("FooBar")):equals("foo.bar")
  t:expect(s.directory_case("FooBar")):equals("foo/bar")
end)

prova.test("str is_* predicates answer for each case",
  { proves = "increment-4: prova.str casing predicates" }, function(t)
  t:expect(s.is_snake_case("hello_world")):is_true()
  t:expect(s.is_snake_case("HelloWorld")):is_false()
  t:expect(s.is_camel_case("helloWorld")):is_true()
  t:expect(s.is_pascal_case("HelloWorld")):is_true()
  t:expect(s.is_kebab_case("hello-world")):is_true()
  t:expect(s.is_constant_case("HELLO_WORLD")):is_true()
  t:expect(s.is_constant_case("hello_world")):is_false()
end)

prova.test("str speaks plurals and ordinals",
  { proves = "increment-4: prova.str pluralize/singularize/ordinalize" }, function(t)
  t:expect(s.pluralize("user")):equals("users")
  t:expect(s.singularize("users")):equals("user")
  t:expect(s.ordinalize("1")):equals("1st")
  t:expect(s.ordinalize("22")):equals("22nd")
  t:expect(s.deordinalize("1st")):equals("1")
end)

prova.test("str claims no ambient global — the bare name stays the user's",
  { proves = "increment-4: prova.str is canonical-only by default" }, function(t)
  -- Same collision contract as `path`: without an explicit [globals] inject entry, the bare
  -- name reads nil and is free to assign; the canonical surface is always there.
  ---@diagnostic disable-next-line: undefined-global  (nil-by-default is exactly the claim)
  t:expect(str == nil):is_true()
  t:expect(type(prova.str.snake_case)):equals("function")
end)
