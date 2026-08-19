//! The `websocket` kernel transport — full-duplex message turns over the http upgrade path
//! (docs/design/mocks-proxies-drivers.md, proofs/spec/websocket).
//!
//! The protocol frames messages natively, so unlike raw `socket` there is NO framing strategy:
//! a turn is a ws message, always. Full-duplex means the mock is not only request→response —
//! `on_connect` lets the server side PUSH unprompted (the scripted-conversation model, not VCR).
//! Journals speak the §6 spine (seq/source/matched) from day one.
//!
//! ws:// only, matching http's no-TLS stance. Single-threaded `spawn_local`, like every mock.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use mlua::{Function, Lua, Table, UserData, UserDataFields, UserDataMethods, Value};
use tokio_tungstenite::tungstenite::Message;

use super::turn::{Codec, Selector};
use crate::model::parse_duration;

const DEFAULT_RECV_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) fn make(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set("connect", connect_fn(lua)?)?;
    t.set("mock", mock_fn(lua)?)?;
    t.set("proxy", proxy_fn(lua)?)?;
    Ok(t)
}

fn err(msg: impl Into<String>) -> mlua::Error {
    mlua::Error::RuntimeError(msg.into())
}

fn msg_bytes(m: &Message) -> Option<Vec<u8>> {
    match m {
        Message::Text(s) => Some(s.as_bytes().to_vec()),
        Message::Binary(b) => Some(b.clone()),
        _ => None, // ping/pong/close are transport plumbing, not turns
    }
}

// ── the driver: websocket.connect ──────────────────────────────────────────────────────────────

type WsClient = tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
>;

struct ClientUd {
    ws: Rc<RefCell<Option<WsClient>>>,
    codec: Codec,
}

/// Every option `websocket.connect`'s `recv` honors — the same closed set, and the same two
/// reasons for closing it, as `socket`'s (`recv{ wehre = … }` returns the WRONG turn rather than
/// none).
const RECV_OPTS: &[&str] = &["timeout", "where"];

impl UserData for ClientUd {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_async_method("send", |lua, this, data: Value| async move {
            let payload = this.codec.encode(&lua, &data)?;
            let Some(mut ws) = this.ws.borrow_mut().take() else {
                return Err(err("send: connection is closed or busy"));
            };
            let text = String::from_utf8_lossy(&payload).to_string();
            let r = ws.send(Message::Text(text)).await;
            *this.ws.borrow_mut() = Some(ws);
            r.map_err(|e| err(format!("send: {e}")))
        });

        methods.add_async_method("recv", |lua, this, opts: Option<Table>| async move {
            let (dur, sel) = match &opts {
                Some(t) => {
                    crate::opts::reject_unknown(t, RECV_OPTS, "recv")?;
                    let dur = match t.get::<Option<String>>("timeout")? {
                        Some(s) => {
                            parse_duration(&s).ok_or_else(|| err(format!("bad timeout {s:?}")))?
                        }
                        None => DEFAULT_RECV_TIMEOUT,
                    };
                    (
                        dur,
                        Selector::parse("recv", this.codec, t.get::<Option<Value>>("where")?)?,
                    )
                }
                None => (DEFAULT_RECV_TIMEOUT, Selector::Any),
            };
            let Some(mut ws) = this.ws.borrow_mut().take() else {
                return Err(err("recv: connection is closed or busy"));
            };
            let codec = this.codec;
            let mut skipped = 0usize;
            let res = tokio::time::timeout(dur, async {
                loop {
                    match ws.next().await {
                        None => return Err(err("recv: connection closed")),
                        Some(Err(e)) => return Err(err(format!("recv: {e}"))),
                        Some(Ok(m)) => {
                            if let Some(b) = msg_bytes(&m) {
                                // Decoding happens here, inside the loop and never held across the
                                // await, so a skipped turn costs one decode and nothing else.
                                if sel.is_any() || sel.accepts(&codec.decode(&lua, &b)?)? {
                                    return Ok(b);
                                }
                                skipped += 1;
                                continue;
                            }
                            if matches!(m, Message::Close(_)) {
                                return Err(err("recv: connection closed"));
                            }
                        }
                    }
                }
            })
            .await;
            *this.ws.borrow_mut() = Some(ws);
            match res {
                Err(_) => Err(err(format!(
                    "recv: timed out after {dur:?}{}",
                    super::turn::waited(skipped)
                ))),
                Ok(Err(e)) => Err(e),
                Ok(Ok(b)) => this.codec.decode(&lua, &b),
            }
        });

