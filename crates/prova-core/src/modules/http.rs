use std::time::{Duration, Instant};

use mlua::{
    Function, Lua, LuaSerdeExt, Table, UserData, UserDataFields, UserDataMethods, Value,
};


/// A response from the `http` module: `res.status`, `res.body`, `res.headers`, `res:json()`,
/// `res:save(path)`.
///
/// The body is held as **bytes**, never a Rust `String`
/// (docs/design/agent-ergonomics.md#http-binary-response-corrupted). `reqwest`'s `text()` does a
/// LOSSY UTF-8 conversion, replacing each invalid byte with U+FFFD — three bytes out for one byte
/// in — so a zip came back inflated and unopenable while every cheap check still passed: status
/// 200, a plausible `#body`, and `body:sub(1, 2) == "PK"`, because ASCII survives. A proof that
/// sniffed the magic number asserted nothing and said so in green.
///
/// Nothing about the fix needs a new accessor to be correct: a **Lua string is a byte string**, so
/// handing Lua the raw bytes makes `res.body` exact for binary and unchanged for text. `save` is
/// here for the case that should never round-trip through Lua at all — "I need the artifact on
/// disk" — because `fs.write` takes a UTF-8 `String` and would reject those very bytes.
struct HttpResponse {
    status: u16,
    body: Vec<u8>,
    headers: Vec<(String, String)>,
}

impl UserData for HttpResponse {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("status", |_, this| Ok(this.status));
        // Bytes in, bytes out: `create_string` takes `&[u8]` and Lua strings are not UTF-8
        // constrained, so this is exact for a zip and identical to before for JSON.
        fields.add_field_method_get("body", |lua, this| lua.create_string(&this.body));
        fields.add_field_method_get("headers", |lua, this| {
            let table = lua.create_table()?;
            for (k, v) in &this.headers {
                table.set(k.clone(), v.clone())?;
            }
            Ok(table)
        });
    }
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        // Decode the body as JSON into a Lua value; raises on non-JSON. JSON nulls become
        // Lua nil (not mlua's null sentinel) so `t:expect(body.field):is_nil()` holds.
        methods.add_method("json", |lua, this, ()| {
            let value: serde_json::Value = serde_json::from_slice(&this.body).map_err(|e| {
                mlua::Error::RuntimeError(format!("response body is not JSON: {e}"))
            })?;
            let opts = mlua::SerializeOptions::new()
                .serialize_none_to_null(false)
                .serialize_unit_to_null(false);
            lua.to_value_with(&value, opts)
        });
        // `res:save(path)` — write the body to disk byte-for-byte, creating parent directories.
        // Returns the path, so `local zip = http.get(url):save(dir .. "/app.zip")` reads as one
        // move. This is the verb that ends "a proof about HTTP requires curl".
        methods.add_method("save", |_, this, path: String| {
            if let Some(parent) = std::path::Path::new(&path).parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    mlua::Error::RuntimeError(format!("res:save {path:?}: {e}"))
                })?;
            }
            std::fs::write(&path, &this.body)
                .map_err(|e| mlua::Error::RuntimeError(format!("res:save {path:?}: {e}")))?;
            Ok(path)
        });
    }
}

pub(crate) fn make(lua: &Lua) -> mlua::Result<Table> {
    let http = lua.create_table()?;
    http.set("get", method_fn(lua, reqwest::Method::GET)?)?;
    http.set("post", method_fn(lua, reqwest::Method::POST)?)?;
    http.set("put", method_fn(lua, reqwest::Method::PUT)?)?;
    http.set("patch", method_fn(lua, reqwest::Method::PATCH)?)?;
    http.set("delete", method_fn(lua, reqwest::Method::DELETE)?)?;
    http.set("head", method_fn(lua, reqwest::Method::HEAD)?)?;
    http.set("options", method_fn(lua, reqwest::Method::OPTIONS)?)?;
    http.set("wait_for", wait_for_fn(lua)?)?;
    // http.client{ base_url, headers?, timeout? } → a reusable REST client that prefixes base_url
    // and merges default headers (per-call headers/timeout override).
    http.set("client", client_fn(lua)?)?;
    // http.mock(ctx, opts?) → the `mock` facet: a real HTTP server, in-process, that you stub and
    // then assert on. `client` attaches to a real one, `mock` provisions a fake one.
    #[cfg(feature = "mock")]
    http.set("mock", super::mock::mock_fn(lua)?)?;
    #[cfg(feature = "mock")]
    http.set("proxy", super::mock::proxy_fn(lua)?)?;
    Ok(http)
}

