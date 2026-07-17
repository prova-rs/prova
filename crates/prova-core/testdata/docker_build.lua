-- PROOF 4a (containerized SUT) — the PRIMITIVE. `docker.build{}` turns a Dockerfile + a context
-- directory into a real local image; `docker.run{ image = <that> }` then runs it exactly like a
-- pulled one. This is the layer under the convenience: an image the project BUILT is just an image,
-- so everything already proved about running containers (ports, env, networks, aliases) applies to
-- the system under test with no new machinery.
--
-- The real bar is not "returns a string" — it is that the built image RUNS and carries what the
-- Dockerfile put in it: the context was actually sent, and build args were actually applied.
--
-- Run standalone: prova crates/prova-core/testdata/docker_build.lua   (requires docker)

-- A tiny build context authored inline, so the proof is self-contained and readable.
local function write_context(dir, dockerfile_path)
  fs.write(dir .. "/payload.txt", "from-the-context")
  fs.write(dir .. "/" .. dockerfile_path, table.concat({
    "FROM alpine:3.20",
    "ARG GREETING=unset",
    "COPY payload.txt /payload.txt",
    "RUN echo \"$GREETING\" > /greeting.txt",
    "CMD [\"sleep\", \"120\"]",
  }, "\n"))
end

prova.test("docker.build produces a runnable image from a Dockerfile + context",
           { requires = { "docker" } }, function(t)
  local dir = t:tempdir()
  write_context(dir, "Dockerfile")

  local image = docker.build{
    context = dir,
    tag = "prova-build-proof:primitive",
    buildargs = { GREETING = "from-a-build-arg" },
  }

  -- The build returns the image ref, so it feeds docker.run directly.
  t:expect(image):equals("prova-build-proof:primitive")

  -- THE PROOF: the built image runs, and carries what the Dockerfile put in it.
  local c = t:manage(docker.run{ image = image, command = "sleep 120" })
  t:expect(c:run({ "cat", "/payload.txt" })):contains("from-the-context")    -- the context was sent
  t:expect(c:run({ "cat", "/greeting.txt" })):contains("from-a-build-arg")   -- buildargs applied
end)

prova.test("docker.build takes a dockerfile off the context root, as real projects ship it",
           { requires = { "docker" } }, function(t)
  -- The archetypes ship `.platform/docker/local/Dockerfile` — a Dockerfile nested well below the
  -- context root. That path IS the common case, not an exotic one.
  local dir = t:tempdir()
  write_context(dir, ".platform/docker/local/Dockerfile")

  local image = docker.build{
    context = dir,
    dockerfile = ".platform/docker/local/Dockerfile",
    tag = "prova-build-proof:nested",
  }

  local c = t:manage(docker.run{ image = image, command = "sleep 120" })
  -- COPY resolved against the CONTEXT root, not the Dockerfile's directory.
  t:expect(c:run({ "cat", "/payload.txt" })):contains("from-the-context")
end)

prova.test("a failing build raises with the builder's own output", { requires = { "docker" } },
           function(t)
  local dir = t:tempdir()
  fs.write(dir .. "/Dockerfile", table.concat({
    "FROM alpine:3.20",
    "RUN echo i-am-the-build-log && exit 7",   -- a build that fails on purpose
  }, "\n"))

  -- A build failure must be an ERROR carrying the builder's log — never a silent success handing
  -- back an image ref that does not exist. Docker streams build failures as an error payload rather
  -- than a transport error, so this is the case a naive stream-drain gets wrong.
  local ok, err = pcall(function()
    return docker.build{ context = dir, tag = "prova-build-proof:fails" }
  end)
  t:expect(ok, "a failing build must raise"):equals(false)
  t:expect(tostring(err)):contains("i-am-the-build-log")
end)