        methods.add_async_method("close", |_, this, ()| async move {
            let taken = this.ws.borrow_mut().take(); // borrow released before the await
            if let Some(mut ws) = taken {
                let _ = ws.close(None).await;
            }
            Ok(())
        });
    }
}

/// Every option `websocket.connect` honors.
const CONNECT_OPTS: &[&str] = &["codec", "url"];

fn connect_fn(lua: &Lua) -> mlua::Result<Function> {
    lua.create_async_function(|lua, (ctx, opts): (Value, Option<Table>)| async move {
        super::runtime_only("websocket.connect")?;
        // The retired positional spelling, refused with the new one. See socket.rs's twin: this is
        // a FIRST-ARGUMENT change, which the closed-opts gate structurally cannot see.
        if let Value::String(url) = &ctx {
            return Err(err(format!(
                "websocket.connect(ctx, {{ url = {:?} }}): the url is now a named option and the \
                 context comes first, so the connection is closed with the scope instead of \
                 leaking until GC",
                url.to_string_lossy()
            )));
        }
        let opts = opts.ok_or_else(|| {
            err("websocket.connect(ctx, { url = \"ws://…\" }): the options table is required")
        })?;
        crate::opts::reject_unknown(&opts, CONNECT_OPTS, "websocket.connect")?;
        let url = opts
            .get::<Option<String>>("url")?
            .ok_or_else(|| err("websocket.connect(ctx, { url = \"ws://…\" }): url is required"))?;
        let codec = Codec::parse("websocket.connect", opts.get::<Option<Value>>("codec")?)?;
        if !url.starts_with("ws://") {
            return Err(err(format!(
                "websocket.connect: url must be ws:// (no TLS in v1), got {url:?}"
            )));
        }
        let (ws, _resp) = tokio_tungstenite::connect_async(&url)
            .await
            .map_err(|e| err(format!("websocket.connect {url}: {e}")))?;
        let ud = lua.create_userdata(ClientUd {
            ws: Rc::new(RefCell::new(Some(ws))),
            codec,
        })?;
        super::manage("websocket.connect", &ctx, &ud)?;
        Ok(ud)
    })
}

// ── the mock: terminate (and push — full duplex) ───────────────────────────────────────────────

#[derive(Default)]
struct MockState {
    stubs: Vec<(Vec<u8>, Option<Vec<u8>>)>,
    journal: Vec<super::wiretap::JournalRow>,
    on_connect: Option<Function>,
}

struct MockUd {
    url: String,
    state: Rc<RefCell<MockState>>,
    shutdown: RefCell<Option<tokio::sync::oneshot::Sender<()>>>,
}

struct WsStub {
    state: Rc<RefCell<MockState>>,
    idx: usize,
}

impl UserData for WsStub {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("reply", |_, this, data: mlua::String| {
            this.state.borrow_mut().stubs[this.idx].1 = Some(data.as_bytes().to_vec());
            Ok(())
        });
    }
}

/// The server side of one live connection, handed to `on_connect` so the mock can PUSH.
type WsServer = tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>;
type SharedSink =
    Rc<RefCell<Option<futures::stream::SplitSink<WsServer, Message>>>>;

struct ServerConnUd {
    sink: SharedSink,
}

