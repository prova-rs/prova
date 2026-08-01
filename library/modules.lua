---@meta
--- Prova first-party module annotations: fs, shell, http, archetect.
--- These modules are globally available inside test files (no require needed) and also
--- `require`-able by name. `archetect` is a plugin over `prova-core`, not a built-in.

------------------------------------------------------------------------------------------
-- Handles
------------------------------------------------------------------------------------------

---A file handle rooted within a tree. Read its contents, or assert on it via `expect`.
---@class prova.FileHandle
---@field path string   # absolute path
local FileHandle = {}
--- Read this file's contents as a string.
---@return string contents
function FileHandle:read() end

---A directory handle.
---@class prova.DirHandle
---@field path string
local DirHandle = {}

---A tree handle rooted at a directory (e.g. a render destination).
---@class prova.Tree
---@field path string   # absolute root
local Tree = {}
--- A handle to a file at `rel` within this tree — chain matchers off it (`expect(t:file("a.txt")):exists()`).
---@param rel string
---@return prova.FileHandle
function Tree:file(rel) end
--- A handle to a directory at `rel` within this tree.
---@param rel string
---@return prova.DirHandle
function Tree:dir(rel) end
---Serializable snapshot of the whole layout (for `:matches_snapshot()`).
---@return table
function Tree:tree() end

------------------------------------------------------------------------------------------
-- net
------------------------------------------------------------------------------------------

---@class prova.net
net = {}
--- An OS-assigned free TCP port on 127.0.0.1 — for a locally `shell.spawn`ed app that needs a
--- dynamic port (a container gets its random host port from `docker.run` instead).
---@return integer
function net.free_port() end

------------------------------------------------------------------------------------------
-- fs
------------------------------------------------------------------------------------------

