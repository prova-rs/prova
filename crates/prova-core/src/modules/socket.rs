//! The `socket` kernel transport — low-level byte streams with the full Mock/Proxy/Driver triad
//! (docs/design/mocks-proxies-drivers.md, proofs/spec/socket + proofs/spec/faults).
//!
//! One namespace, unified by ADDRESS SCHEME: `tcp://host:port` and `unix:///path` are just
//! addresses — listen, connect, proxy, and the byte model are identical; only address parsing
//! differs. A raw byte stream has no natural "request" unit, so mocks and transcripts take a
//! FRAMING strategy that turns bytes into matchable turns — and framing, the `codec` that turns a
//! turn into a value, and the `where` selector that picks one all live in [`super::turn`], shared
//! with every other stream transport. The byte-level proxy is the universal wiretap: put it in
//! front of anything TCP and you get direction-tagged transcripts plus the fault vocabulary
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

use super::turn::{read_frame, Codec, Framing, Selector};
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
pub(super) enum Addr {
    Tcp(String),
    #[cfg(unix)]
    Unix(std::path::PathBuf),
}

pub(super) fn parse_addr(s: &str) -> mlua::Result<Addr> {
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

pub(super) enum Stream {
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

pub(super) enum Acceptor {
    Tcp(tokio::net::TcpListener),
    #[cfg(unix)]
    Unix(tokio::net::UnixListener, std::path::PathBuf),
}

impl Acceptor {
    /// Bind synchronously so the resolved address is known — and accepting — before we return.
    /// `tcp://host:0` resolves the ephemeral port into the advertised `.addr`.
    ///
    /// Synchronous binding is also what closes the spawn race for `stdio.mock`: the socket is
    /// LISTENING before its address (and therefore the shim that dials it) exists, so a SUT that
    /// spawns the shim instantly cannot arrive before the mock is ready.
    pub(super) fn bind(addr: &Addr) -> mlua::Result<(Self, String)> {
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

    pub(super) async fn accept(&self) -> std::io::Result<Stream> {
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

// ── the driver: Conn (originate) ───────────────────────────────────────────────────────────────

/// One live connection. The stream is `take()`n out for the duration of an I/O call so a
/// concurrent call errors ("busy") instead of panicking a `RefCell` across an await.
struct Conn {
    stream: Rc<RefCell<Option<Stream>>>,
    buf: Rc<RefCell<Vec<u8>>>,
    framing: Framing,
    codec: Codec,
}

impl Conn {
    fn new(stream: Stream, framing: Framing, codec: Codec) -> Conn {
        Conn {
            stream: Rc::new(RefCell::new(Some(stream))),
            buf: Rc::new(RefCell::new(Vec::new())),
            framing,
            codec,
        }
    }
}

/// What `recv` was asked for: a byte count (raw only), a bound, and which turn to stop on.
#[derive(Debug)]
struct RecvArgs {
    want: usize,
    timeout: Duration,
    selector: Selector,
}

/// Every option `recv` honors. Closed like every other opts surface: `recv{ wehre = … }` reading
/// as "the next turn, unfiltered" is the silent-drop disease at its most expensive, because the
/// wrong turn still arrives and the proof still asserts on it.
const RECV_OPTS: &[&str] = &["timeout", "where"];

fn recv_args(
    framing: &Framing,
    codec: Codec,
    a: Option<Value>,
    b: Option<Table>,
) -> mlua::Result<RecvArgs> {
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
    let (timeout, selector) = match &opts {
        Some(t) => {
            crate::opts::reject_unknown(t, RECV_OPTS, "recv")?;
            let dur = match t.get::<Option<String>>("timeout")? {
                Some(s) => parse_duration(&s).ok_or_else(|| err(format!("bad duration {s:?}")))?,
                None => DEFAULT_IO_TIMEOUT,
            };
            let sel = Selector::parse("recv", codec, t.get::<Option<Value>>("where")?)?;
            // A raw stream has no turns to select BETWEEN — `where` there would read as a filter
            // and behave as nothing at all.
            if framing.is_raw() && !sel.is_any() {
                return Err(err(
                    "recv: `where` selects among TURNS, and this connection is unframed — set \
                     framing so the stream has turns to choose from",
                ));
            }
            (dur, sel)
        }
        None => (DEFAULT_IO_TIMEOUT, Selector::Any),
    };
    Ok(RecvArgs { want, timeout, selector })
}

impl UserData for Conn {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_async_method("send", |lua, this, data: Value| async move {
            let wire = this.framing.encode(&this.codec.encode(&lua, &data)?);
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
                let args = recv_args(&this.framing, this.codec, a, b)?;
                let dur = args.timeout;
                let Some(mut s) = this.stream.borrow_mut().take() else {
                    return Err(err("recv: connection is closed or busy"));
                };
                let mut buf = std::mem::take(&mut *this.buf.borrow_mut());
                let (codec, sel) = (this.codec, &args.selector);
                let mut skipped = 0usize;
                let res = tokio::time::timeout(dur, async {
                    if this.framing.is_raw() {
                        return super::turn::read_exact_buffered(&mut s, &mut buf, args.want)
                            .await
                            .map(Some)
                            .map_err(|e| err(format!("recv: {e}")));
                    }
                    super::turn::read_until(&mut s, &mut buf, &this.framing, |payload| {
                        if sel.is_any() {
                            return Ok(true);
                        }
                        let hit = sel.accepts(&codec.decode(&lua, payload)?)?;
                        if !hit {
                            skipped += 1;
                        }
                        Ok(hit)
                    })
                    .await
                })
                .await;
                *this.buf.borrow_mut() = buf;
                *this.stream.borrow_mut() = Some(s);
                match res {
                    // Naming the skipped count separates "nothing arrived" from "turns arrived and
                    // none was the one asked for" — the same failure with opposite causes.
                    Err(_) => Err(err(format!("recv: timed out after {dur:?}{}", super::turn::waited(skipped)))),
                    Ok(Err(e)) => Err(e),
                    Ok(Ok(None)) => {
                        Err(err(format!("recv: connection closed{}", super::turn::waited(skipped))))
                    }
                    Ok(Ok(Some(payload))) => this.codec.decode(&lua, &payload),
                }
            },
        );

        methods.add_method("close", |_, this, ()| {
            this.stream.borrow_mut().take();
            Ok(())
        });
    }
}

/// Every option `socket.connect` honors.
const CONNECT_OPTS: &[&str] = &["addr", "codec", "framing"];

fn connect_fn(lua: &Lua) -> mlua::Result<Function> {
    lua.create_async_function(|lua, (ctx, opts): (Value, Option<Table>)| async move {
        super::runtime_only("socket.connect")?;
        // The retired positional spelling. `Closed` cannot see this — it changed the FIRST
        // ARGUMENT, not a key — so the shape is checked here, and refused with the new spelling
        // rather than left to fail as "expected the test context, got a string".
        if let Value::String(addr) = &ctx {
            return Err(err(format!(
                "socket.connect(ctx, {{ addr = {:?} }}): the address is now a named option and the \
                 context comes first, so the connection is closed with the scope instead of \
                 leaking until GC",
                addr.to_string_lossy()
            )));
        }
        let opts = opts.ok_or_else(|| {
            err("socket.connect(ctx, { addr = \"tcp://…\" }): the options table is required")
        })?;
        crate::opts::reject_unknown(&opts, CONNECT_OPTS, "socket.connect")?;
        let addr = opts
            .get::<Option<String>>("addr")?
            .ok_or_else(|| err("socket.connect(ctx, { addr = \"tcp://…\" }): addr is required"))?;
        let framing = Framing::parse("socket.connect", opts.get::<Option<Value>>("framing")?)?;
        let codec = Codec::parse("socket.connect", opts.get::<Option<Value>>("codec")?)?;
        let ud = lua.create_userdata(Conn::new(dial(&addr).await?, framing, codec))?;
        super::manage("socket.connect", &ctx, &ud)?;
        Ok(ud)
    })
}

// ── the driver: Listener (originate, server side) ─────────────────────────────────────────────

struct ListenerUd {
    addr: String,
    acceptor: Rc<RefCell<Option<Acceptor>>>,
    framing: Framing,
    codec: Codec,
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
                Ok(Ok(stream)) => Ok(Conn::new(stream, this.framing.clone(), this.codec)),
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
        crate::opts::reject_unknown(&opts, LISTEN_OPTS, "socket.listen")?;
        let framing = Framing::parse("socket.listen", opts.get::<Option<Value>>("framing")?)?;
        let codec = Codec::parse("socket.listen", opts.get::<Option<Value>>("codec")?)?;
        let (acceptor, addr) = Acceptor::bind(&parse_addr(&addr_s)?)?;
        let ud = lua.create_userdata(ListenerUd {
            addr,
            acceptor: Rc::new(RefCell::new(Some(acceptor))),
            framing,
            codec,
        })?;
        super::manage("socket.listen", &ctx, &ud)?;
        Ok(ud)
    })
}

/// Bind an address and hand back the acceptor plus the RESOLVED address string — the one call
/// `stdio.mock`/`stdio.proxy` need, so the spawnable postures never reach into `Addr`/`Acceptor`
/// themselves. Synchronous, so the endpoint is accepting before its address exists anywhere.
pub(super) fn bind_at(addr: &str) -> mlua::Result<(Acceptor, String)> {
    Acceptor::bind(&parse_addr(addr)?)
}

/// A snapshot of the mock's §6 journal rows, for a handle that wraps this state rather than
/// owning it (`stdio.mock` is this mock, reached by spawn).
pub(super) fn journal_of(state: &Rc<RefCell<MockState>>) -> Vec<super::wiretap::JournalRow> {
    state.borrow().journal.clone()
}

/// The turn models a byte-turn replay can read. `socket` and `stdio` record an identical format —
/// a session captured through one replays through the other.
pub(super) const BYTE_TURN_KINDS: &[&str] = &["socket", "stdio"];

// ── the mock: terminate ────────────────────────────────────────────────────────────────────────

/// One stub: what selects the turn, and what to answer with.
///
/// The key is a [`Selector`] — the SAME type `recv{ where = … }` takes — so `:on` and `where` are
/// one code path rather than two matchers that can disagree. With `codec = "json"`,
/// `:on{ method = "tools/list" }` subset-matches the DECODED turn, which is what an MCP or LSP stub
/// needs: it keys on a field, never on an exact serialization (key order and whitespace are the
/// client's business, not the stub's).
pub(super) struct Stub {
    key: Selector,
    /// Bytes as they go on the wire — the codec has already been applied at `:reply` time.
    reply: Option<Vec<u8>>,
}

#[derive(Default)]
pub(super) struct MockState {
    stubs: Vec<Stub>,
    journal: Vec<super::wiretap::JournalRow>,
}

impl MockState {
    /// The first stub whose selector accepts this turn, and its answer. `Ok(None)` is unmatched —
    /// which the caller makes LOUD; a mock never guesses.
    fn answer(&self, lua: &Lua, codec: Codec, turn: &[u8]) -> mlua::Result<Option<Option<Vec<u8>>>> {
        // Decode ONCE for the whole stub list rather than per stub. A byte-keyed selector does not
        // need it at all, so a bytes-codec mock never pays for a decode it cannot use.
        let decoded = match self.stubs.iter().any(|s| s.needs_decode()) {
            true => Some(codec.decode(lua, turn)?),
            false => None,
        };
        for stub in &self.stubs {
            let hit = match (&stub.key, &decoded) {
                (Selector::Bytes(b), _) => b == turn,
                (_, Some(v)) => stub.key.accepts(v)?,
                // A shape stub with nothing decoded cannot match; unreachable given the check
                // above, and a silent `false` beats a panic if that ever stops being true.
                (_, None) => false,
            };
            if hit {
                return Ok(Some(stub.reply.clone()));
            }
        }
        Ok(None)
    }
}

impl Stub {
    fn needs_decode(&self) -> bool {
        !matches!(self.key, Selector::Bytes(_))
    }
}

struct MockUd {
    addr: String,
    state: Rc<RefCell<MockState>>,
    codec: Codec,
    shutdown: RefCell<Option<tokio::sync::oneshot::Sender<()>>>,
}

pub(super) struct MockStub {
    state: Rc<RefCell<MockState>>,
    codec: Codec,
    idx: usize,
}

impl UserData for MockStub {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("reply", |lua, this, data: Value| {
            // Encoded HERE, once, at declaration time — not per delivery. The codec is the same
            // dial the driver's `:send` uses, so a json mock answers with a table.
            let payload = this.codec.encode(lua, &data)?;
            this.state.borrow_mut().stubs[this.idx].reply = Some(payload);
            Ok(())
        });
    }
}

/// Register `:on(turn_or_shape)` — shared by `socket.mock` and, through the relay adapter,
/// `stdio.mock`. There is one mock implementation; the two namespaces differ only in how the SUT
/// REACHES it (dial an address, or spawn a shim that dials it for them).
pub(super) fn add_on_method<T, M>(methods: &mut M, state: fn(&T) -> &Rc<RefCell<MockState>>, codec: fn(&T) -> Codec)
where
    T: UserData + 'static,
    M: UserDataMethods<T>,
{
    methods.add_method("on", move |lua, this, turn: Value| {
        let c = codec(this);
        let key = Selector::parse_stub("on", c, turn)?;
        let st = state(this);
        let idx = {
            let mut s = st.borrow_mut();
            s.stubs.push(Stub { key, reply: None });
            s.stubs.len() - 1
        };
        lua.create_userdata(MockStub { state: st.clone(), codec: c, idx })
    });
}

impl UserData for MockUd {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("addr", |_, this| Ok(this.addr.clone()));
        fields.add_field_method_get("endpoint", |_, this| Ok(this.addr.clone()));
    }
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        add_on_method(methods, |t: &MockUd| &t.state, |t: &MockUd| t.codec);
        super::wiretap::add_received_method(methods);
        super::wiretap::add_shutdown_methods(methods);
    }
}

