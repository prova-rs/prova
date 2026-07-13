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
---@param rel string
---@return prova.FileHandle
function Tree:file(rel) end
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
---Create a temp dir. Not auto-cleaned; pair with `ctx:defer` or use `ctx:tempdir()`.
---@return string path
function fs.tempdir() end
---@param path string
function fs.remove_all(path) end
---@param path string
---@return string
function fs.read(path) end
---Write `contents` to `path`, creating parent directories as needed.
---@param path string
---@param contents string
function fs.write(path, contents) end
---@param path string
---@return boolean
function fs.exists(path) end
---@param root string
---@param pattern string   # e.g. "**/*.rs"
---@return string[]
function fs.glob(root, pattern) end

------------------------------------------------------------------------------------------
-- shell
------------------------------------------------------------------------------------------

---@class prova.ShellResult
---@field code integer
---@field stdout string
---@field stderr string
---@field duration number   # seconds
local ShellResult = {}
---@return boolean          # code == 0
function ShellResult:ok() end

---@class prova.ShellOpts
---@field cwd? string
---@field env? table<string,string>
---@field timeout? string     # e.g. "120s"
---@field check? boolean      # if true, non-zero exit raises instead of returning

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

---@class prova.SpawnOpts
---@field cwd? string
---@field env? table<string,string>

---@class prova.shell
shell = {}
---@param command string
---@param opts? prova.ShellOpts
---@return prova.ShellResult
function shell.run(command, opts) end
--- Start a long-running command in the background (a booted app, a mock server) and return a
--- handle. stdout/stderr are discarded. Pair with `ctx:defer(function() proc:stop() end)`.
---@param command string
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
---@param url string
---@param opts? prova.HttpOpts
---@return prova.HttpResponse
function http.get(url, opts) end
---@param url string
---@param opts? prova.HttpOpts
---@return prova.HttpResponse
function http.post(url, opts) end
---@param url string
---@param opts? prova.HttpOpts
---@return prova.HttpResponse
function http.put(url, opts) end
---@param url string
---@param opts? prova.HttpOpts
---@return prova.HttpResponse
function http.patch(url, opts) end
---@param url string
---@param opts? prova.HttpOpts
---@return prova.HttpResponse
function http.delete(url, opts) end
---@param url string
---@param opts? prova.HttpOpts
---@return prova.HttpResponse
function http.head(url, opts) end
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
function HttpClient:get(path, opts) end
---@param path string
---@param opts? prova.HttpOpts
---@return prova.HttpResponse
function HttpClient:post(path, opts) end
---@param path string
---@param opts? prova.HttpOpts
---@return prova.HttpResponse
function HttpClient:put(path, opts) end
---@param path string
---@param opts? prova.HttpOpts
---@return prova.HttpResponse
function HttpClient:patch(path, opts) end
---@param path string
---@param opts? prova.HttpOpts
---@return prova.HttpResponse
function HttpClient:delete(path, opts) end
---@param path string
---@param opts? prova.HttpOpts
---@return prova.HttpResponse
function HttpClient:head(path, opts) end
---@param path string
---@param opts? prova.HttpOpts
---@return prova.HttpResponse
function HttpClient:options(path, opts) end
---@param path string
---@param opts? prova.WaitOpts
---@return prova.HttpResponse
function HttpClient:wait_for(path, opts) end

--- Build a reusable REST client bound to a base URL and default headers.
---@param opts prova.HttpClientOpts
---@return prova.HttpClient
function http.client(opts) end

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

