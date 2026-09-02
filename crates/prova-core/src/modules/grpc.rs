use std::collections::HashMap;
use std::time::{Duration, Instant};

use mlua::{Lua, LuaSerdeExt, Table, UserData, UserDataMethods, Value};
use prost::Message as _;
use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor, SerializeOptions};
use prost_types::FileDescriptorProto;
use tonic::codec::{Codec, DecodeBuf, Decoder, EncodeBuf, Encoder};
use tonic::codegen::http::uri::PathAndQuery;
pub(super) use tonic::transport::Channel;
use tonic::{Request, Status};


fn err(msg: impl Into<String>) -> mlua::Error {
    mlua::Error::RuntimeError(msg.into())
}

pub(crate) fn make(lua: &Lua) -> mlua::Result<Table> {
    let grpc = lua.create_table()?;
    // grpc.client(addr, { timeout = "30s" }) → a Client (reflection is performed here, once).
    grpc.set(
        "client",
        lua.create_async_function(|lua, (addr, opts): (String, Option<Table>)| async move {
            super::runtime_only("grpc.client")?;
            let timeout = opt_duration(&opts, "timeout")?;
            let (tls, tls_requested) = tls_opts(&opts, "grpc.client")?;
            let channel = connect_channel_tls(&addr, &tls, tls_requested).await?;
            let pool = build_pool(&channel).await?;
            lua.create_userdata(Client {
                channel,
                pool,
                timeout,
            })
        })?,
    )?;
    // grpc.wait_for(addr, { timeout = "30s", every = "500ms" }) — poll until the server answers a
    // reflection ListServices (boot-then-probe, mirroring http.wait_for).
    grpc.set(
        "wait_for",
        lua.create_async_function(|_, (addr, opts): (String, Option<Table>)| async move {
            super::runtime_only("grpc.wait_for")?;
            let timeout = opt_duration(&opts, "timeout")?.unwrap_or(Duration::from_secs(30));
            let every = opt_duration(&opts, "every")?.unwrap_or(Duration::from_millis(500));
            // Parsed BEFORE the loop: a contradictory address/policy must fail immediately, not
            // after burning the whole readiness budget retrying something that cannot work.
            let (tls, tls_requested) = tls_opts(&opts, "grpc.wait_for")?;
            let deadline = Instant::now() + timeout;
            loop {
                if let Ok(channel) = connect_channel_tls(&addr, &tls, tls_requested).await {
                    if list_services(&channel).await.is_ok() {
                        return Ok(());
                    }
                }
                if Instant::now() >= deadline {
                    return Err(err(format!(
                        "grpc.wait_for timed out after {timeout:?} waiting for {addr}"
                    )));
                }
                tokio::time::sleep(every).await;
            }
        })?,
    )?;
    // grpc.mock(ctx, { proto = … }) → the `mock` facet on the grpc namespace. Unlike `client`, it
    // must be told its schema: reflection teaches a client about a server, and a mock *is* the
    // server, so there is nobody to learn from.
    #[cfg(feature = "grpc-mock")]
    grpc.set("mock", super::grpc_mock::mock_fn(lua)?)?;
    #[cfg(feature = "grpc-mock")]
    grpc.set("proxy", super::grpc_mock::proxy_fn(lua)?)?;
    Ok(grpc)
}

/// A connected client bound to one server. `client:call(method, req)` returns the response as a
/// table; `client:call_status(method, req)` returns `{ ok, code, message, response }` so a test
/// can assert on gRPC status codes (e.g. `NotFound`, `InvalidArgument`) without raising.
struct Client {
    channel: Channel,
    pool: DescriptorPool,
    timeout: Option<Duration>,
}

