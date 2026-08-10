use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::time::Duration;

use mlua::{
    Function, Lua, LuaSerdeExt, ObjectLike, Table, UserData, UserDataFields, UserDataMethods,
    Value,
};
use prost_reflect::{
    DescriptorPool, DeserializeOptions, DynamicMessage, MessageDescriptor, MethodDescriptor,
    SerializeOptions,
};
use tonic::codegen::Service as _;
use tonic::{Code, Request as TonicRequest, Response as TonicResponse, Status};

use super::grpc::DynCodec;
use crate::model::parse_duration;

fn err(msg: impl Into<String>) -> mlua::Error {
    mlua::Error::RuntimeError(msg.into())
}

/// hyper's http2 spawns per-stream tasks through this. The stock `TokioExecutor` uses
/// `tokio::spawn` and would force `Send` all the way down to the Lua handler; `spawn_local`
/// keeps every stream on the thread that owns the Lua state.
#[derive(Clone)]
struct LocalExec;

impl<F> hyper::rt::Executor<F> for LocalExec
where
    F: Future<Output = ()> + 'static,
{
    fn execute(&self, fut: F) {
        tokio::task::spawn_local(fut);
    }
}

/// A resolved answer. `response` stays JSON rather than a built `DynamicMessage` because a stub
/// may match several methods (`method_matches`), so the output descriptor to build against is
/// not known until a call actually arrives.
#[derive(Clone)]
struct ReplySpec {
    code: Code,
    message: String,
    response: Option<serde_json::Value>,
    delay: Option<Duration>,
}

#[derive(Clone)]
enum Reply {
    Unset,
    Data(ReplySpec),
    Handler(Function),
}

struct Stub {
    method: Option<String>,
    method_matches: Option<String>,
    reply: Reply,
}

struct Recorded {
    method: String,
    request: serde_json::Value,
    code: String,
    matched: bool,
    /// §6: who composed the answer — "stub" | "unmatched" (grpc has no passthrough dial yet;
    /// the vocabulary is shared with the http facet and prova.double).
    source: &'static str,
    error: Option<String>,
}

#[derive(Default)]
struct MockState {
    stubs: Vec<Stub>,
    journal: Vec<Recorded>,
    /// See the http facet: errors from our own stubs, tracked apart from a status the mock
    /// legitimately answered with.
    handler_errors: Vec<String>,
    allow_handler_errors: bool,
    /// Set on a `grpc.proxy` (proofs/spec/cassettes/grpc): the interpose behavior that replaces
    /// the stub path in `answer`. `None` for an ordinary mock.
    proxy: Option<Proxy>,
    /// The fault dial (cohesion) — only a proxy sets these; a plain mock leaves them default.
    faults: GrpcFaults,
}

/// A grpc.proxy's interpose behavior — record forwards to a real upstream and captures pairs;
/// replay answers from the cassette. A proxy is the mock's dial, exactly as http.proxy is.
enum Proxy {
    Record {
        channel: super::grpc::Channel,
        pool: DescriptorPool,
        turns: Vec<GrpcTurn>,
        cassette: String,
        /// The schema learned from the upstream, stored in the cassette so replay is
        /// self-describing (needs no proto and no upstream).
        fds_bytes: Vec<u8>,
        /// Literal strings scrubbed from the serialized cassette at flush time.
        redact: Vec<String>,
    },
    Replay {
        turns: Vec<GrpcTurn>,
        consumed: Vec<bool>,
    },
}

/// The fault dial on a grpc.proxy (cohesion — the shared vocabulary). `latency`/`drop`/`after`
/// are transport-generic; the L7 byte faults (corrupt/throttle) stay socket-only by design.
#[derive(Default)]
struct GrpcFaults {
    latency: Option<Duration>,
    dropped: bool,
}

/// One recorded unary call: method + request select the response (or the error code).
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct GrpcTurn {
    method: String,
    request: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    response: Option<serde_json::Value>,
    code: String,
}

/// The on-disk grpc cassette: the schema (so replay needs no proto/upstream) plus the turns.
#[derive(serde::Serialize, serde::Deserialize)]
struct GrpcCassette {
    version: u32,
    kind: String,
    /// The encoded `FileDescriptorSet`, base64'd — the self-describing part.
    fds: String,
    turns: Vec<GrpcTurn>,
}

type Shared = Rc<RefCell<MockState>>;

struct GrpcMock {
    url: String,
    host: String,
    port: u16,
    /// See the http facet: the host-gateway name a container reaches this mock at when `network`
    /// was requested. `None` → loopback-only.
    network_host: Option<String>,
    state: Shared,
    shutdown: RefCell<Option<tokio::sync::oneshot::Sender<()>>>,
}

struct StubHandle {
    state: Shared,
    idx: usize,
}

