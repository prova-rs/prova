use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::{Request, Response};
use mlua::{
    Function, Lua, LuaSerdeExt, ObjectLike, Table, UserData, UserDataFields, UserDataMethods,
    Value,
};

use crate::model::parse_duration;

/// A resolved response — what a `:reply{…}` table parsed to, or what a handler's returned table
/// parsed to. One type for both paths, so a handler cannot express a response a declarative stub
/// can't (and vice versa).
#[derive(Clone)]
struct ReplySpec {
    status: u16,
    body: Vec<u8>,
    headers: Vec<(String, String)>,
    delay: Option<Duration>,
}

impl ReplySpec {
    fn plain(status: u16, msg: &str) -> Self {
        ReplySpec {
            status,
            body: msg.as_bytes().to_vec(),
            headers: vec![("content-type".into(), "text/plain".into())],
            delay: None,
        }
    }
}

#[derive(Clone)]
enum Reply {
    /// `m:on{…}` was called but `:reply(…)` never was. A silent 200 would make a forgotten reply
    /// look like a passing test, so this answers 501 and records why.
    Unset,
    Data(ReplySpec),
    Handler(Function),
}

struct Stub {
    method: Option<String>,
    path: Option<String>,
    path_matches: Option<String>,
    route: Option<Vec<Seg>>,
    reply: Reply,
}

/// One segment of a compiled `route`.
///
/// **Why `route` is its own key rather than an extension of `path`.** A literal colon is legal in
/// a URL path and real APIs use it — Google's custom methods are spelled `/v1/models/x:predict`.
/// Quietly reinterpreting `path` would break those. So exact-match keeps its meaning and
/// templating gets a name that says so: `path` (exact) · `path_matches` (Lua pattern) · `route`
/// (`:name` captures). Which one is in play is never ambiguous.
#[derive(Clone)]
enum Seg {
    Lit(String),
    Param(String),
}

fn compile_route(spec: &str) -> Vec<Seg> {
    spec.split('/')
        .map(|seg| match seg.strip_prefix(':') {
            Some(name) => Seg::Param(name.to_string()),
            None => Seg::Lit(seg.to_string()),
        })
        .collect()
}

/// Match a path against a compiled route, capturing params. Segment-wise, so a `:id` can never
/// swallow a `/` — which is the default failure of the hand-rolled `(.+)$` this replaces.
fn match_route(route: &[Seg], path: &str) -> Option<Vec<(String, String)>> {
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() != route.len() {
        return None;
    }
    let mut params = Vec::new();
    for (seg, part) in route.iter().zip(parts.iter()) {
        match seg {
            Seg::Lit(l) => {
                if l != part {
                    return None;
                }
            }
            Seg::Param(name) => {
                if part.is_empty() {
                    return None;
                }
                params.push((name.clone(), (*part).to_string()));
            }
        }
    }
    Some(params)
}

/// The request as both the handler and the journal see it — deliberately the *same shape*, so
/// `req.path` in a handler and `m:received()[1].path` in an assertion are the same field.
struct RequestData {
    method: String,
    path: String,
    query: Vec<(String, String)>,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

struct Recorded {
    req: RequestData,
    params: Vec<(String, String)>,
    status: u16,
    matched: bool,
    /// Who composed the answer: "stub" | "passthrough" | "replay" | "unmatched". `matched` stays
    /// narrowly "a stub matched", so a forwarded request reads as matched=false, source=passthrough.
    source: &'static str,
    error: Option<String>,
}

// -- cassettes ------------------------------------------------------------------------------

/// A recorded exchange. Request headers are kept (they are often the thing under test — an
/// idempotency key, a tenant id) but redacted; see `REDACTED_HEADERS`.
#[derive(serde::Serialize, serde::Deserialize)]
struct Cassette {
    version: u32,
    entries: Vec<Entry>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Entry {
    request: CassetteRequest,
    response: CassetteResponse,
}

/// `BTreeMap` so a cassette is byte-stable across runs: an unordered map would produce a
/// different file every record and turn every re-record into an unreadable diff.
#[derive(serde::Serialize, serde::Deserialize)]
struct CassetteRequest {
    method: String,
    path: String,
    query: BTreeMap<String, String>,
    headers: BTreeMap<String, String>,
    body: String,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct CassetteResponse {
    status: u16,
    headers: BTreeMap<String, String>,
    body: String,
}

/// Recording real traffic writes real traffic to a file someone will commit. These are redacted
/// by default — a cassette carrying a live bearer token is a security incident, not a bug. This
/// is a floor, not a guarantee: a bespoke auth header needs `redact = { … }`, and a cassette is
/// real traffic that deserves a read before it is committed.
const REDACTED_HEADERS: &[&str] = &[
    "authorization",
    "proxy-authorization",
    "cookie",
    "set-cookie",
    "x-api-key",
    "api-key",
    "x-auth-token",
];

const REDACTION: &str = "REDACTED";

/// Hop-by-hop headers, which describe *this* connection and must not be copied onto another one.
/// Forwarding `content-length`/`transfer-encoding` in particular makes the upstream describe a
/// body we then re-frame ourselves — a corrupt response that looks like a mock bug.
const HOP_BY_HOP: &[&str] = &[
    "host",
    "content-length",
    "transfer-encoding",
    "connection",
    "keep-alive",
    "upgrade",
    "proxy-connection",
    "te",
    "trailer",
];

fn redact_into(
    headers: &[(String, String)],
    extra: &[String],
    out: &mut BTreeMap<String, String>,
) {
    for (k, v) in headers {
        let redacted = REDACTED_HEADERS.contains(&k.as_str())
            || extra.iter().any(|e| e.eq_ignore_ascii_case(k));
        out.insert(
            k.clone(),
            if redacted {
                REDACTION.to_string()
            } else {
                v.clone()
            },
        );
    }
}

/// The replay key: method + path + query. Request *headers* are deliberately excluded — matching
/// on them would make a cassette break on a rotated token or a changed date, which is drift the
/// suite should not be reporting.
fn replay_key(method: &str, path: &str, query: &BTreeMap<String, String>) -> String {
    let q: Vec<String> = query.iter().map(|(k, v)| format!("{k}={v}")).collect();
    format!("{} {}?{}", method.to_ascii_uppercase(), path, q.join("&"))
}

struct Replay {
    entries: Vec<Entry>,
    consumed: Vec<bool>,
}

impl Replay {
    fn load(path: &str) -> mlua::Result<Self> {
        let text = std::fs::read_to_string(path).map_err(|e| {
            mlua::Error::RuntimeError(format!("http.mock: reading cassette {path:?}: {e}"))
        })?;
        let c: Cassette = serde_json::from_str(&text).map_err(|e| {
            mlua::Error::RuntimeError(format!("http.mock: parsing cassette {path:?}: {e}"))
        })?;
        let n = c.entries.len();
        Ok(Replay {
            entries: c.entries,
            consumed: vec![false; n],
        })
    }

