-- BuildKit secrets on `docker.build`. The motivating case is a production Dockerfile that reads a
-- private registry token via `RUN --mount=type=secret,id=…`: without secrets support such an image
-- cannot be built by a proof at all, so the SUT has to be a hand-built artifact and the suite stops
-- proving what CI publishes.
--
-- A build arg is NOT a substitute, and the last proof here is the one that says why: build args are
-- recorded in image history, secrets are not.

-- Validation is hermetic — no daemon needed to reject a malformed spec.
prova.group("secret spec validation", function(g)
  g:test("a secret needs exactly one source", function(t)
    local dir = t:tempdir()
    fs.write(dir .. "/Dockerfile", "FROM scratch\n")

    local ok, err = pcall(function()
      docker.build{ context = dir, secrets = { tok = { env = "A", value = "b" } } }
    end)
    t:expect(ok):is_falsy()
    t:expect(tostring(err)):contains("exactly one of")
  end)

  g:test("a bare string is refused rather than guessed", function(t)
    -- Ambiguous between a path and a literal, and guessing wrong either leaks the value or mounts
    -- the wrong bytes. The error must name the shape it wants.
    local dir = t:tempdir()
    fs.write(dir .. "/Dockerfile", "FROM scratch\n")

    local ok, err = pcall(function()
      docker.build{ context = dir, secrets = { tok = "a-literal-token" } }
    end)
    t:expect(ok):is_falsy()
    t:expect(tostring(err)):contains("must be a table")
  end)

  g:test("a missing secret file fails before the builder runs", function(t)
    local dir = t:tempdir()
    fs.write(dir .. "/Dockerfile", "FROM scratch\n")

    local ok, err = pcall(function()
      docker.build{ context = dir, secrets = { tok = { file = dir .. "/nope" } } }
    end)
    t:expect(ok):is_falsy()
    t:expect(tostring(err)):contains("does not exist")
  end)
end)

prova.group("secrets reach the build", { requires = { "docker" } }, function(g)
  -- One Dockerfile shape for every source: consume the secret and prove its content, so a secret
  -- that arrives empty or unmounted fails the build rather than passing silently.
  local function context_reading(id)
    local dir = fs.tempdir()
    fs.write(
      dir .. "/Dockerfile",
      "FROM busybox\n"
        .. "RUN --mount=type=secret,id="
        .. id
        .. " test \"$(cat /run/secrets/"
        .. id
        .. ")\" = expected-token\n"
    )
    return dir
  end

  g:test("from a literal value", function(t)
    local dir = context_reading("tok")
    t:defer(function() fs.remove_all(dir) end)

    local ref = docker.build{
      context = dir,
      tag = "prova-secret-value:test",
      secrets = { tok = { value = "expected-token" } },
      nocache = true, -- else a cached layer proves nothing about this run's secret
    }
    t:expect(ref):equals("prova-secret-value:test")
  end)

  g:test("from a file", function(t)
    local dir = context_reading("tok")
    t:defer(function() fs.remove_all(dir) end)
    local secret_file = t:tempdir() .. "/token"
    fs.write(secret_file, "expected-token")

    local ref = docker.build{
      context = dir,
      tag = "prova-secret-file:test",
      secrets = { tok = { file = secret_file } },
      nocache = true,
    }
    t:expect(ref):equals("prova-secret-file:test")
  end)

  g:test("a wrong secret fails the build, so the assertion is real", function(t)
    -- Without this, every test above would also pass if the secret were never mounted and the
    -- Dockerfile's `test` silently succeeded.
    local dir = context_reading("tok")
    t:defer(function() fs.remove_all(dir) end)

    local ok = pcall(function()
      docker.build{
        context = dir,
        tag = "prova-secret-wrong:test",
        secrets = { tok = { value = "the-wrong-token" } },
        nocache = true,
      }
    end)
    t:expect(ok):is_falsy()
  end)

  g:test("the inline secret's temp file does not outlive the build", function(t)
    local dir = context_reading("tok")
    t:defer(function() fs.remove_all(dir) end)

    -- TMPDIR commonly ends in a slash; glob returns normalized absolute paths, so strip it or the
    -- comparison below fails on a doubled separator.
    local temp_root = (os.getenv("TMPDIR") or "/tmp"):gsub("/+$", "")

    -- A decoy makes the search self-verifying. Without it, "found nothing" is indistinguishable from
    -- "looked in the wrong place with the wrong pattern", and the proof would pass vacuously.
    local decoy = temp_root .. "/prova-secret-decoy/tok"
    fs.write(decoy, "decoy")
    t:defer(function() fs.remove_all(temp_root .. "/prova-secret-decoy") end)
    t:expect(fs.glob(temp_root, "prova-*/tok")):contains(decoy)

    docker.build{
      context = dir,
      tag = "prova-secret-cleanup:test",
      secrets = { tok = { value = "expected-token" } },
      nocache = true,
    }

    -- The same search now finds the decoy and nothing else: no leftover file holds the secret.
    local found = fs.glob(temp_root, "prova-*/tok")
    t:expect(found):has_length(1)
    t:expect(found[1]):equals(decoy)
  end)

  g:test("a secret is not recorded in image history, unlike a build arg", function(t)
    -- The substantive reason this feature exists rather than telling people to use buildargs.
    local dir = fs.tempdir()
    t:defer(function() fs.remove_all(dir) end)
    fs.write(
      dir .. "/Dockerfile",
      "FROM busybox\n"
        .. "ARG LEAKY\n"
        .. "RUN --mount=type=secret,id=tok test -s /run/secrets/tok && echo $LEAKY > /tmp/x\n"
    )

    docker.build{
      context = dir,
      tag = "prova-secret-history:test",
      buildargs = { LEAKY = "arg-is-visible" },
      secrets = { tok = { value = "secret-is-not-visible" } },
      nocache = true,
    }

    local history = shell.run({
      "docker", "history", "--no-trunc", "--format", "{{.CreatedBy}}", "prova-secret-history:test",
    })
    t:expect(history.stdout):contains("arg-is-visible")
    t:expect(history.stdout):never():contains("secret-is-not-visible")
  end)
end)
