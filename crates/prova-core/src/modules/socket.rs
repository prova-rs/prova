//! The `socket` kernel transport — low-level byte streams with the full Mock/Proxy/Driver triad
//! (docs/design/mocks-proxies-drivers.md, proofs/spec/socket + proofs/spec/faults).
//!
//! One namespace, unified by ADDRESS SCHEME: `tcp://host:port` and `unix:///path` are just
//! addresses — listen, connect, proxy, and the byte model are identical; only address parsing
//! differs. A raw byte stream has no natural "request" unit, so mocks and transcripts take a
//! FRAMING strategy (`"line"`, `{ length_prefixed = n }`, `{ delimiter = "…" }`) that turns bytes
//! into matchable turns. The byte-level proxy is the universal wiretap: put it in front of
//! anything TCP and you get direction-tagged transcripts plus the fault vocabulary
//! (`latency`/`drop`/`corrupt`/`throttle`/`after`) with zero protocol knowledge —
//! toxiproxy-in-process, no extra daemon.
//!
//! Everything here is `spawn_local`'d single-thread alongside the Lua state, exactly like
//! `http.mock` — see that module for why a `LocalSet` exists.

use std::cell::RefCell;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};
use std::time::Duration;

use mlua::{Function, Lua, Table, UserData, UserDataFields, UserDataMethods, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};

use crate::model::parse_duration;

const DEFAULT_IO_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) fn make(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set("listen", listen_fn(lua)?)?;
    t.set("connect", connect_fn(lua)?)?;
    t.set("mock", mock_fn(lua)?)?;
    t.set("proxy", proxy_fn(lua)?)?;
    Ok(t)
}

fn err(msg: impl Into<String>) -> mlua::Error {
    mlua::Error::RuntimeError(msg.into())
}

// ── addressing ─────────────────────────────────────────────────────────────────────────────────

/// A scheme-unified address. The scheme *is* the platform capability: `unix://` exists only where
/// `cfg(unix)` does, and the spec suite carries `requires = { "unix" }` on those legs.
enum Addr {
    Tcp(String),
    #[cfg(unix)]
    Unix(std::path::PathBuf),
}

fn parse_addr(s: &str) -> mlua::Result<Addr> {
    if let Some(hostport) = s.strip_prefix("tcp://") {
        return Ok(Addr::Tcp(hostport.to_string()));
    }
    if let Some(path) = s.strip_prefix("unix://") {
        #[cfg(unix)]
        return Ok(Addr::Unix(std::path::PathBuf::from(path)));
        #[cfg(not(unix))]
        return Err(err(format!(
            "socket: unix:// addresses need a unix platform (got {s})"
        )));
    }
    Err(err(format!(
        "socket: address must be tcp://host:port or unix:///path, got {s:?}"
    )))
}

// ── streams (tcp | unix behind one type) ───────────────────────────────────────────────────────

enum Stream {
    Tcp(tokio::net::TcpStream),
    #[cfg(unix)]
    Unix(tokio::net::UnixStream),
}

impl AsyncRead for Stream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Stream::Tcp(s) => Pin::new(s).poll_read(cx, buf),
            #[cfg(unix)]
            Stream::Unix(s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Stream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match self.get_mut() {
            Stream::Tcp(s) => Pin::new(s).poll_write(cx, data),
            #[cfg(unix)]
            Stream::Unix(s) => Pin::new(s).poll_write(cx, data),
        }
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Stream::Tcp(s) => Pin::new(s).poll_flush(cx),
            #[cfg(unix)]
            Stream::Unix(s) => Pin::new(s).poll_flush(cx),
        }
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Stream::Tcp(s) => Pin::new(s).poll_shutdown(cx),
            #[cfg(unix)]
            Stream::Unix(s) => Pin::new(s).poll_shutdown(cx),
        }
    }
}

async fn dial(addr: &str) -> mlua::Result<Stream> {
    match parse_addr(addr)? {
        Addr::Tcp(hp) => Ok(Stream::Tcp(
            tokio::net::TcpStream::connect(&hp)
                .await
                .map_err(|e| err(format!("socket.connect {addr}: {e}")))?,
        )),
        #[cfg(unix)]
        Addr::Unix(p) => Ok(Stream::Unix(
            tokio::net::UnixStream::connect(&p)
                .await
                .map_err(|e| err(format!("socket.connect {addr}: {e}")))?,
        )),
    }
}

// ── listeners ──────────────────────────────────────────────────────────────────────────────────

enum Acceptor {
    Tcp(tokio::net::TcpListener),
    #[cfg(unix)]
    Unix(tokio::net::UnixListener, std::path::PathBuf),
}

impl Acceptor {
    /// Bind synchronously so the resolved address is known — and accepting — before we return.
    /// `tcp://host:0` resolves the ephemeral port into the advertised `.addr`.
    fn bind(addr: &Addr) -> mlua::Result<(Self, String)> {
        match addr {
            Addr::Tcp(hp) => {
                let std_l = std::net::TcpListener::bind(hp)
                    .map_err(|e| err(format!("socket: bind tcp://{hp}: {e}")))?;
                std_l
                    .set_nonblocking(true)
                    .map_err(|e| err(format!("socket: set_nonblocking: {e}")))?;
                let local = std_l
                    .local_addr()
                    .map_err(|e| err(format!("socket: local_addr: {e}")))?;
                let l = tokio::net::TcpListener::from_std(std_l)
                    .map_err(|e| err(format!("socket: from_std: {e}")))?;
                Ok((Acceptor::Tcp(l), format!("tcp://{local}")))
            }
            #[cfg(unix)]
            Addr::Unix(p) => {
                let l = tokio::net::UnixListener::bind(p)
                    .map_err(|e| err(format!("socket: bind unix://{}: {e}", p.display())))?;
                Ok((
                    Acceptor::Unix(l, p.clone()),
                    format!("unix://{}", p.display()),
                ))
            }
        }
    }

    async fn accept(&self) -> std::io::Result<Stream> {
        match self {
            Acceptor::Tcp(l) => l.accept().await.map(|(s, _)| Stream::Tcp(s)),
            #[cfg(unix)]
            Acceptor::Unix(l, _) => l.accept().await.map(|(s, _)| Stream::Unix(s)),
        }
    }
}

impl Drop for Acceptor {
    fn drop(&mut self) {
        // A unix socket leaves its path behind; reap it so a re-bind in the same tempdir works.
        #[cfg(unix)]
        if let Acceptor::Unix(_, p) = self {
            let _ = std::fs::remove_file(p);
        }
    }
}

// ── framing ────────────────────────────────────────────────────────────────────────────────────

