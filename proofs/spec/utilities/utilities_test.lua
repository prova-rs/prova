--- base64 / hash / uuid / url — ten-line bindings each, sized here as the whole contract.

prova.test("base64 encodes and decodes, round-tripping binary-ish text", { spec = "api-freeze §1 - utility belt" }, function(t)
  t:expect(base64.encode("prova")):equals("cHJvdmE=")
  t:expect(base64.decode("cHJvdmE=")):equals("prova")
end)

prova.test("hash.sha256 hex-digests the NIST vector", { spec = "api-freeze §1 - utility belt" }, function(t)
  t:expect(hash.sha256("abc"))
    :equals("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
end)

prova.test("hash.hmac_sha256 is keyed and deterministic", { spec = "api-freeze §1 - utility belt" }, function(t)
  local a = hash.hmac_sha256("key", "message")
  t:expect(a):has_length(64)
  t:expect(a):matches("^%x+$")
  t:expect(a):equals(hash.hmac_sha256("key", "message"))
  t:expect(a):never():equals(hash.hmac_sha256("other-key", "message"))
end)

prova.test("uuid.v4 emits distinct RFC-shaped ids", { spec = "api-freeze §1 - utility belt" }, function(t)
  local id = uuid.v4()
  t:expect(id):has_length(36)
  t:expect(id):matches("^%x%x%x%x%x%x%x%x%-%x%x%x%x%-4%x%x%x%-%x%x%x%x%-%x%x%x%x%x%x%x%x%x%x%x%x$")
  t:expect(uuid.v4()):never():equals(id)
end)

prova.test("url.parse exposes the structured parts", { spec = "api-freeze §1 - utility belt" }, function(t)
  local u = url.parse("https://example.com:8443/path?q=1")
  t:expect(u.scheme):equals("https")
  t:expect(u.host):equals("example.com")
  t:expect(u.port):equals(8443)
  t:expect(u.path):equals("/path")
  t:expect(u.query):equals("q=1")
end)

prova.test("url.encode percent-encodes a component", { spec = "api-freeze §1 - utility belt" }, function(t)
  t:expect(url.encode("a b&c")):equals("a%20b%26c")
end)
