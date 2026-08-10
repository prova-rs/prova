use std::time::{Duration, Instant};

use mlua::{
    Function, Lua, LuaSerdeExt, Table, UserData, UserDataFields, UserDataMethods, Value,
};

use crate::model::parse_duration;

/// A response from the `http` module: `res.status`, `res.body`, `res.headers`, `res:json()`.
struct HttpResponse {
    status: u16,
    body: String,
    headers: Vec<(String, String)>,
}

impl UserData for HttpResponse {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("status", |_, this| Ok(this.status));
        fields.add_field_method_get("body", |_, this| Ok(this.body.clone()));
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
            let value: serde_json::Value = serde_json::from_str(&this.body).map_err(|e| {
                mlua::Error::RuntimeError(format!("response body is not JSON: {e}"))
            })?;
            let opts = mlua::SerializeOptions::new()
                .serialize_none_to_null(false)
                .serialize_unit_to_null(false);
            lua.to_value_with(&value, opts)
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
                    let (expected, timeout, every) = params?;
                    let deadline = Instant::now() + timeout;
                    loop {
                        let prepared = Prepared {
                            method: reqwest::Method::GET,
                            url: url.clone(),
                            headers: base_headers.clone(),
                            body: None,
                            timeout: Some(every),
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
        let base_url = opts.get::<Option<String>>("base_url")?.ok_or_else(|| {
            mlua::Error::RuntimeError("http.client requires a `base_url`".into())
        })?;
        let mut headers = Vec::new();
        if let Some(hdrs) = opts.get::<Option<Table>>("headers")? {
            for pair in hdrs.pairs::<String, String>() {
                let (k, v) = pair?;
                headers.push((k, v));
            }
        }
        let timeout = opts
            .get::<Option<String>>("timeout")?
            .and_then(|s| parse_duration(&s));
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
}

fn method_fn(lua: &Lua, method: reqwest::Method) -> mlua::Result<Function> {
    lua.create_async_function(move |lua, (url, opts): (String, Option<Table>)| {
        let prepared = build_prepared(&lua, method.clone(), url, Vec::new(), None, opts);
        async move {
            let resp = send(prepared?).await?;
            lua.create_userdata(resp)
        }
    })
}

/// Build an owned request spec from `opts`, layered over optional defaults (a client's base
/// headers/timeout). Per-call `headers` override defaults by name; `json`/`body`/`timeout` in
/// `opts` win. Synchronous, so nothing borrows Lua across the await.
fn build_prepared(
    lua: &Lua,
    method: reqwest::Method,
    url: String,
    mut headers: Vec<(String, String)>,
    mut timeout: Option<Duration>,
    opts: Option<Table>,
) -> mlua::Result<Prepared> {
    let mut body = None;
    if let Some(opts) = opts {
        if let Some(hdrs) = opts.get::<Option<Table>>("headers")? {
            for pair in hdrs.pairs::<String, String>() {
                let (k, v) = pair?;
                upsert_header(&mut headers, k, v);
            }
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
        } else if let Some(raw) = opts.get::<Option<String>>("body")? {
            body = Some(raw.into_bytes());
        }
        if let Some(t) = opts
            .get::<Option<String>>("timeout")?
            .and_then(|s| parse_duration(&s))
        {
            timeout = Some(t);
        }
    }
    Ok(Prepared {
        method,
        url,
        headers,
        body,
        timeout,
    })
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
    let client = reqwest::Client::new();
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
    let body = resp
        .text()
        .await
        .map_err(|e| mlua::Error::RuntimeError(format!("reading http response body: {e}")))?;
    Ok(HttpResponse {
        status,
        body,
        headers,
    })
}

/// `http.wait_for(url, { status = 200, timeout = "30s", every = "500ms" })` — poll GET until the
/// endpoint returns the expected status or the deadline elapses. The boot-then-probe primitive.
fn wait_for_fn(lua: &Lua) -> mlua::Result<Function> {
    lua.create_async_function(|lua, (url, opts): (String, Option<Table>)| {
        let params = wait_params(&opts);
        async move {
            let (expected, timeout, every) = params?;
            let deadline = Instant::now() + timeout;
            loop {
                let prepared = Prepared {
                    method: reqwest::Method::GET,
                    url: url.clone(),
                    headers: Vec::new(),
                    body: None,
                    timeout: Some(every),
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

fn wait_params(opts: &Option<Table>) -> mlua::Result<(u16, Duration, Duration)> {
    let mut status = 200;
    let mut timeout = Duration::from_secs(30);
    let mut every = Duration::from_millis(500);
    if let Some(opts) = opts {
        if let Some(s) = opts.get::<Option<u16>>("status")? {
            status = s;
        }
        if let Some(t) = opts
            .get::<Option<String>>("timeout")?
            .and_then(|s| parse_duration(&s))
        {
            timeout = t;
        }
        if let Some(e) = opts
            .get::<Option<String>>("every")?
            .and_then(|s| parse_duration(&s))
        {
            every = e;
        }
    }
    Ok((status, timeout, every))
}