/// What turns bytes into matchable TURNS. `Raw` is the absence of framing: `send` writes bytes
/// verbatim and `recv(n)` reads exact counts — the driver-level escape hatch.
#[derive(Clone, Debug)]
enum Framing {
    Raw,
    Line,
    LengthPrefixed(usize),
    Delimiter(Vec<u8>),
}

impl Framing {
    fn parse(v: Option<Value>) -> mlua::Result<Framing> {
        match v {
            None | Some(Value::Nil) => Ok(Framing::Raw),
            Some(Value::String(s)) => match s.to_string_lossy().as_ref() {
                "line" => Ok(Framing::Line),
                other => Err(err(format!(
                    "socket: unknown framing {other:?} (a string framing is \"line\"; tables are \
                     {{ length_prefixed = n }} or {{ delimiter = \"…\" }})"
                ))),
            },
            Some(Value::Table(t)) => {
                let lp = t.get::<Option<usize>>("length_prefixed")?;
                let delim = t.get::<Option<mlua::String>>("delimiter")?;
                match (lp, delim) {
                    (Some(n), None) if (1..=8).contains(&n) => Ok(Framing::LengthPrefixed(n)),
                    (Some(n), None) => Err(err(format!(
                        "socket: length_prefixed must be 1..=8 bytes, got {n}"
                    ))),
                    (None, Some(d)) if !d.as_bytes().is_empty() => {
                        Ok(Framing::Delimiter(d.as_bytes().to_vec()))
                    }
                    (None, Some(_)) => Err(err("socket: delimiter must be non-empty")),
                    _ => Err(err(
                        "socket: framing table is { length_prefixed = n } OR { delimiter = \"…\" }",
                    )),
                }
            }
            Some(other) => Err(err(format!(
                "socket: framing must be a string or table, got a {}",
                other.type_name()
            ))),
        }
    }

    fn is_raw(&self) -> bool {
        matches!(self, Framing::Raw)
    }

    /// Wrap one payload into its on-wire form.
    fn encode(&self, payload: &[u8]) -> Vec<u8> {
        match self {
            Framing::Raw => payload.to_vec(),
            Framing::Line => {
                let mut v = payload.to_vec();
                v.push(b'\n');
                v
            }
            Framing::LengthPrefixed(n) => {
                let mut v = Vec::with_capacity(n + payload.len());
                let len = payload.len() as u64;
                for i in (0..*n).rev() {
                    v.push(((len >> (8 * i)) & 0xff) as u8);
                }
                v.extend_from_slice(payload);
                v
            }
            Framing::Delimiter(d) => {
                let mut v = payload.to_vec();
                v.extend_from_slice(d);
                v
            }
        }
    }
}

/// Read one frame from `stream`, consuming `buf` leftovers first. `Ok(None)` is clean EOF.
async fn read_frame(
    stream: &mut Stream,
    buf: &mut Vec<u8>,
    framing: &Framing,
) -> std::io::Result<Option<Vec<u8>>> {
    let needle: &[u8] = match framing {
        Framing::Line => b"\n",
        Framing::Delimiter(d) => d,
        Framing::LengthPrefixed(_) => &[],
        Framing::Raw => {
            return Err(std::io::Error::other(
                "read_frame called without framing (internal)",
            ))
        }
    };
    loop {
        if let Framing::LengthPrefixed(n) = framing {
            if buf.len() >= *n {
                let mut len: u64 = 0;
                for b in buf.iter().take(*n) {
                    len = (len << 8) | *b as u64;
                }
                let total = *n + len as usize;
                if buf.len() >= total {
                    let payload = buf[*n..total].to_vec();
                    buf.drain(..total);
                    return Ok(Some(payload));
                }
            }
        } else if let Some(pos) = buf.windows(needle.len()).position(|w| w == needle) {
            let payload = buf[..pos].to_vec();
            buf.drain(..pos + needle.len());
            return Ok(Some(payload));
        }
        let mut chunk = [0u8; 16 * 1024];
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Ok(None); // EOF; any partial frame in `buf` never completed
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

/// Read exactly `want` bytes (raw mode), consuming leftovers first.
async fn read_exact_buffered(
    stream: &mut Stream,
    buf: &mut Vec<u8>,
    want: usize,
) -> std::io::Result<Vec<u8>> {
    while buf.len() < want {
        let mut chunk = [0u8; 16 * 1024];
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!("connection closed with {}/{want} bytes read", buf.len()),
            ));
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let out = buf[..want].to_vec();
    buf.drain(..want);
    Ok(out)
}

// ── the driver: Conn (originate) ───────────────────────────────────────────────────────────────

/// One live connection. The stream is `take()`n out for the duration of an I/O call so a
/// concurrent call errors ("busy") instead of panicking a `RefCell` across an await.
struct Conn {
    stream: Rc<RefCell<Option<Stream>>>,
    buf: Rc<RefCell<Vec<u8>>>,
    framing: Framing,
}

impl Conn {
    fn new(stream: Stream, framing: Framing) -> Conn {
        Conn {
            stream: Rc::new(RefCell::new(Some(stream))),
            buf: Rc::new(RefCell::new(Vec::new())),
            framing,
        }
    }
}

fn recv_args(framing: &Framing, a: Option<Value>, b: Option<Table>) -> mlua::Result<(usize, Duration)> {
    // raw: recv(n, opts?) — framed: recv(opts?)
    let (want, opts) = match (framing.is_raw(), a) {
        (true, Some(Value::Integer(n))) if n > 0 => (n as usize, b),
        (true, other) => {
            return Err(err(format!(
                "recv(n): without framing you read exact byte counts — pass n (got {})",
                other.map(|v| v.type_name().to_string()).unwrap_or_else(|| "nothing".into())
            )))
        }
        (false, Some(Value::Table(t))) => (0, Some(t)),
        (false, None | Some(Value::Nil)) => (0, b),
        (false, Some(other)) => {
            return Err(err(format!(
                "recv(opts?): framed connections read whole turns — no byte count (got a {})",
                other.type_name()
            )))
        }
    };
    let timeout = match &opts {
        Some(t) => match t.get::<Option<String>>("timeout")? {
            Some(s) => parse_duration(&s).ok_or_else(|| err(format!("bad duration {s:?}")))?,
            None => DEFAULT_IO_TIMEOUT,
        },
        None => DEFAULT_IO_TIMEOUT,
    };
    Ok((want, timeout))
}

impl UserData for Conn {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_async_method("send", |_, this, data: mlua::String| async move {
            let wire = this.framing.encode(&data.as_bytes());
            let Some(mut s) = this.stream.borrow_mut().take() else {
                return Err(err("send: connection is closed or busy"));
            };
            let r = s.write_all(&wire).await;
            *this.stream.borrow_mut() = Some(s);
            r.map_err(|e| err(format!("send: {e}")))
        });

        methods.add_async_method(
            "recv",
            |lua, this, (a, b): (Option<Value>, Option<Table>)| async move {
                let (want, dur) = recv_args(&this.framing, a, b)?;
                let Some(mut s) = this.stream.borrow_mut().take() else {
                    return Err(err("recv: connection is closed or busy"));
                };
                let mut buf = std::mem::take(&mut *this.buf.borrow_mut());
                let res = tokio::time::timeout(dur, async {
                    if this.framing.is_raw() {
                        read_exact_buffered(&mut s, &mut buf, want).await.map(Some)
                    } else {
                        read_frame(&mut s, &mut buf, &this.framing).await
                    }
                })
                .await;
                *this.buf.borrow_mut() = buf;
                *this.stream.borrow_mut() = Some(s);
                match res {
                    Err(_) => Err(err(format!("recv: timed out after {dur:?}"))),
                    Ok(Err(e)) => Err(err(format!("recv: {e}"))),
                    Ok(Ok(None)) => Err(err("recv: connection closed")),
                    Ok(Ok(Some(payload))) => Ok(lua.create_string(&payload)?),
                }
            },
        );

        methods.add_method("close", |_, this, ()| {
            this.stream.borrow_mut().take();
            Ok(())
        });
    }
}