impl UserData for ServerConnUd {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_async_method("send", |_, this, data: mlua::String| async move {
            let Some(mut sink) = this.sink.borrow_mut().take() else {
                return Err(err("send: connection is closed or busy"));
            };
            let text = String::from_utf8_lossy(&data.as_bytes()).to_string();
            let r = sink.send(Message::Text(text)).await;
            *this.sink.borrow_mut() = Some(sink);
            r.map_err(|e| err(format!("send: {e}")))
        });
    }
}

impl UserData for MockUd {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("url", |_, this| Ok(this.url.clone()));
        fields.add_field_method_get("endpoint", |_, this| Ok(this.url.clone()));
    }
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("on", |lua, this, turn: mlua::String| {
            let idx = {
                let mut s = this.state.borrow_mut();
                s.stubs.push((turn.as_bytes().to_vec(), None));
                s.stubs.len() - 1
            };
            lua.create_userdata(WsStub {
                state: this.state.clone(),
                idx,
            })
        });

        methods.add_method("on_connect", |_, this, f: Function| {
            this.state.borrow_mut().on_connect = Some(f);
            Ok(())
        });

        super::wiretap::add_received_method(methods);
        super::wiretap::add_shutdown_methods(methods);
    }
}

super::wiretap::impl_journal!(MockUd);
super::wiretap::impl_shutdown!(MockUd);

/// `websocket.mock` honors NO options — the constructor reads none at all, so every key an author
/// passed was dropped whole (docs/design/agent-ergonomics.md#module-opts-silently-ignored). An
/// empty accepted set says that out loud instead of accepting a table and ignoring it.
const MOCK_OPTS: &[&str] = &[];

/// Every option `websocket.proxy` honors.
const PROXY_OPTS: &[&str] = &["upstream"];

fn mock_fn(lua: &Lua) -> mlua::Result<Function> {
    lua.create_function(|lua, (ctx, opts): (Value, Option<Table>)| {
        if let Some(o) = &opts {
            crate::opts::reject_unknown(o, MOCK_OPTS, "websocket.mock")?;
        }
        let std_listener = std::net::TcpListener::bind(("127.0.0.1", 0))
            .map_err(|e| err(format!("websocket.mock: bind: {e}")))?;
        std_listener
            .set_nonblocking(true)
            .map_err(|e| err(format!("websocket.mock: set_nonblocking: {e}")))?;
        let port = std_listener
            .local_addr()
            .map_err(|e| err(format!("websocket.mock: local_addr: {e}")))?
            .port();
        let listener = tokio::net::TcpListener::from_std(std_listener)
            .map_err(|e| err(format!("websocket.mock: from_std: {e}")))?;

        let state: Rc<RefCell<MockState>> = Rc::default();
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();

        let accept_state = state.clone();
        super::wiretap::spawn_accept_loop(listener, rx, move |stream| {
            let conn_state = accept_state.clone();
            tokio::task::spawn_local(async move {
                let Ok(ws) = tokio_tungstenite::accept_async(stream).await else {
                    return;
                };
                let (sink, mut reader) = ws.split();
                let sink: SharedSink = Rc::new(RefCell::new(Some(sink)));

                // Full duplex: the on_connect script may push before any request.
                let hook = conn_state.borrow().on_connect.clone();
                if let Some(f) = hook {
                    let conn = ServerConnUd { sink: sink.clone() };
                    // An async Lua hook: `conn:send` awaits, so call async.
                    let _ = f.call_async::<()>(conn).await;
                }

                while let Some(Ok(m)) = reader.next().await {
                    let Some(turn) = msg_bytes(&m) else {
                        if matches!(m, Message::Close(_)) {
                            break;
                        }
                        continue;
                    };
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
                                s.journal.push(super::wiretap::JournalRow {
                                    data: turn,
                                    matched: false,
                                    source: "unmatched",
                                });
                                None
                            }
                        }
                    };
                    if let Some(r) = reply {
                        let Some(mut sk) = sink.borrow_mut().take() else { break };
                        let text = String::from_utf8_lossy(&r).to_string();
                        let sent = sk.send(Message::Text(text)).await;
                        *sink.borrow_mut() = Some(sk);
                        if sent.is_err() {
                            break;
                        }
                    }
                }
            });
        });

        let ud = lua.create_userdata(MockUd {
            url: format!("ws://127.0.0.1:{port}"),
            state,
            shutdown: RefCell::new(Some(tx)),
        })?;
        super::manage("websocket.mock", &ctx, &ud)?;
        Ok(ud)
    })
}