--- Declarative archetype check — prova's answer to the pytest harness's `manifest.yaml`, matched
--- field-for-field but as real Lua you can extend. Renders once (headless) and registers the
--- standard tests. Anything a prompt needs and the answers omit falls back to its default; a prompt
--- with no default and no answer errors (headless never hangs).
---@class prova.VerifySpec
---@field source string                      # local path or git URL
---@field name? string                       # label for the generated tests (default "archetype")
---@field answers? table<string,any>         # prompt answers as data
---@field switches? string[]
---@field defaults? boolean                  # headless defaults for unanswered prompts (default true)
---@field project_dir? string                # assert relative to this subdirectory the render produces
---@field expected_files? string[]           # must exist (relative to project_dir)
---@field absent_files? string[]             # must NOT exist
---@field yaml_globs? string[]               # each glob must match ≥1 file; each match must parse
---@field fully_rendered? boolean            # assert no leftover template markers (default true)
---@field requires? string[]                 # capabilities gating the build step (else skip)
---@field build_steps? (string|string[])[]   # commands run sequentially in project_dir
---@field env? table<string,string>          # extra environment for build_steps
---@field timeout? string                    # per build step (default "600s")

--- Render an archetype and register the standard layout/fully-rendered/yaml/build checks. Returns the
--- shared render fixture so you can add your own tests against the same output (the superset pattern).
---@param spec prova.VerifySpec
---@return prova.Fixture
function archetect.verify(spec) end

------------------------------------------------------------------------------------------
-- docker (testcontainers-style ephemeral dependencies, via the docker CLI)
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
--- Run a command inside the container (`sh -c`); returns (code, stdout, stderr).
---@param command string
---@return integer, string, string
function Container:exec(command) end
--- Force-remove the container. Idempotent.
function Container:stop() end

---@class prova.DockerWait
---@field port? integer       # wait until this container port accepts a TCP connection
---@field log? string         # wait until the logs contain this substring
---@field timeout? string     # default "30s"
---@field every? string       # poll interval, default "250ms"

---@class prova.DockerRunOpts
---@field image string
---@field ports? integer[]              # container ports to publish to random host ports
---@field env? table<string,string>
---@field wait? prova.DockerWait        # readiness gate

---@class prova.docker
docker = {}
--- Start a container (detached, `--rm`) and return a handle once it is ready.
---@param opts prova.DockerRunOpts
---@return prova.Container
function docker.run(opts) end