fn connect_fn(lua: &Lua) -> mlua::Result<Function> {
    lua.create_async_function(|_, (addr, opts): (String, Option<Table>)| async move {
        super::runtime_only("socket.connect")?;
        let framing = Framing::parse(match &opts {
            Some(t) => t.get::<Option<Value>>("framing")?,
            None => None,
        })?;
        Ok(Conn::new(dial(&addr).await?, framing))
    })
}

// ── the driver: Listener (originate, server side) ─────────────────────────────────────────────

struct ListenerUd {
    addr: String,
    acceptor: Rc<RefCell<Option<Acceptor>>>,
    framing: Framing,
}

impl UserData for ListenerUd {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("addr", |_, this| Ok(this.addr.clone()));
        fields.add_field_method_get("endpoint", |_, this| Ok(this.addr.clone()));
    }
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_async_method("accept", |_, this, opts: Option<Table>| async move {
            let dur = match &opts {
                Some(t) => match t.get::<Option<String>>("timeout")? {
                    Some(s) => parse_duration(&s).ok_or_else(|| err(format!("bad duration {s:?}")))?,
                    None => DEFAULT_IO_TIMEOUT,
                },
                None => DEFAULT_IO_TIMEOUT,
            };
            let Some(acc) = this.acceptor.borrow_mut().take() else {
                return Err(err("accept: listener is closed or busy"));
            };
            let res = tokio::time::timeout(dur, acc.accept()).await;
            *this.acceptor.borrow_mut() = Some(acc);
            match res {
                Err(_) => Err(err(format!("accept: timed out after {dur:?}"))),
                Ok(Err(e)) => Err(err(format!("accept: {e}"))),
                Ok(Ok(stream)) => Ok(Conn::new(stream, this.framing.clone())),
            }
        });
        methods.add_method("stop", |_, this, ()| {
            this.acceptor.borrow_mut().take();
            Ok(())
        });
    }
}

fn listen_fn(lua: &Lua) -> mlua::Result<Function> {
    lua.create_function(|lua, (ctx, opts): (Value, Table)| {
        let addr_s = opts
            .get::<Option<String>>("addr")?
            .unwrap_or_else(|| "tcp://127.0.0.1:0".to_string());
        let framing = Framing::parse(opts.get::<Option<Value>>("framing")?)?;
        let (acceptor, addr) = Acceptor::bind(&parse_addr(&addr_s)?)?;
        let ud = lua.create_userdata(ListenerUd {
            addr,
            acceptor: Rc::new(RefCell::new(Some(acceptor))),
            framing,
        })?;
        super::manage("socket.listen", &ctx, &ud)?;
        Ok(ud)
    })
}

// ── the mock: terminate ────────────────────────────────────────────────────────────────────────

#[derive(Default)]
struct MockState {
    stubs: Vec<(Vec<u8>, Option<Vec<u8>>)>,
    journal: Vec<super::wiretap::JournalRow>,
}

struct MockUd {
    addr: String,
    state: Rc<RefCell<MockState>>,
    shutdown: RefCell<Option<tokio::sync::oneshot::Sender<()>>>,
}

struct MockStub {
    state: Rc<RefCell<MockState>>,
    idx: usize,
}

impl UserData for MockStub {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("reply", |_, this, data: mlua::String| {
            this.state.borrow_mut().stubs[this.idx].1 = Some(data.as_bytes().to_vec());
            Ok(())
        });
    }
}

impl UserData for MockUd {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("addr", |_, this| Ok(this.addr.clone()));
        fields.add_field_method_get("endpoint", |_, this| Ok(this.addr.clone()));
    }
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("on", |lua, this, turn: mlua::String| {
            let idx = {
                let mut s = this.state.borrow_mut();
                s.stubs.push((turn.as_bytes().to_vec(), None));
                s.stubs.len() - 1
            };
            lua.create_userdata(MockStub {
                state: this.state.clone(),
                idx,
            })
        });

        super::wiretap::add_received_method(methods);
        super::wiretap::add_shutdown_methods(methods);
    }
}

super::wiretap::impl_journal!(MockUd);
super::wiretap::impl_shutdown!(MockUd);

fn mock_fn(lua: &Lua) -> mlua::Result<Function> {
    lua.create_function(|lua, (ctx, opts): (Value, Option<Table>)| {
        let opts = opts.ok_or_else(|| {
            err("socket.mock(ctx, { framing = … }): framing is required — matching needs turns")
        })?;
        let framing = Framing::parse(opts.get::<Option<Value>>("framing")?)?;
        if framing.is_raw() {
            return Err(err(
                "socket.mock: framing is required — a mock matches TURNS, and raw bytes have no \
                 turn boundary (use socket.listen/accept to script raw exchanges)",
            ));
        }
        let addr_s = opts
            .get::<Option<String>>("addr")?
            .unwrap_or_else(|| "tcp://127.0.0.1:0".to_string());
        let (acceptor, addr) = Acceptor::bind(&parse_addr(&addr_s)?)?;
        let state: Rc<RefCell<MockState>> = Rc::default();
        let (tx, mut rx) = tokio::sync::oneshot::channel::<()>();

        let accept_state = state.clone();
        let f = framing.clone();
        tokio::task::spawn_local(async move {
            loop {
                tokio::select! {
                    _ = &mut rx => break,
                    accepted = acceptor.accept() => {
                        let Ok(mut stream) = accepted else { break };
                        let conn_state = accept_state.clone();
                        let framing = f.clone();
                        tokio::task::spawn_local(async move {
                            let mut buf = Vec::new();
                            while let Ok(Some(turn)) = read_frame(&mut stream, &mut buf, &framing).await {
                                let reply = {
                                    let mut s = conn_state.borrow_mut();
                                    match s.stubs.iter().find(|(k, _)| *k == turn) {
                                        Some((_, r)) => {
                                            let r = r.clone();
                                            s.journal.push(super::wiretap::JournalRow {
                                                data: turn,
                                                matched: true,
                                                source: "stub",
                                            });
                                            r
                                        }
                                        None => {
                                            // The §6 rule: an unmatched turn is journaled — it is
                                            // the most interesting thing a mock can record — and
                                            // the connection closes LOUD instead of guessing.
                                            s.journal.push(super::wiretap::JournalRow {
                                                data: turn,
                                                matched: false,
                                                source: "unmatched",
                                            });
                                            break;
                                        }
                                    }
                                };
                                if let Some(r) = reply {
                                    if stream.write_all(&framing.encode(&r)).await.is_err() {
                                        break;
                                    }
                                }
                            }
                        });
                    }
                }
            }
        });

        let ud = lua.create_userdata(MockUd {
            addr,
            state,
            shutdown: RefCell::new(Some(tx)),
        })?;
        super::manage("socket.mock", &ctx, &ud)?;
        Ok(ud)
    })
}

