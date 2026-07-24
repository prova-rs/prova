--- `yaml` grows its encode half: dump / dump_all (multi-doc, k8s-shaped) round-tripping the
--- shipped parse / parse_all.

prova.test("yaml.dump emits a document parse can read back", function(t)
  local doc = yaml.parse(yaml.dump({ kind = "Service", metadata = { name = "svc" } }))
  t:expect(doc.kind):equals("Service")
  t:expect(doc.metadata.name):equals("svc")
end)

prova.test("yaml.dump_all emits a multi-doc stream round-tripping parse_all", function(t)
  local docs = yaml.parse_all(yaml.dump_all({ { kind = "Service" }, { kind = "Deployment" } }))
  t:expect(#docs):equals(2)
  t:expect(docs[1].kind):equals("Service")
  t:expect(docs[2].kind):equals("Deployment")
end)

-- Shipped today; pinned so the module's whole contract lives here. Born graduated.
prova.test("yaml.parse_all splits a k8s manifest stream", { spec = false }, function(t)
  local docs = yaml.parse_all("kind: Service\n---\nkind: Deployment\n")
  t:expect(docs[1].kind):equals("Service")
  t:expect(docs[2].kind):equals("Deployment")
end)
