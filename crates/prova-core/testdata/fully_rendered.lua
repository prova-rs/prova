-- is_fully_rendered: the signature archetype check. A rendered tree must carry no leftover jinja
-- markers in file contents or path segments; GitHub Actions `${{ … }}` expressions are allowed.

prova.test("a fully-rendered tree passes", function(t)
  local dir = t:tempdir()
  fs.write(dir .. "/Cargo.toml", 'name = "widget"\n')
  fs.write(dir .. "/src/main.rs", "fn main() {}\n")
  -- GitHub Actions expression is NOT a leftover marker.
  fs.write(dir .. "/.github/workflows/ci.yml", "run: echo ${{ github.token }}\n")
  t:expect(dir):is_fully_rendered()
end)

prova.test("a leftover content marker is caught", function(t)
  local dir = t:tempdir()
  fs.write(dir .. "/Cargo.toml", 'name = "{{ project_name }}"\n')
  t:expect(dir):never():is_fully_rendered()
end)

prova.test("a leftover block/comment marker is caught", function(t)
  local dir = t:tempdir()
  fs.write(dir .. "/README.md", "{% if enabled %}on{% endif %}\n")
  t:expect(dir):never():is_fully_rendered()
end)

prova.test("an unrendered path segment is caught", function(t)
  local dir = t:tempdir()
  fs.write(dir .. "/{{ project_name }}/main.rs", "fn main() {}\n")
  t:expect(dir):never():is_fully_rendered()
end)