/// The client's options, mirrored: what `call_status` *reports* is what `:reply` *takes*. One
/// spelling in both directions, so a test reads the same as the failure it reproduces.
fn serialize_opts() -> SerializeOptions {
    SerializeOptions::new()
        .skip_default_fields(false)
        .use_proto_field_name(true)
        .stringify_64_bit_integers(false)
}

fn deserialize_opts() -> DeserializeOptions {
    DeserializeOptions::new().deny_unknown_fields(false)
}

pub(crate) fn mock_fn(lua: &Lua) -> mlua::Result<Function> {
    lua.create_function(|lua, (ctx, opts): (Value, Option<Table>)| {
        let opts = opts.ok_or_else(|| {
            err(
                "grpc.mock(ctx, opts): a mock must be told its schema — pass `proto = \"…\"`. \
                 Unlike grpc.client, it cannot learn one by reflection: it *is* the server.",
            )
        })?;
        let server = start(lua, &opts)?;
        let ud = lua.create_userdata(server)?;
        match ctx {
            Value::UserData(c) => {
                let _: Value = c.call_method("manage", &ud)?;
            }
            Value::Nil => return Err(err(
                "grpc.mock(ctx, opts): pass the test or fixture context (`t` / `ctx`) so the \
                     server is torn down with the scope",
            )),
            other => {
                return Err(err(format!(
                    "grpc.mock(ctx, opts): expected the test or fixture context, got a {}",
                    other.type_name()
                )))
            }
        }
        Ok(ud)
    })
}

/// Compile the schema, stand up reflection, bind, and spawn the accept loop. Everything that can
/// fail does so here, synchronously — a bad `.proto` is an error at the `grpc.mock(…)` call site
/// with the compiler's own diagnostic, not a mystery `Unimplemented` at the first call.
fn start(lua: &Lua, opts: &Table) -> mlua::Result<GrpcMock> {
    let (pool, fds_bytes) = compile_schema(opts)?;
    let allow_handler_errors = opts
        .get::<Option<bool>>("allow_handler_errors")?
        .unwrap_or(false);
    // `network` — opt into a host-gateway vantage, binding all interfaces. Same contract as
    // http.mock: true → host.docker.internal, a string overrides the host name.
    let network_host = parse_network(opts)?;
    let state: Shared = Rc::new(RefCell::new(MockState {
        allow_handler_errors,
        ..Default::default()
    }));
    serve(lua, pool, fds_bytes, state, network_host)
}

fn parse_network(opts: &Table) -> mlua::Result<Option<String>> {
    Ok(match opts.get::<Option<Value>>("network")? {
        Some(Value::Boolean(true)) => Some("host.docker.internal".to_string()),
        Some(Value::String(name)) => Some(name.to_string_lossy().to_string()),
        Some(Value::Boolean(false)) | None | Some(Value::Nil) => None,
        Some(other) => {
            return Err(err(format!(
                "grpc.mock: `network` must be true or a host name, got a {}",
                other.type_name()
            )))
        }
    })
}

/// Stand up reflection, bind, and spawn the accept loop over a prepared (pool, fds, state) —
/// shared by `grpc.mock` (Lua stubs) and `grpc.proxy` (native forward/replay in the state).
fn serve(
    lua: &Lua,
    pool: DescriptorPool,
    fds_bytes: Vec<u8>,
    state: Shared,
    network_host: Option<String>,
) -> mlua::Result<GrpcMock> {
    let reflect_v1 = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(&fds_bytes)
        .build_v1()
        .map_err(|e| err(format!("grpc.mock: building reflection service: {e}")))?;
    let reflect_v1alpha = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(&fds_bytes)
        .build_v1alpha()
        .map_err(|e| {
            err(format!(
                "grpc.mock: building v1alpha reflection service: {e}"
            ))
        })?;

    let bind_ip = if network_host.is_some() {
        "0.0.0.0"
    } else {
        "127.0.0.1"
    };
    let std_listener = std::net::TcpListener::bind((bind_ip, 0))
        .map_err(|e| err(format!("grpc.mock: bind: {e}")))?;
    std_listener
        .set_nonblocking(true)
        .map_err(|e| err(format!("grpc.mock: set_nonblocking: {e}")))?;
    let port = std_listener
        .local_addr()
        .map_err(|e| err(format!("grpc.mock: local_addr: {e}")))?
        .port();
    let listener = tokio::net::TcpListener::from_std(std_listener)
        .map_err(|e| err(format!("grpc.mock: from_std: {e}")))?;

    let (tx, mut rx) = tokio::sync::oneshot::channel::<()>();

    let accept_state = state.clone();
    let accept_lua = lua.clone();
    tokio::task::spawn_local(async move {
        loop {
            tokio::select! {
                _ = &mut rx => break,
                accepted = listener.accept() => {
                    let Ok((stream, _peer)) = accepted else { break };
                    let io = hyper_util::rt::TokioIo::new(stream);
                    let conn_state = accept_state.clone();
                    let conn_lua = accept_lua.clone();
                    let conn_pool = pool.clone();
                    let r1 = reflect_v1.clone();
                    let r1a = reflect_v1alpha.clone();
                    tokio::task::spawn_local(async move {
                        let svc = hyper::service::service_fn(move |req: hyper::Request<hyper::body::Incoming>| {
                            let state = conn_state.clone();
                            let lua = conn_lua.clone();
                            let pool = conn_pool.clone();
                            let mut r1 = r1.clone();
                            let mut r1a = r1a.clone();
                            async move {
                                let path = req.uri().path().to_string();
                                // Reflection is served by the real crate; it never touches Lua,
                                // so its Send future sits happily inside this !Send one.
                                let resp = if path.starts_with("/grpc.reflection.v1.ServerReflection/") {
                                    r1.call(req).await.unwrap_or_else(|e| match e {})
                                } else if path.starts_with("/grpc.reflection.v1alpha.ServerReflection/") {
                                    r1a.call(req).await.unwrap_or_else(|e| match e {})
                                } else {
                                    dispatch(lua, state, pool, &path, req).await
                                };
                                Ok::<_, std::convert::Infallible>(resp)
                            }
                        });
                        // gRPC is HTTP/2 with prior knowledge (no TLS, no upgrade) — exactly what
                        // http2::Builder::serve_connection does. LocalExec is what keeps the
                        // per-stream tasks off `tokio::spawn` and thus off the Send requirement.
                        let _ = hyper::server::conn::http2::Builder::new(LocalExec)
                            .serve_connection(io, svc)
                            .await;
                    });
                }
            }
        }
    });

    Ok(GrpcMock {
        url: format!("http://127.0.0.1:{port}"),
        host: "127.0.0.1".to_string(),
        port,
        network_host,
        state,
        shutdown: RefCell::new(Some(tx)),
    })
}

