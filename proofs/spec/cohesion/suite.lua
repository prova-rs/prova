-- The cohesion pass (docs/design/mocks-proxies-drivers.md): close the seams that keep the
-- Mock/Proxy/Driver surface from reading as ONE API — grpc.proxy speaking the shared fault
-- vocabulary, a universal `.endpoint` alias (the "same value" promise made literal), and `:close()`
-- everywhere a proxy is torn down. Spec-first.
suite.config{ name = "spec-cohesion" }