// ── the proxy: interpose (wiretap + faults) ────────────────────────────────────────────────────

#[derive(Default)]
struct WsProxyState {
    transcript: Vec<super::wiretap::TranscriptRow>,
    latency: Option<Duration>,
    dropped: bool,
}

struct WsProxyUd {
    url: String,
    state: Rc<RefCell<WsProxyState>>,
    shutdown: RefCell<Option<tokio::sync::oneshot::Sender<()>>>,
}

impl UserData for WsProxyUd {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("url", |_, this| Ok(this.url.clone()));
        fields.add_field_method_get("endpoint", |_, this| Ok(this.url.clone()));
    }
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        super::wiretap::add_transcript_method(methods);
        // The fault vocabulary rides the substrate — the ws proxy speaks the same verbs as socket.
        methods.add_method("latency", |_, this, d: String| {
            this.state.borrow_mut().latency =
                Some(parse_duration(&d).ok_or_else(|| err(format!("bad duration {d:?}")))?);
            Ok(())
        });
        methods.add_method("drop", |_, this, ()| {
            this.state.borrow_mut().dropped = true;
            Ok(())
        });
        super::wiretap::add_shutdown_methods(methods);
    }
}

super::wiretap::impl_transcript!(WsProxyUd);
super::wiretap::impl_shutdown!(WsProxyUd);

/// One direction of an interposed ws connection: forward each message turn, recording it and
/// applying the current faults (latency before delivery; drop severs).
type BoxSink = Box<dyn futures::Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin>;

async fn ws_pump(
    mut src: futures::stream::SplitStream<
        impl futures::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
    >,
    sink: Rc<RefCell<Option<BoxSink>>>,
    dir: &'static str,
    state: Rc<RefCell<WsProxyState>>,
) {
    while let Some(Ok(m)) = src.next().await {
        let Some(bytes) = msg_bytes(&m) else {
            if matches!(m, Message::Close(_)) {
                break;
            }
            continue;
        };
        let (latency, dropped) = {
            let s = state.borrow();
            (s.latency, s.dropped)
        };
        if dropped {
            break;
        }
        if let Some(l) = latency {
            tokio::time::sleep(l).await;
        }
        state.borrow_mut().transcript.push(super::wiretap::TranscriptRow {
            dir,
            data: bytes.clone(),
        });
        let Some(mut sk) = sink.borrow_mut().take() else { break };
        let text = String::from_utf8_lossy(&bytes).to_string();
        let sent = sk.send(Message::Text(text)).await;
        *sink.borrow_mut() = Some(sk);
        if sent.is_err() {
            break;
        }
    }
}

type DynSink = Rc<RefCell<Option<BoxSink>>>;