super::wiretap::impl_journal!(MockUd);
super::wiretap::impl_shutdown!(MockUd);

/// Every option `socket.mock` honors — closed by construction
/// (docs/design/agent-ergonomics.md#module-opts-silently-ignored).
///
/// `codec` landed here 2026-08-19 with `stdio.mock`, which needed shape matching — and since a
/// stdio mock IS this mock reached by spawn, both got it in one change. It was deliberately absent
/// until the behavior existed: accepting an option before it is honored is the silent drop this
/// gate exists to refuse.
const MOCK_OPTS: &[&str] = &["addr", "codec", "framing"];

/// Every option `socket.listen` honors — the raw acceptor, not the mock.
const LISTEN_OPTS: &[&str] = &["addr", "codec", "framing"];

/// Every option `socket.proxy` honors. `upstream`/`framing`/`mode`/`cassette` are read through
/// `proxy_config`, so the closed set spans both readers — a gate that only knew this function's
/// own `get` calls would refuse four options that work.
const PROXY_OPTS: &[&str] = &["addr", "cassette", "framing", "mode", "redact", "upstream"];

fn mock_fn(lua: &Lua) -> mlua::Result<Function> {
    lua.create_function(|lua, (ctx, opts): (Value, Option<Table>)| {
        let opts = opts.ok_or_else(|| {
            err("socket.mock(ctx, { framing = … }): framing is required — matching needs turns")
        })?;
        crate::opts::reject_unknown(&opts, MOCK_OPTS, "socket.mock")?;
        let framing = Framing::parse("socket.mock", opts.get::<Option<Value>>("framing")?)?;
        let codec = Codec::parse("socket.mock", opts.get::<Option<Value>>("codec")?)?;
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
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();

        serve_mock(lua, acceptor, state.clone(), framing, codec, rx);

        let ud = lua.create_userdata(MockUd {
            addr,
            state,
            codec,
            shutdown: RefCell::new(Some(tx)),
        })?;
        super::manage("socket.mock", &ctx, &ud)?;
        Ok(ud)
    })
}

