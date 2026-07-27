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
use mlua::{Function, Lua, ObjectLike, Table, UserData, UserDataFields, UserDataMethods, Value};
use tokio_tungstenite::tungstenite::Message;

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
}

impl UserData for ClientUd {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_async_method("send", |_, this, data: mlua::String| async move {
            let Some(mut ws) = this.ws.borrow_mut().take() else {
                return Err(err("send: connection is closed or busy"));
            };
            let text = String::from_utf8_lossy(&data.as_bytes()).to_string();
            let r = ws.send(Message::Text(text)).await;
            *this.ws.borrow_mut() = Some(ws);
            r.map_err(|e| err(format!("send: {e}")))
        });

        methods.add_async_method("recv", |lua, this, opts: Option<Table>| async move {
            let dur = match &opts {
                Some(t) => match t.get::<Option<String>>("timeout")? {
                    Some(s) => parse_duration(&s).ok_or_else(|| err(format!("bad timeout {s:?}")))?,
                    None => DEFAULT_RECV_TIMEOUT,
                },
                None => DEFAULT_RECV_TIMEOUT,
            };
            let Some(mut ws) = this.ws.borrow_mut().take() else {
                return Err(err("recv: connection is closed or busy"));
            };
            let res = tokio::time::timeout(dur, async {
                loop {
                    match ws.next().await {
                        None => return Err(err("recv: connection closed")),
                        Some(Err(e)) => return Err(err(format!("recv: {e}"))),
                        Some(Ok(m)) => {
                            if let Some(b) = msg_bytes(&m) {
                                return Ok(b);
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
                Err(_) => Err(err(format!("recv: timed out after {dur:?}"))),
                Ok(Err(e)) => Err(e),
                Ok(Ok(b)) => Ok(lua.create_string(&b)?),
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

fn connect_fn(lua: &Lua) -> mlua::Result<Function> {
    lua.create_async_function(|_, url: String| async move {
        if !url.starts_with("ws://") {
            return Err(err(format!(
                "websocket.connect: url must be ws:// (no TLS in v1), got {url:?}"
            )));
        }
        let (ws, _resp) = tokio_tungstenite::connect_async(&url)
            .await
            .map_err(|e| err(format!("websocket.connect {url}: {e}")))?;
        Ok(ClientUd {
            ws: Rc::new(RefCell::new(Some(ws))),
        })
    })
}

// ── the mock: terminate (and push — full duplex) ───────────────────────────────────────────────

struct WsRecorded {
    data: Vec<u8>,
    matched: bool,
    source: &'static str,
}

#[derive(Default)]
struct MockState {
    stubs: Vec<(Vec<u8>, Option<Vec<u8>>)>,
    journal: Vec<WsRecorded>,
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

        methods.add_method("received", |lua, this, filter: Option<Value>| {
            let entries: Vec<Table> = {
                let s = this.state.borrow();
                s.journal
                    .iter()
                    .enumerate()
                    .map(|(i, r)| {
                        let t = lua.create_table()?;
                        t.set("seq", i + 1)?;
                        t.set("data", lua.create_string(&r.data)?)?;
                        t.set("matched", r.matched)?;
                        t.set("source", r.source)?;
                        Ok(t)
                    })
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

        methods.add_method("stop", |_, this, ()| {
            if let Some(tx) = this.shutdown.borrow_mut().take() {
                let _ = tx.send(());
            }
            Ok(())
        });
        methods.add_method("close", |_, this, ()| {
            if let Some(tx) = this.shutdown.borrow_mut().take() {
                let _ = tx.send(());
            }
            Ok(())
        });
    }
}

fn mock_fn(lua: &Lua) -> mlua::Result<Function> {
    lua.create_function(|lua, (ctx, _opts): (Value, Option<Table>)| {
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
        let (tx, mut rx) = tokio::sync::oneshot::channel::<()>();

        let accept_state = state.clone();
        tokio::task::spawn_local(async move {
            loop {
                tokio::select! {
                    _ = &mut rx => break,
                    accepted = listener.accept() => {
                        let Ok((stream, _peer)) = accepted else { break };
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
                                            s.journal.push(WsRecorded {
                                                data: turn,
                                                matched: true,
                                                source: "stub",
                                            });
                                            r
                                        }
                                        None => {
                                            s.journal.push(WsRecorded {
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
                    }
                }
            }
        });

        let ud = lua.create_userdata(MockUd {
            url: format!("ws://127.0.0.1:{port}"),
            state,
            shutdown: RefCell::new(Some(tx)),
        })?;
        match ctx {
            Value::UserData(c) => {
                let _: Value = c.call_method("manage", &ud)?;
            }
            _ => {
                return Err(err(
                    "websocket.mock(ctx): pass the test or fixture context (`t` / `ctx`)",
                ))
            }
        }
        Ok(ud)
    })
}

// ── the proxy: interpose (wiretap + faults) ────────────────────────────────────────────────────

struct WsTurnRec {
    dir: &'static str,
    data: Vec<u8>,
}

#[derive(Default)]
struct WsProxyState {
    transcript: Vec<WsTurnRec>,
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
    }
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("transcript", |lua, this, ()| {
            let out = lua.create_table()?;
            let s = this.state.borrow();
            for (i, rec) in s.transcript.iter().enumerate() {
                let t = lua.create_table()?;
                t.set("seq", i + 1)?;
                t.set("dir", rec.dir)?;
                t.set("data", lua.create_string(&rec.data)?)?;
                out.set(i + 1, t)?;
            }
            Ok(out)
        });
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
        methods.add_method("stop", |_, this, ()| {
            if let Some(tx) = this.shutdown.borrow_mut().take() {
                let _ = tx.send(());
            }
            Ok(())
        });
        methods.add_method("close", |_, this, ()| {
            if let Some(tx) = this.shutdown.borrow_mut().take() {
                let _ = tx.send(());
            }
            Ok(())
        });
    }
}

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
        state.borrow_mut().transcript.push(WsTurnRec {
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
        let (tx, mut rx) = tokio::sync::oneshot::channel::<()>();
        let accept_state = state.clone();

        tokio::task::spawn_local(async move {
            loop {
                tokio::select! {
                    _ = &mut rx => break,
                    accepted = listener.accept() => {
                        let Ok((stream, _peer)) = accepted else { break };
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
                    }
                }
            }
        });

        let ud = lua.create_userdata(WsProxyUd {
            url: format!("ws://127.0.0.1:{port}"),
            state,
            shutdown: RefCell::new(Some(tx)),
        })?;
        match ctx {
            Value::UserData(c) => {
                let _: Value = c.call_method("manage", &ud)?;
            }
            _ => {
                return Err(err(
                    "websocket.proxy(ctx): pass the test or fixture context (`t` / `ctx`)",
                ))
            }
        }
        Ok(ud)
    })
}
