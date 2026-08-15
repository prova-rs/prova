--- A container's configuration is carried IN, not baked into an image
--- (docs/design/agent-ergonomics.md#containerized-mounts).
---
--- Anything a container read from disk used to have to arrive inside its image, so a Keycloak
--- realm, a router config and an archetype catalog each cost a Dockerfile whose whole job was one
--- `COPY` — one topology built three images that existed only to carry a file, and a one-line
--- config change cost an image build.
---
--- `files` carries content over the SAME API as everything else (`PUT /containers/{id}/archive`,
--- into a created but not-yet-started container). Deliberately NOT a bind mount: `binds` is one
--- defaulted field away in bollard's HostConfig and it is the wrong answer, because a bind names a
--- path on the DAEMON's filesystem. Against a remote or rootless daemon a scope tempdir is not
--- there, and Docker's classic answer to a missing bind source is an empty directory — so the
--- container boots, finds no config, and fails later as an auth error that names nothing about
--- mounts.
---
--- The parse half needs no daemon and is proven without one; the delivery half requires docker.

local function why(fn)
  local ok, err = pcall(fn)
  return ok and "" or tostring(err)
end

prova.group("what a `files` entry must say", function(g)
  g:test("a bare string is refused rather than guessed", {
    covers = "docs/design/agent-ergonomics.md#containerized-mounts",
    proves = "the same rule `docker.build`'s `secrets` already follows: a string is ambiguous between a literal and a path, and guessing wrong either writes the path as content or reads a file the author never named — both of which produce a container that starts and then misbehaves",
  }, function(t)
    local err = why(function() docker.run{ image = "busybox", files = { ["/a"] = "hi" } } end)
    t:expect(err, "refused"):contains("must be a table")
    t:expect(err, "…naming all three shapes"):contains("`text`")
    t:expect(err, "…including the file form"):contains("`file`")
    t:expect(err, "…and the directory form"):contains("`dir`")
  end)

  g:test("exactly one source, and a relative path is not a container path", {
    covers = "docs/design/agent-ergonomics.md#containerized-mounts",
    proves = "two sources have no defensible precedence and a relative path has nothing to resolve against until the image's entrypoint runs — both are refusals rather than guesses, at the call site rather than three seconds into a boot",
  }, function(t)
    t:expect(why(function()
      docker.run{ image = "busybox", files = { ["/a"] = { text = "h", file = "/etc/hosts" } } }
    end), "two sources"):contains("exactly one")

    t:expect(why(function()
      docker.run{ image = "busybox", files = { ["a/b"] = { text = "h" } } }
    end), "a relative container path"):contains("ABSOLUTE")
  end)

  g:test("a source that is not there fails before the daemon is touched", {
    covers = "docs/design/agent-ergonomics.md#containerized-mounts",
    proves = "this is the bind mount's worst behavior, refused up front instead: a missing source there becomes an EMPTY DIRECTORY inside the container, which is a running container with the wrong contents and no message anywhere",
  }, function(t)
    t:expect(why(function()
      docker.run{ image = "busybox", files = { ["/a"] = { file = "/nope/absent" } } }
    end), "a missing file is named"):contains("does not exist")

    t:expect(why(function()
      docker.run{ image = "busybox", files = { ["/a"] = { dir = "/nope/absent" } } }
    end), "…as is a missing directory"):contains("not a directory")
  end)

  g:test("an entry's own keys are closed too", {
    covers = "docs/design/agent-ergonomics.md#module-opts-silently-ignored",
    proves = "a typo INSIDE an entry is the worst place for one — `txt` would leave the entry with no source at all, so the closed set has to reach the nested table and not just the `files` key itself",
  }, function(t)
    local err = why(function()
      docker.run{ image = "busybox", files = { ["/a"] = { txt = "h" } } }
    end)
    t:expect(err, "the nested key is refused"):contains("txt")
    t:expect(err, "…with the spelling meant"):contains("text")
  end)
end)

prova.group("what the container actually sees", { requires = { "docker" } }, function(g)
  g:test("all three sources land, at paths the image never created, before the first command", {
    covers = "docs/design/agent-ergonomics.md#containerized-mounts",
    proves = "the assertion has to be made by the CONTAINER, at boot: an upload that landed after start would satisfy any host-side check while the process that needed the config had already read nothing. `/opt/deep/nested` exists in no image, which is what makes the parent-directory entries load-bearing rather than incidental",
  }, function(t)
    local dir = t:tempdir("materials")
    fs.mkdir(dir .. "/catalog")
    fs.write(dir .. "/catalog/one.yaml", "kind: archetype\n")
    fs.write(dir .. "/router.yaml", "listen: 4000\n")

    local c = t:manage(docker.run{
      image = "busybox:latest",
      -- The container reads everything as its FIRST act, so a late upload could not pass this.
      command = { "sh", "-c",
        "cat /opt/deep/nested/realm.json; cat /etc/router.yaml; ls /etc/archetypes; sleep 30" },
      files = {
        ["/opt/deep/nested/realm.json"] = { text = '{"realm":"prova"}\n' },
        ["/etc/router.yaml"]            = { file = dir .. "/router.yaml" },
        ["/etc/archetypes"]             = { dir = dir .. "/catalog" },
      },
    })

    local logs = prova.retry(function()
      local out = c:logs()
      return out:find("one.yaml") and out or nil
    end, { timeout = "20s", message = "the container never read its files:\n" .. c:logs() })

    t:expect(logs, "the literal landed under a path no image created"):contains('{"realm":"prova"}')
    t:expect(logs, "the host file landed"):contains("listen: 4000")
    t:expect(logs, "the directory landed, with its contents"):contains("one.yaml")
  end)

  g:test("mode is honored, and a source file keeps its own", {
    covers = "docs/design/agent-ergonomics.md#containerized-mounts",
    proves = "carrying a script in is useless if it arrives unexecutable, and the fix must not be `mode` on every entry — a file that was executable on the host staying executable is what makes the common case need no ceremony. Asserted by RUNNING them, since a mode that looks right and does not exec is the same silent wrong one layer down",
  }, function(t)
    local dir = t:tempdir("scripts")
    fs.write(dir .. "/tool.sh", "#!/bin/sh\necho from-source-mode\n")
    shell.run({ "chmod", "0755", dir .. "/tool.sh" })

    local c = t:manage(docker.run{
      image = "busybox:latest",
      command = { "sh", "-c",
        "/usr/local/bin/hook.sh; /usr/local/bin/tool.sh; stat -c %a /etc/plain.txt; sleep 30" },
      files = {
        ["/usr/local/bin/hook.sh"] = { text = "#!/bin/sh\necho from-explicit-mode\n", mode = "0755" },
        ["/usr/local/bin/tool.sh"] = { file = dir .. "/tool.sh" },
        ["/etc/plain.txt"]         = { text = "x\n" },
      },
    })

    local logs = prova.retry(function()
      local out = c:logs()
      return out:find("644") and out or nil
    end, { timeout = "20s", message = "the scripts never ran:\n" .. c:logs() })

    t:expect(logs, "an explicit mode makes it executable"):contains("from-explicit-mode")
    t:expect(logs, "…and a source file keeps the bit it had"):contains("from-source-mode")
    t:expect(logs, "…while an ordinary file is not executable"):contains("644")
  end)
end)

prova.test("a recipe carries its own config, and a caller overrides one entry by path", {
  covers = "docs/design/agent-ergonomics.md#containerized-mounts",
  proves = "this is the case that cost three purpose-built images: a recipe's config had to live in an image, so a consumer wanting one line different had to fork the Dockerfile. Merging by PATH is what makes overriding a single file possible without restating the rest — and the recipe's other entries surviving the override is the half that would silently regress",
  requires = { "docker" },
}, function(t)
  local recipe = prova.containerized{
    name = "cfgdemo",
    image = "busybox",
    tag = "latest",
    port = 1,
    -- Readiness IS the assertion here: the recipe is ready once the container has READ its
    -- config, which is a stronger gate than a port and removes the race between `sleep` and the
    -- poll that a port-based wait would introduce.
    wait = { log = "recipe-only" },
    command = { "sh", "-c", "cat /etc/a.conf /etc/b.conf; sleep 600" },
    files = {
      ["/etc/a.conf"] = { text = "from-recipe\n" },
      ["/etc/b.conf"] = { text = "recipe-only\n" },
    },
    url = function(host_port) return "tcp://" .. host_port end,
  }

  local res = recipe.container(t, {
    -- One entry replaced; the recipe's other file must survive untouched.
    files = { ["/etc/a.conf"] = { text = "from-caller\n" } },
  })

  -- No retry needed: `wait` above already held until the line appeared.
  local logs = res.container:logs()

  t:expect(logs, "the caller's entry wins at that path"):contains("from-caller")
  t:expect(logs, "…and the recipe's own entry is still there"):contains("recipe-only")
  t:expect(logs, "…with no trace of what was overridden"):never():contains("from-recipe")
end)
