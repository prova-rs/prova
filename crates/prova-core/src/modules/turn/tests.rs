//! The turn model's unit tests: framing boundaries, the codec dial, and the `where` selector.
//!
//! Framing is where a byte stream becomes messages, so a wrong boundary does not error — it hands
//! the proof a DIFFERENT message and lets it assert confidently on the wrong bytes. That is why
//! these are exhaustive out of proportion to their size.

use super::*;

fn lua() -> Lua {
    Lua::new()
}

fn framing_from(l: &Lua, build: impl FnOnce(&Table)) -> mlua::Result<Framing> {
    let t = l.create_table().unwrap();
    build(&t);
    Framing::parse("socket", Some(Value::Table(t)))
}

fn s(l: &Lua, v: &str) -> Option<Value> {
    Some(Value::String(l.create_string(v).unwrap()))
}

// ── framing: the wire envelope ─────────────────────────────────────────────────────────────────

/// The length prefix is big-endian and exactly `n` wide; both halves matter, because a reader that
/// disagrees about either resynchronizes on garbage.
#[test]
fn a_length_prefix_is_big_endian_and_exactly_as_wide_as_declared() {
    assert_eq!(Framing::LengthPrefixed(1).encode(b"hi"), vec![2, b'h', b'i']);
    assert_eq!(Framing::LengthPrefixed(2).encode(b"hi"), vec![0, 2, b'h', b'i']);
    assert_eq!(
        Framing::LengthPrefixed(4).encode(b"hi"),
        vec![0, 0, 0, 2, b'h', b'i'],
        "the width is the declared one, not the smallest that fits"
    );

    // A payload whose length needs more than one byte, so byte ORDER is observable.
    let long = vec![b'x'; 300];
    let framed = Framing::LengthPrefixed(2).encode(&long);
    assert_eq!(&framed[..2], &[0x01, 0x2c], "300 = 0x012C, most significant first");
    assert_eq!(framed.len(), 302);
}

/// An empty payload still gets a header — a frame that vanishes when empty desynchronizes the
/// stream for every message after it.
#[test]
fn an_empty_payload_is_still_a_frame() {
    assert_eq!(Framing::LengthPrefixed(2).encode(b""), vec![0, 0]);
    assert_eq!(Framing::Line.encode(b""), vec![b'\n']);
    assert_eq!(Framing::Delimiter(b"||".to_vec()).encode(b""), b"||".to_vec());
    assert_eq!(Framing::ContentLength.encode(b""), b"Content-Length: 0\r\n\r\n".to_vec());
    assert!(Framing::Raw.encode(b"").is_empty(), "raw is the one with no envelope");
}

#[test]
fn line_and_delimiter_append_their_terminator_and_raw_appends_nothing() {
    assert_eq!(Framing::Line.encode(b"a"), b"a\n".to_vec());
    assert_eq!(Framing::Delimiter(b"\r\n".to_vec()).encode(b"a"), b"a\r\n".to_vec());
    assert_eq!(Framing::Raw.encode(b"a"), b"a".to_vec());
}

/// The two table forms are alternatives, not a combination: a table naming both has no defensible
/// reading, and silently preferring one would frame every message the other way.
#[test]
fn a_framing_table_names_exactly_one_strategy() {
    let l = lua();
    assert!(matches!(
        framing_from(&l, |t| t.set("length_prefixed", 4).unwrap()).unwrap(),
        Framing::LengthPrefixed(4)
    ));
    assert!(matches!(
        framing_from(&l, |t| t.set("delimiter", "||").unwrap()).unwrap(),
        Framing::Delimiter(_)
    ));

    let both = framing_from(&l, |t| {
        t.set("length_prefixed", 4).unwrap();
        t.set("delimiter", "||").unwrap();
    });
    assert!(both.is_err(), "both is refused rather than ranked");
    let neither = framing_from(&l, |_| {});
    assert!(neither.is_err(), "and so is neither");
}

/// A prefix wider than 8 bytes cannot be read back into a u64, and a zero-width one frames
/// nothing. Both are refused at parse time rather than producing a stream nobody can decode.
#[test]
fn a_length_prefix_width_outside_one_to_eight_is_refused() {
    let l = lua();
    for width in [0usize, 9, 64] {
        let got = framing_from(&l, |t| t.set("length_prefixed", width).unwrap());
        assert!(got.is_err(), "width {width} must be refused");
        assert!(got.unwrap_err().to_string().contains("1..=8"), "…naming the range");
    }
    // An empty delimiter would match at every position, so it is refused too.
    assert!(framing_from(&l, |t| t.set("delimiter", "").unwrap()).is_err());
}

