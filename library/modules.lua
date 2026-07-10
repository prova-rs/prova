---@meta
--- Assay first-party module annotations: fs, shell, http, archetect.
--- These modules are globally available inside test files (no require needed) and also
--- `require`-able by name. `archetect` is a plugin over `assay-core`, not a built-in.

------------------------------------------------------------------------------------------
-- Handles
------------------------------------------------------------------------------------------

---A file handle rooted within a tree. Read its contents, or assert on it via `expect`.
---@class assay.FileHandle
---@field path string   # absolute path
local FileHandle = {}
---@return string contents
function FileHandle:read() end

---A directory handle.
---@class assay.DirHandle
---@field path string
local DirHandle = {}

---A tree handle rooted at a directory (e.g. a render destination).
---@class assay.Tree
---@field path string   # absolute root
local Tree = {}
---@param rel string
---@return assay.FileHandle
function Tree:file(rel) end
---@param rel string
---@return assay.DirHandle
function Tree:dir(rel) end
---Serializable snapshot of the whole layout (for `:matches_snapshot()`).
---@return table
function Tree:tree() end

------------------------------------------------------------------------------------------
-- fs
------------------------------------------------------------------------------------------

---@class assay.fs
fs = {}
---Create a temp dir. Not auto-cleaned; pair with `ctx:defer` or use `ctx:tempdir()`.
---@return string path
function fs.tempdir() end
---@param path string
function fs.remove_all(path) end
---@param path string
---@return string
function fs.read(path) end
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

---@class assay.ShellResult
---@field code integer
---@field stdout string
---@field stderr string
---@field duration number   # seconds
local ShellResult = {}
---@return boolean          # code == 0
function ShellResult:ok() end

---@class assay.ShellOpts
---@field cwd? string
---@field env? table<string,string>
---@field timeout? string     # e.g. "120s"
---@field check? boolean      # if true, non-zero exit raises instead of returning

---@class assay.shell
shell = {}
---@param command string
---@param opts? assay.ShellOpts
---@return assay.ShellResult
function shell.run(command, opts) end

------------------------------------------------------------------------------------------
-- http (blocking in v1)
------------------------------------------------------------------------------------------

---@class assay.HttpResponse
---@field status integer
---@field body string
---@field headers table<string,string>
local HttpResponse = {}
---Decode the body as JSON (raises on non-JSON).
---@return table
function HttpResponse:json() end

---@class assay.HttpOpts
---@field headers? table<string,string>
---@field json? table            # request body, JSON-encoded
---@field timeout? string

---@class assay.WaitOpts : assay.HttpOpts
---@field status? integer        # expected status (default 200)
---@field every? string          # poll interval, e.g. "500ms"

---@class assay.http
http = {}
---@param url string
---@param opts? assay.HttpOpts
---@return assay.HttpResponse
function http.get(url, opts) end
---@param url string
---@param opts? assay.HttpOpts
---@return assay.HttpResponse
function http.post(url, opts) end
---@param url string
---@param opts? assay.HttpOpts
---@return assay.HttpResponse
function http.put(url, opts) end
---@param url string
---@param opts? assay.HttpOpts
---@return assay.HttpResponse
function http.delete(url, opts) end
---Poll until the endpoint responds as expected or the timeout elapses.
---@param url string
---@param opts? assay.WaitOpts
---@return assay.HttpResponse
function http.wait_for(url, opts) end

------------------------------------------------------------------------------------------
-- archetect (plugin: in-process render via archetect-core)
------------------------------------------------------------------------------------------

---@class assay.RenderOpts
---@field source string                    # local path or git URL
---@field answers? table<string,any>       # prompt answers as data
---@field switches? string[]
---@field defaults? boolean                # use defaults for unanswered prompts (headless)
---@field destination? string              # optional; a temp dir is used if omitted

---A render result: a `Tree` plus the ordered IO-protocol write operations it intended.
---@class assay.RenderResult : assay.Tree
---@field writes table[]                   # ordered WriteFile/WriteDirectory ops

---@class assay.archetect
archetect = {}
---Render an archetype in-process and return its output tree.
---@param opts assay.RenderOpts
---@return assay.RenderResult
function archetect.render(opts) end