/// `proto` → a descriptor pool + the encoded set reflection serves. Includes default to each
/// file's own directory, which is what makes the common single-file case need no `includes` at
/// all; declare them explicitly the moment an import crosses a directory.
fn compile_schema(opts: &Table) -> mlua::Result<(DescriptorPool, Vec<u8>)> {
    let protos: Vec<String> = match opts.get::<Option<Value>>("proto")? {
        Some(Value::String(s)) => vec![s.to_string_lossy().to_string()],
        Some(Value::Table(t)) => {
            let mut v = Vec::new();
            for p in t.sequence_values::<String>() {
                v.push(p?);
            }
            v
        }
        Some(other) => {
            return Err(err(format!(
                "grpc.mock: `proto` must be a path or a list of paths, got a {}",
                other.type_name()
            )))
        }
        None => return Err(err(
            "grpc.mock: pass `proto = \"path/to/service.proto\"` — a mock must be told the \
                 schema it serves",
        )),
    };
    if protos.is_empty() {
        return Err(err("grpc.mock: `proto` is empty"));
    }

    let mut includes: Vec<String> = Vec::new();
    if let Some(t) = opts.get::<Option<Table>>("includes")? {
        for p in t.sequence_values::<String>() {
            includes.push(p?);
        }
    }
    if includes.is_empty() {
        for p in &protos {
            if let Some(parent) = std::path::Path::new(p).parent() {
                let d = parent.to_string_lossy().to_string();
                if !d.is_empty() && !includes.contains(&d) {
                    includes.push(d);
                }
            }
        }
    }

    let fds = protox::compile(&protos, &includes).map_err(|e| {
        // protox's own diagnostic names the file, line, and column. Surface it verbatim rather
        // than flattening it into "bad proto".
        err(format!("grpc.mock: compiling {protos:?}: {e}"))
    })?;
    let bytes = prost::Message::encode_to_vec(&fds);
    // Decode from bytes rather than converting types: it keeps this independent of whether
    // protox and prost-reflect happen to agree on a prost-types version.
    let pool = DescriptorPool::decode(bytes.as_slice())
        .map_err(|e| err(format!("grpc.mock: building descriptor pool: {e}")))?;
    Ok((pool, bytes))
}

/// Route one non-reflection request to the dynamic unary handler.
async fn dispatch(
    lua: Lua,
    state: Shared,
    pool: DescriptorPool,
    path: &str,
    req: hyper::Request<hyper::body::Incoming>,
) -> hyper::Response<tonic::body::Body> {
    // "/pkg.Service/Method" → "pkg.Service/Method"
    let full = path.trim_start_matches('/').to_string();
    let Some(desc) = lookup_method(&pool, &full) else {
        // A method the *schema* doesn't define. Distinct from one it defines that nobody
        // stubbed: this is "your test is wrong", that is "add a stub".
        return status_only(Status::new(
            Code::Unimplemented,
            format!("grpc.mock: no method {full:?} in the schema it was given"),
        ));
    };
    let codec = DynCodec {
        decode_into: desc.input(), // a server decodes the request
    };
    let mut grpc = tonic::server::Grpc::new(codec);
    let svc = DynService {
        lua,
        state,
        method: full,
        output: desc.output(),
    };
    grpc.unary(svc, req).await
}