/// Absent framing is RAW — the escape hatch where `send` writes verbatim. An unknown name names
/// the alternatives rather than falling back, since a silent fallback to raw would send every
/// message unframed.
#[test]
fn absent_framing_is_raw_and_an_unknown_name_is_refused() {
    let l = lua();
    assert!(Framing::parse("socket", None).unwrap().is_raw());
    assert!(Framing::parse("socket", Some(Value::Nil)).unwrap().is_raw());
    assert!(matches!(Framing::parse("socket", s(&l, "line")).unwrap(), Framing::Line));
    assert!(matches!(
        Framing::parse("socket", s(&l, "content_length")).unwrap(),
        Framing::ContentLength
    ));

    let err = Framing::parse("socket", s(&l, "lines")).unwrap_err().to_string();
    assert!(err.contains("length_prefixed"), "the alternatives are named: {err}");
    assert!(err.contains("content_length"), "…including the newest one: {err}");
    assert!(
        Framing::parse("socket", Some(Value::Integer(4))).is_err(),
        "a number is not a framing"
    );
}

// ── framing: the scanner ───────────────────────────────────────────────────────────────────────

/// One scanner serves the blocking reader and the proxy pump. A frame is handed over only when it
/// is WHOLE — the partial case has to return None rather than a short payload, because a short
/// payload is a silently corrupted turn that every assertion downstream then trusts.
#[test]
fn a_frame_is_taken_only_when_whole_and_leftovers_survive() {
    let mut buf = b"one\ntwo\nthr".to_vec();
    assert_eq!(Framing::Line.take_frame(&mut buf).unwrap(), b"one".to_vec());
    assert_eq!(Framing::Line.take_frame(&mut buf).unwrap(), b"two".to_vec());
    assert!(Framing::Line.take_frame(&mut buf).is_none(), "a partial line is not a frame");
    assert_eq!(buf, b"thr".to_vec(), "and its bytes are still buffered");

    let mut lp = vec![0, 3, b'a', b'b'];
    assert!(Framing::LengthPrefixed(2).take_frame(&mut lp).is_none());
    lp.push(b'c');
    assert_eq!(Framing::LengthPrefixed(2).take_frame(&mut lp).unwrap(), b"abc".to_vec());
    assert!(lp.is_empty());

    // Raw has no frames by construction — the caller reads exact byte counts instead.
    let mut raw = b"anything".to_vec();
    assert!(Framing::Raw.take_frame(&mut raw).is_none());
    assert_eq!(raw, b"anything".to_vec(), "and nothing is consumed");
}

/// LSP's envelope: a header block, `\r\n\r\n`, then exactly `Content-Length` bytes. Real servers
/// send other headers alongside it and disagree about case, so both have to survive; and the body
/// length must come from the header rather than from a scan, since a JSON body legitimately
/// contains `\r\n\r\n` inside a string.
#[test]
fn content_length_reads_the_declared_body_past_other_headers() {
    let mut buf = b"Content-Length: 5\r\n\r\nhello".to_vec();
    assert_eq!(Framing::ContentLength.take_frame(&mut buf).unwrap(), b"hello".to_vec());
    assert!(buf.is_empty());

    let mut multi =
        b"Content-Type: application/vscode-jsonrpc\r\ncontent-length: 2\r\n\r\nhi".to_vec();
    assert_eq!(
        Framing::ContentLength.take_frame(&mut multi).unwrap(),
        b"hi".to_vec(),
        "other headers are skipped and the name is case-insensitive"
    );

    // A body carrying the header terminator inside it: the declared length is authoritative.
    let body = b"{\"s\":\"a\\r\\n\\r\\nb\"}";
    let mut framed = Framing::ContentLength.encode(body);
    framed.extend_from_slice(b"Content-Length: 1\r\n\r\nx");
    assert_eq!(Framing::ContentLength.take_frame(&mut framed).unwrap(), body.to_vec());
    assert_eq!(
        Framing::ContentLength.take_frame(&mut framed).unwrap(),
        b"x".to_vec(),
        "and the stream stays synchronized for the next frame"
    );
}

/// A header block with no `Content-Length` would otherwise wedge the reader forever, waiting on a
/// length that is never coming. It yields no frame — and, crucially, consumes nothing, so the
/// caller's read loop hits EOF and reports rather than spinning.
#[test]
fn a_content_length_block_without_the_header_yields_nothing() {
    let mut buf = b"Content-Type: text/plain\r\n\r\nbody".to_vec();
    assert!(Framing::ContentLength.take_frame(&mut buf).is_none());
    assert_eq!(buf.len(), b"Content-Type: text/plain\r\n\r\nbody".len(), "nothing consumed");

    let mut partial = b"Content-Length: 10\r\n\r\nshort".to_vec();
    assert!(
        Framing::ContentLength.take_frame(&mut partial).is_none(),
        "a body shorter than declared is not yet a frame"
    );
}

// ── codec: turn ↔ value ────────────────────────────────────────────────────────────────────────

/// The default is the identity. Making `json` opt-in matters because a decode failure on a stream
/// that was never json would report as "turn is not json" for every turn — a confusing way to say
/// "you set the wrong dial".
#[test]
fn the_default_codec_is_bytes_and_an_unknown_one_is_refused() {
    let l = lua();
    assert_eq!(Codec::parse("stdio.spawn", None).unwrap(), Codec::Bytes);
    assert_eq!(Codec::parse("stdio.spawn", s(&l, "bytes")).unwrap(), Codec::Bytes);
    assert_eq!(Codec::parse("stdio.spawn", s(&l, "json")).unwrap(), Codec::Json);

    let err = Codec::parse("stdio.spawn", s(&l, "JSON")).unwrap_err().to_string();
    assert!(err.contains("\"bytes\" and \"json\""), "the alternatives are named: {err}");
}