impl UserData for Client {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_async_method(
            "call",
            |lua, this, (method, req): (String, Option<Value>)| async move {
                let input = build_request(&lua, &this.pool, &method, req)?;
                match invoke(&this.channel, &this.pool, &method, input, this.timeout).await {
                    Ok(msg) => response_to_lua(&lua, &msg),
                    Err(status) => Err(err(format!(
                        "grpc call {method} failed: {} ({})",
                        status.message(),
                        status.code()
                    ))),
                }
            },
        );
        methods.add_async_method(
            "call_status",
            |lua, this, (method, req): (String, Option<Value>)| async move {
                let input = build_request(&lua, &this.pool, &method, req)?;
                let out = lua.create_table()?;
                match invoke(&this.channel, &this.pool, &method, input, this.timeout).await {
                    Ok(msg) => {
                        out.set("ok", true)?;
                        out.set("code", "Ok")?;
                        out.set("message", "")?;
                        out.set("response", response_to_lua(&lua, &msg)?)?;
                    }
                    Err(status) => {
                        out.set("ok", false)?;
                        out.set("code", format!("{:?}", status.code()))?;
                        out.set("message", status.message().to_string())?;
                        out.set("response", Value::Nil)?;
                    }
                }
                Ok(out)
            },
        );
    }
}

pub(super) async fn connect_channel(addr: &str) -> mlua::Result<Channel> {
    connect_channel_tls(addr, &super::tls::Tls::default(), false).await
}

/// Connect, deciding plaintext against TLS from the address and the policy
/// (docs/design/architecture.md#tls-everywhere).
///
/// **Three spellings, one meaning, no ambiguity.** `https://host:port` says TLS in the address;
/// `tls = true` promotes a bare `host:port` (the only way to ask for *verified* TLS without
/// writing a scheme); and `insecure`/`ca_cert` imply it, because neither option means anything
/// over plaintext and making an author write `tls = true` beside `insecure = true` would be
/// ceremony. What is NOT allowed is a contradiction — `http://` with a TLS option — since the
/// alternative is picking a winner the author cannot predict, and the loser here is silent.
pub(super) async fn connect_channel_tls(
    addr: &str,
    tls: &super::tls::Tls,
    tls_requested: bool,
) -> mlua::Result<Channel> {
    let wants_tls = tls_requested || !tls.is_default() || addr.starts_with("https://");
    if wants_tls && addr.starts_with("http://") {
        return Err(err(format!(
            "grpc: {addr:?} is an http:// address but TLS was asked for — drop the TLS options for              plaintext, or name the endpoint as https://"
        )));
    }
    let uri = if addr.contains("://") {
        addr.to_string()
    } else if wants_tls {
        format!("https://{addr}")
    } else {
        format!("http://{addr}")
    };
    let endpoint = Channel::from_shared(uri)
        .map_err(|e| err(format!("grpc: invalid address {addr:?}: {e}")))?;
    let endpoint = if wants_tls {
        #[cfg(feature = "tls")]
        {
            tls.apply_tonic(endpoint, "grpc")?
        }
        #[cfg(not(feature = "tls"))]
        return Err(super::tls::unavailable("grpc", "a TLS endpoint"));
    } else {
        endpoint
    };
    endpoint
        .connect()
        .await
        .map_err(|e| err(format!("grpc: could not connect to {addr}: {e}")))
}

/// Serialize a pool back to the encoded `FileDescriptorSet` bytes reflection serves — how a
/// `grpc.proxy` in record mode captures the schema it learned from the upstream into a
/// self-describing cassette (proofs/spec/cassettes/grpc). The bytes round-trip through the same
/// tonic-reflection builder the mock's protox bytes do, so the prost-types versions agree.
pub(super) fn pool_to_fds_bytes(pool: &DescriptorPool) -> Vec<u8> {
    let set = prost_types::FileDescriptorSet {
        file: pool.file_descriptor_protos().cloned().collect(),
    };
    set.encode_to_vec()
}

/// Build a pool from encoded `FileDescriptorSet` bytes (a cassette's stored schema) — the
/// inverse of `pool_to_fds_bytes`, for a `grpc.proxy` replaying with no upstream to reflect.
pub(super) fn pool_from_fds_bytes(bytes: &[u8]) -> mlua::Result<DescriptorPool> {
    let set = prost_types::FileDescriptorSet::decode(bytes)
        .map_err(|e| err(format!("grpc: decoding cassette schema: {e}")))?;
    let mut pool = DescriptorPool::new();
    pool.add_file_descriptor_protos(set.file)
        .map_err(|e| err(format!("grpc: building pool from cassette: {e}")))?;
    Ok(pool)
}