fn lookup_method(pool: &DescriptorPool, full: &str) -> Option<MethodDescriptor> {
    let (service, method) = full.split_once('/')?;
    pool.get_service_by_name(service)?
        .methods()
        .find(|m| m.name() == method)
}

fn status_only(status: Status) -> hyper::Response<tonic::body::Body> {
    status.into_http()
}

/// The bridge from tonic's server machinery to Lua. Its `Future` is deliberately a plain
/// (non-`Send`) boxed future — the property that makes this whole facet possible.
struct DynService {
    lua: Lua,
    state: Shared,
    method: String,
    output: prost_reflect::MessageDescriptor,
}

impl tonic::server::UnaryService<DynamicMessage> for DynService {
    type Response = DynamicMessage;
    type Future =
        Pin<Box<dyn Future<Output = Result<TonicResponse<DynamicMessage>, Status>> + 'static>>;

    fn call(&mut self, request: TonicRequest<DynamicMessage>) -> Self::Future {
        let lua = self.lua.clone();
        let state = self.state.clone();
        let method = self.method.clone();
        let output = self.output.clone();
        Box::pin(async move { answer(lua, state, method, output, request.into_inner()).await })
    }
}

async fn answer(
    lua: Lua,
    state: Shared,
    method: String,
    output: prost_reflect::MessageDescriptor,
    request: DynamicMessage,
) -> Result<TonicResponse<DynamicMessage>, Status> {
    let req_json = message_to_json(&request)
        .map_err(|e| Status::internal(format!("grpc.mock: decoding request: {e}")))?;

    // A grpc.proxy replaces the stub path with native forward (record) or cassette (replay).
    if state.borrow().proxy.is_some() {
        // The fault dial, consulted before any answer (cohesion, mirrors the socket/http proxy).
        let (latency, dropped) = {
            let s = state.borrow();
            (s.faults.latency, s.faults.dropped)
        };
        if dropped {
            return Err(Status::unavailable("grpc.proxy: dropped (fault injection)"));
        }
        if let Some(l) = latency {
            tokio::time::sleep(l).await;
        }
        return proxy_answer(&state, &method, &output, request, &req_json).await;
    }

    let matched_idx = match find_match(&lua, &state, &method) {
        Ok(i) => i,
        Err(e) => {
            record(
                &state,
                &method,
                &req_json,
                "Internal",
                false,
                Some(e.to_string()),
            );
            return Err(Status::internal(format!("grpc.mock: matching failed: {e}")));
        }
    };
    // Clone the reply out before awaiting into Lua: a handler may re-enter this same RefCell.
    let reply = matched_idx.map(|i| state.borrow().stubs[i].reply.clone());

    let (spec, error) = match reply {
        None => (
            ReplySpec {
                code: Code::Unimplemented,
                message: format!("grpc.mock: no stub for {method:?}"),
                response: None,
                delay: None,
            },
            None,
        ),
        Some(Reply::Unset) => (
            ReplySpec {
                code: Code::Internal,
                message: format!("grpc.mock: stub for {method:?} has no :reply(…)"),
                response: None,
                delay: None,
            },
            Some(format!("stub for {method:?} has no :reply(…)")),
        ),
        Some(Reply::Data(d)) => (d, None),
        Some(Reply::Handler(f)) => run_handler(&lua, f, &method, &req_json).await,
    };

    if let Some(d) = spec.delay {
        tokio::time::sleep(d).await;
    }

    if spec.code != Code::Ok {
        record(
            &state,
            &method,
            &req_json,
            &format!("{:?}", spec.code),
            matched_idx.is_some(),
            error,
        );
        return Err(Status::new(spec.code, spec.message));
    }

    let json = spec
        .response
        .unwrap_or(serde_json::Value::Object(Default::default()));
    let msg = DynamicMessage::deserialize_with_options(output, &json, &deserialize_opts())
        .map_err(|e| {
            let m = format!("grpc.mock: building the reply for {method:?}: {e}");
            record(
                &state,
                &method,
                &req_json,
                "Internal",
                true,
                Some(m.clone()),
            );
            Status::internal(m)
        })?;
    record(
        &state,
        &method,
        &req_json,
        "Ok",
        matched_idx.is_some(),
        error,
    );
    Ok(TonicResponse::new(msg))
}

async fn run_handler(
    lua: &Lua,
    f: Function,
    method: &str,
    req_json: &serde_json::Value,
) -> (ReplySpec, Option<String>) {
    let internal = |m: String| {
        (
            ReplySpec {
                code: Code::Internal,
                message: m.clone(),
                response: None,
                delay: None,
            },
            Some(m),
        )
    };
    let tbl = match req_to_lua(lua, method, req_json) {
        Ok(t) => t,
        Err(e) => return internal(format!("grpc.mock: handler input: {e}")),
    };
    match f.call_async::<Value>(tbl).await {
        Ok(Value::Table(t)) => match parse_reply(lua, &t) {
            Ok(s) => (s, None),
            Err(e) => internal(format!("grpc.mock: handler reply: {e}")),
        },
        Ok(other) => internal(format!(
            "grpc.mock: handler must return a reply table, returned a {}",
            other.type_name()
        )),
        Err(e) => internal(format!("grpc.mock: handler raised: {e}")),
    }
}

