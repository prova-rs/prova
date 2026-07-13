-- yaml.parse (single doc) and yaml.parse_all (multi-doc `---` stream, as in k8s manifests).

prova.test("parses a single document into a table", function(t)
  local doc = yaml.parse("name: widget\nreplicas: 3\nlabels:\n  app: web\n")
  t:expect(doc.name):equals("widget")
  t:expect(doc.replicas):equals(3)
  t:expect(doc.labels.app):equals("web")
end)

prova.test("parses a multi-document stream", function(t)
  local docs = yaml.parse_all("kind: Service\n---\nkind: Deployment\n---\nkind: ConfigMap\n")
  t:expect(#docs):equals(3)
  t:expect(docs[1].kind):equals("Service")
  t:expect(docs[3].kind):equals("ConfigMap")
end)

prova.test("raises on invalid yaml", function(t)
  local ok = pcall(function() yaml.parse("key: [unterminated\n") end)
  t:expect(ok):is_false()
end)