// ── the proxy: interpose (wiretap + faults) ────────────────────────────────────────────────────

/// The fault dial, consulted per turn/chunk by the pump loops. One vocabulary for every stream
/// transport (api-freeze §7): continuous conditions, distinct from the mocks' one-shot `delay`.
#[derive(Default)]
struct Faults {
    latency: Option<Duration>,
    corrupt: bool,
    throttle_bytes_per_sec: Option<f64>,
}

#[derive(Default)]
struct ProxyState {
    transcript: Vec<super::wiretap::TranscriptRow>,
    faults: Faults,
    /// Set in record mode: each (request-turn → response-turn) pair is appended here and flushed
    /// to the cassette on close. Absent otherwise.
    recorder: Option<Rc<super::cassette::Recorder>>,
}

struct ProxyUd {
    addr: String,
    state: Rc<RefCell<ProxyState>>,
    /// `true` = sever every live pump and refuse new conns. A watch so pumps die mid-await.
    dropped: Rc<tokio::sync::watch::Sender<bool>>,
    shutdown: RefCell<Option<tokio::sync::oneshot::Sender<()>>>,
}

/// The cassette posture of a proxy (docs/design/mocks-proxies-drivers.md). `Passthrough` is the
/// plain bidirectional wiretap (the fault/full-duplex path); the other three drive the
/// request/response turn loop, because a cassette IS the VCR discipline.
#[derive(Clone, Debug, PartialEq)]
enum Mode {
    Passthrough,
    Record,
    Replay,
}

/// One interposed connection in a cassette mode: a synchronous request→response turn loop, which
/// is what makes the recording coherent (a bidirectional pump has no turn pairing). Record forwards
/// to the upstream and captures each pair; replay answers from the player and needs no upstream —
/// a miss closes the connection LOUD (the recv on the client side then errors).
async fn turn_loop(
    client: Stream,
    upstream: Option<String>,
    framing: Framing,
    state: Rc<RefCell<ProxyState>>,
    player: Option<Rc<RefCell<super::cassette::Player>>>,
) {
    let mut client = client;
    let mut cbuf = Vec::new();
    // Record mode holds one live upstream connection for the conversation.
    let mut up: Option<(Stream, Vec<u8>)> = match &upstream {
        Some(addr) => match dial(addr).await {
            Ok(s) => Some((s, Vec::new())),
            Err(_) => return,
        },
        None => None,
    };
    loop {
        let req = match read_frame(&mut client, &mut cbuf, &framing).await {
            Ok(Some(r)) => r,
            _ => break, // client EOF ends the conversation
        };
        state.borrow_mut().transcript.push(super::wiretap::TranscriptRow {
            dir: "up",
            data: req.clone(),
        });
        let resp: Option<Vec<u8>> = if let Some((up_stream, ubuf)) = up.as_mut() {
            // Record: forward the request, read one response turn, capture the pair.
            if up_stream.write_all(&framing.encode(&req)).await.is_err() {
                break;
            }
            match read_frame(up_stream, ubuf, &framing).await {
                Ok(Some(r)) => {
                    if let Some(rec) = &state.borrow().recorder {
                        rec.record(
                            super::cassette::encode_bytes(&req),
                            super::cassette::encode_bytes(&r),
                            None,
                        );
                    }
                    Some(r)
                }
                _ => None,
            }
        } else if let Some(p) = &player {
            // Replay: answer from the cassette; a miss is None → break, closing the connection.
            p.borrow_mut()
                .answer(&super::cassette::encode_bytes(&req))
                .map(|turn| super::cassette::decode_bytes(&turn.response))
        } else {
            None
        };
        let Some(resp) = resp else { break };
        state.borrow_mut().transcript.push(super::wiretap::TranscriptRow {
            dir: "down",
            data: resp.clone(),
        });
        if client.write_all(&framing.encode(&resp)).await.is_err() {
            break;
        }
    }
}

fn parse_rate(s: &str) -> mlua::Result<f64> {
    let (num, mult) = if let Some(n) = s.strip_suffix("mbps") {
        (n, 1_000_000.0)
    } else if let Some(n) = s.strip_suffix("kbps") {
        (n, 1_000.0)
    } else if let Some(n) = s.strip_suffix("bps") {
        (n, 1.0)
    } else {
        return Err(err(format!(
            "throttle: rate must end in bps/kbps/mbps, got {s:?}"
        )));
    };
    let v: f64 = num
        .trim()
        .parse()
        .map_err(|_| err(format!("throttle: bad rate {s:?}")))?;
    Ok(v * mult / 8.0) // bits → bytes per second
}

/// Forward one hunk of data with the current faults applied: latency first (continuous, both
/// directions), then corruption (same length, different bytes), then throttled writes.
async fn forward(data: Vec<u8>, faults: (Option<Duration>, bool, Option<f64>), dst: &mut (impl AsyncWrite + Unpin)) -> std::io::Result<()> {
    let (latency, corrupt, throttle) = faults;
    if let Some(l) = latency {
        tokio::time::sleep(l).await;
    }
    let mut data = data;
    if corrupt {
        for b in data.iter_mut() {
            *b ^= 0x55;
        }
    }
    match throttle {
        None => dst.write_all(&data).await,
        Some(bps) => {
            const CHUNK: usize = 512;
            for piece in data.chunks(CHUNK) {
                dst.write_all(piece).await?;
                dst.flush().await?;
                let secs = piece.len() as f64 / bps;
                tokio::time::sleep(Duration::from_secs_f64(secs)).await;
            }
            Ok(())
        }
    }
}