fn record(
    state: &Shared,
    method: &str,
    request: &serde_json::Value,
    code: &str,
    matched: bool,
    error: Option<String>,
) {
    // Only a *stub's* failure counts as a handler error. A mock that deliberately answers
    // `NotFound` is doing its job; a handler that raised is our bug.
    if let Some(e) = &error {
        state.borrow_mut().handler_errors.push(e.clone());
    }
    state.borrow_mut().journal.push(Recorded {
        method: method.to_string(),
        request: request.clone(),
        code: code.to_string(),
        matched,
        source: if matched { "stub" } else { "unmatched" },
        error,
    });
}

fn find_match(lua: &Lua, state: &Shared, method: &str) -> mlua::Result<Option<usize>> {
    let candidates: Vec<(usize, Option<String>)> = {
        let s = state.borrow();
        s.stubs
            .iter()
            .enumerate()
            .filter(|(_, stub)| stub.method.as_ref().is_none_or(|m| m == method))
            .map(|(i, stub)| (i, stub.method_matches.clone()))
            .collect()
    };
    for (i, pat) in candidates {
        match pat {
            None => return Ok(Some(i)),
            Some(p) => {
                let string: Table = lua.globals().get("string")?;
                let matcher: Function = string.get("match")?;
                let r: Value = matcher.call((method.to_string(), p))?;
                if !matches!(r, Value::Nil) {
                    return Ok(Some(i));
                }
            }
        }
    }
    Ok(None)
}

fn message_to_json(msg: &DynamicMessage) -> Result<serde_json::Value, String> {
    let mut ser = serde_json::Serializer::new(Vec::new());
    msg.serialize_with_options(&mut ser, &serialize_opts())
        .map_err(|e| e.to_string())?;
    serde_json::from_slice(&ser.into_inner()).map_err(|e| e.to_string())
}

fn req_to_lua(lua: &Lua, method: &str, req_json: &serde_json::Value) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set("method", method.to_string())?;
    t.set("request", lua.to_value(req_json)?)?;
    Ok(t)
}

fn recorded_to_lua(lua: &Lua, r: &Recorded, seq: usize) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set("seq", seq)?; // §6: monotonic per mock, 1-based
    t.set("method", r.method.clone())?;
    t.set("request", lua.to_value(&r.request)?)?;
    t.set("code", r.code.clone())?;
    t.set("matched", r.matched)?;
    t.set("source", r.source)?;
    if let Some(e) = &r.error {
        t.set("error", e.clone())?;
    }
    Ok(t)
}

/// Parse `code` the way the client *prints* it (`format!("{:?}", status.code())` → `NotFound`),
/// so what a failure reports is what you write to reproduce it. Accepted case-insensitively;
/// an unknown name is rejected at the call site with the valid set, never silently downgraded to
/// `Unknown` — a status that quietly became the wrong status is a test that lies.
fn parse_code(name: &str) -> mlua::Result<Code> {
    const NAMES: &[(&str, Code)] = &[
        ("ok", Code::Ok),
        ("cancelled", Code::Cancelled),
        ("unknown", Code::Unknown),
        ("invalidargument", Code::InvalidArgument),
        ("deadlineexceeded", Code::DeadlineExceeded),
        ("notfound", Code::NotFound),
        ("alreadyexists", Code::AlreadyExists),
        ("permissiondenied", Code::PermissionDenied),
        ("resourceexhausted", Code::ResourceExhausted),
        ("failedprecondition", Code::FailedPrecondition),
        ("aborted", Code::Aborted),
        ("outofrange", Code::OutOfRange),
        ("unimplemented", Code::Unimplemented),
        ("internal", Code::Internal),
        ("unavailable", Code::Unavailable),
        ("dataloss", Code::DataLoss),
        ("unauthenticated", Code::Unauthenticated),
    ];
    let key = name.replace(['_', '-'], "").to_ascii_lowercase();
    NAMES
        .iter()
        .find(|(n, _)| *n == key)
        .map(|(_, c)| *c)
        .ok_or_else(|| {
            err(format!(
                "grpc.mock: unknown status code {name:?}. Valid: Ok, Cancelled, Unknown, \
                 InvalidArgument, DeadlineExceeded, NotFound, AlreadyExists, PermissionDenied, \
                 ResourceExhausted, FailedPrecondition, Aborted, OutOfRange, Unimplemented, \
                 Internal, Unavailable, DataLoss, Unauthenticated"
            ))
        })
}