/// A reusable REST client bound to a base URL and default headers — the ergonomic path for a suite
/// that hits one service many times (base URL + auth declared once). Methods mirror the free
/// functions: `client:get/post/put/patch/delete/head/options(path, opts)` and `client:wait_for`.
struct HttpClient {
    base_url: String,
    headers: Vec<(String, String)>,
    timeout: Option<Duration>,
}

impl UserData for HttpClient {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        client_method(methods, "get", reqwest::Method::GET);
        client_method(methods, "post", reqwest::Method::POST);
        client_method(methods, "put", reqwest::Method::PUT);
        client_method(methods, "patch", reqwest::Method::PATCH);
        client_method(methods, "delete", reqwest::Method::DELETE);
        client_method(methods, "head", reqwest::Method::HEAD);
        client_method(methods, "options", reqwest::Method::OPTIONS);
        methods.add_async_method(
            "wait_for",
            |lua, this, (path, opts): (String, Option<Table>)| {
                let url = join_url(&this.base_url, &path);
                let base_headers = this.headers.clone();
                let params = wait_params(&opts);
                async move {
                    let p = params?;
                    // Per-call headers layer OVER the client's defaults by name — the same
                    // precedence `build_prepared` gives an ordinary request, so `client:get` and
                    // `client:wait_for` cannot disagree about whose Authorization wins.
                    let mut headers = base_headers.clone();
                    for (k, v) in p.headers {
                        upsert_header(&mut headers, k, v);
                    }
                    let (expected, timeout, every) = (p.status, p.timeout, p.every);
                    let deadline = Instant::now() + timeout;
                    loop {
                        let prepared = Prepared {
                            method: reqwest::Method::GET,
                            url: url.clone(),
                            headers: headers.clone(),
                            body: None,
                            timeout: Some(every),
                            redirects: None,
                        };
                        if let Ok(resp) = send(prepared).await {
                            if resp.status == expected {
                                return lua.create_userdata(resp);
                            }
                        }
                        if Instant::now() >= deadline {
                            return Err(mlua::Error::RuntimeError(format!(
                                "http client wait_for timed out after {timeout:?} waiting for {expected} at {url}"
                            )));
                        }
                        tokio::time::sleep(every).await;
                    }
                }
            },
        );
    }
}

fn client_fn(lua: &Lua) -> mlua::Result<Function> {
    lua.create_function(|lua, opts: Table| {
        let (base_url, headers, timeout) = super::client_opts(&opts, "http.client", "base_url")?;
        lua.create_userdata(HttpClient {
            base_url,
            headers,
            timeout,
        })
    })
}

fn client_method<M: UserDataMethods<HttpClient>>(
    methods: &mut M,
    name: &'static str,
    method: reqwest::Method,
) {
    methods.add_async_method(
        name,
        move |lua, this, (path, opts): (String, Option<Table>)| {
            let url = join_url(&this.base_url, &path);
            let prepared = build_prepared(
                &lua,
                method.clone(),
                url,
                this.headers.clone(),
                this.timeout,
                opts,
            );
            async move {
                let resp = send(prepared?).await?;
                lua.create_userdata(resp)
            }
        },
    );
}

/// Join a client `base_url` with a per-call `path`. An absolute `path` (starting with a scheme)
/// is used verbatim; otherwise exactly one `/` separates them.
fn join_url(base: &str, path: &str) -> String {
    if path.starts_with("http://") || path.starts_with("https://") {
        return path.to_string();
    }
    if path.is_empty() {
        return base.to_string();
    }
    let b = base.strip_suffix('/').unwrap_or(base);
    let p = path.strip_prefix('/').unwrap_or(path);
    format!("{b}/{p}")
}