/// Turn a Lua request table into a wire-ready `DynamicMessage` for `method`'s input type.
fn build_request(
    lua: &Lua,
    pool: &DescriptorPool,
    method: &str,
    req: Option<Value>,
) -> mlua::Result<DynamicMessage> {
    let desc = method_descriptor(pool, method)?;
    let json: serde_json::Value = match req {
        Some(v) => lua.from_value(v)?,
        None => serde_json::Value::Object(Default::default()),
    };
    DynamicMessage::deserialize(desc.input(), &json)
        .map_err(|e| err(format!("grpc: building request for {method}: {e}")))
}

/// Serialize a response message to a Lua table. `skip_default_fields(false)` keeps zero/empty
/// fields present so assertions can see the full message shape. Field names mirror how requests
/// are written — proto (snake_case) names, not proto3-JSON camelCase — and 64-bit ints arrive
/// as Lua numbers rather than strings (tests assert `res.id`, not `res.id == "3"`; Lua numbers
/// are exact through 2^53, far beyond any test-scale id).
fn response_to_lua(lua: &Lua, msg: &DynamicMessage) -> mlua::Result<Value> {
    let opts = SerializeOptions::new()
        .skip_default_fields(false)
        .use_proto_field_name(true)
        .stringify_64_bit_integers(false);
    let value = msg
        .serialize_with_options(serde_json::value::Serializer, &opts)
        .map_err(|e| err(format!("grpc: decoding response: {e}")))?;
    lua.to_value(&value)
}

fn method_descriptor(
    pool: &DescriptorPool,
    method: &str,
) -> mlua::Result<prost_reflect::MethodDescriptor> {
    // Accept "pkg.Service/Method" or "/pkg.Service/Method".
    let trimmed = method.trim_start_matches('/');
    let (service, method_name) = trimmed.rsplit_once('/').ok_or_else(|| {
        err(format!(
            "grpc: method must be \"package.Service/Method\", got {method:?}"
        ))
    })?;
    let svc = pool.get_service_by_name(service).ok_or_else(|| {
        err(format!(
            "grpc: service {service:?} not found via reflection (known: {})",
            pool.services()
                .map(|s| s.full_name().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ))
    })?;
    let method = svc.methods().find(|m| m.name() == method_name);
    method.ok_or_else(|| {
        err(format!(
            "grpc: method {method_name:?} not found on {service}"
        ))
    })
}

pub(super) async fn invoke(
    channel: &Channel,
    pool: &DescriptorPool,
    method: &str,
    input: DynamicMessage,
    timeout: Option<Duration>,
) -> Result<DynamicMessage, Status> {
    let desc = method_descriptor(pool, method).map_err(|e| Status::internal(e.to_string()))?;
    let path: PathAndQuery = format!("/{}/{}", desc.parent_service().full_name(), desc.name())
        .parse()
        .map_err(|e| Status::internal(format!("grpc: bad method path: {e}")))?;
    let mut grpc = tonic::client::Grpc::new(channel.clone());
    grpc.ready()
        .await
        .map_err(|e| Status::unavailable(format!("grpc: service not ready: {e}")))?;
    let codec = DynCodec {
        decode_into: desc.output(), // a client decodes the reply
    };
    let mut request = Request::new(input);
    if let Some(t) = timeout {
        request.set_timeout(t);
    }
    let resp = grpc.unary(request, path, codec).await?;
    Ok(resp.into_inner())
}

// A tonic codec that speaks `DynamicMessage` on both ends: the encoder just prost-encodes
// whatever message it is handed; the decoder builds an empty message of a known descriptor and
// merges the incoming bytes into it. This is the whole trick that lets one client call any
// method dynamically.
//
// It is direction-agnostic, which is why `grpc.mock` shares it rather than owning a mirror copy:
// the only thing that differs between the two ends is *what to decode into* — a client decodes
// the method's output (the reply), a server decodes its input (the request). Hence
// `decode_into` rather than `output`: naming it for the reply was naming it for one caller.
#[derive(Clone)]
pub(super) struct DynCodec {
    pub(super) decode_into: MessageDescriptor,
}

impl Codec for DynCodec {
    type Encode = DynamicMessage;
    type Decode = DynamicMessage;
    type Encoder = DynEncoder;
    type Decoder = DynDecoder;
    fn encoder(&mut self) -> DynEncoder {
        DynEncoder
    }
    fn decoder(&mut self) -> DynDecoder {
        DynDecoder {
            decode_into: self.decode_into.clone(),
        }
    }
}

pub(super) struct DynEncoder;
impl Encoder for DynEncoder {
    type Item = DynamicMessage;
    type Error = Status;
    fn encode(&mut self, item: DynamicMessage, dst: &mut EncodeBuf<'_>) -> Result<(), Status> {
        item.encode(dst)
            .map_err(|e| Status::internal(format!("grpc: encoding message: {e}")))
    }
}

pub(super) struct DynDecoder {
    decode_into: MessageDescriptor,
}
impl Decoder for DynDecoder {
    type Item = DynamicMessage;
    type Error = Status;
    fn decode(&mut self, src: &mut DecodeBuf<'_>) -> Result<Option<DynamicMessage>, Status> {
        let mut msg = DynamicMessage::new(self.decode_into.clone());
        msg.merge(src)
            .map_err(|e| Status::internal(format!("grpc: decoding message: {e}")))?;
        Ok(Some(msg))
    }
}

// -- reflection ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Rv {
    V1,
    V1alpha,
}

