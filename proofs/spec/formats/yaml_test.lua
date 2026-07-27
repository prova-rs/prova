--- `yaml` — the encode half: encode / encode_all (multi-doc, k8s-shaped) round-tripping
--- decode / decode_all.

prova.test("yaml.encode emits a document decode can read back", function(t)
  local doc = yaml.decode(yaml.encode({ kind = "Service", metadata = { name = "svc" } }))
  t:expect(doc.kind):equals("Service")
  t:expect(doc.metadata.name):equals("svc")
end)

prova.test("yaml.encode_all emits a multi-doc stream round-tripping decode_all", function(t)
  local docs = yaml.decode_all(yaml.encode_all({ { kind = "Service" }, { kind = "Deployment" } }))
  t:expect(#docs):equals(2)
  t:expect(docs[1].kind):equals("Service")
  t:expect(docs[2].kind):equals("Deployment")
end)

-- Shipped before this suite existed; pinned so the module's whole contract lives here.
prova.test("yaml.decode_all splits a k8s manifest stream", function(t)
  local docs = yaml.decode_all("kind: Service\n---\nkind: Deployment\n")
  t:expect(docs[1].kind):equals("Service")
  t:expect(docs[2].kind):equals("Deployment")
end)