/// One direction of an interposed connection. Framed: turn-by-turn (transcript records turns).
/// Raw: chunk-by-chunk (transcript records chunks). Dies instantly when `dropped` flips.
async fn pump(
    mut src: impl AsyncRead + Unpin,
    mut dst: impl AsyncWrite + Unpin,
    dir: &'static str,
    framing: Framing,
    state: Rc<RefCell<ProxyState>>,
    mut dropped: tokio::sync::watch::Receiver<bool>,
) {
    let mut buf = Vec::new();
    loop {
        let read = async {
            if framing.is_raw() {
                let mut chunk = [0u8; 16 * 1024];
                // A raw stream's `src` is a ReadHalf without our buffered reader; read directly.
                match src.read(&mut chunk).await {
                    Ok(0) => None,
                    Ok(n) => Some(chunk[..n].to_vec()),
                    Err(_) => None,
                }
            } else {
                // Framed reads use the shared frame scanner over a plain read loop.
                loop {
                    let needle_hit = {
                        match &framing {
                            Framing::Line => buf.iter().position(|b| *b == b'\n').map(|p| (p, 1)),
                            Framing::Delimiter(d) => buf
                                .windows(d.len())
                                .position(|w| w == &d[..])
                                .map(|p| (p, d.len())),
                            Framing::LengthPrefixed(n) => {
                                if buf.len() >= *n {
                                    let mut len: u64 = 0;
                                    for b in buf.iter().take(*n) {
                                        len = (len << 8) | *b as u64;
                                    }
                                    let total = *n + len as usize;
                                    if buf.len() >= total {
                                        Some((total, 0))
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                }
                            }
                            Framing::Raw => unreachable!(),
                        }
                    };
                    if let Some((pos, skip)) = needle_hit {
                        let payload: Vec<u8>;
                        if let Framing::LengthPrefixed(n) = &framing {
                            payload = buf[*n..pos].to_vec();
                            buf.drain(..pos);
                        } else {
                            payload = buf[..pos].to_vec();
                            buf.drain(..pos + skip);
                        }
                        return Some(payload);
                    }
                    let mut chunk = [0u8; 16 * 1024];
                    match src.read(&mut chunk).await {
                        Ok(0) | Err(_) => return None,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                }
            }
        };
        let data = tokio::select! {
            _ = dropped.wait_for(|d| *d) => break,
            d = read => match d { Some(d) => d, None => break },
        };
        let faults = {
            let s = state.borrow();
            (
                s.faults.latency,
                s.faults.corrupt,
                s.faults.throttle_bytes_per_sec,
            )
        };
        state.borrow_mut().transcript.push(super::wiretap::TranscriptRow {
            dir,
            data: data.clone(),
        });
        let wire = if framing.is_raw() {
            data
        } else {
            framing.encode(&data)
        };
        let deliver = forward(wire, faults, &mut dst);
        let ok = tokio::select! {
            _ = dropped.wait_for(|d| *d) => false,
            r = deliver => r.is_ok(),
        };
        if !ok {
            break;
        }
    }
}

/// A scheduled fault: `p:after("100ms")` returns this; the verb then arms the fuse.
struct FuseUd {
    delay: Duration,
    state: Rc<RefCell<ProxyState>>,
    dropped: Rc<tokio::sync::watch::Sender<bool>>,
}

impl UserData for FuseUd {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("drop", |_, this, ()| {
            let delay = this.delay;
            let dropped = this.dropped.clone();
            tokio::task::spawn_local(async move {
                tokio::time::sleep(delay).await;
                let _ = dropped.send(true);
            });
            Ok(())
        });
        methods.add_method("latency", |_, this, d: String| {
            let dur = parse_duration(&d).ok_or_else(|| err(format!("bad duration {d:?}")))?;
            let delay = this.delay;
            let state = this.state.clone();
            tokio::task::spawn_local(async move {
                tokio::time::sleep(delay).await;
                state.borrow_mut().faults.latency = Some(dur);
            });
            Ok(())
        });
        methods.add_method("corrupt", |_, this, ()| {
            let delay = this.delay;
            let state = this.state.clone();
            tokio::task::spawn_local(async move {
                tokio::time::sleep(delay).await;
                state.borrow_mut().faults.corrupt = true;
            });
            Ok(())
        });
        methods.add_method("throttle", |_, this, r: String| {
            let bps = parse_rate(&r)?;
            let delay = this.delay;
            let state = this.state.clone();
            tokio::task::spawn_local(async move {
                tokio::time::sleep(delay).await;
                state.borrow_mut().faults.throttle_bytes_per_sec = Some(bps);
            });
            Ok(())
        });
    }
}

super::wiretap::impl_transcript!(ProxyUd);

impl UserData for ProxyUd {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("addr", |_, this| Ok(this.addr.clone()));
        fields.add_field_method_get("endpoint", |_, this| Ok(this.addr.clone()));
    }
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        super::wiretap::add_transcript_method(methods);

        methods.add_method("latency", |_, this, d: String| {
            this.state.borrow_mut().faults.latency = Some(parse_duration(&d).ok_or_else(|| err(format!("bad duration {d:?}")))?);
            Ok(())
        });
        methods.add_method("corrupt", |_, this, ()| {
            this.state.borrow_mut().faults.corrupt = true;
            Ok(())
        });
        methods.add_method("throttle", |_, this, r: String| {
            this.state.borrow_mut().faults.throttle_bytes_per_sec = Some(parse_rate(&r)?);
            Ok(())
        });
        methods.add_method("drop", |_, this, ()| {
            let _ = this.dropped.send(true);
            Ok(())
        });
        methods.add_method("after", |lua, this, d: String| {
            lua.create_userdata(FuseUd {
                delay: parse_duration(&d).ok_or_else(|| err(format!("bad duration {d:?}")))?,
                state: this.state.clone(),
                dropped: this.dropped.clone(),
            })
        });

        // In a record mode, close is the cassette flush point — same as http.proxy.
        methods.add_method("stop", |_, this, ()| {
            let _ = this.dropped.send(true);
            if let Some(tx) = this.shutdown.borrow_mut().take() {
                let _ = tx.send(());
                flush_recorder(&this.state)?;
            }
            Ok(())
        });
        methods.add_method("close", |_, this, ()| {
            let _ = this.dropped.send(true);
            if let Some(tx) = this.shutdown.borrow_mut().take() {
                let _ = tx.send(());
                flush_recorder(&this.state)?;
            }
            Ok(())
        });
    }
}

fn flush_recorder(state: &Rc<RefCell<ProxyState>>) -> mlua::Result<()> {
    let rec = state.borrow().recorder.clone();
    if let Some(rec) = rec {
        rec.flush()
            .map_err(|e| err(format!("socket.proxy: writing cassette: {e}")))?;
    }
    Ok(())
}