/// An owned, Lua-free request spec, prepared synchronously so nothing borrows Lua across the
/// await.
struct Prepared {
    method: reqwest::Method,
    url: String,
    headers: Vec<(String, String)>,
    body: Option<Vec<u8>>,
    timeout: Option<Duration>,
    /// How many redirects to follow. `None` is reqwest's default policy; `Some(0)` returns the
    /// 3xx itself (docs/design/agent-ergonomics.md#http-redirect-control).
    redirects: Option<usize>,
}

fn method_fn(lua: &Lua, method: reqwest::Method) -> mlua::Result<Function> {
    lua.create_async_function(move |lua, (url, opts): (String, Option<Table>)| {
        let name = format!("http.{}", method.as_str().to_ascii_lowercase());
        let prepared = build_prepared(&lua, method.clone(), url, Vec::new(), None, opts);
        async move {
            super::runtime_only(&name)?;
            let resp = send(prepared?).await?;
            lua.create_userdata(resp)
        }
    })
}

/// Every per-call request option — closed by construction
/// (docs/design/agent-ergonomics.md#module-opts-silently-ignored). A dropped key here is
/// especially quiet: `http.post(url, { jsno = payload })` sends an empty body to a live endpoint
/// and reports whatever it says back, so the proof fails somewhere far from the typo — or worse,
/// passes.
const REQUEST_OPTS: &[&str] = &[
    "body",
    "content_type",
    "form",
    "headers",
    "json",
    "redirects",
    "timeout",
];

/// The three ways to say "the body". They are mutually exclusive by construction: silently
/// preferring one (as an `if json … else if body` chain does) means the request sent is not the
/// request written, and the author debugs the endpoint instead of the call.
const BODY_OPTS: &[&str] = &["json", "form", "body"];

/// Every option the polling verbs (`http.wait_for`, `client:wait_for`) honor.
///
/// `headers` is here because readiness against a service behind auth was otherwise unprovable:
/// the free function sent none, so a health endpoint requiring a bearer token had to be waited on
/// with a hand-rolled `http.get` retry loop — the exact "reach for a host tool" pressure the http
/// module exists to remove (docs/design/agent-ergonomics.md#http-wait-for-cannot-authenticate).
/// `client:wait_for` always carried the CLIENT's defaults, so the two verbs disagreed about
/// whether polling could authenticate at all.
const WAIT_OPTS: &[&str] = &["every", "headers", "status", "timeout"];