#[test]
fn a_json_turn_round_trips_through_the_codec() {
    let l = lua();
    let t = l.create_table().unwrap();
    t.set("id", 7).unwrap();
    t.set("method", "tools/call").unwrap();

    let wire = Codec::Json.encode(&l, &Value::Table(t)).unwrap();
    let back = Codec::Json.decode(&l, &wire).unwrap();
    let Value::Table(back) = back else { panic!("a json object decodes to a table") };
    assert_eq!(back.get::<i64>("id").unwrap(), 7);
    assert_eq!(back.get::<String>("method").unwrap(), "tools/call");
}

/// Sending a table down a byte stream would otherwise stringify into `table: 0x…` and go out on
/// the wire — a turn the peer cannot read, from a proof that looks correct.
#[test]
fn a_table_on_a_byte_stream_is_refused_naming_the_codec() {
    let l = lua();
    let t = l.create_table().unwrap();
    let err = Codec::Bytes.encode(&l, &Value::Table(t)).unwrap_err().to_string();
    assert!(err.contains("codec = \"json\""), "the cure is named: {err}");
}

/// A decode failure quotes the offending turn. Without it the message is "not json" about a stream
/// the author cannot see — and the usual cause is a server writing a log line to stdout, which the
/// quoted bytes identify instantly.
#[test]
fn a_non_json_turn_fails_loudly_and_quotes_itself() {
    let l = lua();
    let err = Codec::Json.decode(&l, b"INFO starting up").unwrap_err().to_string();
    assert!(err.contains("INFO starting up"), "the turn is quoted: {err}");
}

// ── selector: which turn ───────────────────────────────────────────────────────────────────────

#[test]
fn a_shape_selector_subset_matches_the_decoded_turn() {
    let l = lua();
    let shape = l.create_table().unwrap();
    shape.set("id", 2).unwrap();
    let sel = Selector::parse("recv", Codec::Json, Some(Value::Table(shape))).unwrap();

    let hit = Codec::Json.decode(&l, br#"{"id":2,"result":{"ok":true}}"#).unwrap();
    let miss = Codec::Json.decode(&l, br#"{"id":1,"result":{}}"#).unwrap();
    let notification = Codec::Json.decode(&l, br#"{"method":"log/message"}"#).unwrap();

    assert!(sel.accepts(&hit).unwrap(), "the reply matches on the field named");
    assert!(!sel.accepts(&miss).unwrap(), "another id does not");
    assert!(!sel.accepts(&notification).unwrap(), "and an id-less notification does not");
}

/// The negative control the shape test needs: with no selector every turn matches, so the test
/// above is measuring the matcher rather than an accident of ordering.
#[test]
fn an_absent_selector_accepts_the_next_turn_whatever_it_is() {
    let l = lua();
    let sel = Selector::parse("recv", Codec::Json, None).unwrap();
    assert!(sel.is_any());
    for raw in [&br#"{"id":1}"#[..], &br#"{"method":"x"}"#[..], b"3"] {
        let turn = Codec::Json.decode(&l, raw).unwrap();
        assert!(sel.accepts(&turn).unwrap());
    }
}

#[test]
fn a_predicate_selector_runs_over_the_turn() {
    let l = lua();
    let f: mlua::Function = l
        .load(r#"function(turn) return type(turn) == "string" and turn:match("^ready") ~= nil end"#)
        .eval()
        .unwrap();
    let sel = Selector::parse("recv", Codec::Bytes, Some(Value::Function(f))).unwrap();

    let ready = Codec::Bytes.decode(&l, b"ready to serve").unwrap();
    let other = Codec::Bytes.decode(&l, b"still booting").unwrap();
    assert!(sel.accepts(&ready).unwrap());
    assert!(!sel.accepts(&other).unwrap());
}

/// A table `where` over byte turns can only ever match nothing, so it would present as a timeout —
/// the single least informative failure this API can produce. Refusing names both cures.
#[test]
fn a_shape_selector_over_byte_turns_is_refused_not_left_to_time_out() {
    let l = lua();
    let shape = l.create_table().unwrap();
    shape.set("id", 1).unwrap();
    let err = Selector::parse("recv", Codec::Bytes, Some(Value::Table(shape)))
        .unwrap_err()
        .to_string();
    assert!(err.contains("codec = \"json\""), "the first cure is named: {err}");
    assert!(err.contains("function predicate"), "the second cure too: {err}");

    let err = Selector::parse("recv", Codec::Json, Some(Value::Integer(3)))
        .unwrap_err()
        .to_string();
    assert!(err.contains("table"), "a scalar `where` is refused: {err}");
}
