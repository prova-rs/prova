//! `socket`'s unit tests, in their own file: the module sits near the file-size limit, and the
//! tests are the part that can move without splitting the implementation. (The framing tests moved
//! again in 2026-08 — to `turn/tests.rs`, with the type.)

use super::*;
use super::super::turn::read_exact_buffered;

/// recv's arity IS the framing: raw connections read exact byte counts (n required),
/// framed connections read whole turns (a byte count is refused, taught) — and the
/// timeout opt parses through either form.
#[test]
fn recv_args_follow_the_framing() {
    let lua = Lua::new();
    let args = |f: &Framing, a: Option<Value>| recv_args(f, Codec::Bytes, a, None);

    let got = args(&Framing::Raw, Some(Value::Integer(64))).unwrap();
    assert_eq!((got.want, got.timeout), (64, DEFAULT_IO_TIMEOUT));
    assert!(args(&Framing::Raw, None).is_err(), "raw without n");
    assert!(args(&Framing::Raw, Some(Value::Integer(0))).is_err(), "zero bytes");

    let got = args(&Framing::Line, None).unwrap();
    assert_eq!((got.want, got.timeout), (0, DEFAULT_IO_TIMEOUT));
    assert!(got.selector.is_any(), "no `where` reads the next turn, whatever it is");
    assert!(
        args(&Framing::Line, Some(Value::Integer(64))).is_err(),
        "framed refuses a byte count"
    );
    let opts = lua.create_table().unwrap();
    opts.set("timeout", "250ms").unwrap();
    let got = args(&Framing::Line, Some(Value::Table(opts))).unwrap();
    assert_eq!(got.timeout, Duration::from_millis(250));
    let opts = lua.create_table().unwrap();
    opts.set("timeout", "eleventy").unwrap();
    assert!(args(&Framing::Line, Some(Value::Table(opts))).is_err());
}

/// `recv` is a closed opts surface like every other. It matters more here than most: a
/// dropped `where` still RETURNS a turn — the wrong one — and the proof asserts on it
/// confidently. A dropped `timeout` is the unbounded read this whole API exists to prevent.
#[test]
fn recv_refuses_an_option_it_cannot_honor() {
    let lua = Lua::new();
    let opts = lua.create_table().unwrap();
    opts.set("wehre", "x").unwrap();
    let e = recv_args(&Framing::Line, Codec::Json, Some(Value::Table(opts)), None)
        .unwrap_err()
        .to_string();
    // The refusal is the contract; the "did you mean" is a bonus `suggest::nearest` withholds
    // when nothing is close enough, and a transposition is past its bar. What must hold is
    // that the bad key is NAMED and the accepted set is listed — that is the one jump to a fix.
    assert!(e.contains("wehre"), "the key prova cannot honor is named: {e}");
    assert!(e.contains("timeout, where"), "and the accepted set is listed: {e}");
}

/// `where` selects among TURNS. On an unframed connection there are none, so the option can
/// only ever be a no-op — and a no-op filter reads as configured, which is the failure the
/// closed-opts doctrine exists to prevent.
#[test]
fn where_on_an_unframed_connection_is_refused() {
    let lua = Lua::new();
    let f: mlua::Function = lua.load("function() return true end").eval().unwrap();
    let opts = lua.create_table().unwrap();
    opts.set("where", f).unwrap();
    let e = recv_args(&Framing::Raw, Codec::Bytes, Some(Value::Integer(4)), Some(opts))
        .unwrap_err()
        .to_string();
    assert!(e.contains("set framing"), "the cure is named: {e}");
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
                local c = socket.connect(ctx, { addr = srv.addr })
                c:send("\1\2\3")
                local conn = srv:accept()
                local raw = conn:recv(3, { timeout = "5s" })
                conn:send("\4")
                local back = c:recv(1, { timeout = "5s" })
                c:close()

                -- terminate: a framed mock answers turns; strays journal as unmatched
                local m = socket.mock(ctx, { addr = "tcp://127.0.0.1:0", framing = "line" })
                m:on("PING"):reply("PONG")
                local mc = socket.connect(ctx, { addr = m.addr, framing = "line" })
                mc:send("PING")
                local answered = mc:recv({ timeout = "5s" })
                mc:send("STRAY")

                -- interpose: the proxy wiretaps in front of the mock
                local p = socket.proxy(ctx, { upstream = m.addr, framing = "line" })
                local pc = socket.connect(ctx, { addr = p.addr, framing = "line" })
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