/// Resolve `socket.proxy`'s posture from its options: the mode (with `auto` collapsed to
/// record/replay by the cassette's presence, exactly as http.proxy does, so downstream sees only
/// the three real behaviors), the framing/cassette/upstream preconditions enforced at the call
/// site rather than on first connect.
fn proxy_config(opts: &Table) -> mlua::Result<(Option<String>, Framing, Mode, Option<String>)> {
    let upstream = opts.get::<Option<String>>("upstream")?;
    let framing = Framing::parse(opts.get::<Option<Value>>("framing")?)?;
    let cassette = opts.get::<Option<String>>("cassette")?;
    let mode_str = opts
        .get::<Option<String>>("mode")?
        .unwrap_or_else(|| "passthrough".to_string());

    let mode = match mode_str.as_str() {
        "passthrough" => Mode::Passthrough,
        "record" => Mode::Record,
        "replay" => Mode::Replay,
        "auto" => {
            let cas = cassette.as_ref().ok_or_else(|| {
                err("socket.proxy: mode \"auto\" needs a `cassette` — nothing to key on")
            })?;
            if std::path::Path::new(cas).exists() {
                Mode::Replay
            } else {
                Mode::Record
            }
        }
        other => {
            return Err(err(format!(
                "socket.proxy: mode must be passthrough|record|replay|auto, got {other:?}"
            )))
        }
    };

    // Cassettes require framing (matching needs turns) and a cassette path; replay needs no
    // upstream, every other mode does.
    if mode != Mode::Passthrough && framing.is_raw() {
        return Err(err(
            "socket.proxy: a cassette needs framing — a raw byte stream has no turn to key on",
        ));
    }
    let cassette = if mode == Mode::Passthrough {
        None
    } else {
        Some(cassette.ok_or_else(|| {
            err(format!("socket.proxy: mode {mode_str:?} needs a `cassette`"))
        })?)
    };
    if mode != Mode::Replay {
        let up = upstream
            .as_ref()
            .ok_or_else(|| err(format!("socket.proxy: mode {mode_str:?} needs `upstream`")))?;
        parse_addr(up)?; // fail at the call site, not on first connect
    }
    Ok((upstream, framing, mode, cassette))
}