/// Build an owned request spec from `opts`, layered over optional defaults (a client's base
/// headers/timeout). Per-call `headers` override defaults by name; the body (exactly one of
/// `json`/`form`/`body`), `content_type`, `timeout` and `redirects` in `opts` win. Synchronous, so
/// nothing borrows Lua across the await.
fn build_prepared(
    lua: &Lua,
    method: reqwest::Method,
    url: String,
    mut headers: Vec<(String, String)>,
    mut timeout: Option<Duration>,
    opts: Option<Table>,
) -> mlua::Result<Prepared> {
    let mut body = None;
    let mut redirects = None;
    if let Some(opts) = opts {
        crate::opts::reject_unknown(&opts, REQUEST_OPTS, "http request options")?;
        if let Some(hdrs) = opts.get::<Option<Table>>("headers")? {
            for pair in hdrs.pairs::<String, String>() {
                let (k, v) = pair?;
                upsert_header(&mut headers, k, v);
            }
        }
        let mut given: Vec<&str> = Vec::new();
        for key in BODY_OPTS {
            if !matches!(opts.get::<Value>(*key)?, Value::Nil) {
                given.push(key);
            }
        }
        if given.len() > 1 {
            return Err(mlua::Error::RuntimeError(format!(
                "http: `{}` name the body more than once — pass exactly one of `json` (a table, \
                 sent as application/json), `form` (a table, sent as \
                 application/x-www-form-urlencoded), or `body` (a string, sent verbatim)",
                given.join("` and `")
            )));
        }
        if let Some(json) = opts.get::<Option<Value>>("json")? {
            let value: serde_json::Value = lua.from_value(json)?;
            let encoded = serde_json::to_vec(&value).map_err(|e| {
                mlua::Error::RuntimeError(format!("http: encoding json body: {e}"))
            })?;
            upsert_header(
                &mut headers,
                "content-type".into(),
                "application/json".into(),
            );
            body = Some(encoded);
        } else if let Some(form) = opts.get::<Option<Table>>("form")? {
            // The shape OAuth 2.0 token endpoints require, and the reason two proofs shelled out
            // to curl (docs/design/agent-ergonomics.md#http-form-and-raw-bodies).
            let mut ser = form_urlencoded::Serializer::new(String::new());
            for pair in form.pairs::<String, Value>() {
                let (k, v) = pair?;
                let v = match v {
                    Value::String(s) => s.to_str()?.to_string(),
                    Value::Integer(i) => i.to_string(),
                    Value::Number(n) => n.to_string(),
                    Value::Boolean(b) => b.to_string(),
                    other => {
                        return Err(mlua::Error::RuntimeError(format!(
                            "http: form field `{k}` must be a scalar, got {}",
                            other.type_name()
                        )))
                    }
                };
                ser.append_pair(&k, &v);
            }
            upsert_header(
                &mut headers,
                "content-type".into(),
                "application/x-www-form-urlencoded".into(),
            );
            body = Some(ser.finish().into_bytes());
        } else if let Some(raw) = opts.get::<Option<mlua::String>>("body")? {
            // A Lua string is bytes, so a raw body is sent byte-for-byte — the request-side twin
            // of the response-side fix, and what makes uploading a binary artifact possible.
            body = Some(raw.as_bytes().to_vec());
        }
        // Set last so it wins over the type `json`/`form` implied — the point of naming it.
        if let Some(ct) = opts.get::<Option<String>>("content_type")? {
            upsert_header(&mut headers, "content-type".into(), ct);
        }
        if let Some(s) = opts.get::<Option<String>>("timeout")? {
            timeout = Some(crate::model::require_duration("http", "timeout", &s).map_err(mlua::Error::RuntimeError)?);
        }
        redirects = parse_redirects(&opts)?;
    }
    Ok(Prepared {
        method,
        url,
        headers,
        body,
        timeout,
        redirects,
    })
}

/// `redirects = false` (or `0`) returns the 3xx itself; `redirects = N` caps the chain; absent is
/// reqwest's default policy (docs/design/agent-ergonomics.md#http-redirect-control).
///
/// One key rather than a `redirects` / `max_redirects` pair: they would be two spellings of one
/// question, and a table carrying both would have to invent a precedence rule nobody could guess.
fn parse_redirects(opts: &Table) -> mlua::Result<Option<usize>> {
    match opts.get::<Value>("redirects")? {
        Value::Nil => Ok(None),
        // `true` is "the default policy", stated rather than assumed.
        Value::Boolean(true) => Ok(None),
        Value::Boolean(false) => Ok(Some(0)),
        Value::Integer(n) if n >= 0 => Ok(Some(n as usize)),
        other => Err(mlua::Error::RuntimeError(format!(
            "http: `redirects` must be false (return the 3xx), true (follow, the default), or a \
             non-negative count, got {}",
            other.type_name()
        ))),
    }
}

/// Insert or replace a header by case-insensitive name (so a per-call header overrides a client
/// default rather than sending both).
fn upsert_header(headers: &mut Vec<(String, String)>, key: String, value: String) {
    match headers
        .iter_mut()
        .find(|(k, _)| k.eq_ignore_ascii_case(&key))
    {
        Some(existing) => existing.1 = value,
        None => headers.push((key, value)),
    }
}