---@class prova.fs
fs = {}
---Create a temp dir. Not auto-cleaned; pair with `ctx:defer` or use `ctx:tempdir()`. The returned
---path is absolute and `/`-normalized on every OS (no `\`, no `\\?\` prefix — safe to embed in
---TOML, shell strings, and patterns).
---@return string path
function fs.tempdir() end
--- Delete a path and everything under it. No error if it is already gone.
---@param path string
function fs.remove_all(path) end
--- Read a whole file as a string. Raises if it does not exist — check `fs.exists` first when absence
--- is a normal outcome rather than a bug.
---@param path string
---@return string
function fs.read(path) end
---Write `contents` to `path`, creating parent directories as needed.
---@param path string
---@param contents string
function fs.write(path, contents) end
--- Whether a file or directory exists.
---@param path string
---@return boolean
function fs.exists(path) end
--- Every path under `root` matching a glob `pattern` (`*` within a segment, `**` across segments).
--- Returns **ABSOLUTE**, `/`-normalized paths, not paths relative to `root` — strip `root` yourself
--- if you need relative ones (a report keyed on absolute paths differs per machine). Directories are
--- matched too, so pattern for what you want (`"**/*.rs"`, not `"**"`). Order is unspecified — sort it.
---@param root string
---@param pattern string   # e.g. "**/*.rs"
---@return string[] absolute paths
function fs.glob(root, pattern) end
--- Create a directory and every missing parent (like `mkdir -p`); idempotent (no error if it already
--- exists). The platform-agnostic replacement for `shell.run("mkdir -p ...")`.
---@param path string
function fs.mkdir(path) end

------------------------------------------------------------------------------------------
-- shell
------------------------------------------------------------------------------------------

---@class prova.ShellResult
---@field code integer
---@field stdout string
---@field stderr string
---@field duration number   # seconds
local ShellResult = {}
--- Whether the command succeeded (`code == 0`). Sugar for the common check — `shell.run` does not
--- raise on a non-zero exit unless you pass `opts.check`.
---@return boolean          # code == 0
function ShellResult:ok() end

---@class prova.ShellOpts
---@field cwd? string
---@field env? table<string, string|number|boolean>   # scalars coerce — ports stay numbers
---@field timeout? string     # e.g. "120s"
---@field check? boolean      # non-zero exit raises, carrying the tail of BOTH stdout and stderr
---@field merge_stderr? boolean  # fold stderr into stdout (the portable replacement for `2>&1`); stderr is then empty
---@field stdin? string          # feed the program's stdin (the portable replacement for `printf x | cmd`)

--- A long-running process from `shell.spawn`. Prefer `ctx:defer(function() proc:stop() end)` so it
--- is stopped during teardown; `stop`/`wait` are async.
---@class prova.Process
---@field pid integer|nil       # OS process id (nil if it could not be determined)
local Process = {}
--- Kill the process (SIGKILL) and reap it. Idempotent.
function Process:stop() end
--- Wait for the process to exit; returns its exit code (or nil if signalled / already reaped).
---@return integer|nil
function Process:wait() end
--- Whether the process is still running (reaps it if it has already exited).
---@return boolean
function Process:running() end
--- The process's combined stdout+stderr so far (bounded: last 64KB, oldest dropped). Never-blind
--- boots: assert on it, or print it when readiness times out.
---@return string
function Process:output() end

---@class prova.SpawnOpts
---@field cwd? string
---@field env? table<string, string|number|boolean>   # scalars coerce — ports stay numbers

---@class prova.shell
shell = {}
--- Run a command to completion. A **string** goes through a shell, so `"cargo build --release"`
--- works verbatim. An **argv table** (`{"psql", "-tAc", sql}`) runs the program directly — no
--- shell, no quoting — which is how you pass content (SQL, source, JSON, paths with spaces) safely.
---@param command string|string[]
---@param opts? prova.ShellOpts
---@return prova.ShellResult
function shell.run(command, opts) end
--- Start a long-running command in the background (a booted app, a mock server) and return a
--- handle. stdout/stderr are discarded. Pair with `ctx:defer(function() proc:stop() end)`.
--- Takes a shell string or an **argv table** (no shell, no quoting), exactly like `shell.run`.
---@param command string|string[]
---@param opts? prova.SpawnOpts
---@return prova.Process
function shell.spawn(command, opts) end

------------------------------------------------------------------------------------------
-- http (blocking in v1)
------------------------------------------------------------------------------------------

---@class prova.HttpResponse
---@field status integer
---@field body string
---@field headers table<string,string>
local HttpResponse = {}
---Decode the body as JSON (raises on non-JSON).
---@return table
function HttpResponse:json() end

---@class prova.HttpOpts
---@field headers? table<string,string>
---@field json? table            # request body, JSON-encoded
---@field timeout? string

---@class prova.WaitOpts : prova.HttpOpts
---@field status? integer        # expected status (default 200)
---@field every? string          # poll interval, e.g. "500ms"

---@class prova.http
http = {}
--- Issue a GET. Does **not** raise on 4xx/5xx — assert on `res.status` yourself; only a transport failure (DNS, refused, timeout) raises.
---@param url string
---@param opts? prova.HttpOpts
---@return prova.HttpResponse
function http.get(url, opts) end
--- Issue a POST. Set a body with `opts.body`/`opts.json`. Does **not** raise on 4xx/5xx — assert on `res.status`.
---@param url string
---@param opts? prova.HttpOpts
---@return prova.HttpResponse
function http.post(url, opts) end
--- Issue a PUT. Does **not** raise on 4xx/5xx — assert on `res.status`.
---@param url string
---@param opts? prova.HttpOpts
---@return prova.HttpResponse
function http.put(url, opts) end
--- Issue a PATCH. Does **not** raise on 4xx/5xx — assert on `res.status`.
---@param url string
---@param opts? prova.HttpOpts
---@return prova.HttpResponse
function http.patch(url, opts) end
--- Issue a DELETE. Does **not** raise on 4xx/5xx — assert on `res.status`.
---@param url string
---@param opts? prova.HttpOpts
---@return prova.HttpResponse
function http.delete(url, opts) end
--- Issue a HEAD — status + headers, no body. Cheap liveness/existence check.
---@param url string
---@param opts? prova.HttpOpts
---@return prova.HttpResponse
function http.head(url, opts) end
--- Issue an OPTIONS — the allowed methods/CORS preflight for a resource.
---@param url string
---@param opts? prova.HttpOpts
---@return prova.HttpResponse
function http.options(url, opts) end
---Poll until the endpoint responds as expected or the timeout elapses.
---@param url string
---@param opts? prova.WaitOpts
---@return prova.HttpResponse
function http.wait_for(url, opts) end

---@class prova.HttpClientOpts
---@field base_url string                  # prefixed onto each call's path
---@field headers? table<string,string>    # default headers (per-call headers override by name)
---@field timeout? string                  # default per-call timeout

--- A reusable REST client: base URL + default headers declared once. `path` is joined onto
--- `base_url` (an absolute URL is used verbatim); per-call `opts` override the defaults.
---@class prova.HttpClient
local HttpClient = {}
---@param path string
---@param opts? prova.HttpOpts
---@return prova.HttpResponse
--- Issue a GET against this client's base URL. Does **not** raise on 4xx/5xx — assert on `res.status`.
function HttpClient:get(path, opts) end
---@param path string
---@param opts? prova.HttpOpts
---@return prova.HttpResponse
--- Issue a POST against this client's base URL. Does **not** raise on 4xx/5xx.
function HttpClient:post(path, opts) end
---@param path string
---@param opts? prova.HttpOpts
---@return prova.HttpResponse
--- Issue a PUT against this client's base URL. Does **not** raise on 4xx/5xx.
function HttpClient:put(path, opts) end
---@param path string
---@param opts? prova.HttpOpts
---@return prova.HttpResponse
--- Issue a PATCH against this client's base URL. Does **not** raise on 4xx/5xx.
function HttpClient:patch(path, opts) end
---@param path string
---@param opts? prova.HttpOpts
---@return prova.HttpResponse
--- Issue a DELETE against this client's base URL. Does **not** raise on 4xx/5xx.
function HttpClient:delete(path, opts) end
---@param path string
---@param opts? prova.HttpOpts
---@return prova.HttpResponse
--- Issue a HEAD against this client's base URL — status + headers, no body.
function HttpClient:head(path, opts) end
---@param path string
---@param opts? prova.HttpOpts
---@return prova.HttpResponse
--- Issue an OPTIONS against this client's base URL.
function HttpClient:options(path, opts) end
---@param path string
---@param opts? prova.WaitOpts
---@return prova.HttpResponse
--- Poll a path until it answers as expected (readiness). Retries the real request rather than sleeping.
function HttpClient:wait_for(path, opts) end

--- Build a reusable REST client bound to a base URL and default headers.
---@param opts prova.HttpClientOpts
---@return prova.HttpClient
function http.client(opts) end

-- ---------------------------------------------------------------------------------------------
-- http.mock — the `mock` facet: provision a *fake* one
-- ---------------------------------------------------------------------------------------------

--- A request as the mock saw it. A handler's argument and a journal entry are the **same shape**, so
--- `req.path` in a handler and `m:received()[1].path` in an assertion mean the same thing.
---@class prova.MockRequest
---@field method string                    # "GET", "POST", … (uppercase)
---@field path string                      # path only; the query string is parsed into `query`
---@field query table<string,string>       # parsed and percent-decoded
---@field headers table<string,string>     # header names lowercased (HTTP names are case-insensitive)
---@field body string                      # the raw request bytes
---@field json? any                        # the body decoded, when it parses as JSON; nil otherwise
---@field params table<string,string>      # captures from the stub's `route` (empty for other stubs)
---@field status? integer                  # journal only: the status the mock answered with
---@field matched? boolean                 # journal only: whether a *stub* matched
---@field source? string                   # journal only: "stub" | "passthrough" | "replay" | "unmatched"
---@field error? string                    # journal only: why a handler or upstream failed, if it did

--- A stub's response. `json` and `body` are mutually exclusive — a response has one body.
---@class prova.MockReply
---@field status? integer                  # default 200
---@field json? any                        # encoded as JSON; sets content-type unless you set it
---@field body? string                     # a raw body
---@field headers? table<string,string>
---@field delay? string                    # hold the response this long ("250ms") — fault injection

--- What a request must look like for a stub to answer it. Omitted fields don't constrain, so
--- `m:on{ path = "/x" }` matches any method. **First matching stub wins**, in declaration order.
---@class prova.MockMatch
---@field method? string                   # matched case-insensitively
---@field path? string                     # exact — a literal `:` is NOT a param (`/models/x:predict`)
---@field path_matches? string             # a Lua pattern (the same dialect as `:matches(pat)`)
---@field route? string                    # a template: "/orders/:id" → `req.params.id`. Segment-wise,
---                                        #   so a param never swallows a `/`.

---@class prova.MockStub
local MockStub = {}

--- Answer matching requests with `reply` — a response table (terse), or a **function** taking the
--- request and returning one (general). The function is real Lua, run on this Lua state at request
--- time while the coroutine driving the SUT is suspended, so it can compute from the request and
--- close over test locals. That is why there is no response-templating language to learn.
---@param reply prova.MockReply|fun(req: prova.MockRequest): prova.MockReply
function MockStub:reply(reply) end

--- A mock HTTP server: a real server, in this process, that you stub and then assert on. Grammar-
--- shaped like any resource — wire `m.url` into the system under test exactly as you would a
--- database's, and it is torn down with the scope that owns it.
---@class prova.MockServer
---@field url string       # "http://127.0.0.1:<port>"
---@field endpoint string  # the driver-target alias == url
---@field host string      # "127.0.0.1"
---@field port integer     # the bound port (random, so parallel tests don't collide)
---@field network? { url: string, host: string, port: integer }  # cross-substrate vantage; present only with `network`
local MockServer = {}

--- Register a stub. Returns the stub so you can `:reply(…)` it.
---@param match prova.MockMatch
---@return prova.MockStub
function MockServer:on(match) end

--- Everything the mock was asked, in order — **as data**, for the ordinary matchers to assert on
--- (`t:expect(m:received()):has_length(1)`). There is no `verify(count, pattern)` DSL because
--- `t:expect` already exists. Unmatched requests are recorded too: a call you did not predict is
--- usually the most interesting thing a mock can tell you.
---@param filter? { method?: string, path?: string }
---@return prova.MockRequest[]
function MockServer:received(filter) end

--- Stop serving. Idempotent — the owning scope calls this too.
function MockServer:stop() end

--- The observe dial. A proxy is not a second concept: it is a mock whose *unmatched* requests are
--- forwarded rather than 404'd. Stubs always win, so you can stub one endpoint and let the rest reach
--- the real service (partial mocking).
---@class prova.MockOpts
---@field passthrough? string   # forward unmatched requests to this base URL — the dependency stays REAL
---@field record? string        # write forwarded exchanges to this cassette on teardown (needs `passthrough`)
---@field replay? string        # answer from a cassette; no dependency, no network (excludes `passthrough`)
---@field redact? string[]      # extra header names to redact in the cassette (auth/cookies are redacted anyway)
---@field allow_handler_errors? boolean  # a raising `:reply` handler normally FAILS the owning scope at
---                                      #   teardown (a SUT with a fallback would otherwise swallow the
---                                      #   500 and hide prova's own bug). Set true when the error path
---                                      #   is the subject of the test.
---@field network? boolean|string        # expose a `.network` vantage for a containerized/VM'd SUT to reach
---                                      #   this HOST-bound mock. Binds 0.0.0.0 (a real LAN exposure, hence
---                                      #   opt-in). `true` → `host.docker.internal`; a string overrides the
---                                      #   host name for another substrate.

--- Provision a mock HTTP server, tied to `ctx`'s scope. The fourth facet: `client` attaches to a
--- real dependency, `container` provisions a real one, `wait_for` probes one — `mock` provisions a
--- **fake** one. For a **stateful fake**, close over a table in your fixture and mutate it from a
--- `:reply` handler (it is real Lua); assert on that table directly. Reach for a mock on the boundary
--- you cannot run, for behavior the real thing won't produce on demand, or to assert on the
--- interaction itself — if you *can* run the real thing, run it (`prova.containerized`). The listener
--- is bound before this returns, so the first request cannot race it — no `prova.retry` needed.
--- See `examples/ordering_test.lua` for a worked stateful fake.
---@param ctx prova.Context|prova.TestContext
---@param opts? prova.MockOpts
---@return prova.MockServer
function http.mock(ctx, opts) end

------------------------------------------------------------------------------------------
-- archetect (plugin: in-process render via archetect-core)
------------------------------------------------------------------------------------------

---@class prova.RenderOpts
---@field source string                    # local path or git URL
---@field answers? table<string,any>       # prompt answers as data
---@field switches? string[]
---@field defaults? boolean                # use defaults for unanswered prompts (headless)
---@field destination? string              # optional; a temp dir is used if omitted

---A render result: a `Tree` plus the ordered IO-protocol write operations it intended.
---@class prova.RenderResult : prova.Tree
---@field writes table[]                   # ordered WriteFile/WriteDirectory ops

---@class prova.archetect
archetect = {}
---Render an archetype in-process and return its output tree.
---@param opts prova.RenderOpts
---@return prova.RenderResult
function archetect.render(opts) end

--- The checks `archetect.verify` registers against a rendering — prova's answer to the pytest
--- harness's `manifest.yaml`, matched field-for-field but as real Lua you can extend.
---@class prova.VerifyChecks
---@field name? string                       # label for the generated tests (default "archetype")
---@field project_dir? string                # assert relative to this subdirectory the render produces
---@field expected_files? string[]           # must exist (relative to project_dir)
---@field absent_files? string[]             # must NOT exist
---@field yaml_globs? string[]               # each glob must match ≥1 file; each match must parse
---@field fully_rendered? boolean            # assert no leftover template markers (default true)
---@field requires? string[]                 # capabilities gating the build step (else skip)
---@field build_steps? (string|string[])[]   # commands run sequentially in project_dir
---@field env? table<string,string>          # extra environment for build_steps
---@field timeout? string                    # per build step (default "600s")

--- The one-shot form's spec: the checks plus the render itself. Anything a prompt needs and the
--- answers omit falls back to its default; a prompt with no default and no answer errors (headless
--- never hangs).
---@class prova.VerifySpec : prova.VerifyChecks
---@field source string                      # local path or git URL
---@field answers? table<string,any>         # prompt answers as data
---@field switches? string[]
---@field defaults? boolean                  # headless defaults for unanswered prompts (default true)
---@field scope? prova.Scope                 # scope of the render fixture it creates (default Scope.File)

--- Register the standard layout/fully-rendered/yaml/build checks against a rendering, returning the
--- render fixture so you can hang boot/probe fixtures and extra tests off the same output.
---
--- Two forms over one core — the compositional form makes render → verify → black-box one pipeline:
---   archetect.verify{ source = ..., <checks> }        -- one-shot: renders for you
---   archetect.verify(project_fixture, { <checks> })   -- checks a render fixture you declared
---@param spec prova.VerifySpec
---@return prova.Fixture
---@overload fun(fixture: prova.Fixture, checks: prova.VerifyChecks): prova.Fixture
function archetect.verify(spec) end

------------------------------------------------------------------------------------------
-- docker (testcontainers-style ephemeral dependencies, via the Docker daemon API / bollard)
------------------------------------------------------------------------------------------

--- A running container from `docker.run`. Prefer `ctx:defer(function() c:stop() end)` so it is
--- removed during teardown; `stop`/`logs`/`exec` are async.
---@class prova.Container
---@field id string
local Container = {}
--- The host port a published container port maps to.
---@param container_port integer
---@return integer
function Container:host_port(container_port) end
--- Convenience: "127.0.0.1:<host_port>" for a published container port.
---@param container_port integer
---@return string
function Container:endpoint(container_port) end
--- The container's combined stdout+stderr logs.
---@return string
function Container:logs() end
--- Run a command inside the container (`sh -c`); returns (code, stdout, stderr). Low-level and
--- non-raising — prefer `Container:run` for driving a CLI.
---@param command string
---@return integer, string, string
function Container:exec(command) end
--- Run a command inside the container and return its stdout, raising on a non-zero exit. The
--- exec-CLI SDK entry point: pass an **argv table** to run a CLI directly (no shell, no quoting), or
--- a **string** to run under `sh -c` (for pipes/globs). `opts.stdin` is piped to the process.
---@param command string|string[]
---@param opts? { stdin?: string }
---@return string stdout
function Container:run(command, opts) end
--- The alias this container answers to on its user-defined network (from `docker.run`'s `alias`),
--- or nil if it joined no network or joined one without an alias. Siblings resolve it by DNS.
---@return string|nil
function Container:network_alias() end
--- Force-remove the container. Idempotent.
function Container:stop() end

--- A user-defined bridge network from `docker.network` — containers joined to it resolve each
--- other by name/alias over Docker's embedded DNS. Manage it with `ctx:manage(net)` so it is
--- removed on teardown, LIFO, *after* its containers.
---@class prova.Network
---@field name string
local Network = {}
--- Remove the network. Idempotent; retries briefly while endpoints are still detaching.
function Network:stop() end

--- Readiness gate. Give exactly ONE signal — `port`, `log`, or `cmd` — each honest about a different
--- observable. Use `port` when a listening socket means serving (Redis, nginx); use `cmd` when it does
--- not (Postgres binds TCP then finishes startup — `pg_isready` is the race-free check).
---@class prova.DockerWait
---@field port? integer       # ready when this container port is in LISTEN state (asks the container, not the host)
---@field log? string         # ready when the logs contain this substring
---@field cmd? string[]       # ready when this command, run in the container, exits 0 (e.g. {"pg_isready","-U","postgres"})
---@field timeout? string     # default "30s"
---@field every? string       # poll interval, default "250ms"

---@class prova.DockerRunOpts
---@field image string
---@field command? string|string[]      # override the image CMD ("bin/pulsar standalone" or a list)
---@field ports? (integer|{container:integer, host:integer})[]  # container ports → random host ports (or a fixed host port)
---@field env? table<string,string>
---@field wait? prova.DockerWait        # readiness gate
---@field network? prova.Network|string # a user-defined network to join at create time (handle or name)
---@field alias? string                 # DNS alias to answer to on `network` (requires `network`)
---@field extra_hosts? string[]        # `"name:ip"` entries added to the container's /etc/hosts, e.g. "host.docker.internal:host-gateway" (Linux) to reach a host-bound mock

---@class prova.DockerNetworkOpts
---@field name? string                  # override the generated unique "prova-net-<...>" name

---@class prova.DockerBuildSecret
---@field env? string                   # read the secret from this variable in prova's own environment
---@field file? string                   # a file already on disk (must exist)
---@field value? string                  # a literal value; written to a 0600 temp file for the build and removed after

---@class prova.DockerBuildOpts
---@field context string                # the build-context directory
---@field dockerfile? string            # relative to `context` (default "Dockerfile"); `COPY` still resolves against the context root
---@field tag? string                   # default: a stable tag derived from `context`, so rebuilds replace it and the layer cache hits
---@field buildargs? table<string,string|number|boolean>
---@field secrets? table<string, prova.DockerBuildSecret> # BuildKit secrets by id, for `RUN --mount=type=secret,id=…`. Exactly one of env/file/value per secret
---@field target? string                # multi-stage build target
---@field pull? boolean                 # always re-pull the base image (default false)
---@field nocache? boolean              # ignore the build cache (default false)

---@class prova.docker
docker = {}
--- Start a container (detached, `--rm`) and return a handle once it is ready. The image is pulled
--- only if it is not already local (`docker run`'s own rule), so a locally-built image works.
---@param opts prova.DockerRunOpts
---@return prova.Container
function docker.run(opts) end
--- Build a local image from a Dockerfile and return its ref, ready for `docker.run{ image = … }`.
--- Honors `.dockerignore`, BuildKit cache mounts, and BuildKit secrets. Raises with the builder's
--- log if the build fails.
---
--- A production Dockerfile that reads a private registry token via
--- `RUN --mount=type=secret,id=npm` is built by naming that id in `secrets`:
--- `secrets = { npm = { env = "NPM_TOKEN" } }`. Prefer this over `buildargs` for anything sensitive
--- — build args are recorded in image history, which is what BuildKit secrets exist to avoid.
---@param opts prova.DockerBuildOpts
---@return string image                 # the image ref (the resolved `tag`)
function docker.build(opts) end
--- Create a user-defined bridge network (embedded DNS). Manage it with `ctx:manage`.
---@param opts? prova.DockerNetworkOpts
---@return prova.Network
function docker.network(opts) end

------------------------------------------------------------------------------------------
-- sqlite (an embedded database via sqlx — the only bundled resource client; needs no docker)
------------------------------------------------------------------------------------------

--- A database connection from `sqlite.client`. Methods are async; pair with `ctx:manage(conn)` to
--- close it on teardown. Use `?` placeholders in SQL.
---@class prova.Connection
local Connection = {}
--- Run a statement (INSERT/UPDATE/DDL); returns the number of rows affected.
---@param sql string
---@param params? any[]
---@return integer
function Connection:execute(sql, params) end
--- Run a query; returns a list of rows, each a table of column name -> value (NULL -> nil).
---@param sql string
---@param params? any[]
---@return table<string, any>[]
function Connection:query(sql, params) end
--- Query returning a single scalar (first column of the first row), or nil.
---@param sql string
---@param params? any[]
---@return any
function Connection:query_value(sql, params) end
--- Close the connection pool.
function Connection:close() end

---@class prova.sqlite
sqlite = {}
--- Open a SQLite database by URL (`sqlite://<path>?mode=rwc`, or `sqlite::memory:`). Nothing to
--- provision — there is no `sqlite.container`.
---@param url string
---@return prova.Connection
function sqlite.client(url) end

------------------------------------------------------------------------------------------
-- grpc (native dynamic client via server reflection; no `grpcurl`, no `.proto` files)
------------------------------------------------------------------------------------------

--- A connected gRPC client from `grpc.client`. It learned the server's schema at connect time via
--- gRPC Server Reflection, so calls take a plain request table and return a response table — no
--- generated code. Methods are async; the server must have reflection enabled. Plaintext-only in v1.
---@class prova.GrpcClient
local GrpcClient = {}
--- Invoke a unary method (`"package.Service/Method"`), raising on a non-OK gRPC status.
---@param method string
---@param request? table
---@return table response
function GrpcClient:call(method, request) end
--- Like `call`, but never raises: returns `{ ok, code, message, response }` so a test can assert on
--- the gRPC status code (e.g. `"NotFound"`, `"InvalidArgument"`). `response` is nil unless `ok`.
---@param method string
---@param request? table
---@return prova.GrpcStatus
function GrpcClient:call_status(method, request) end

---@class prova.GrpcStatus
---@field ok boolean
---@field code string          # gRPC status code name, e.g. "Ok" | "NotFound" | "InvalidArgument"
---@field message string
---@field response? table

---@class prova.GrpcClientOpts
---@field timeout? string      # per-call deadline, e.g. "30s"

---@class prova.GrpcWaitOpts
---@field timeout? string      # overall deadline (default "30s")
---@field every? string        # poll interval (default "500ms")

---@class prova.grpc
grpc = {}
--- A client for the gRPC server at `addr` (`"host:port"` or `"http://host:port"`), performing
--- reflection once to discover its services. Must be called inside a fixture or test body (async).
---@param addr string
---@param opts? prova.GrpcClientOpts
---@return prova.GrpcClient
function grpc.client(addr, opts) end
--- Poll until the server answers a reflection request or the timeout elapses (boot-then-probe).
---@param addr string
---@param opts? prova.GrpcWaitOpts
function grpc.wait_for(addr, opts) end

-- ---------------------------------------------------------------------------------------------
-- grpc.mock — the `mock` facet on the grpc namespace
-- ---------------------------------------------------------------------------------------------

--- An RPC as the mock saw it. A handler's argument and a journal entry are the **same shape**.
---@class prova.GrpcMockCall
---@field method string                 # "package.Service/Method"
---@field request any                   # the decoded request message, as a table
---@field code? string                  # journal only: the status the mock answered ("Ok", "NotFound", …)
---@field matched? boolean              # journal only: whether any stub matched
---@field error? string                 # journal only: why a handler failed, if it did

--- What a stub answers: a `response` message, or a non-Ok `code` — not both, because an RPC returns
--- a message or a status. `code` uses the spelling `client:call_status` **reports**, so what a
--- failure tells you is what you write to reproduce it.
---@class prova.GrpcMockReply
---@field response? any                 # the reply message, as a table (default: an empty message)
---@field code? string                  # "NotFound", "ResourceExhausted", … (default "Ok")
---@field message? string               # the status message, for a non-Ok code
---@field delay? string                 # hold the reply this long ("250ms") — fault injection

--- Which RPCs a stub answers. Omitted fields don't constrain. **First matching stub wins**, in
--- declaration order.
---@class prova.GrpcMockMatch
---@field method? string                # exact: "package.Service/Method"
---@field method_matches? string        # a Lua pattern (the same dialect as `:matches(pat)`)

---@class prova.GrpcMockStub
local GrpcMockStub = {}

--- Answer matching RPCs with `reply` — a table, or a **function** taking the call and returning one.
--- The function is real Lua, run at request time on this Lua state while the coroutine driving the
--- system under test is suspended, so it can compute from the request and close over test locals.
---@param reply prova.GrpcMockReply|fun(call: prova.GrpcMockCall): prova.GrpcMockReply
function GrpcMockStub:reply(reply) end

--- A mock gRPC server: a real server, in this process. It **serves reflection**, so `grpc.client`
--- drives it with no special case — and `m.url` wires into a system under test exactly as a real
--- service's would.
---@class prova.GrpcMock
---@field url string       # "http://127.0.0.1:<port>"
---@field host string      # "127.0.0.1"
---@field port integer     # the bound port (random, so parallel tests don't collide)
---@field endpoint string  # the driver-target alias == "host:port" (what grpc.client takes)
---@field network? { url: string, host: string, port: integer }  # cross-substrate vantage; present only with `network`
local GrpcMock = {}

---@param match prova.GrpcMockMatch
---@return prova.GrpcMockStub
--- Register a canned response for a method on this mock server.
function GrpcMock:on(match) end

--- Every RPC the mock was asked, in order — as data, for the ordinary matchers. Unstubbed calls are
--- recorded too (they answer `Unimplemented`): a call you did not predict is usually the most
--- interesting thing a mock can tell you.
---@param filter? { method?: string }
---@return prova.GrpcMockCall[]
function GrpcMock:received(filter) end

--- Stop serving. Idempotent — the owning scope calls this too. In a `grpc.proxy` record mode,
--- the cassette flush point.
function GrpcMock:stop() end
--- Alias for `:stop()` — the proxy grammar's teardown verb.
function GrpcMock:close() end
--- (grpc.proxy) Continuous latency on every call, both directions ("2s"). The transport-generic
--- fault verb; byte-level corrupt/throttle stay socket-only.
---@param duration string
function GrpcMock:latency(duration) end
--- (grpc.proxy) Sever — subsequent calls fail with `Unavailable`.
function GrpcMock:drop() end
--- (grpc.proxy) Schedule a fault: `p:after("100ms"):drop()` — healthy first, injured later.
---@param delay string
---@return prova.GrpcMock   # a scheduled handle speaking drop/latency
function GrpcMock:after(delay) end

---@class prova.GrpcMockOpts
---@field proto string|string[]         # `.proto` path(s), compiled at runtime (pure Rust; no protoc)
---@field includes? string[]            # import paths (default: each proto's own directory)
---@field allow_handler_errors? boolean # a raising `:reply` handler normally FAILS the owning scope at
---                                     #   teardown; set true when the error path is the subject
---@field network? boolean|string       # expose a `.network` host-gateway vantage (binds 0.0.0.0); see http.mock

--- Provision a mock gRPC server, tied to `ctx`'s scope.
---
--- Unlike `grpc.client`, a mock **must be told its schema**: the client needs no `.proto` because it
--- learns one *from the server* by reflection, and a mock is the server — there is nobody to learn
--- from. Hence `proto`.
---
--- Reach for it on the boundary you cannot run, for status codes the real service won't produce on
--- demand, or to assert on the **interaction itself**. If you can run the real service, run it.
---@param ctx prova.Context|prova.TestContext
---@param opts prova.GrpcMockOpts
---@return prova.GrpcMock
function grpc.mock(ctx, opts) end

---@class prova.GrpcProxyOpts
---@field upstream? string   # the real gRPC server (host:port), needed by passthrough/record/auto
---@field cassette? string   # the recording file, needed by record/replay/auto
---@field mode? string       # "passthrough" (default) | "record" | "replay" | "auto"
---@field redact? string[]   # literal strings scrubbed from the cassette at record time

--- Interpose on a real gRPC dependency (docs/design/mocks-proxies-drivers.md). Record captures the
--- upstream's schema (learned by reflection) AND each unary call into a cassette, so a replay proxy
--- needs no `.proto` and no upstream — the cassette is self-describing. The match key is the full
--- method plus the request message; a replay miss is a loud `Unavailable` naming the cassette.
--- Same object as `grpc.mock` — its `:received()` journal and `:on` stubs apply. Close (or scope
--- exit) is the flush point. Returns a mock (dial the SUT's gRPC client at `.host`/`.port`).
---@param ctx prova.Context|prova.TestContext
---@param opts prova.GrpcProxyOpts
---@return prova.GrpcMock
function grpc.proxy(ctx, opts) end

------------------------------------------------------------------------------------------
-- yaml (parse YAML text to Lua values — k8s manifests, CI configs, compose files)
------------------------------------------------------------------------------------------

---@class prova.yaml
yaml = {}
--- Parse a single YAML document into a Lua value. Raises on invalid YAML.
---@param text string
---@return any
function yaml.decode(text) end
--- Parse a multi-document YAML stream (`---`-separated, as in Kubernetes manifests) into a list of
--- Lua values. Raises on the first invalid document.
---@param text string
---@return any[]
function yaml.decode_all(text) end
--- Emit one value as a YAML document. Carries the json sentinels: `json.null` emits an explicit
--- null; `json.array{}` forces an empty sequence.
---@param v any
---@return string
function yaml.encode(v) end
--- Emit a list of values as one `---`-separated multi-document stream (the Kubernetes manifest
--- shape) — the exact inverse of `yaml.decode_all`.
---@param docs any[]
---@return string
function yaml.encode_all(docs) end

------------------------------------------------------------------------------------------
-- json / toml / csv — tech-first format modules: decode AND encode together (api-freeze §1)
------------------------------------------------------------------------------------------

---@class prova.JsonEncodeOpts
---@field pretty? boolean    # indented output (default compact)

---@class prova.json
---@field null lightuserdata # explicit-null sentinel: emit null from encode, assert null in shapes
json = {}
--- Parse JSON into a Lua value. `null` decodes to `nil` — top-level or nested — so absent and
--- null both read as nil (assert explicit null with `json.null` in a `:matches` shape instead).
---@param s string
---@return any
function json.decode(s) end
--- Encode a Lua value as JSON text (compact by default). A bare empty table encodes as an object
--- (`{}`); wrap it in `json.array` to force `[]`; `json.null` emits an explicit null.
---@param v any
---@param opts? prova.JsonEncodeOpts
---@return string
function json.encode(v, opts) end
--- Mark a table as an array for encoding (forces `[]` when empty). Returns the same table.
---@param t table
---@return table
function json.array(t) end

---@class prova.toml
toml = {}
--- Parse TOML text into a Lua value. Raises on invalid TOML.
---@param s string
---@return table
function toml.decode(s) end
--- Encode a table as TOML text. TOML has no null, so `json.null` is an encode error here.
---@param v table
---@return string
function toml.encode(v) end

---@class prova.CsvOpts
---@field delimiter? string  # single-byte field delimiter (default ",")
---@field headers? string[]  # encode only: column order (default: first row's keys, sorted)

---@class prova.csv
csv = {}
--- Parse CSV text, header-aware: the first record names the columns, every remaining record
--- becomes a map keyed by header (the `prova.parse.table` row shape; values stay strings).
---@param s string
---@param opts? prova.CsvOpts
---@return table<string, string>[]
function csv.decode(s, opts) end
--- Encode a list of header-keyed row maps as CSV text with a header line. Quoting is automatic
--- (RFC 4180).
---@param rows table<string, any>[]
---@param opts? prova.CsvOpts
---@return string
function csv.encode(rows, opts) end

------------------------------------------------------------------------------------------
-- base64 / hash / uuid / url — the utility belt (api-freeze §1)
------------------------------------------------------------------------------------------

---@class prova.base64
base64 = {}
--- Base64-encode a string (standard alphabet, padded). Binary-safe.
---@param s string
---@return string
function base64.encode(s) end
--- Decode base64 text to the original bytes. Raises on invalid input.
---@param s string
---@return string
function base64.decode(s) end

---@class prova.hash
hash = {}
--- SHA-256 digest as lowercase hex.
---@param s string
---@return string
function hash.sha256(s) end
--- HMAC-SHA-256 of `msg` keyed by `key`, as lowercase hex.
---@param key string
---@param msg string
---@return string
function hash.hmac_sha256(key, msg) end

---@class prova.uuid
uuid = {}
--- A random (v4) RFC 4122 UUID, hyphenated lowercase.
---@return string
function uuid.v4() end

---@class prova.UrlParts
---@field scheme string
---@field host? string
---@field port? integer      # written, or the scheme's well-known default
---@field path string
---@field query? string
---@field fragment? string

---@class prova.url
url = {}
--- Parse a URL into its structured parts. Raises on an invalid URL.
---@param s string
---@return prova.UrlParts
function url.parse(s) end
--- Percent-encode a string as one URL component: everything but RFC 3986 unreserved characters
--- is escaped (space → `%20`, not `+`).
---@param s string
---@return string
function url.encode(s) end

------------------------------------------------------------------------------------------
-- graphql (POST { query, variables } → { data, errors } over HTTP — the third transport)
------------------------------------------------------------------------------------------

--- A GraphQL client bound to one endpoint (queries and mutations share the transport).
---@class prova.GraphqlClient
local GraphqlClient = {}
--- Run a query/mutation and return its `data`. Raises if the response carries GraphQL `errors`.
---@param query string
---@param variables? table
---@return any data
function GraphqlClient:query(query, variables) end
--- Like `query`, but never raises: returns `{ data, errors, status }` so a test can assert on
--- GraphQL errors (mirrors grpc's `call_status`). `data`/`errors` are nil when absent/null.
---@param query string
---@param variables? table
---@return prova.GraphqlResult
function GraphqlClient:execute(query, variables) end

---@class prova.GraphqlResult
---@field status integer
---@field data? any
---@field errors? table[]

---@class prova.GraphqlClientOpts
---@field url string
---@field headers? table<string,string>
---@field timeout? string

---@class prova.graphql
graphql = {}
--- Build a GraphQL client for one endpoint.
---@param opts prova.GraphqlClientOpts
---@return prova.GraphqlClient
function graphql.client(opts) end

-- ---------------------------------------------------------------------------------------------
-- http.proxy — the interpose posture as its own verb (mocks-proxies-drivers.md)
-- ---------------------------------------------------------------------------------------------

---@class prova.HttpProxyOpts
---@field upstream? string   # the real dependency's base url (needed by passthrough/record/auto)
---@field cassette? string   # the recording file (needed by record/replay/auto)
---@field mode? string       # "passthrough" (default) | "record" | "replay" | "auto"
---@field redact? string[]   # extra headers scrubbed at record time (defaults already cover auth)

--- Interpose on a real HTTP dependency: forward traffic, record it into a cassette, or replay a
--- cassette with the upstream gone ("a proxy in record mode manufactures a mock"). Sugar over
--- `http.mock`'s dial — same object back, so stubs/journal/faults all apply. `mode = "auto"`
--- records when the cassette file is absent and replays when present; a replay miss is a loud
--- 502 naming the cassette. Close (or scope exit) is the cassette flush point.
---@param ctx any            # the test or fixture context (`t` / `ctx`)
---@param opts prova.HttpProxyOpts
---@return prova.MockServer
function http.proxy(ctx, opts) end

-- ---------------------------------------------------------------------------------------------
-- socket — the L4 kernel transport: the full triad over tcp:// and unix:// (one namespace,
-- unified by address scheme; framing turns bytes into matchable turns)
-- ---------------------------------------------------------------------------------------------

---@alias prova.SocketFraming string|{ length_prefixed?: integer, delimiter?: string }

---@class prova.SocketConn
local SocketConn = {}
--- Write one turn (framed) or raw bytes (no framing). Async.
---@param data string
function SocketConn:send(data) end
--- Framed: read one whole turn — `recv(opts?)`. Raw: read exactly n bytes — `recv(n, opts?)`.
--- Errors loud on timeout (default 10s) or a closed connection; never hangs.
---@param n_or_opts? integer|{ timeout?: string }
---@param opts? { timeout?: string }
---@return string
function SocketConn:recv(n_or_opts, opts) end
--- Close the connection.
function SocketConn:close() end

---@class prova.SocketListener
---@field addr string        # the resolved address ("tcp://127.0.0.1:<port>" / "unix://…")
local SocketListener = {}
--- Accept one connection (async, timeout'd — default 10s).
---@param opts? { timeout?: string }
---@return prova.SocketConn
function SocketListener:accept(opts) end
--- Stop listening (what `ctx:manage` calls; a unix socket path is reaped).
function SocketListener:stop() end

---@class prova.SocketStub
local SocketStub = {}
--- Answer this turn with `data` (framed onto the wire).
---@param data string
function SocketStub:reply(data) end

---@class prova.SocketMock
---@field addr string        # endpoint symmetry: the same string a driver's connect takes
---@field endpoint string    # the driver-target alias == addr
local SocketMock = {}
--- Stub one framed turn (exact bytes). First matching stub wins; an unmatched turn is journaled
--- (§6: matched=false, source="unmatched") and the connection closes LOUD.
---@param turn string
---@return prova.SocketStub
function SocketMock:on(turn) end
--- The §6 journal: `{ seq, data, matched, source }[]`, filterable by subset table or predicate.
---@param filter? table|fun(entry: table): boolean
---@return table[]
function SocketMock:received(filter) end
--- Stop the mock (what `ctx:manage` calls).
function SocketMock:stop() end

---@class prova.SocketProxy
---@field addr string        # dial this instead of the upstream — the wiretap is transparent
local SocketProxy = {}
--- The direction-tagged transcript: `{ seq, dir ("up"|"down"), data }[]` — framed: turns; raw: chunks.
---@return table[]
function SocketProxy:transcript() end
--- Continuous latency on every turn/chunk, both directions ("2s", "300ms").
---@param duration string
function SocketProxy:latency(duration) end
--- Sever every live connection and refuse new ones. Loud, immediate.
function SocketProxy:drop() end
--- Corrupt bytes in flight (length preserved, contents not).
function SocketProxy:corrupt() end
--- Rate-limit the stream ("8kbps", "1mbps").
---@param rate string
function SocketProxy:throttle(rate) end
--- Put a fuse on any fault: `p:after("500ms"):drop()` — healthy first, injured later.
---@param delay string
---@return prova.SocketProxy   # a scheduled handle speaking drop/latency/corrupt/throttle
function SocketProxy:after(delay) end
--- Stop the proxy (what `ctx:manage` calls). In a record cassette mode, the flush point.
function SocketProxy:stop() end
--- Alias for `:stop()` — the proxy grammar's spelling; in record mode it flushes the cassette.
function SocketProxy:close() end

---@class prova.socket
socket = {}
--- Listen (originate, server side): bind `tcp://host:port` (`:0` = ephemeral, resolved into
--- `.addr`) or `unix:///path`, accept raw or framed connections. Torn down with the scope.
---@param ctx any
---@param opts { addr?: string, framing?: prova.SocketFraming }
---@return prova.SocketListener
function socket.listen(ctx, opts) end
--- Connect (originate): dial a `tcp://`/`unix://` address. Async.
---@param addr string
---@param opts? { framing?: prova.SocketFraming }
---@return prova.SocketConn
function socket.connect(addr, opts) end
--- Terminate: a mock that answers framed turns synthetically. Framing is REQUIRED — matching
--- needs turns; raw exchanges are scripted with `socket.listen`/`accept` instead.
---@param ctx any
---@param opts { addr?: string, framing: prova.SocketFraming }
---@return prova.SocketMock
function socket.mock(ctx, opts) end
--- Interpose: the universal wiretap. Put it in front of ANY tcp/unix dependency for transcripts
--- and fault injection (latency/drop/corrupt/throttle/after) with zero protocol knowledge.
---
--- With a `cassette` + `mode` it becomes a framed-turn recorder: `record` captures request→response
--- turns (flushed on `:close()`), `replay` answers from the cassette with the upstream gone,
--- `auto` records when the file is absent and replays when present. A replay miss severs the
--- connection loud. Cassettes require `framing` (a raw byte stream has no turn to key on).
---@param ctx any
---@param opts { upstream?: string, addr?: string, framing?: prova.SocketFraming, cassette?: string, mode?: string, redact?: string[] }
---@return prova.SocketProxy
function socket.proxy(ctx, opts) end

-- ---------------------------------------------------------------------------------------------
-- terminal — the pty transport: drive TUIs/CLIs, observe a real screen
-- ---------------------------------------------------------------------------------------------

---@class prova.Screen
---@field rows integer
---@field cols integer
local Screen = {}
--- The rendered frame text.
---@return string
function Screen:text() end
--- One rendered line (0-based — screen geometry is coordinates, not Lua arrays).
---@param n integer
---@return string
function Screen:line(n) end
--- Whether the rendered frame contains `s`.
---@param s string
---@return boolean
function Screen:contains(s) end
--- One styled cell (0-based row, col): `{ char, fg, bg, bold }` with named ANSI colors ("red").
---@param row integer
---@param col integer
---@return { char: string, fg: string, bg: string, bold: boolean }
function Screen:cell(row, col) end
--- The snapshot protocol: what `t:expect(screen):matches_snapshot(name)` compares (the frame text).
---@return string
function Screen:snapshot_text() end

---@class prova.Term
local Term = {}
--- Write bytes to the pty (include "\r" to press enter).
---@param data string
function Term:send(data) end
--- Block until the output transcript contains `pattern` (plain bytes), with a timeout (default
--- 10s) — observe-until-match, never a sleep. The failure carries the current screen.
---@param pattern string
---@param opts? { timeout?: string }
function Term:expect(pattern, opts) end
--- Settle the frame: returns once output has been quiet for a beat (or at EOF). The anti-sleep.
---@param opts? { timeout?: string }
function Term:wait_stable(opts) end
--- A frozen frame of the screen — plain data, safe to assert on.
---@return prova.Screen
function Term:screen() end
--- Resize the pty (a real SIGWINCH the program observes — `stty size` reports the new numbers).
---@param cols integer
---@param rows integer
function Term:resize(cols, rows) end
--- Deliver a POSIX signal to the child ("INT", "TERM", …) — prove clean Ctrl-C handling.
---@param name string
function Term:signal(name) end
--- Reap the child and report `{ code }` (polls; default timeout 30s).
---@param opts? { timeout?: string }
---@return { code: integer }
function Term:wait(opts) end
--- Kill the child and close the pty (what `ctx:manage` calls).
function Term:stop() end

---@class prova.TerminalMockStep
local TerminalMockStep = {}
--- Complete the pair: when stdin contains the expected bytes, answer with `reply`.
---@param reply string
function TerminalMockStep:send(reply) end

---@class prova.TerminalMock
---@field env table<string,string>   # PATH-prefixed env — hand to whatever spawns the SUT
---@field path string                # the shim's absolute path
local TerminalMock = {}
--- Script one expectation: `fake:expect("password:"):send("hunter2\n")`.
---@param pattern string
---@return prova.TerminalMockStep
function TerminalMock:expect(pattern) end
--- Remove the shim (what `ctx:manage` calls).
function TerminalMock:stop() end

---@class prova.terminal
terminal = {}
--- Spawn a program on a real pty (openpty/ConPTY behind one API) and drive it interactively.
---@param ctx any
---@param opts { cmd: string[], cols?: integer, rows?: integer, cwd?: string, env?: table<string,string> }
---@return prova.Term
function terminal.spawn(ctx, opts) end
--- Shadow an interactive CLI on PATH with a scripted responder — the SUT shells out to `ssh`
--- (or an installer), and you script the other side.
---@param ctx any
---@param opts { as: string }
---@return prova.TerminalMock
function terminal.mock(ctx, opts) end

---@class prova.TerminalProxy
---@field env table<string,string>   # PATH-prefixed env — hand to whatever spawns the SUT
---@field path string                # the shim's absolute path
local TerminalProxy = {}
--- Stop the proxy (what `ctx:manage` calls). In record mode, flushes the cassette (outside the
--- shim dir) — the close/scope-exit flush point.
function TerminalProxy:stop() end
--- Alias for `:stop()` — the proxy grammar's teardown verb.
function TerminalProxy:close() end

--- Interpose on an interactive CLI the SUT shells out to (the last cell in the transport matrix):
--- shadow a command NAME on PATH, forward to the REAL program on the pty the SUT already allocated,
--- and record the session — or replay a recorded one with no real program. The terminal cassette is
--- the full-duplex, asciinema-shaped kind: the raw terminal stream with VT sequences intact, so a
--- session recorded once (a ConPTY session on Windows) replays deterministically on every platform.
--- Modes: passthrough (default) | record | replay | auto.
---@param ctx any
---@param opts { as: string, upstream?: string, cassette?: string, mode?: string }
---@return prova.TerminalProxy
function terminal.proxy(ctx, opts) end

-- ---------------------------------------------------------------------------------------------
-- websocket — full-duplex message turns over the http upgrade path (ws:// only)
-- ---------------------------------------------------------------------------------------------

---@class prova.WsConn
local WsConn = {}
--- Send one message turn. Async.
---@param data string
function WsConn:send(data) end
--- Receive the next message turn (async, timeout'd — default 10s; loud on close).
---@param opts? { timeout?: string }
---@return string
function WsConn:recv(opts) end
--- Close the connection (the ws close handshake).
function WsConn:close() end

---@class prova.WsStub
local WsStub = {}
--- Answer this message turn.
---@param data string
function WsStub:reply(data) end

---@class prova.WsMock
---@field url string   # "ws://127.0.0.1:<port>"
---@field endpoint string  # the driver-target alias == url
local WsMock = {}
--- Stub one message turn (exact text). Unmatched turns are journaled (§6), never guessed at.
---@param turn string
---@return prova.WsStub
function WsMock:on(turn) end
--- Full duplex: run this when a client connects — `conn:send(…)` pushes unprompted.
---@param handler fun(conn: prova.WsConn)
function WsMock:on_connect(handler) end
--- The §6 journal: `{ seq, data, matched, source }[]`, filterable by subset table or predicate.
---@param filter? table|fun(entry: table): boolean
---@return table[]
function WsMock:received(filter) end
--- Stop the mock (what `ctx:manage` calls).
function WsMock:stop() end

---@class prova.websocket
websocket = {}
--- Connect (originate): dial a `ws://` url. Async.
---@param url string
---@return prova.WsConn
function websocket.connect(url) end
--- Terminate (and push): a real ws server in this process, stubbed and asserted on.
---@param ctx any
---@param opts? table
---@return prova.WsMock
function websocket.mock(ctx, opts) end

-- ---------------------------------------------------------------------------------------------
-- shell.proxy — the process transport's interpose posture: the journaling PATH shim
-- ---------------------------------------------------------------------------------------------

---@class prova.ShellShimStub
local ShellShimStub = {}
--- Answer matching invocations synthetically: `{ stdout?, code? }` (code defaults to 0).
---@param reply { stdout?: string, code?: integer }
function ShellShimStub:reply(reply) end

---@class prova.ShellShim
---@field env table<string,string>   # PATH-prefixed env — hand to whatever spawns the SUT
---@field path string                # the shim's absolute path
local ShellShim = {}
--- Stub invocations whose argv starts with this prefix: `{ argv = { "status" } }` answers
--- `git status` (and `git status --short`). Stubs always win over the upstream.
---@param match { argv: string[] }
---@return prova.ShellShimStub
function ShellShim:on(match) end
--- The §6 journal, one entry per invocation: `{ seq, argv, stdin, matched, source }` where
--- source is "stub" | "target" (forwarded) | "unmatched". Filter by subset table or predicate.
---@param filter? table|fun(entry: table): boolean
---@return table[]
function ShellShim:received(filter) end
--- Remove the shim and its journal (what `ctx:manage` calls). In record mode, the cassette flush point.
function ShellShim:stop() end
--- Alias for `:stop()` — the proxy grammar's teardown verb.
function ShellShim:close() end

--- Shadow a command NAME on PATH and interpose on every invocation the SUT makes: pass through
--- to `upstream` and journal (spy), stub selected argv prefixes (stubs always win), or — with no
--- upstream — terminate synthetically, where an unstubbed call fails LOUD (exit 127). Shadow the
--- name the SUT execs, never a shell builtin (`sh` answers those itself, PATH unconsulted).
---
--- With a `cassette` + `mode` it records a real CLI once and replays it forever: `record` captures
--- argv+stdin → stdout+exit (flushed at `:stop()`, into a file OUTSIDE the shim dir), `replay`
--- answers from the recording with no upstream and no real binary, `auto` records-then-replays.
--- A replay miss exits non-zero and is journaled as a miss.
---@param ctx any
---@param opts { as: string, upstream?: string, cassette?: string, mode?: string, redact?: string[] }
---@return prova.ShellShim
function shell.proxy(ctx, opts) end

---@class prova.WsProxy
---@field url string   # "ws://127.0.0.1:<port>" — dial this instead of the upstream
local WsProxy = {}
--- The direction-tagged message transcript: `{ seq, dir ("up"|"down"), data }[]`.
---@return table[]
function WsProxy:transcript() end
--- Continuous latency on every message turn, both directions ("2s").
---@param duration string
function WsProxy:latency(duration) end
--- Sever the connection.
function WsProxy:drop() end
--- Stop the proxy (what `ctx:manage` calls).
function WsProxy:stop() end

--- Interpose on a real ws dependency: forward message turns, record a direction-tagged transcript,
--- and apply the fault vocabulary (the last cell in the transport matrix).
---@param ctx any
---@param opts { upstream: string }
---@return prova.WsProxy
function websocket.proxy(ctx, opts) end

------------------------------------------------------------------------------------------
-- path — pure, platform-agnostic path algebra. Canonical access is `prova.path`; the bare
-- name is too collision-prone to squat, so it is ambient only when a package lists "path"
-- in `[globals] inject` (hence a `local` table here, not a global stub — kept LAST in this
-- file so the local never shadows the many `path` parameters above).
------------------------------------------------------------------------------------------

--- String-based on purpose (NOT the OS path type, which renders `\` on Windows): every function
--- accepts either separator and emits `/`-normalized strings, so the same proof assertions hold
--- on every OS. No filesystem access — pure string laws.
---@class prova.path
local path = {}
--- Join segments with `/`; empty segments contribute nothing, and an absolute later segment
--- resets the join (`path.join("a", "/etc", "b")` → `"/etc/b"`).
---@param ... string
---@return string
function path.join(...) end
--- Everything before the last component — `"."` for a bare name, the root for a first-level
--- entry (`"/a"` → `"/"`, `"C:/a"` → `"C:/"`). A trailing slash does not make a component.
---@param p string
---@return string
function path.dirname(p) end
--- The last component (`""` for a bare root, which has none).
---@param p string
---@return string
function path.basename(p) end
--- The extension of the last component WITHOUT the dot (`"txt"`, not `".txt"`); `""` when there
--- is none. A dotfile (`".gitignore"`) is all stem, no extension.
---@param p string
---@return string
function path.ext(p) end
--- The last component minus its extension (`"b.tar.gz"` → `"b.tar"`).
---@param p string
---@return string
function path.stem(p) end
--- Collapse `.`/`..`/duplicate separators, strip any trailing slash, emit `/`. Purely lexical:
--- leading `..` in a relative path survives; `..` cannot climb above a root; `""` → `"."`.
---@param p string
---@return string
function path.normalize(p) end
--- Whether `p` is absolute — unix (`"/…"`), drive (`"C:/…"` or `"C:\…"`), or UNC (`"//server/…"`).
---@param p string
---@return boolean
function path.is_absolute(p) end

------------------------------------------------------------------------------------------
-- str — string utilities + the archetect casing vocabulary. Canonical access is
-- `prova.str`; like `path`, the bare name is ambient only when a package lists "str" in
-- `[globals] inject` (hence a `local` table, kept last so it never shadows a parameter).
------------------------------------------------------------------------------------------

--- The casing half CALLS archetect's own inflections and MIRRORS archetect's filter names
--- (`constant_case` is archetect's name for screaming-snake, so it is prova's too) — one
--- implementation, one vocabulary, so a proof asserting on an archetype's rendered output uses
--- the very function the archetype's templates did.
---@class prova.str
local str = {}
--- Strip surrounding whitespace from both ends.
---@param s string
---@return string
function str.trim(s) end
--- Split on a plain (non-pattern) separator, KEEPING empty fields — `"a,,c"` has three fields.
--- The separator must be non-empty.
---@param s string
---@param sep string
---@return string[]
function str.split(s, sep) end
--- The portable line reader: split on `\n`, absorbing a preceding `\r`; a trailing newline yields
--- no phantom empty line. Same result whether the text came from a unix or a Windows program.
---@param s string
---@return string[]
function str.lines(s) end
---`helloWorld` (archetect's `camel_case` filter)
---@param s string
---@return string
function str.camel_case(s) end
---`FooBar`, singularized (`"foo_bars"` → `"FooBar"`) — archetect's `class_case`
---@param s string
---@return string
function str.class_case(s) end
---`HELLO-WORLD` (archetect's `cobol_case`)
---@param s string
---@return string
function str.cobol_case(s) end
---`HELLO_WORLD` — archetect's name for screaming-snake
---@param s string
---@return string
function str.constant_case(s) end
---`foo/bar` (archetect's `directory_case`)
---@param s string
---@return string
function str.directory_case(s) end
---`hello-world` (archetect's `kebab_case`)
---@param s string
---@return string
function str.kebab_case(s) end
---`foo.bar` (archetect's `package_case`)
---@param s string
---@return string
function str.package_case(s) end
---`HelloWorld` (archetect's `pascal_case`)
---@param s string
---@return string
function str.pascal_case(s) end
---`Hello world` (archetect's `sentence_case`)
---@param s string
---@return string
function str.sentence_case(s) end
---`hello_world` (archetect's `snake_case`)
---@param s string
---@return string
function str.snake_case(s) end
---`Hello World` (archetect's `title_case`)
---@param s string
---@return string
function str.title_case(s) end
---`Hello-World` (archetect's `train_case`)
---@param s string
---@return string
function str.train_case(s) end
---Whether `s` is already camelCase.
---@param s string
---@return boolean
function str.is_camel_case(s) end
---Whether `s` is already ClassCase (pascal, singular).
---@param s string
---@return boolean
function str.is_class_case(s) end
---Whether `s` is already COBOL-CASE.
---@param s string
---@return boolean
function str.is_cobol_case(s) end
---Whether `s` is already CONSTANT_CASE.
---@param s string
---@return boolean
function str.is_constant_case(s) end
---Whether `s` is already directory/case.
---@param s string
---@return boolean
function str.is_directory_case(s) end
---Whether `s` is already kebab-case.
---@param s string
---@return boolean
function str.is_kebab_case(s) end
---Whether `s` is already package.case.
---@param s string
---@return boolean
function str.is_package_case(s) end
---Whether `s` is already PascalCase.
---@param s string
---@return boolean
function str.is_pascal_case(s) end
---Whether `s` is already Sentence case.
---@param s string
---@return boolean
function str.is_sentence_case(s) end
---Whether `s` is already snake_case.
---@param s string
---@return boolean
function str.is_snake_case(s) end
---Whether `s` is already Title Case.
---@param s string
---@return boolean
function str.is_title_case(s) end
---Whether `s` is already Train-Case.
---@param s string
---@return boolean
function str.is_train_case(s) end
---`"user"` → `"users"` (archetect's `pluralize`)
---@param s string
---@return string
function str.pluralize(s) end
---`"users"` → `"user"` (archetect's `singularize`)
---@param s string
---@return string
function str.singularize(s) end
---`"1"` → `"1st"`, `"22"` → `"22nd"` (archetect's `ordinalize`)
---@param s string
---@return string
function str.ordinalize(s) end
---`"1st"` → `"1"` (archetect's `deordinalize`)
---@param s string
---@return string
function str.deordinalize(s) end