/// Build a descriptor pool for every service the server advertises, via reflection. Negotiates
/// the reflection protocol version (v1, falling back to the older v1alpha many servers still use).
///
/// Some servers (tonic among them) answer `file_containing_symbol` with ONLY the named file,
/// not its transitive imports — grpcurl chases the missing imports with `file_by_filename`
/// follow-ups, and so do we. Files are then added to the pool in dependency order, since a
/// dependent added before its import fails pool construction.
pub(super) async fn build_pool(channel: &Channel) -> mlua::Result<DescriptorPool> {
    let (services, rv) = list_services_negotiated(channel).await?;
    let mut files: HashMap<String, FileDescriptorProto> = HashMap::new();
    let decode_into = |raw: Vec<Vec<u8>>,
                           files: &mut HashMap<String, FileDescriptorProto>|
     -> mlua::Result<()> {
        for bytes in raw {
            let fdp = FileDescriptorProto::decode(bytes.as_slice())
                .map_err(|e| err(format!("grpc: decoding file descriptor: {e}")))?;
            let name = fdp.name().to_string();
            files.entry(name).or_insert(fdp);
        }
        Ok(())
    };
    for service in &services {
        // The reflection service describes itself; skip it — we only want the app's schema.
        if service.starts_with("grpc.reflection.") {
            continue;
        }
        let raw = files_for_symbol(channel, rv, service).await.map_err(|e| {
            err(format!(
                "grpc: reflecting {service}: {} ({})",
                e.message(),
                e.code()
            ))
        })?;
        decode_into(raw, &mut files)?;
    }

    // Chase missing imports until the set is closed (bounded — a real
    // schema's import graph is shallow; 32 rounds is generous).
    for _ in 0..32 {
        let missing: Vec<String> = files
            .values()
            .flat_map(|f| f.dependency.iter().cloned())
            .filter(|dep| !files.contains_key(dep))
            .collect();
        if missing.is_empty() {
            break;
        }
        for dep in missing {
            let raw = files_for_filename(channel, rv, &dep).await.map_err(|e| {
                err(format!(
                    "grpc: fetching imported file {dep}: {} ({})",
                    e.message(),
                    e.code()
                ))
            })?;
            decode_into(raw, &mut files)?;
        }
    }

    // Dependency order: imports before importers. A dependency we never
    // obtained doesn't block ordering — the pool reports the precise
    // failure. A stalled round (cycle) appends the rest as-is, same
    // reasoning.
    let held: std::collections::HashSet<String> = files.keys().cloned().collect();
    let mut ordered: Vec<FileDescriptorProto> = Vec::with_capacity(files.len());
    let mut placed: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut remaining: Vec<FileDescriptorProto> = files.into_values().collect();
    while !remaining.is_empty() {
        let before = remaining.len();
        let (ready, blocked): (Vec<_>, Vec<_>) = remaining.into_iter().partition(|f| {
            f.dependency
                .iter()
                .all(|d| placed.contains(d) || !held.contains(d))
        });
        for f in &ready {
            placed.insert(f.name().to_string());
        }
        ordered.extend(ready);
        remaining = blocked;
        if remaining.len() == before {
            ordered.append(&mut remaining);
        }
    }

    let mut pool = DescriptorPool::new();
    pool.add_file_descriptor_protos(ordered)
        .map_err(|e| err(format!("grpc: building descriptor pool: {e}")))?;
    Ok(pool)
}

