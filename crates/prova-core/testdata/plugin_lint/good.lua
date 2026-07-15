-- A conformant resource plugin: returns a namespace with the grammar facets.
return prova.containerized{
  name = "demo", image = "demo", tag = "1", port = 1234,
  url = function(hp) return "demo://127.0.0.1:" .. hp end,
  client = function(url) return { url = url } end,
}