    /// First *unconsumed* entry for this key. Consuming means repeated identical calls replay in
    /// recorded order (create → read-back reproduces instead of collapsing onto one answer),
    /// while different endpoints stay order-independent — a SUT that interleaves two calls is
    /// not doing anything wrong.
    fn take(&mut self, key: &str) -> Option<&CassetteResponse> {
        for (i, e) in self.entries.iter().enumerate() {
            if self.consumed[i] {
                continue;
            }
            if replay_key(&e.request.method, &e.request.path, &e.request.query) == key {
                self.consumed[i] = true;
                return Some(&e.response);
            }
        }
        None
    }
}

#[derive(Default)]
struct MockState {
    stubs: Vec<Stub>,
    journal: Vec<Recorded>,
    /// The dial. A proxy is a mock whose unmatched requests forward instead of 404 — one option,
    /// not a second concept, so stubs/journal/grammar are untouched by any of this.
    passthrough: Option<String>,
    record: Option<String>,
    replay: Option<Replay>,
    redact: Vec<String>,
    recorded: Vec<Entry>,
    /// Errors from *our own* stubs — a handler that raised, returned the wrong shape, or whose
    /// reply would not parse. Tracked apart from the journal's `error` field, which also covers
    /// a dead upstream and a replay miss: those are the *dependency* misbehaving (a 502 is a
    /// true report), whereas these are prova-side bugs wearing the dependency's clothes.
    handler_errors: Vec<String>,
    /// Opt out of strictness, for a test whose subject *is* the error path.
    allow_handler_errors: bool,
    /// The `latency` fault verb (continuous, distinct from a reply's one-shot `delay`) — the
    /// http face of the shared proxy vocabulary. Applied to every request while set.
    latency: Option<std::time::Duration>,
    /// The `drop` fault verb: sever — new requests are refused with an immediate 502.
    dropped: bool,
}

/// `Rc`/`RefCell` rather than `Arc`/`Mutex` on purpose: every task that touches this is
/// `spawn_local`'d onto the same thread as the Lua state, so a cross-thread lock would be
/// ceremony around a contention that cannot happen.
type Shared = Rc<RefCell<MockState>>;

struct MockServer {
    url: String,
    host: String,
    port: u16,
    /// The DNS name a container/VM/pod reaches this host-bound mock at, when `network` was
    /// requested — `host.docker.internal` by default (the Docker substrate's name for the host),
    /// overridable for another substrate. `None` → loopback-only, no cross-substrate vantage.
    network_host: Option<String>,
    state: Shared,
    shutdown: RefCell<Option<tokio::sync::oneshot::Sender<()>>>,
}

struct StubHandle {
    state: Shared,
    idx: usize,
}

/// `http.mock(ctx, opts?)` → a managed mock server.
pub(crate) fn mock_fn(lua: &Lua) -> mlua::Result<Function> {
    lua.create_function(|lua, (ctx, opts): (Value, Option<Table>)| {
        let server = start(lua, opts.as_ref())?;
        let ud = lua.create_userdata(server)?;
        // Tie the server's life to the caller's scope, exactly as a container's is. Going
        // through `ctx:manage` rather than reimplementing teardown means a mock is reaped by
        // the same LIFO machinery, in the same order, as every other resource — including under
        // `prova up`, where the scope is held until a signal rather than ending with a test.
        match ctx {
            Value::UserData(c) => {
                let _: Value = c.call_method("manage", &ud)?;
            }
            Value::Nil => {
                return Err(mlua::Error::RuntimeError(
                    "http.mock(ctx): pass the test or fixture context (`t` / `ctx`) so the \
                     server is torn down with the scope"
                        .into(),
                ))
            }
            other => {
                return Err(mlua::Error::RuntimeError(format!(
                    "http.mock(ctx): expected the test or fixture context, got a {}",
                    other.type_name()
                )))
            }
        }
        Ok(ud)
    })
}

/// Bind synchronously (so the port is known and the socket is accepting before we return), then
/// `spawn_local` the accept loop onto the engine's `LocalSet`.
fn start(lua: &Lua, opts: Option<&Table>) -> mlua::Result<MockServer> {
    let mut init = MockState::default();
    // A mock is a *host* process; a container reaches it not by a DNS alias (it is not on the
    // docker network) but at the host gateway. `network` opts into that: it binds all interfaces
    // (a real LAN exposure, hence off by default) and exposes a `.network` vantage the SUT wires
    // in. `true` → `host.docker.internal`; a string overrides the host name for another substrate.
    let mut network_host: Option<String> = None;
    if let Some(o) = opts {
        match o.get::<Option<Value>>("network")? {
            Some(Value::Boolean(true)) => {
                network_host = Some("host.docker.internal".to_string())
            }
            Some(Value::String(name)) => {
                network_host = Some(name.to_string_lossy().to_string())
            }
            Some(Value::Boolean(false)) | None | Some(Value::Nil) => {}
            Some(other) => {
                return Err(mlua::Error::RuntimeError(format!(
                    "http.mock: `network` must be true or a host name, got a {}",
                    other.type_name()
                )))
            }
        }
        init.passthrough = o.get::<Option<String>>("passthrough")?;
        init.record = o.get::<Option<String>>("record")?;
        init.allow_handler_errors = o
            .get::<Option<bool>>("allow_handler_errors")?
            .unwrap_or(false);
        let replay_path = o.get::<Option<String>>("replay")?;
        if let Some(t) = o.get::<Option<Table>>("redact")? {
            // Stored VERBATIM: `redact_into` compares header names case-insensitively already,
            // and the same list doubles as literal strings scrubbed from the serialized
            // cassette (cross-transport floor) — where case must be preserved.
            for h in t.sequence_values::<String>() {
                init.redact.push(h?);
            }
        }
        // Invalid states, rejected at the call site rather than surfacing as a confusing 404 or
        // an empty cassette three tests later.
        if init.passthrough.is_some() && replay_path.is_some() {
            return Err(mlua::Error::RuntimeError(
                "http.mock: `passthrough` and `replay` are mutually exclusive — one forwards to \
                 a real dependency, the other answers from a recording of one"
                    .into(),
            ));
        }
        if init.record.is_some() && init.passthrough.is_none() {
            return Err(mlua::Error::RuntimeError(
                "http.mock: `record` needs `passthrough` — a cassette records what a real \
                 dependency answered, and there is nothing to record without one"
                    .into(),
            ));
        }
        if let Some(p) = replay_path {
            init.replay = Some(Replay::load(&p)?);
        }
    }

    // Loopback unless a cross-substrate vantage was asked for. Binding all interfaces is the
    // security-relevant bit, so it is gated on the same explicit `network` request.
    let bind_ip = if network_host.is_some() {
        "0.0.0.0"
    } else {
        "127.0.0.1"
    };
    let std_listener = std::net::TcpListener::bind((bind_ip, 0))
        .map_err(|e| mlua::Error::RuntimeError(format!("http.mock: bind: {e}")))?;
    std_listener
        .set_nonblocking(true)
        .map_err(|e| mlua::Error::RuntimeError(format!("http.mock: set_nonblocking: {e}")))?;
    let port = std_listener
        .local_addr()
        .map_err(|e| mlua::Error::RuntimeError(format!("http.mock: local_addr: {e}")))?
        .port();
    let listener = tokio::net::TcpListener::from_std(std_listener)
        .map_err(|e| mlua::Error::RuntimeError(format!("http.mock: from_std: {e}")))?;

    let state: Shared = Rc::new(RefCell::new(init));
    let (tx, mut rx) = tokio::sync::oneshot::channel::<()>();

    let accept_state = state.clone();
    let accept_lua = lua.clone();
    // `spawn_local`, never `tokio::spawn`: this task holds a `Lua` handle (to call handlers),
    // and mlua handles are `!Send`. See `engine::block_on_local` for why a `LocalSet` exists.
    tokio::task::spawn_local(async move {
        loop {
            tokio::select! {
                _ = &mut rx => break,
                accepted = listener.accept() => {
                    let Ok((stream, _peer)) = accepted else { break };
                    let io = hyper_util::rt::TokioIo::new(stream);
                    let conn_state = accept_state.clone();
                    let conn_lua = accept_lua.clone();
                    tokio::task::spawn_local(async move {
                        let svc = service_fn(move |req: Request<Incoming>| {
                            let s = conn_state.clone();
                            let l = conn_lua.clone();
                            async move { handle(l, s, req).await }
                        });
                        // http1 specifically: it puts no `Send` bound on the service or its
                        // future, which is what lets a Lua handler live inside one. axum and
                        // anything tower-shaped bound it `Send` and cannot express this.
                        let _ = hyper::server::conn::http1::Builder::new()
                            .serve_connection(io, svc)
                            .await;
                    });
                }
            }
        }
    });

    Ok(MockServer {
        // `url`/`host` remain loopback: they are how *this* process (the test) probes the mock,
        // and 0.0.0.0 includes loopback. The cross-substrate address lives on `.network`.
        url: format!("http://127.0.0.1:{port}"),
        host: "127.0.0.1".to_string(),
        port,
        network_host,
        state,
        shutdown: RefCell::new(Some(tx)),
    })
}

async fn handle(
    lua: Lua,
    state: Shared,
    req: Request<Incoming>,
) -> Result<Response<Full<Bytes>>, std::convert::Infallible> {
    let rd = read_request(req).await;

    // The fault dial, consulted before any answer is composed (see the socket proxy for the
    // vocabulary): `latency` delays every request; `drop` refuses them outright.
    let (latency, dropped) = {
        let s = state.borrow();
        (s.latency, s.dropped)
    };
    if dropped {
        return Ok(respond(
            &state,
            rd,
            Vec::new(),
            ReplySpec::plain(502, "prova http.mock: dropped (fault injection)"),
            false,
            "unmatched",
            None,
        ));
    }
    if let Some(l) = latency {
        tokio::time::sleep(l).await;
    }

    // Resolve the match and clone the reply out before doing anything that can re-enter Lua: a
    // handler may legitimately call `m:on{…}` or `m:received()`, which borrows this same
    // RefCell. Holding a borrow across an await into Lua would panic at runtime.
    let hit = match find_match(&lua, &state, &rd) {
        Ok(h) => h,
        Err(e) => {
            return Ok(respond(
                &state,
                rd,
                Vec::new(),
                ReplySpec::plain(500, &format!("mock: matching failed: {e}")),
                false,
                "stub",
                Some(e.to_string()),
            ))
        }
    };
    let params: Vec<(String, String)> =
        hit.as_ref().map(|(_, p)| p.clone()).unwrap_or_default();
    let reply = hit
        .as_ref()
        .map(|(i, _)| state.borrow().stubs[*i].reply.clone());

    // A stub always wins over the dial. That is what makes *partial* mocking work: stub the one
    // endpoint you need to control, let everything else reach the real service.
    let (spec, source, error) = match reply {
        Some(Reply::Unset) => (
            ReplySpec::plain(501, "prova http.mock: stub matched but has no :reply(…)"),
            "stub",
            Some("stub matched but has no :reply(…)".to_string()),
        ),
        Some(Reply::Data(d)) => (d, "stub", None),
        Some(Reply::Handler(f)) => {
            let (s, e) = run_handler(&lua, f, &rd, &params).await;
            (s, "stub", e)
        }
        None => unmatched(&state, &rd).await,
    };

    if let Some(d) = spec.delay {
        tokio::time::sleep(d).await;
    }
    let matched = hit.is_some();
    Ok(respond(&state, rd, params, spec, matched, source, error))
}

/// No stub matched: consult the dial. Replay answers from a recording; passthrough forwards to
/// the real dependency; otherwise it is a 404, exactly as in Phase A.
async fn unmatched(
    state: &Shared,
    rd: &RequestData,
) -> (ReplySpec, &'static str, Option<String>) {
    let (has_replay, passthrough) = {
        let s = state.borrow();
        (s.replay.is_some(), s.passthrough.clone())
    };

    if has_replay {
        let query: BTreeMap<String, String> = rd.query.iter().cloned().collect();
        let key = replay_key(&rd.method, &rd.path, &query);
        let hit = {
            let mut s = state.borrow_mut();
            s.replay
                .as_mut()
                .and_then(|r| r.take(&key))
                .map(|resp| ReplySpec {
                    status: resp.status,
                    body: resp.body.clone().into_bytes(),
                    headers: resp
                        .headers
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect(),
                    delay: None,
                })
        };
        return match hit {
            Some(spec) => (spec, "replay", None),
            // Strict on purpose. Inventing an answer for a call the cassette never recorded
            // would let the SUT change behavior without the suite noticing — the exact failure
            // a cassette exists to catch.
            None => {
                // 502, not 404: a miss is the *recording infrastructure* failing the request,
                // and a 404 reads like a plausible real answer an SUT might handle gracefully.
                let msg = format!(
                    "prova http.mock: cassette has no unconsumed entry for {key} — re-record it \
                     if the system under test legitimately changed"
                );
                (ReplySpec::plain(502, &msg), "replay", Some(msg))
            }
        };
    }

    if let Some(base) = passthrough {
        return match forward(&base, rd).await {
            Ok(spec) => {
                record_exchange(state, rd, &spec);
                (spec, "passthrough", None)
            }
            // 502 is the honest status: *we* are a gateway and the upstream did not answer.
            // Reporting the mock's own failure as a 500 would blame the SUT for our plumbing.
            Err(e) => (
                ReplySpec::plain(502, &format!("prova http.mock: upstream {base}: {e}")),
                "passthrough",
                Some(e),
            ),
        };
    }

    (
        ReplySpec::plain(404, "prova http.mock: no matching stub"),
        "unmatched",
        None,
    )
}

/// Forward one request to the real dependency, verbatim but for the hop-by-hop headers.
async fn forward(base: &str, rd: &RequestData) -> Result<ReplySpec, String> {
    let mut url = format!("{}{}", base.trim_end_matches('/'), rd.path);
    if !rd.query.is_empty() {
        let q: Vec<String> = rd
            .query
            .iter()
            .map(|(k, v)| {
                format!(
                    "{}={}",
                    form_urlencoded::byte_serialize(k.as_bytes()).collect::<String>(),
                    form_urlencoded::byte_serialize(v.as_bytes()).collect::<String>()
                )
            })
            .collect();
        url.push('?');
        url.push_str(&q.join("&"));
    }
    let method = reqwest::Method::from_bytes(rd.method.as_bytes())
        .map_err(|e| format!("bad method {:?}: {e}", rd.method))?;
    let mut req = reqwest::Client::new().request(method, &url);
    for (k, v) in &rd.headers {
        if HOP_BY_HOP.contains(&k.as_str()) {
            continue;
        }
        req = req.header(k.as_str(), v.as_str());
    }
    if !rd.body.is_empty() {
        req = req.body(rd.body.clone());
    }
    let resp = req.send().await.map_err(|e| e.to_string())?;
    let status = resp.status().as_u16();
    let headers: Vec<(String, String)> = resp
        .headers()
        .iter()
        .filter(|(k, _)| !HOP_BY_HOP.contains(&k.as_str()))
        .map(|(k, v)| {
            (
                k.as_str().to_ascii_lowercase(),
                v.to_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    let body = resp.bytes().await.map_err(|e| e.to_string())?.to_vec();
    Ok(ReplySpec {
        status,
        body,
        headers,
        delay: None,
    })
}

/// Append a forwarded exchange to the pending cassette. Only *forwarded* traffic is recorded: a
/// cassette is a recording of the real dependency, and recording our own stubs back to ourselves
/// would make replay assert that the mock agrees with the mock.
fn record_exchange(state: &Shared, rd: &RequestData, spec: &ReplySpec) {
    let mut s = state.borrow_mut();
    if s.record.is_none() {
        return;
    }
    let mut req_headers = BTreeMap::new();
    redact_into(&rd.headers, &s.redact, &mut req_headers);
    let mut resp_headers = BTreeMap::new();
    redact_into(&spec.headers, &s.redact, &mut resp_headers);
    s.recorded.push(Entry {
        request: CassetteRequest {
            method: rd.method.clone(),
            path: rd.path.clone(),
            query: rd.query.iter().cloned().collect(),
            headers: req_headers,
            body: String::from_utf8_lossy(&rd.body).to_string(),
        },
        response: CassetteResponse {
            status: spec.status,
            headers: resp_headers,
            body: String::from_utf8_lossy(&spec.body).to_string(),
        },
    });
}

/// Call a Lua reply handler. An error here must not be silent: it answers 500 *and* lands in the
/// journal, so a broken handler is visible to an assertion rather than looking like the
/// dependency legitimately failed.
async fn run_handler(
    lua: &Lua,
    f: Function,
    rd: &RequestData,
    params: &[(String, String)],
) -> (ReplySpec, Option<String>) {
    let req_tbl = match req_to_lua(lua, rd, params) {
        Ok(t) => t,
        Err(e) => {
            return (
                ReplySpec::plain(500, "mock: handler input"),
                Some(e.to_string()),
            )
        }
    };
    match f.call_async::<Value>(req_tbl).await {
        Ok(Value::Table(t)) => match parse_reply(lua, &t) {
            Ok(s) => (s, None),
            Err(e) => (
                ReplySpec::plain(500, &format!("mock: handler reply: {e}")),
                Some(e.to_string()),
            ),
        },
        Ok(other) => {
            let msg = format!(
                "mock: handler must return a response table, returned a {}",
                other.type_name()
            );
            (ReplySpec::plain(500, &msg), Some(msg))
        }
        Err(e) => (
            ReplySpec::plain(500, &format!("mock: handler raised: {e}")),
            Some(e.to_string()),
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn respond(
    state: &Shared,
    req: RequestData,
    params: Vec<(String, String)>,
    spec: ReplySpec,
    matched: bool,
    source: &'static str,
    error: Option<String>,
) -> Response<Full<Bytes>> {
    // A stub-sourced error is *our* bug, not the dependency's: track it so `stop()` can fail the
    // owning scope. Without this a SUT with a retry or a fallback swallows the 500 and the suite
    // goes green over a broken handler, blaming the dependency for flakiness.
    if source == "stub" {
        if let Some(e) = &error {
            state.borrow_mut().handler_errors.push(e.clone());
        }
    }
    // Record *every* request, matched or not. An unmatched call is usually the most interesting
    // thing a mock can tell you — it is the SUT doing something you did not predict.
    state.borrow_mut().journal.push(Recorded {
        req,
        params,
        status: spec.status,
        matched,
        source,
        error,
    });

    let mut builder = Response::builder().status(spec.status);
    for (k, v) in &spec.headers {
        builder = builder.header(k.as_str(), v.as_str());
    }
    builder
        .body(Full::new(Bytes::from(spec.body)))
        .unwrap_or_else(|e| {
            Response::builder()
                .status(500)
                .body(Full::new(Bytes::from(format!("mock: bad response: {e}"))))
                .expect("500 with a plain body is always constructible")
        })
}

async fn read_request(req: Request<Incoming>) -> RequestData {
    let method = req.method().as_str().to_string();
    let uri = req.uri().clone();
    let path = uri.path().to_string();
    let query = uri
        .query()
        .map(|q| form_urlencoded::parse(q.as_bytes()).into_owned().collect())
        .unwrap_or_default();
    let headers = req
        .headers()
        .iter()
        .map(|(k, v)| {
            // Lowercase: HTTP header names are case-insensitive, so a journal that preserved the
            // sender's casing would make `r.headers["X-Idempotency-Key"]` work or not depending
            // on which client wrote the request. One spelling, always.
            (
                k.as_str().to_ascii_lowercase(),
                v.to_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    let body = req
        .into_body()
        .collect()
        .await
        .map(|c| c.to_bytes().to_vec())
        .unwrap_or_default();
    RequestData {
        method,
        path,
        query,
        headers,
        body,
    }
}

/// First match wins — insertion order. A later, more specific stub does not override an earlier
/// general one, because "most specific wins" needs a specificity ranking, and every ranking is a
/// rule you have to know before you can read a test.
type Candidate = (usize, Option<String>, Option<Vec<Seg>>);
/// The matching stub's index plus the params its `route` captured.
type Hit = (usize, Vec<(String, String)>);

fn find_match(lua: &Lua, state: &Shared, rd: &RequestData) -> mlua::Result<Option<Hit>> {
    // Collect the patterns first: matching calls back into Lua (`string.match`), and Lua could
    // re-enter this RefCell.
    let candidates: Vec<Candidate> = {
        let s = state.borrow();
        s.stubs
            .iter()
            .enumerate()
            .filter(|(_, stub)| {
                stub.method
                    .as_ref()
                    .is_none_or(|m| rd.method.eq_ignore_ascii_case(m))
                    && stub.path.as_ref().is_none_or(|p| &rd.path == p)
            })
            .map(|(i, stub)| (i, stub.path_matches.clone(), stub.route.clone()))
            .collect()
    };
    for (i, pat, route) in candidates {
        if let Some(r) = route {
            match match_route(&r, &rd.path) {
                Some(params) => return Ok(Some((i, params))),
                None => continue,
            }
        }
        match pat {
            None => return Ok(Some((i, Vec::new()))),
            // Lua patterns, not regex — `path_matches` must mean exactly what `:matches(pat)`
            // means everywhere else in the assertion surface, so ask Lua rather than reimplement.
            Some(p) => {
                let string: Table = lua.globals().get("string")?;
                let matcher: Function = string.get("match")?;
                let r: Value = matcher.call((rd.path.clone(), p))?;
                if !matches!(r, Value::Nil) {
                    return Ok(Some((i, Vec::new())));
                }
            }
        }
    }
    Ok(None)
}

fn req_to_lua(lua: &Lua, rd: &RequestData, params: &[(String, String)]) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set("method", rd.method.clone())?;
    t.set("path", rd.path.clone())?;
    let q = lua.create_table()?;
    for (k, v) in &rd.query {
        q.set(k.clone(), v.clone())?;
    }
    t.set("query", q)?;
    let h = lua.create_table()?;
    for (k, v) in &rd.headers {
        h.set(k.clone(), v.clone())?;
    }
    t.set("headers", h)?;
    t.set("body", lua.create_string(&rd.body)?)?;
    let p = lua.create_table()?;
    for (k, v) in params {
        p.set(k.clone(), v.clone())?;
    }
    t.set("params", p)?;
    // `json` is a convenience, not a contract: nil when the body isn't JSON. Unlike the http
    // client's `res:json()` (which raises), a request body you didn't send isn't your bug — and
    // a handler wants to branch on shape, not defend against a raise.
    if let Ok(jv) = serde_json::from_slice::<serde_json::Value>(&rd.body) {
        t.set("json", lua.to_value(&jv)?)?;
    }
    Ok(t)
}

fn recorded_to_lua(lua: &Lua, r: &Recorded, seq: usize) -> mlua::Result<Table> {
    let t = req_to_lua(lua, &r.req, &r.params)?;
    t.set("seq", seq)?; // §6: monotonic per mock, 1-based — ordering falls out of the journal
    t.set("status", r.status)?;
    t.set("matched", r.matched)?;
    t.set("source", r.source)?;
    if let Some(e) = &r.error {
        t.set("error", e.clone())?;
    }
    Ok(t)
}

fn parse_reply(lua: &Lua, t: &Table) -> mlua::Result<ReplySpec> {
    let status = t.get::<Option<u16>>("status")?.unwrap_or(200);
    if !(100..=599).contains(&status) {
        return Err(mlua::Error::RuntimeError(format!(
            "mock reply: status must be 100..599, got {status}"
        )));
    }

    let mut headers: Vec<(String, String)> = Vec::new();
    if let Some(h) = t.get::<Option<Table>>("headers")? {
        for pair in h.pairs::<String, String>() {
            let (k, v) = pair?;
            headers.push((k.to_ascii_lowercase(), v));
        }
    }

    let json = t.get::<Option<Value>>("json")?.filter(|v| !v.is_nil());
    let body_str = t.get::<Option<String>>("body")?;
    if json.is_some() && body_str.is_some() {
        return Err(mlua::Error::RuntimeError(
            "mock reply: has both `json` and `body` — a response has one body, not two".into(),
        ));
    }

    let body = match (json, body_str) {
        (Some(j), _) => {
            let jv: serde_json::Value = lua.from_value(j)?;
            let bytes = serde_json::to_vec(&jv).map_err(|e| {
                mlua::Error::RuntimeError(format!("mock reply: encoding `json`: {e}"))
            })?;
            if !headers.iter().any(|(k, _)| k == "content-type") {
                headers.push(("content-type".into(), "application/json".into()));
            }
            bytes
        }
        (None, Some(b)) => b.into_bytes(),
        (None, None) => Vec::new(),
    };

    let delay = match t.get::<Option<String>>("delay")? {
        Some(s) => Some(parse_duration(&s).ok_or_else(|| {
            mlua::Error::RuntimeError(format!("mock reply: bad `delay` duration {s:?}"))
        })?),
        None => None,
    };

    Ok(ReplySpec {
        status,
        body,
        headers,
        delay,
    })
}

impl UserData for MockServer {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        // The grammar's fields, same as any resource: wire `m.url` into the SUT exactly the way
        // you wire a database's.
        fields.add_field_method_get("url", |_, this| Ok(this.url.clone()));
        // The universal driver-target alias (cohesion): every addressable mock exposes
        // `.endpoint` = the exact string its driver consumes, one name across transports.
        fields.add_field_method_get("endpoint", |_, this| Ok(this.url.clone()));
        fields.add_field_method_get("host", |_, this| Ok(this.host.clone()));
        fields.add_field_method_get("port", |_, this| Ok(this.port));
        // `.network` — the vantage a containerized/VM'd SUT wires in, present only when
        // `network` was requested. Mirrors a container resource's `.network`, but the address is
        // the host gateway rather than a DNS alias, because a mock is a host process.
        fields.add_field_method_get("network", |lua, this| {
            let Some(host) = &this.network_host else {
                return Ok(Value::Nil);
            };
            let t = lua.create_table()?;
            t.set("url", format!("http://{host}:{}", this.port))?;
            t.set("host", host.clone())?;
            t.set("port", this.port)?;
            Ok(Value::Table(t))
        });
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        // m:on{ method?, path?, path_matches? } → a stub handle to :reply on.
        methods.add_method("on", |lua, this, spec: Table| {
            let stub = Stub {
                method: spec
                    .get::<Option<String>>("method")?
                    .map(|m| m.to_ascii_uppercase()),
                path: spec.get::<Option<String>>("path")?,
                path_matches: spec.get::<Option<String>>("path_matches")?,
                route: spec
                    .get::<Option<String>>("route")?
                    .as_deref()
                    .map(compile_route),
                reply: Reply::Unset,
            };
            let idx = {
                let mut s = this.state.borrow_mut();
                s.stubs.push(stub);
                s.stubs.len() - 1
            };
            lua.create_userdata(StubHandle {
                state: this.state.clone(),
                idx,
            })
        });

        // m:received(filter?) → the journal, as plain Lua tables. Deliberately *data*, not a
        // `verify(count, pattern)` DSL: `t:expect` already asserts, and the matchers were never
        // stringly-typed. Filters are the §6 contract (see `journal_keep`): table = subset
        // match over the exposed entry, function = predicate. Entries are materialized before
        // filtering so a predicate that re-enters the mock can't hit a live borrow.
        methods.add_method("received", |lua, this, filter: Option<Value>| {
            let entries: Vec<Table> = {
                let s = this.state.borrow();
                s.journal
                    .iter()
                    .enumerate()
                    .map(|(i, r)| recorded_to_lua(lua, r, i + 1))
                    .collect::<mlua::Result<_>>()?
            };
            let out = lua.create_table()?;
            let mut n = 0;
            for entry in entries {
                if super::journal_keep(lua, &filter, &entry)? {
                    n += 1;
                    out.set(n, entry)?;
                }
            }
            Ok(out)
        });

        // `stop` is what `ctx:manage` calls; idempotent, so an explicit stop plus scope teardown
        // is not an error.
        // `stop` is what `ctx:manage` calls; idempotent, so an explicit stop plus scope teardown
        // is not an error. The cassette is written here rather than per-request so a suite that
        // fails mid-way still leaves a coherent file — teardown runs on failure too.
        //
        // Raising here is how a handler error reaches a report: a handler runs on a server task,
        // outside any test's stack, so there is nowhere for it to land at the time. `ctx:manage`
        // calls this at scope end and a raising teardown is its own reported leaf — so this needs
        // no mock-specific reporting path at all.
        methods.add_method("stop", |_, this, ()| {
            if let Some(tx) = this.shutdown.borrow_mut().take() {
                let _ = tx.send(());
                write_cassette(&this.state)?;
            }
            let errs = take_handler_errors(&this.state);
            if !errs.is_empty() {
                return Err(handler_error_report("http.mock", &errs));
            }
            Ok(())
        });
        // `close` is `stop` — the proxy grammar says close (a proxy is a connection-shaped
        // thing), the resource grammar says stop; both flush the cassette on the way out.
        methods.add_method("close", |_, this, ()| {
            if let Some(tx) = this.shutdown.borrow_mut().take() {
                let _ = tx.send(());
                write_cassette(&this.state)?;
            }
            let errs = take_handler_errors(&this.state);
            if !errs.is_empty() {
                return Err(handler_error_report("http.mock", &errs));
            }
            Ok(())
        });

        // The fault vocabulary's http face (proofs/spec/faults). `latency`/`drop`/`after` are
        // native here; `corrupt`/`throttle` are byte-level conditions — interpose a
        // `socket.proxy` in front for those, and the error says so instead of half-faking it.
        methods.add_method("latency", |_, this, d: String| {
            let dur = parse_duration(&d).ok_or_else(|| {
                mlua::Error::RuntimeError(format!("latency: bad duration {d:?}"))
            })?;
            this.state.borrow_mut().latency = Some(dur);
            Ok(())
        });
        methods.add_method("drop", |_, this, ()| {
            this.state.borrow_mut().dropped = true;
            Ok(())
        });
        methods.add_method("after", |lua, this, d: String| {
            let delay = parse_duration(&d).ok_or_else(|| {
                mlua::Error::RuntimeError(format!("after: bad duration {d:?}"))
            })?;
            lua.create_userdata(HttpFuse {
                delay,
                state: this.state.clone(),
            })
        });
        methods.add_method("corrupt", |_, _this, ()| {
            Err::<(), _>(mlua::Error::RuntimeError(
                "corrupt is a byte-level fault — interpose `socket.proxy` in front of this \
                 endpoint for wire corruption; http.proxy speaks latency/drop/after"
                    .into(),
            ))
        });
        methods.add_method("throttle", |_, _this, _r: String| {
            Err::<(), _>(mlua::Error::RuntimeError(
                "throttle is a byte-level fault — interpose `socket.proxy` in front of this \
                 endpoint for rate limits; http.proxy speaks latency/drop/after"
                    .into(),
            ))
        });
    }
}

/// A scheduled http fault (`p:after("100ms"):drop()`), mirroring the socket proxy's fuse.
struct HttpFuse {
    delay: std::time::Duration,
    state: Shared,
}

impl UserData for HttpFuse {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("drop", |_, this, ()| {
            let delay = this.delay;
            let state = this.state.clone();
            tokio::task::spawn_local(async move {
                tokio::time::sleep(delay).await;
                state.borrow_mut().dropped = true;
            });
            Ok(())
        });
        methods.add_method("latency", |_, this, d: String| {
            let dur = parse_duration(&d).ok_or_else(|| {
                mlua::Error::RuntimeError(format!("latency: bad duration {d:?}"))
            })?;
            let delay = this.delay;
            let state = this.state.clone();
            tokio::task::spawn_local(async move {
                tokio::time::sleep(delay).await;
                state.borrow_mut().latency = Some(dur);
            });
            Ok(())
        });
    }
}

/// `http.proxy(ctx, { upstream?, cassette?, mode?, redact? })` — the interpose posture as its
/// own verb (docs/design/mocks-proxies-drivers.md), implemented as sugar over the mock's dial:
/// a proxy IS a mock whose unmatched requests forward (or replay). Modes:
///   passthrough (default) — forward, record nothing
///   record  — forward AND capture the cassette (flushed on close/scope exit)
///   replay  — answer from the cassette, upstream not needed (or consulted)
///   auto    — record when the cassette file is absent, replay when present
pub(crate) fn proxy_fn(lua: &Lua) -> mlua::Result<Function> {
    lua.create_function(|lua, (ctx, opts): (Value, Table)| {
        let upstream = opts.get::<Option<String>>("upstream")?;
        let cassette = opts.get::<Option<String>>("cassette")?;
        let mode = opts
            .get::<Option<String>>("mode")?
            .unwrap_or_else(|| "passthrough".to_string());
        let redact = opts.get::<Option<Value>>("redact")?;

        let translated = lua.create_table()?;
        let need_upstream = |what: &str| {
            mlua::Error::RuntimeError(format!(
                "http.proxy: mode {what:?} needs `upstream` — there is nothing to forward to"
            ))
        };
        let need_cassette = |what: &str| {
            mlua::Error::RuntimeError(format!(
                "http.proxy: mode {what:?} needs `cassette` — there is nothing to record into \
                 or replay from"
            ))
        };
        match mode.as_str() {
            "passthrough" => {
                let up = upstream.ok_or_else(|| need_upstream("passthrough"))?;
                translated.set("passthrough", up)?;
            }
            "record" => {
                let up = upstream.ok_or_else(|| need_upstream("record"))?;
                let cas = cassette.ok_or_else(|| need_cassette("record"))?;
                translated.set("passthrough", up)?;
                translated.set("record", cas)?;
            }
            "replay" => {
                let cas = cassette.ok_or_else(|| need_cassette("replay"))?;
                translated.set("replay", cas)?;
            }
            "auto" => {
                let cas = cassette.ok_or_else(|| need_cassette("auto"))?;
                if std::path::Path::new(&cas).exists() {
                    translated.set("replay", cas)?;
                } else {
                    let up = upstream.ok_or_else(|| need_upstream("auto (recording)"))?;
                    translated.set("passthrough", up)?;
                    translated.set("record", cas)?;
                }
            }
            other => {
                return Err(mlua::Error::RuntimeError(format!(
                    "http.proxy: mode must be passthrough|record|replay|auto, got {other:?}"
                )))
            }
        }
        if let Some(r) = redact {
            translated.set("redact", r)?;
        }

        let server = start(lua, Some(&translated))?;
        let ud = lua.create_userdata(server)?;
        match ctx {
            Value::UserData(c) => {
                let _: Value = c.call_method("manage", &ud)?;
            }
            Value::Nil => {
                return Err(mlua::Error::RuntimeError(
                    "http.proxy(ctx): pass the test or fixture context (`t` / `ctx`) so the \
                     proxy is torn down with the scope"
                        .into(),
                ))
            }
            other => {
                return Err(mlua::Error::RuntimeError(format!(
                    "http.proxy(ctx): expected the test or fixture context, got a {}",
                    other.type_name()
                )))
            }
        }
        Ok(ud)
    })
}

/// Drain the handler errors — so an explicit `m:stop()` followed by scope teardown reports once,
/// not twice.
fn take_handler_errors(state: &Shared) -> Vec<String> {
    let mut s = state.borrow_mut();
    if s.allow_handler_errors {
        s.handler_errors.clear();
        return Vec::new();
    }
    std::mem::take(&mut s.handler_errors)
}

pub(super) fn handler_error_report(ns: &str, errs: &[String]) -> mlua::Error {
    let n = errs.len();
    let plural = if n == 1 { "" } else { "s" };
    mlua::Error::RuntimeError(format!(
        "{ns}: {n} reply-handler error{plural} — the mock's own stub failed, so a green run here \
         would be reporting prova's bug as the dependency's. First: {}\n\
         If the error path is the subject of the test, pass `allow_handler_errors = true`.",
        errs[0]
    ))
}

fn write_cassette(state: &Shared) -> mlua::Result<()> {
    let mut s = state.borrow_mut();
    let Some(path) = s.record.clone() else {
        return Ok(());
    };
    let cassette = Cassette {
        version: 1,
        entries: std::mem::take(&mut s.recorded),
    };
    let text = serde_json::to_string_pretty(&cassette)
        .map_err(|e| mlua::Error::RuntimeError(format!("http.mock: encoding cassette: {e}")))?;
    // The literal-string floor on top of by-header-name redaction: a secret named in `redact`
    // never survives into the file even if it rode in a query param or body, not a header.
    let text = super::cassette::scrub(text, &s.redact);
    std::fs::write(&path, text).map_err(|e| {
        mlua::Error::RuntimeError(format!("http.mock: writing cassette {path:?}: {e}"))
    })
}

impl UserData for StubHandle {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("reply", |lua, this, v: Value| {
            let reply = match v {
                // The primitive. `topologies.md`: the convenience never removes it.
                Value::Function(f) => Reply::Handler(f),
                // The convenience — and the form `prova up` can serve with no test in scope, and
                // that a cassette round-trips to.
                Value::Table(t) => Reply::Data(parse_reply(lua, &t)?),
                other => {
                    return Err(mlua::Error::RuntimeError(format!(
                        "mock :reply expects a response table or a handler function, got a {}",
                        other.type_name()
                    )))
                }
            };
            this.state.borrow_mut().stubs[this.idx].reply = reply;
            Ok(())
        });
    }
}