/// Flatten an error's `source()` chain into the message.
///
/// `reqwest`'s Display is uninformative on a failed send — "error sending request for url (…)" —
/// while the actionable part (connection refused, dns error, certificate expired, timed out) sits
/// one or more levels down in `source()`. Dropping it makes a whole class of failure
/// undiagnosable: intermittent egress and an expired CA read identically, and the only way to
/// tell them apart is to stop using prova and reach for curl.
fn why(e: &(dyn std::error::Error + 'static)) -> String {
    let mut parts = vec![e.to_string()];
    let mut cur = e.source();
    while let Some(c) = cur {
        let s = c.to_string();
        // Skip a link that merely repeats its parent — reqwest/hyper often re-wrap verbatim.
        if !parts.iter().any(|p| p == &s) {
            parts.push(s);
        }
        cur = c.source();
    }
    parts.join(": ")
}

async fn send(prepared: Prepared) -> mlua::Result<HttpResponse> {
    // The redirect policy is a CLIENT property in reqwest, not a per-request one, so a bounded
    // request builds its own client. The default path keeps the shared-nothing client it always
    // had — this adds a branch, not a cost.
    let client = match prepared.redirects {
        None => reqwest::Client::new(),
        Some(0) => reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| mlua::Error::RuntimeError(format!("http: building client: {e}")))?,
        Some(n) => reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::limited(n))
            .build()
            .map_err(|e| mlua::Error::RuntimeError(format!("http: building client: {e}")))?,
    };
    let mut req = client.request(prepared.method, &prepared.url);
    for (k, v) in prepared.headers {
        req = req.header(k, v);
    }
    if let Some(body) = prepared.body {
        req = req.body(body);
    }
    if let Some(timeout) = prepared.timeout {
        req = req.timeout(timeout);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| mlua::Error::RuntimeError(format!("http request failed: {}", why(&e))))?;
    let status = resp.status().as_u16();
    let headers = resp
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or_default().to_string()))
        .collect();
    // `bytes()`, never `text()`: `text()` is a LOSSY UTF-8 conversion that silently rewrites every
    // invalid byte to U+FFFD (docs/design/agent-ergonomics.md#http-binary-response-corrupted).
    let body = resp
        .bytes()
        .await
        .map_err(|e| mlua::Error::RuntimeError(format!("reading http response body: {e}")))?
        .to_vec();
    Ok(HttpResponse {
        status,
        body,
        headers,
    })
}

/// `http.wait_for(url, { status = 200, headers = {…}, timeout = "30s", every = "500ms" })` — poll
/// GET until the endpoint returns the expected status or the deadline elapses. The boot-then-probe
/// primitive, and `headers` is what lets it be pointed at a health endpoint behind auth.
fn wait_for_fn(lua: &Lua) -> mlua::Result<Function> {
    lua.create_async_function(|lua, (url, opts): (String, Option<Table>)| {
        let params = wait_params(&opts);
        async move {
            super::runtime_only("http.wait_for")?;
            let p = params?;
            let (expected, timeout, every) = (p.status, p.timeout, p.every);
            let deadline = Instant::now() + timeout;
            loop {
                let prepared = Prepared {
                    method: reqwest::Method::GET,
                    url: url.clone(),
                    headers: p.headers.clone(),
                    body: None,
                    timeout: Some(every),
                    redirects: None,
                };
                if let Ok(resp) = send(prepared).await {
                    if resp.status == expected {
                        return lua.create_userdata(resp);
                    }
                }
                if Instant::now() >= deadline {
                    return Err(mlua::Error::RuntimeError(format!(
                        "http.wait_for timed out after {timeout:?} waiting for {expected} at {url}"
                    )));
                }
                tokio::time::sleep(every).await;
            }
        }
    })
}

/// What the polling verbs need, owned — parsed synchronously so nothing borrows Lua across the
/// await. A struct rather than a tuple since `headers` made it four fields, and a bare
/// `(u16, Duration, Duration, Vec<..>)` at two call sites is a positional puzzle.
struct WaitParams {
    status: u16,
    timeout: Duration,
    every: Duration,
    /// Layered OVER a client's defaults by name, exactly as a per-call request's headers are.
    headers: Vec<(String, String)>,
}