fn proxy_fn(lua: &Lua) -> mlua::Result<Function> {
    lua.create_function(|lua, (ctx, opts): (Value, Table)| {
        let (upstream, framing, mode, cassette) = proxy_config(&opts)?;

        let addr_s = opts
            .get::<Option<String>>("addr")?
            .unwrap_or_else(|| "tcp://127.0.0.1:0".to_string());
        let (acceptor, addr) = Acceptor::bind(&parse_addr(&addr_s)?)?;

        let mut init = ProxyState::default();
        let player = if mode == Mode::Replay {
            let path = cassette
                .as_ref()
                .ok_or_else(|| err("socket.proxy: replay needs a `cassette`"))?;
            let p = super::cassette::Player::load(path)
                .map_err(|e| err(format!("socket.proxy: {e}")))?;
            Some(Rc::new(RefCell::new(p)))
        } else {
            None
        };
        if mode == Mode::Record {
            let redact = opts
                .get::<Option<Vec<String>>>("redact")?
                .unwrap_or_default();
            let path = cassette
                .clone()
                .ok_or_else(|| err("socket.proxy: recording needs a `cassette`"))?;
            init.recorder = Some(Rc::new(
                super::cassette::Recorder::new(path, "socket").with_redactions(redact),
            ));
        }
        let state: Rc<RefCell<ProxyState>> = Rc::new(RefCell::new(init));
        let (drop_tx, drop_rx) = tokio::sync::watch::channel(false);
        let drop_tx = Rc::new(drop_tx);
        let (tx, mut rx) = tokio::sync::oneshot::channel::<()>();

        let accept_state = state.clone();
        let accept_drop = drop_rx.clone();
        let f = framing.clone();
        tokio::task::spawn_local(async move {
            loop {
                tokio::select! {
                    _ = &mut rx => break,
                    accepted = acceptor.accept() => {
                        let Ok(client) = accepted else { break };
                        if *accept_drop.borrow() {
                            continue; // dropped: refuse by immediate close (client sees EOF)
                        }
                        let state = accept_state.clone();
                        let framing = f.clone();
                        let upstream = upstream.clone();
                        let drop_rx = accept_drop.clone();
                        let mode = mode.clone();
                        let player = player.clone();
                        tokio::task::spawn_local(async move {
                            if mode == Mode::Passthrough {
                                // The plain wiretap: bidirectional pump (full-duplex + faults).
                                let Some(up_addr) = upstream else { return };
                                let Ok(up) = dial(&up_addr).await else { return };
                                let (cr, cw) = tokio::io::split(client);
                                let (ur, uw) = tokio::io::split(up);
                                let a = pump(cr, uw, "up", framing.clone(), state.clone(), drop_rx.clone());
                                let b = pump(ur, cw, "down", framing, state, drop_rx);
                                tokio::join!(a, b);
                            } else {
                                // A cassette mode: the request/response turn loop. Replay answers
                                // from the player and must NOT dial — an `upstream` may still be
                                // present (auto mode passes one), so drop it here, or a replay
                                // would try to reach a dependency that is gone.
                                let up = if mode == Mode::Replay { None } else { upstream };
                                turn_loop(client, up, framing, state, player).await;
                            }
                        });
                    }
                }
            }
        });

        let ud = lua.create_userdata(ProxyUd {
            addr,
            state,
            dropped: drop_tx,
            shutdown: RefCell::new(Some(tx)),
        })?;
        super::manage("socket.proxy", &ctx, &ud)?;
        Ok(ud)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Framing::parse speaks the whole grammar — the string form, both table forms, the bounds,
    /// and the taught refusals. Encode wraps a payload in its on-wire form, with the length
    /// prefix big-endian across its declared width.
    #[test]
    fn framing_parse_and_encode() {
        let lua = Lua::new();
        assert!(Framing::parse(None).unwrap().is_raw());
        assert!(matches!(
            Framing::parse(Some(Value::String(lua.create_string("line").unwrap()))),
            Ok(Framing::Line)
        ));
        let t = lua.create_table().unwrap();
        t.set("length_prefixed", 4).unwrap();
        assert!(matches!(Framing::parse(Some(Value::Table(t))), Ok(Framing::LengthPrefixed(4))));
        let t = lua.create_table().unwrap();
        t.set("length_prefixed", 9).unwrap();
        assert!(Framing::parse(Some(Value::Table(t))).is_err(), "width capped at 8");
        let t = lua.create_table().unwrap();
        t.set("delimiter", "\u{1}").unwrap();
        assert!(matches!(Framing::parse(Some(Value::Table(t))), Ok(Framing::Delimiter(_))));
        let t = lua.create_table().unwrap();
        t.set("delimiter", "").unwrap();
        assert!(Framing::parse(Some(Value::Table(t))).is_err(), "empty delimiter refused");
        assert!(Framing::parse(Some(Value::Integer(3))).is_err(), "wrong type taught");

        assert_eq!(Framing::Raw.encode(b"ab"), b"ab");
        assert_eq!(Framing::Line.encode(b"ab"), b"ab\n");
        assert_eq!(Framing::LengthPrefixed(2).encode(b"abc"), vec![0, 3, b'a', b'b', b'c']);
        assert_eq!(
            Framing::Delimiter(vec![0xff]).encode(b"x"),
            vec![b'x', 0xff],
            "delimiter appends"
        );
    }

    /// recv's arity IS the framing: raw connections read exact byte counts (n required),
    /// framed connections read whole turns (a byte count is refused, taught) — and the
    /// timeout opt parses through either form.
    #[test]
    fn recv_args_follow_the_framing() {
        let lua = Lua::new();
        let (want, dur) = recv_args(&Framing::Raw, Some(Value::Integer(64)), None).unwrap();
        assert_eq!((want, dur), (64, DEFAULT_IO_TIMEOUT));
        assert!(recv_args(&Framing::Raw, None, None).is_err(), "raw without n");
        assert!(recv_args(&Framing::Raw, Some(Value::Integer(0)), None).is_err(), "zero bytes");

        let (want, dur) = recv_args(&Framing::Line, None, None).unwrap();
        assert_eq!((want, dur), (0, DEFAULT_IO_TIMEOUT));
        assert!(
            recv_args(&Framing::Line, Some(Value::Integer(64)), None).is_err(),
            "framed refuses a byte count"
        );
        let opts = lua.create_table().unwrap();
        opts.set("timeout", "250ms").unwrap();
        let (_, dur) = recv_args(&Framing::Line, Some(Value::Table(opts)), None).unwrap();
        assert_eq!(dur, Duration::from_millis(250));
        let opts = lua.create_table().unwrap();
        opts.set("timeout", "eleventy").unwrap();
        assert!(recv_args(&Framing::Line, Some(Value::Table(opts)), None).is_err());
    }

    /// Rates are declared in bits (bps/kbps/mbps, the units networks speak) and applied in
    /// bytes per second.
    #[test]
    fn parse_rate_converts_bits_to_bytes() {
        assert_eq!(parse_rate("8bps").unwrap(), 1.0);
        assert_eq!(parse_rate("16kbps").unwrap(), 2_000.0);
        assert_eq!(parse_rate("1 mbps").unwrap(), 125_000.0, "whitespace before the unit is fine");
        assert!(parse_rate("100").is_err(), "unitless is refused");
        assert!(parse_rate("fastbps").is_err(), "non-numeric is refused");
    }

    /// The proxy's option contract: passthrough by default, auto collapses on the cassette's
    /// presence, cassettes demand framing (a raw stream has no turn to key on) and a path,
    /// and every non-replay mode validates its upstream at the call site.
    #[test]
    fn proxy_config_speaks_the_mode_contract() {
        let lua = Lua::new();
        let opts = |pairs: &[(&str, &str)]| {
            let t = lua.create_table().unwrap();
            for (k, v) in pairs {
                t.set(*k, *v).unwrap();
            }
            t
        };

        let (up, framing, mode, cas) =
            proxy_config(&opts(&[("upstream", "tcp://127.0.0.1:9")])).unwrap();
        assert_eq!(up.as_deref(), Some("tcp://127.0.0.1:9"));
        assert!(framing.is_raw() && mode == Mode::Passthrough && cas.is_none());

        let missing = std::env::temp_dir().join("prova-socket-ut-no-such.json");
        let _ = std::fs::remove_file(&missing);
        let missing = missing.to_string_lossy().into_owned();
        let (_, _, mode, cas) = proxy_config(&opts(&[
            ("upstream", "tcp://127.0.0.1:9"),
            ("framing", "line"),
            ("mode", "auto"),
            ("cassette", &missing),
        ]))
        .unwrap();
        assert_eq!(mode, Mode::Record, "auto with no cassette on disk records");
        assert_eq!(cas.as_deref(), Some(missing.as_str()));

        for (broken, teaches) in [
            (opts(&[("mode", "record"), ("cassette", "c.json")]), "framing"),
            (opts(&[("mode", "record"), ("framing", "line")]), "cassette"),
            (opts(&[("mode", "auto"), ("framing", "line")]), "cassette"),
            (opts(&[("mode", "record"), ("framing", "line"), ("cassette", "c.json")]), "upstream"),
            (opts(&[("upstream", "not-an-addr"), ("mode", "record"), ("framing", "line"), ("cassette", "c.json")]), "tcp://"),
            (opts(&[("mode", "sideways")]), "passthrough|record|replay|auto"),
        ] {
            let e = proxy_config(&broken).unwrap_err().to_string();
            assert!(e.contains(teaches), "expected the error to teach {teaches:?}: {e}");
        }

        let (up, _, mode, _) = proxy_config(&opts(&[
            ("mode", "replay"),
            ("framing", "line"),
            ("cassette", "c.json"),
        ]))
        .unwrap();
        assert!(up.is_none() && mode == Mode::Replay, "replay needs no upstream");
    }

    /// The scheme IS the parse: tcp:// keeps its host:port, anything else is a taught error.
    #[test]
    fn parse_addr_schemes() {
        assert!(matches!(parse_addr("tcp://127.0.0.1:80"), Ok(Addr::Tcp(hp)) if hp == "127.0.0.1:80"));
        let Err(err) = parse_addr("http://x") else {
            panic!("http:// must not parse as a socket address");
        };
        let err = err.to_string();
        assert!(err.contains("tcp://host:port"), "teaches the grammar: {err}");
        #[cfg(unix)]
        assert!(matches!(
            parse_addr("unix:///tmp/s.sock"),
            Ok(Addr::Unix(p)) if p == std::path::Path::new("/tmp/s.sock")
        ));
    }

    /// The frame readers against a real loopback pair: a frame split across writes completes,
    /// leftovers past a frame boundary carry into the next read, an EOF'd partial frame is None
    /// (never a half-turn), and raw reads report exactly how much arrived before an early close.
    #[test]
    fn frame_readers_recover_turns_across_chunk_boundaries() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .build()
            .unwrap();
        rt.block_on(async {
            let pair = || async {
                let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                let addr = l.local_addr().unwrap();
                let client = tokio::net::TcpStream::connect(addr).await.unwrap();
                let (server, _) = l.accept().await.unwrap();
                (Stream::Tcp(client), server)
            };

            // Line framing: one write carries a whole frame AND the head of the next; the
            // second frame completes only when the rest arrives.
            let (mut conn, mut server) = pair().await;
            let mut buf = Vec::new();
            server.write_all(b"hello\nwor").await.unwrap();
            let frame = read_frame(&mut conn, &mut buf, &Framing::Line).await.unwrap();
            assert_eq!(frame.as_deref(), Some(&b"hello"[..]));
            server.write_all(b"ld\n").await.unwrap();
            let frame = read_frame(&mut conn, &mut buf, &Framing::Line).await.unwrap();
            assert_eq!(frame.as_deref(), Some(&b"world"[..]));
            drop(server);
            let frame = read_frame(&mut conn, &mut buf, &Framing::Line).await.unwrap();
            assert_eq!(frame, None, "a clean EOF with no partial frame is the end of turns");

            // Length-prefixed: the prefix and payload arrive in separate writes.
            let (mut conn, mut server) = pair().await;
            let mut buf = Vec::new();
            server.write_all(&[0, 5, b'a', b'b']).await.unwrap();
            server.write_all(b"cde").await.unwrap();
            let frame = read_frame(&mut conn, &mut buf, &Framing::LengthPrefixed(2)).await.unwrap();
            assert_eq!(frame.as_deref(), Some(&b"abcde"[..]));

            // A partial frame at EOF never surfaces as a turn.
            let (mut conn, mut server) = pair().await;
            let mut buf = Vec::new();
            server.write_all(b"dangling").await.unwrap();
            drop(server);
            let frame = read_frame(&mut conn, &mut buf, &Framing::Line).await.unwrap();
            assert_eq!(frame, None, "an unterminated frame is not a frame");

            // Raw reads: exact counts across chunks, leftovers kept; an early close names the
            // shortfall.
            let (mut conn, mut server) = pair().await;
            let mut buf = Vec::new();
            server.write_all(b"abcdef").await.unwrap();
            let got = read_exact_buffered(&mut conn, &mut buf, 4).await.unwrap();
            assert_eq!(got, b"abcd");
            assert_eq!(buf, b"ef", "the surplus stays buffered for the next read");
            drop(server);
            let err = read_exact_buffered(&mut conn, &mut buf, 4).await.unwrap_err();
            assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
            assert!(err.to_string().contains("2/4"), "names the shortfall: {err}");
        });
    }

    /// The scope-teardown seam's stand-in: accepts the registration so `manage` is satisfied.
    struct StubCtx;
    impl UserData for StubCtx {
        fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
            methods.add_method("manage", |_, _, _ud: mlua::AnyUserData| Ok(()));
        }
    }

    /// All three postures through the module's own Lua surface, hosted like the engine hosts
    /// them: originate (listen/connect, raw bytes with exact counts), terminate (a framed mock
    /// answering turns and journaling the unmatched), and interpose (the proxy's
    /// direction-tagged transcript in front of the mock).
    #[test]
    fn the_three_postures_round_trip_over_loopback() {
        use mlua::ObjectLike;
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let local = tokio::task::LocalSet::new();
        local.block_on(&rt, async {
            let lua = Lua::new();
            lua.globals().set("socket", make(&lua).unwrap()).unwrap();
            lua.globals()
                .set("ctx", lua.create_userdata(StubCtx).unwrap())
                .unwrap();

            let outcome: Table = lua
                .load(
                    r#"
                    -- originate: raw bytes, exact counts
                    local srv = socket.listen(ctx, { addr = "tcp://127.0.0.1:0" })
                    local c = socket.connect(srv.addr)
                    c:send("\1\2\3")
                    local conn = srv:accept()
                    local raw = conn:recv(3, { timeout = "5s" })
                    conn:send("\4")
                    local back = c:recv(1, { timeout = "5s" })
                    c:close()

                    -- terminate: a framed mock answers turns; strays journal as unmatched
                    local m = socket.mock(ctx, { addr = "tcp://127.0.0.1:0", framing = "line" })
                    m:on("PING"):reply("PONG")
                    local mc = socket.connect(m.addr, { framing = "line" })
                    mc:send("PING")
                    local answered = mc:recv({ timeout = "5s" })
                    mc:send("STRAY")

                    -- interpose: the proxy wiretaps in front of the mock
                    local p = socket.proxy(ctx, { upstream = m.addr, framing = "line" })
                    local pc = socket.connect(p.addr, { framing = "line" })
                    pc:send("PING")
                    local through = pc:recv({ timeout = "5s" })

                    return { raw = raw, back = back, answered = answered, through = through,
                             m = m, p = p }
                    "#,
                )
                .eval_async()
                .await
                .unwrap();
            assert_eq!(outcome.get::<mlua::String>("raw").unwrap().as_bytes(), &b"\x01\x02\x03"[..]);
            assert_eq!(outcome.get::<mlua::String>("back").unwrap().as_bytes(), &b"\x04"[..]);
            assert_eq!(outcome.get::<String>("answered").unwrap(), "PONG");
            assert_eq!(outcome.get::<String>("through").unwrap(), "PONG", "the proxy passes untouched");

            // The mock's journal keeps the stray as unmatched (§6: kept, not dropped) and the
            // proxy's transcript tags both directions.
            let m: mlua::AnyUserData = outcome.get("m").unwrap();
            let mut journal: Option<Table> = None;
            for _ in 0..100 {
                let got: Table = m.call_method("received", ()).unwrap();
                // Three turns reached the mock: PING direct, STRAY, PING through the proxy.
                if got.raw_len() == 3 {
                    journal = Some(got);
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            let journal = journal.expect("all three turns journal within the bound");
            let stray: Table = journal.get(2).unwrap();
            assert_eq!(stray.get::<String>("data").unwrap(), "STRAY");
            assert!(!stray.get::<bool>("matched").unwrap());
            assert_eq!(stray.get::<String>("source").unwrap(), "unmatched");

            let p: mlua::AnyUserData = outcome.get("p").unwrap();
            let mut transcript: Option<Table> = None;
            for _ in 0..100 {
                let got: Table = p.call_method("transcript", ()).unwrap();
                if got.raw_len() == 2 {
                    transcript = Some(got);
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            let transcript = transcript.expect("both directions transcribe within the bound");
            let up: Table = transcript.get(1).unwrap();
            let down: Table = transcript.get(2).unwrap();
            assert_eq!(up.get::<String>("dir").unwrap(), "up");
            assert_eq!(up.get::<String>("data").unwrap(), "PING");
            assert_eq!(down.get::<String>("dir").unwrap(), "down");
            assert_eq!(down.get::<String>("data").unwrap(), "PONG");
        });
    }
}