fn proxy_fn(lua: &Lua) -> mlua::Result<Function> {
    lua.create_function(|lua, (ctx, opts): (Value, Table)| {
        crate::opts::reject_unknown(&opts, PROXY_OPTS, "websocket.proxy")?;
        let upstream = opts
            .get::<Option<String>>("upstream")?
            .ok_or_else(|| err("websocket.proxy(ctx, { upstream = … }): upstream is required"))?;
        if !upstream.starts_with("ws://") {
            return Err(err(format!(
                "websocket.proxy: upstream must be ws:// (no TLS in v1), got {upstream:?}"
            )));
        }

        let std_listener = std::net::TcpListener::bind(("127.0.0.1", 0))
            .map_err(|e| err(format!("websocket.proxy: bind: {e}")))?;
        std_listener
            .set_nonblocking(true)
            .map_err(|e| err(format!("websocket.proxy: set_nonblocking: {e}")))?;
        let port = std_listener
            .local_addr()
            .map_err(|e| err(format!("websocket.proxy: local_addr: {e}")))?
            .port();
        let listener = tokio::net::TcpListener::from_std(std_listener)
            .map_err(|e| err(format!("websocket.proxy: from_std: {e}")))?;

        let state: Rc<RefCell<WsProxyState>> = Rc::default();
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let accept_state = state.clone();
        super::wiretap::spawn_accept_loop(listener, rx, move |stream| {
            let state = accept_state.clone();
            let upstream = upstream.clone();
            tokio::task::spawn_local(async move {
                let Ok(client) = tokio_tungstenite::accept_async(stream).await else { return };
                let Ok((up, _)) = tokio_tungstenite::connect_async(&upstream).await else { return };
                let (client_sink, client_stream) = client.split();
                let (up_sink, up_stream) = up.split();
                let client_sink: DynSink = Rc::new(RefCell::new(Some(Box::new(client_sink))));
                let up_sink: DynSink = Rc::new(RefCell::new(Some(Box::new(up_sink))));
                // up: client → upstream · down: upstream → client
                let a = ws_pump(client_stream, up_sink, "up", state.clone());
                let b = ws_pump(up_stream, client_sink, "down", state);
                tokio::join!(a, b);
            });
        });

        let ud = lua.create_userdata(WsProxyUd {
            url: format!("ws://127.0.0.1:{port}"),
            state,
            shutdown: RefCell::new(Some(tx)),
        })?;
        super::manage("websocket.proxy", &ctx, &ud)?;
        Ok(ud)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mlua::ObjectLike;

    /// The scope-teardown seam: a real ctx tears the mock down with the test scope; the stub
    /// only accepts the registration so `manage`'s contract is satisfied.
    struct StubCtx;
    impl UserData for StubCtx {
        fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
            methods.add_method("manage", |_, _, _ud: mlua::AnyUserData| Ok(()));
        }
    }

    /// The hosted harness every test here runs inside: a current-thread runtime + LocalSet
    /// (exactly how the engine hosts the module), one Lua with `websocket` installed and the
    /// stub ctx registered.
    fn harness() -> (tokio::runtime::Runtime, tokio::task::LocalSet, Lua) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let local = tokio::task::LocalSet::new();
        let lua = Lua::new();
        lua.globals().set("websocket", make(&lua).unwrap()).unwrap();
        lua.globals()
            .set("ctx", lua.create_userdata(StubCtx).unwrap())
            .unwrap();
        (rt, local, lua)
    }

    /// The whole transport through its own Lua surface, hosted under a LocalSet exactly as the
    /// engine hosts it: the ctx contract refuses a nil, on_connect pushes before any request
    /// (full duplex), a stubbed turn answers, an unmatched turn journals as unmatched (kept,
    /// not dropped), and recv after close-with-no-traffic times out by name.
    #[test]
    fn mock_and_driver_round_trip_message_turns() {
        let (rt, local, lua) = harness();
        local.block_on(&rt, async {
            let refused = lua
                .load(r#"websocket.mock(nil)"#)
                .exec_async()
                .await
                .unwrap_err()
                .to_string();
            assert!(refused.contains("pass the test or fixture context"), "{refused}");

            let outcome: Table = lua
                .load(
                    r#"
                    local m = websocket.mock(ctx)
                    m:on_connect(function(conn) conn:send("welcome") end)
                    m:on("ping"):reply("pong")
                    local c = websocket.connect(ctx, { url = m.url })
                    local pushed = c:recv({ timeout = "5s" })
                    c:send("ping")
                    local answered = c:recv({ timeout = "5s" })
                    c:send("stray")
                    return { m = m, c = c, url = m.url, pushed = pushed, answered = answered }
                    "#,
                )
                .eval_async()
                .await
                .unwrap();
            assert_eq!(outcome.get::<String>("pushed").unwrap(), "welcome");
            assert_eq!(outcome.get::<String>("answered").unwrap(), "pong");
            assert!(outcome.get::<String>("url").unwrap().starts_with("ws://127.0.0.1:"));

            // The stray turn journals as unmatched. The reader task shares this thread — yield
            // to it until the row lands (bounded, so a regression fails rather than hangs).
            let m: mlua::AnyUserData = outcome.get("m").unwrap();
            let mut rows: Option<Table> = None;
            for _ in 0..100 {
                let got: Table = m.call_method("received", ()).unwrap();
                if got.raw_len() == 2 {
                    rows = Some(got);
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            let rows = rows.expect("both turns journal within the bound");
            let first: Table = rows.get(1).unwrap();
            let second: Table = rows.get(2).unwrap();
            assert_eq!(first.get::<String>("data").unwrap(), "ping");
            assert!(first.get::<bool>("matched").unwrap());
            assert_eq!(first.get::<String>("source").unwrap(), "stub");
            assert_eq!(second.get::<String>("data").unwrap(), "stray");
            assert!(!second.get::<bool>("matched").unwrap());
            assert_eq!(second.get::<String>("source").unwrap(), "unmatched");

            // Close is clean, and a recv on the closed driver says so rather than hanging.
            let c: mlua::AnyUserData = outcome.get("c").unwrap();
            c.call_async_method::<()>("close", ()).await.unwrap();
            let err = c
                .call_async_method::<mlua::String>("recv", ())
                .await
                .unwrap_err()
                .to_string();
            assert!(err.contains("closed"), "{err}");
        });
    }

    /// The interpose posture through the same harness: the proxy in front of the mock passes
    /// traffic untouched, and its transcript records direction-tagged turns — up for the
    /// client's, down for the upstream's. A missing upstream is refused at the call site.
    #[test]
    fn proxy_wiretaps_direction_tagged_turns() {
        let (rt, local, lua) = harness();
        local.block_on(&rt, async {
            let refused = lua
                .load(r#"websocket.proxy(ctx, {})"#)
                .exec_async()
                .await
                .unwrap_err()
                .to_string();
            assert!(refused.contains("upstream is required"), "{refused}");

            let outcome: Table = lua
                .load(
                    r#"
                    local m = websocket.mock(ctx)
                    m:on("ping"):reply("pong")
                    local p = websocket.proxy(ctx, { upstream = m.url })
                    local c = websocket.connect(ctx, { url = p.url })
                    c:send("ping")
                    local answered = c:recv({ timeout = "5s" })
                    return { p = p, url = p.url, answered = answered }
                    "#,
                )
                .eval_async()
                .await
                .unwrap();
            assert_eq!(outcome.get::<String>("answered").unwrap(), "pong", "traffic flows untouched");
            assert!(outcome.get::<String>("url").unwrap().starts_with("ws://"), "endpoint symmetry");

            let p: mlua::AnyUserData = outcome.get("p").unwrap();
            let mut log: Option<Table> = None;
            for _ in 0..100 {
                let got: Table = p.call_method("transcript", ()).unwrap();
                if got.raw_len() == 2 {
                    log = Some(got);
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            let log = log.expect("both directions transcribe within the bound");
            let up: Table = log.get(1).unwrap();
            let down: Table = log.get(2).unwrap();
            assert_eq!(up.get::<String>("dir").unwrap(), "up");
            assert_eq!(up.get::<String>("data").unwrap(), "ping");
            assert_eq!(down.get::<String>("dir").unwrap(), "down");
            assert_eq!(down.get::<String>("data").unwrap(), "pong");
        });
    }
}