/// Try to list services over v1; if the server hasn't implemented v1 reflection, retry v1alpha.
async fn list_services_negotiated(channel: &Channel) -> mlua::Result<(Vec<String>, Rv)> {
    match list_services_v1(channel).await {
        Ok(s) => Ok((s, Rv::V1)),
        Err(status) if status.code() == tonic::Code::Unimplemented => {
            let s = list_services_v1alpha(channel).await.map_err(|e| {
                err(format!(
                    "grpc: server reflection (v1alpha) failed: {} ({})",
                    e.message(),
                    e.code()
                ))
            })?;
            Ok((s, Rv::V1alpha))
        }
        Err(status) => Err(err(format!(
            "grpc: server reflection failed ({}). The server must enable gRPC reflection for \
             prova's dynamic client. {}",
            status.code(),
            status.message()
        ))),
    }
}

/// Version-agnostic `list_services` used by `wait_for` (v1, then v1alpha).
async fn list_services(channel: &Channel) -> mlua::Result<Vec<String>> {
    list_services_negotiated(channel).await.map(|(s, _)| s)
}

async fn files_for_symbol(
    channel: &Channel,
    rv: Rv,
    symbol: &str,
) -> Result<Vec<Vec<u8>>, Status> {
    match rv {
        Rv::V1 => files_for_symbol_v1(channel, symbol).await,
        Rv::V1alpha => files_for_symbol_v1alpha(channel, symbol).await,
    }
}

async fn files_for_filename(
    channel: &Channel,
    rv: Rv,
    filename: &str,
) -> Result<Vec<Vec<u8>>, Status> {
    match rv {
        Rv::V1 => files_for_filename_v1(channel, filename).await,
        Rv::V1alpha => files_for_filename_v1alpha(channel, filename).await,
    }
}