------------------------------------------------------------------------------------------
-- db (one general query API over Postgres/MySQL/SQLite via sqlx's Any driver)
------------------------------------------------------------------------------------------

--- A database connection from `db.connect`. Backend chosen by URL scheme, so one API covers
--- Postgres/MySQL/SQLite. Methods are async; pair with `ctx:manage(conn)` to close it on teardown.
--- Use the backend's own placeholder syntax in SQL (`$1` for Postgres, `?` for MySQL/SQLite).
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

---@class prova.db
db = {}
--- Connect by URL: `postgres://…`, `mysql://…`, or `sqlite://<path>?mode=rwc`.
---@param url string
---@return prova.Connection
function db.connect(url) end

--- Options for the `db.postgres`/`db.mysql` recipes. All optional (sensible defaults).
---@class prova.DbRecipeOpts
---@field user? string           # default "prova"
---@field password? string       # default "prova"
---@field database? string       # default "prova"
---@field image? string          # full image ref; overrides tag
---@field tag? string            # image tag (postgres → "16-alpine", mysql → "8")
---@field root_password? string  # MySQL only, default "root"
---@field timeout? string        # readiness deadline

--- A provisioned ephemeral database: an open (managed) connection, its URL, and the container.
---@class prova.DbResource
---@field url string
---@field conn prova.Connection
---@field container prova.Container

--- Provision an ephemeral Postgres in a container, wait until it accepts connections, and return an
--- open managed connection — the whole `docker.run` + retry + `db.connect` + `ctx:manage` dance in one
--- call. Requires the `docker` module at call time (`requires = { "docker" }` to skip gracefully).
---@param ctx prova.Context
---@param opts? prova.DbRecipeOpts
---@return prova.DbResource
function db.postgres(ctx, opts) end

--- Provision an ephemeral MySQL the same way as `db.postgres`.
---@param ctx prova.Context
---@param opts? prova.DbRecipeOpts
---@return prova.DbResource
function db.mysql(ctx, opts) end

------------------------------------------------------------------------------------------
-- redis (a thin cache client + an ephemeral-container recipe)
------------------------------------------------------------------------------------------

--- A Redis connection from `redis.connect`. Methods are async; `ctx:manage(conn)` for teardown.
---@class prova.RedisConnection
local RedisConnection = {}
--- Get a key's value, or nil if it does not exist.
---@param key string
---@return string|nil
function RedisConnection:get(key) end
--- Set a key to a (string) value.
---@param key string
---@param value string
function RedisConnection:set(key, value) end
--- Delete one or more keys; returns the number removed.
---@param ... string
---@return integer
function RedisConnection:del(...) end
--- Whether a key exists.
---@param key string
---@return boolean
function RedisConnection:exists(key) end
--- Increment a key (by 1, or `by`); returns the new value.
---@param key string
---@param by? integer
---@return integer
function RedisConnection:incr(key, by) end
--- Set a key's time-to-live in seconds.
---@param key string
---@param seconds integer
function RedisConnection:expire(key, seconds) end
--- PING the server (returns "PONG").
---@return string
function RedisConnection:ping() end
--- Run an arbitrary command; returns the raw reply as a Lua value. The escape hatch.
---@param ... string
---@return any
function RedisConnection:command(...) end
--- No-op (the connection drops with the handle); present for `ctx:manage` symmetry.
function RedisConnection:close() end

---@class prova.RedisRecipeOpts
---@field image? string      # full image ref; overrides tag
---@field tag? string        # image tag (default "7-alpine")
---@field timeout? string    # readiness deadline

--- A provisioned ephemeral Redis: an open (managed) connection, its URL, and the container.
---@class prova.RedisResource
---@field url string
---@field conn prova.RedisConnection
---@field container prova.Container

---@class prova.redis
redis = {}
--- Connect to a Redis by URL (`redis://host:port`). Async; call in a fixture/test body.
---@param url string
---@return prova.RedisConnection
function redis.connect(url) end
--- Provision an ephemeral Redis in a container, wait for it, and return an open managed connection —
--- the counterpart to `db.postgres`. Requires the `docker` module at call time.
---@param ctx prova.Context
---@param opts? prova.RedisRecipeOpts
---@return prova.RedisResource
function redis.container(ctx, opts) end

------------------------------------------------------------------------------------------
-- grpc (native dynamic client via server reflection; no `grpcurl`, no `.proto` files)
------------------------------------------------------------------------------------------

--- A connected gRPC client from `grpc.connect`. It learned the server's schema at connect time via
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

---@class prova.GrpcConnectOpts
---@field timeout? string      # per-call deadline, e.g. "30s"

---@class prova.GrpcWaitOpts
---@field timeout? string      # overall deadline (default "30s")
---@field every? string        # poll interval (default "500ms")

---@class prova.grpc
grpc = {}
--- Connect to a gRPC server at `addr` (`"host:port"` or `"http://host:port"`), performing reflection
--- once to discover its services. Must be called inside a fixture or test body (it is async).
---@param addr string
---@param opts? prova.GrpcConnectOpts
---@return prova.GrpcClient
function grpc.connect(addr, opts) end
--- Poll until the server answers a reflection request or the timeout elapses (boot-then-probe).
---@param addr string
---@param opts? prova.GrpcWaitOpts
function grpc.wait_for(addr, opts) end

------------------------------------------------------------------------------------------
-- yaml (parse YAML text to Lua values — k8s manifests, CI configs, compose files)
------------------------------------------------------------------------------------------

---@class prova.yaml
yaml = {}
--- Parse a single YAML document into a Lua value. Raises on invalid YAML.
---@param text string
---@return any
function yaml.parse(text) end
--- Parse a multi-document YAML stream (`---`-separated, as in Kubernetes manifests) into a list of
--- Lua values. Raises on the first invalid document.
---@param text string
---@return any[]
function yaml.parse_all(text) end

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