/// Run the mock's accept loop until `rx` fires. Shared with `stdio.mock`, which is THIS mock
/// reached by spawn instead of dial — there is one implementation, and the shim only carries bytes.
///
/// `spawn_local`, never `tokio::spawn`: matching decodes turns and calls Lua predicates, so this
/// task holds a `Lua` handle and must stay on the engine's thread (the same rule `http.mock`'s
/// handler loop follows).
pub(super) fn serve_mock(
    lua: &Lua,
    acceptor: Acceptor,
    state: Rc<RefCell<MockState>>,
    framing: Framing,
    codec: Codec,
    mut rx: tokio::sync::oneshot::Receiver<()>,
) {
    let lua = lua.clone();
    tokio::task::spawn_local(async move {
        loop {
            tokio::select! {
                _ = &mut rx => break,
                accepted = acceptor.accept() => {
                    let Ok(mut stream) = accepted else { break };
                    let conn_state = state.clone();
                    let framing = framing.clone();
                    let lua = lua.clone();
                    tokio::task::spawn_local(async move {
                        let mut buf = Vec::new();
                        while let Ok(Some(turn)) = read_frame(&mut stream, &mut buf, &framing).await {
                            // Resolve the answer and drop the borrow BEFORE the write: matching
                            // can re-enter Lua (a predicate stub), and a borrow held across that
                            // — or across the await below — is a runtime panic, not a warning.
                            let answer = conn_state.borrow().answer(&lua, codec, &turn);
                            let reply = match answer {
                                Ok(Some(reply)) => {
                                    conn_state.borrow_mut().journal.push(
                                        super::wiretap::JournalRow {
                                            data: turn,
                                            matched: true,
                                            source: "stub",
                                        },
                                    );
                                    reply
                                }
                                // The §6 rule: an unmatched turn is journaled — it is the most
                                // interesting thing a mock can record — and the connection closes
                                // LOUD instead of guessing. A turn the codec cannot even decode
                                // lands here too, and for the same reason: a turn we cannot read
                                // is a turn we cannot have matched, and the journal carries the
                                // raw bytes so the author sees WHAT arrived.
                                Ok(None) | Err(_) => {
                                    conn_state.borrow_mut().journal.push(
                                        super::wiretap::JournalRow {
                                            data: turn,
                                            matched: false,
                                            source: "unmatched",
                                        },
                                    );
                                    break;
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
                // The ONE frame scanner (`super::turn`), same as the blocking reader uses. This
                // loop carried a second copy until 2026-08-18; two scanners is two chances to
                // disagree about where a frame ends, and a disagreement here does not error — it
                // hands the transcript a differently-cut turn.
                loop {
                    if let Some(payload) = framing.take_frame(&mut buf) {
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
    let framing = Framing::parse("socket.proxy", opts.get::<Option<Value>>("framing")?)?;
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
        crate::opts::reject_unknown(&opts, PROXY_OPTS, "socket.proxy")?;
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
            let p = super::cassette::Player::load_of(path, BYTE_TURN_KINDS)
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

/// The unit tests live in their own file — see the note at the top of it.
#[cfg(test)]
#[path = "socket/tests.rs"]
mod tests;