// The two reflection protocol versions have structurally identical messages under different
// module paths; this macro generates the list/file-fetch pair for each so the orchestration above
// stays version-agnostic.
macro_rules! reflection_ops {
    ($modpath:ident, $list_fn:ident, $files_fn:ident, $byname_fn:ident, $drain_fn:ident) => {
        async fn $list_fn(channel: &Channel) -> Result<Vec<String>, Status> {
            use tonic_reflection::pb::$modpath::{
                server_reflection_client::ServerReflectionClient,
                server_reflection_request::MessageRequest,
                server_reflection_response::MessageResponse, ServerReflectionRequest,
            };
            let mut client = ServerReflectionClient::new(channel.clone());
            let req = ServerReflectionRequest {
                host: String::new(),
                message_request: Some(MessageRequest::ListServices(String::new())),
            };
            let stream = futures::stream::iter(std::iter::once(req));
            let mut inner = client.server_reflection_info(stream).await?.into_inner();
            let mut out = Vec::new();
            while let Some(resp) = inner.message().await? {
                match resp.message_response {
                    Some(MessageResponse::ListServicesResponse(list)) => {
                        out.extend(list.service.into_iter().map(|s| s.name));
                    }
                    Some(MessageResponse::ErrorResponse(e)) => {
                        return Err(Status::new(
                            tonic::Code::from(e.error_code),
                            e.error_message,
                        ));
                    }
                    _ => {}
                }
            }
            Ok(out)
        }

        async fn $byname_fn(channel: &Channel, filename: &str) -> Result<Vec<Vec<u8>>, Status> {
            use tonic_reflection::pb::$modpath::server_reflection_request::MessageRequest;
            $drain_fn(channel, MessageRequest::FileByFilename(filename.to_string())).await
        }

        async fn $files_fn(channel: &Channel, symbol: &str) -> Result<Vec<Vec<u8>>, Status> {
            use tonic_reflection::pb::$modpath::server_reflection_request::MessageRequest;
            $drain_fn(channel, MessageRequest::FileContainingSymbol(symbol.to_string())).await
        }

        // The shared drain: one request in, every FileDescriptorResponse's protos out. The two
        // file-fetch requests differ only in their MessageRequest variant.
        async fn $drain_fn(
            channel: &Channel,
            message_request: tonic_reflection::pb::$modpath::server_reflection_request::MessageRequest,
        ) -> Result<Vec<Vec<u8>>, Status> {
            use tonic_reflection::pb::$modpath::{
                server_reflection_client::ServerReflectionClient,
                server_reflection_response::MessageResponse, ServerReflectionRequest,
            };
            let mut client = ServerReflectionClient::new(channel.clone());
            let req = ServerReflectionRequest {
                host: String::new(),
                message_request: Some(message_request),
            };
            let stream = futures::stream::iter(std::iter::once(req));
            let mut inner = client.server_reflection_info(stream).await?.into_inner();
            let mut out = Vec::new();
            while let Some(resp) = inner.message().await? {
                match resp.message_response {
                    Some(MessageResponse::FileDescriptorResponse(fdr)) => {
                        out.extend(fdr.file_descriptor_proto);
                    }
                    Some(MessageResponse::ErrorResponse(e)) => {
                        return Err(Status::new(
                            tonic::Code::from(e.error_code),
                            e.error_message,
                        ));
                    }
                    _ => {}
                }
            }
            Ok(out)
        }
    };
}

reflection_ops!(v1, list_services_v1, files_for_symbol_v1, files_for_filename_v1, drain_fds_v1);
reflection_ops!(v1alpha, list_services_v1alpha, files_for_symbol_v1alpha, files_for_filename_v1alpha, drain_fds_v1alpha);

/// The TLS policy plus whether `tls = true` was written, which a bare `host:port` needs in order
/// to ask for verified TLS (there is no scheme to carry it).
///
/// `tls = false` is honored as "plaintext, stated" rather than ignored: an author who writes it
/// beside `insecure = true` has contradicted themselves, and that is worth a message.
fn tls_opts(opts: &Option<Table>, who: &str) -> mlua::Result<(super::tls::Tls, bool)> {
    let Some(t) = opts else {
        return Ok((super::tls::Tls::default(), false));
    };
    let tls = super::tls::Tls::from_opts(t, who)?;
    match t.get::<Option<bool>>("tls")? {
        Some(false) if !tls.is_default() => Err(err(format!(
            "{who}: `tls = false` contradicts the TLS options beside it — drop one"
        ))),
        Some(requested) => Ok((tls, requested)),
        None => Ok((tls, false)),
    }
}

fn opt_duration(opts: &Option<Table>, key: &str) -> mlua::Result<Option<Duration>> {
    match opts {
        Some(t) => match t.get::<Option<String>>(key)? {
            Some(s) => crate::model::require_duration("grpc", key, &s).map(Some).map_err(mlua::Error::RuntimeError),
            None => Ok(None),
        },
        None => Ok(None),
    }
}