fn parse_reply(lua: &Lua, t: &Table) -> mlua::Result<ReplySpec> {
    let code = match t.get::<Option<String>>("code")? {
        Some(name) => parse_code(&name)?,
        None => Code::Ok,
    };
    let message = t.get::<Option<String>>("message")?.unwrap_or_default();
    let response = match t.get::<Option<Value>>("response")?.filter(|v| !v.is_nil()) {
        Some(v) => Some(lua.from_value::<serde_json::Value>(v)?),
        None => None,
    };
    if code != Code::Ok && response.is_some() {
        return Err(err(
            "grpc.mock reply: has both a non-Ok `code` and a `response` — an RPC answers with a \
             message or a status, not both",
        ));
    }
    let delay = match t.get::<Option<String>>("delay")? {
        Some(s) => Some(
            parse_duration(&s)
                .ok_or_else(|| err(format!("grpc.mock reply: bad `delay` duration {s:?}")))?,
        ),
        None => None,
    };
    Ok(ReplySpec {
        code,
        message,
        response,
        delay,
    })
}

impl UserData for GrpcMock {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("url", |_, this| Ok(this.url.clone()));
        fields.add_field_method_get("host", |_, this| Ok(this.host.clone()));
        fields.add_field_method_get("port", |_, this| Ok(this.port));
        // The universal driver-target alias — what grpc.client(...) takes: "host:port".
        fields.add_field_method_get("endpoint", |_, this| {
            Ok(format!("{}:{}", this.host, this.port))
        });
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
        methods.add_method("on", |lua, this, spec: Table| {
            let stub = Stub {
                method: spec.get::<Option<String>>("method")?,
                method_matches: spec.get::<Option<String>>("method_matches")?,
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

        // The §6 filter contract, same as the http facet — see `journal_keep`.
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

        // Raises on a reply-handler error, exactly as the http facet does — see there for why
        // this rides `ctx:manage` teardown rather than inventing a reporting path. A proxy in
        // record mode flushes its cassette here — the close/scope-exit flush point.
        methods.add_method("stop", |_, this, ()| grpc_stop(this));
        methods.add_method("close", |_, this, ()| grpc_stop(this));

        // The fault vocabulary (cohesion) — meaningful on a grpc.proxy; latency/drop/after are
        // transport-generic. corrupt/throttle are byte-level and stay socket-only by design.
        methods.add_method("latency", |_, this, d: String| {
            let dur = parse_duration(&d)
                .ok_or_else(|| err(format!("grpc.proxy latency: bad duration {d:?}")))?;
            this.state.borrow_mut().faults.latency = Some(dur);
            Ok(())
        });
        methods.add_method("drop", |_, this, ()| {
            this.state.borrow_mut().faults.dropped = true;
            Ok(())
        });
        methods.add_method("after", |lua, this, d: String| {
            let delay = parse_duration(&d)
                .ok_or_else(|| err(format!("grpc.proxy after: bad duration {d:?}")))?;
            lua.create_userdata(GrpcFuse {
                delay,
                state: this.state.clone(),
            })
        });
    }
}

/// A scheduled grpc fault (`p:after("100ms"):drop()`), mirroring the socket/http proxy fuses.
struct GrpcFuse {
    delay: Duration,
    state: Shared,
}

impl UserData for GrpcFuse {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("drop", |_, this, ()| {
            let delay = this.delay;
            let state = this.state.clone();
            tokio::task::spawn_local(async move {
                tokio::time::sleep(delay).await;
                state.borrow_mut().faults.dropped = true;
            });
            Ok(())
        });
        methods.add_method("latency", |_, this, d: String| {
            let dur = parse_duration(&d)
                .ok_or_else(|| err(format!("grpc.proxy latency: bad duration {d:?}")))?;
            let delay = this.delay;
            let state = this.state.clone();
            tokio::task::spawn_local(async move {
                tokio::time::sleep(delay).await;
                state.borrow_mut().faults.latency = Some(dur);
            });
            Ok(())
        });
    }
}

fn grpc_stop(this: &GrpcMock) -> mlua::Result<()> {
    if let Some(tx) = this.shutdown.borrow_mut().take() {
        let _ = tx.send(());
        flush_grpc_cassette(&this.state)?;
    }
    let errs = {
        let mut s = this.state.borrow_mut();
        if s.allow_handler_errors {
            s.handler_errors.clear();
            Vec::new()
        } else {
            std::mem::take(&mut s.handler_errors)
        }
    };
    if !errs.is_empty() {
        return Err(super::mock::handler_error_report("grpc.mock", &errs));
    }
    Ok(())
}

/// Write a record-mode proxy's cassette: the schema learned from the upstream (self-describing)
/// plus the captured turns.
fn flush_grpc_cassette(state: &Shared) -> mlua::Result<()> {
    use base64::Engine;
    let s = state.borrow();
    let Some(Proxy::Record {
        turns,
        cassette,
        fds_bytes,
        redact,
        ..
    }) = &s.proxy
    else {
        return Ok(());
    };
    if cassette.is_empty() {
        return Ok(()); // a passthrough proxy: forward, record nothing
    }
    let cas = GrpcCassette {
        version: 1,
        kind: "grpc".to_string(),
        fds: base64::engine::general_purpose::STANDARD.encode(fds_bytes),
        turns: turns.clone(),
    };
    let text = serde_json::to_string_pretty(&cas)
        .map_err(|e| err(format!("grpc.proxy: encoding cassette: {e}")))?;
    // Cross-transport redaction floor: scrub literal secrets before the file hits disk.
    let text = super::cassette::scrub(text, redact);
    std::fs::write(cassette, text)
        .map_err(|e| err(format!("grpc.proxy: writing cassette {cassette:?}: {e}")))
}

/// The proxy's answer path — record forwards to the upstream and captures the pair; replay
/// answers from the cassette (a miss is a loud `Unavailable`). Nothing here touches Lua, so the
/// borrow discipline is simpler than the stub path's.
async fn proxy_answer(
    state: &Shared,
    method: &str,
    output: &MessageDescriptor,
    request: DynamicMessage,
    req_json: &serde_json::Value,
) -> Result<TonicResponse<DynamicMessage>, Status> {
    // Record: pull the channel + pool out (clones), forward, capture. The RefCell is not held
    // across the await — the forward can be slow and must not block the mock.
    let record_ctx = {
        let s = state.borrow();
        match &s.proxy {
            Some(Proxy::Record { channel, pool, .. }) => Some((channel.clone(), pool.clone())),
            _ => None,
        }
    };
    if let Some((channel, pool)) = record_ctx {
        return match super::grpc::invoke(&channel, &pool, method, request, None).await {
            Ok(resp) => {
                let resp_json = message_to_json(&resp)
                    .map_err(|e| Status::internal(format!("grpc.proxy: encoding reply: {e}")))?;
                push_turn(state, method, req_json.clone(), Some(resp_json), "Ok");
                record(state, method, req_json, "Ok", true, None);
                Ok(TonicResponse::new(resp))
            }
            Err(status) => {
                let code = format!("{:?}", status.code());
                push_turn(state, method, req_json.clone(), None, &code);
                record(state, method, req_json, &code, true, None);
                Err(status)
            }
        };
    }

    // Replay: match method + request against the cassette, consume-once.
    let hit = {
        let mut s = state.borrow_mut();
        if let Some(Proxy::Replay { turns, consumed }) = &mut s.proxy {
            let mut found = None;
            for i in 0..turns.len() {
                if !consumed[i] && turns[i].method == method && &turns[i].request == req_json {
                    consumed[i] = true;
                    found = Some(turns[i].clone());
                    break;
                }
            }
            found
        } else {
            None
        }
    };
    match hit {
        Some(turn) => {
            let code = parse_code(&turn.code)
                .unwrap_or(Code::Internal);
            if code != Code::Ok {
                record(state, method, req_json, &turn.code, true, None);
                return Err(Status::new(code, "grpc.proxy: replayed error"));
            }
            let resp_json = turn.response.unwrap_or(serde_json::Value::Null);
            let msg = json_to_message(output, &resp_json).map_err(|e| {
                Status::internal(format!("grpc.proxy: rebuilding reply from cassette: {e}"))
            })?;
            record(state, method, req_json, "Ok", true, None);
            Ok(TonicResponse::new(msg))
        }
        None => {
            // A miss is the recording infrastructure failing the call, not the service — the
            // Unavailable analog of http's 502, naming the cassette so it can be re-recorded.
            record(state, method, req_json, "Unavailable", false, None);
            Err(Status::unavailable(format!(
                "grpc.proxy: cassette has no recorded call for {method:?} with this request — \
                 re-record if the system under test legitimately changed"
            )))
        }
    }
}

fn push_turn(
    state: &Shared,
    method: &str,
    request: serde_json::Value,
    response: Option<serde_json::Value>,
    code: &str,
) {
    let mut s = state.borrow_mut();
    if let Some(Proxy::Record { turns, .. }) = &mut s.proxy {
        turns.push(GrpcTurn {
            method: method.to_string(),
            request,
            response,
            code: code.to_string(),
        });
    }
}

/// Reverse of `message_to_json`: build a `DynamicMessage` of `desc` from recorded JSON.
fn json_to_message(
    desc: &MessageDescriptor,
    json: &serde_json::Value,
) -> Result<DynamicMessage, String> {
    let text = json.to_string();
    let mut de = serde_json::Deserializer::from_str(&text);
    DynamicMessage::deserialize_with_options(desc.clone(), &mut de, &deserialize_opts())
        .map_err(|e| e.to_string())
}

pub(crate) fn proxy_fn(lua: &Lua) -> mlua::Result<Function> {
    // Async, unlike grpc.mock: a record proxy must dial the upstream and reflect its schema,
    // which is exactly the async work grpc.client does at construction.
    lua.create_async_function(|lua, (ctx, opts): (Value, Table)| async move {
        let server = start_proxy(&lua, &opts).await?;
        let ud = lua.create_userdata(server)?;
        match ctx {
            Value::UserData(c) => {
                let _: Value = c.call_method("manage", &ud)?;
            }
            _ => {
                return Err(err(
                    "grpc.proxy(ctx, opts): pass the test or fixture context (`t` / `ctx`)",
                ))
            }
        }
        Ok(ud)
    })
}

/// Build a proxy: record fetches the upstream schema by reflection and captures pairs; replay
/// serves the cassette's stored schema and answers from its turns. `auto` picks by the
/// cassette's presence, exactly as http.proxy does.
async fn start_proxy(lua: &Lua, opts: &Table) -> mlua::Result<GrpcMock> {
    let upstream = opts.get::<Option<String>>("upstream")?;
    let cassette = opts.get::<Option<String>>("cassette")?;
    let mode_str = opts
        .get::<Option<String>>("mode")?
        .unwrap_or_else(|| "passthrough".to_string());
    let network_host = parse_network(opts)?;

    let mode = match mode_str.as_str() {
        "passthrough" | "record" => "record", // passthrough forwards; it just doesn't flush
        "replay" => "replay",
        "auto" => {
            let cas = cassette
                .as_ref()
                .ok_or_else(|| err("grpc.proxy: mode \"auto\" needs a `cassette`"))?;
            if std::path::Path::new(cas).exists() {
                "replay"
            } else {
                "record"
            }
        }
        other => {
            return Err(err(format!(
                "grpc.proxy: mode must be passthrough|record|replay|auto, got {other:?}"
            )))
        }
    };
    let recording = mode == "record" && mode_str != "passthrough";
    if recording && cassette.is_none() {
        return Err(err(format!("grpc.proxy: mode {mode_str:?} needs a `cassette`")));
    }

    let (pool, fds_bytes, proxy) = if mode == "replay" {
        use base64::Engine;
        let path = cassette
            .clone()
            .ok_or_else(|| err("grpc.proxy: replay needs a `cassette`"))?;
        let text = std::fs::read_to_string(&path)
            .map_err(|e| err(format!("grpc.proxy: reading cassette {path:?}: {e}")))?;
        let cas: GrpcCassette = serde_json::from_str(&text)
            .map_err(|e| err(format!("grpc.proxy: parsing cassette {path:?}: {e}")))?;
        let fds_bytes = base64::engine::general_purpose::STANDARD
            .decode(&cas.fds)
            .map_err(|e| err(format!("grpc.proxy: decoding cassette schema: {e}")))?;
        let pool = super::grpc::pool_from_fds_bytes(&fds_bytes)?;
        let n = cas.turns.len();
        (
            pool,
            fds_bytes,
            Proxy::Replay {
                turns: cas.turns,
                consumed: vec![false; n],
            },
        )
    } else {
        let up = upstream
            .clone()
            .ok_or_else(|| err(format!("grpc.proxy: mode {mode_str:?} needs `upstream`")))?;
        // Dial + reflect, exactly as grpc.client does at construction.
        let channel = super::grpc::connect_channel(&up).await?;
        let pool = super::grpc::build_pool(&channel).await?;
        let fds_bytes = super::grpc::pool_to_fds_bytes(&pool);
        (
            pool.clone(),
            fds_bytes.clone(),
            Proxy::Record {
                channel,
                pool,
                turns: Vec::new(),
                cassette: cassette.clone().unwrap_or_default(),
                fds_bytes,
                redact: opts.get::<Option<Vec<String>>>("redact")?.unwrap_or_default(),
            },
        )
    };

    // A passthrough proxy forwards but never flushes — model it as Record with no cassette by
    // leaving `cassette` empty (flush is a no-op on an empty path check below).
    let proxy = if !recording {
        match proxy {
            Proxy::Record {
                channel,
                pool,
                turns,
                fds_bytes,
                redact,
                ..
            } => Proxy::Record {
                channel,
                pool,
                turns,
                cassette: String::new(),
                fds_bytes,
                redact,
            },
            other => other,
        }
    } else {
        proxy
    };

    let state: Shared = Rc::new(RefCell::new(MockState {
        proxy: Some(proxy),
        ..Default::default()
    }));
    serve(lua, pool, fds_bytes, state, network_host)
}

impl UserData for StubHandle {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("reply", |lua, this, v: Value| {
            let reply = match v {
                Value::Function(f) => Reply::Handler(f),
                Value::Table(t) => Reply::Data(parse_reply(lua, &t)?),
                other => {
                    let msg = format!(
                        "grpc.mock :reply expects a reply table or a handler function, got a {}",
                        other.type_name()
                    );
                    return Err(err(msg));
                }
            };
            this.state.borrow_mut().stubs[this.idx].reply = reply;
            Ok(())
        });
    }
}