fn wait_params(opts: &Option<Table>) -> mlua::Result<WaitParams> {
    let mut p = WaitParams {
        status: 200,
        timeout: Duration::from_secs(30),
        every: Duration::from_millis(500),
        headers: Vec::new(),
    };
    if let Some(opts) = opts {
        crate::opts::reject_unknown(opts, WAIT_OPTS, "http.wait_for")?;
        if let Some(s) = opts.get::<Option<u16>>("status")? {
            p.status = s;
        }
        if let Some(s) = opts.get::<Option<String>>("timeout")? {
            p.timeout = crate::model::require_duration("http.wait_for", "timeout", &s).map_err(mlua::Error::RuntimeError)?;
        }
        if let Some(s) = opts.get::<Option<String>>("every")? {
            p.every = crate::model::require_duration("http.wait_for", "every", &s).map_err(mlua::Error::RuntimeError)?;
        }
        if let Some(hdrs) = opts.get::<Option<Table>>("headers")? {
            for pair in hdrs.pairs::<String, String>() {
                let (k, v) = pair?;
                upsert_header(&mut p.headers, k, v);
            }
        }
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A per-call header must REPLACE a client default rather than joining it — two `Authorization`
    /// headers is a request whose meaning depends on which the server reads first.
    #[test]
    fn a_header_is_replaced_case_insensitively_not_appended() {
        let mut headers = vec![("Authorization".into(), "Bearer old".into())];
        upsert_header(&mut headers, "authorization".into(), "Bearer new".into());
        assert_eq!(headers.len(), 1, "replaced, not appended: {headers:?}");
        assert_eq!(headers[0].1, "Bearer new");
        // …and the original casing survives, since some servers are stricter than the RFC.
        assert_eq!(headers[0].0, "Authorization");

        upsert_header(&mut headers, "X-Trace".into(), "1".into());
        assert_eq!(headers.len(), 2, "a genuinely new header is added");
    }

    /// A client's `base_url` and a per-call path meet at exactly one slash, whichever side brought
    /// one — a doubled slash is a different URL to a router that matches on path.
    #[test]
    fn base_url_and_path_join_at_exactly_one_slash() {
        assert_eq!(join_url("http://h/api", "users"), "http://h/api/users");
        assert_eq!(join_url("http://h/api/", "users"), "http://h/api/users");
        assert_eq!(join_url("http://h/api", "/users"), "http://h/api/users");
        assert_eq!(join_url("http://h/api/", "/users"), "http://h/api/users");
        // An empty path addresses the base itself, without inventing a trailing slash.
        assert_eq!(join_url("http://h/api", ""), "http://h/api");
        // An absolute URL ignores the base entirely — how a client reaches somewhere else once.
        assert_eq!(join_url("http://h/api", "http://other/x"), "http://other/x");
        assert_eq!(join_url("http://h/api", "https://other/x"), "https://other/x");
    }

    /// `redirects` is one key carrying two meanings, so the mapping from Lua value to policy is
    /// where a wrong guess would silently change what a proof observes.
    #[test]
    fn redirects_maps_each_shape_to_a_policy() {
        let lua = mlua::Lua::new();
        let with = |v: Value| {
            let t = lua.create_table().unwrap();
            t.set("redirects", v).unwrap();
            parse_redirects(&t)
        };
        // Absent and `true` are both "the default policy" — stated vs assumed, same behavior.
        assert_eq!(parse_redirects(&lua.create_table().unwrap()).unwrap(), None);
        assert_eq!(with(Value::Boolean(true)).unwrap(), None);
        // `false` and `0` both mean "hand me the 3xx".
        assert_eq!(with(Value::Boolean(false)).unwrap(), Some(0));
        assert_eq!(with(Value::Integer(0)).unwrap(), Some(0));
        assert_eq!(with(Value::Integer(3)).unwrap(), Some(3));
        // A negative cap has no meaning and is refused rather than saturated to zero, which would
        // silently turn "follow" into "do not".
        assert!(with(Value::Integer(-1)).is_err());
        assert!(with(Value::String(lua.create_string("yes").unwrap())).is_err());
    }
}
