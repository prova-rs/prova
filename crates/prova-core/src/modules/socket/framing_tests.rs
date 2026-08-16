//! `Framing` — where a byte stream becomes messages (see the parent module).

use super::*;

fn lua() -> Lua {
    Lua::new()
}

fn framing_from(l: &Lua, build: impl FnOnce(&Table)) -> mlua::Result<Framing> {
    let t = l.create_table().unwrap();
    build(&t);
    Framing::parse(Some(Value::Table(t)))
}

/// Framing is where a byte stream becomes messages, so a wrong boundary does not error — it
/// hands the proof a DIFFERENT message and lets it assert confidently on the wrong bytes.
/// The length prefix is big-endian and exactly `n` wide; both halves matter, because a reader
/// that disagrees about either resynchronizes on garbage.
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
    assert!(Framing::Raw.encode(b"").is_empty(), "raw is the one with no envelope");
}

#[test]
fn line_and_delimiter_append_their_terminator_and_raw_appends_nothing() {
    assert_eq!(Framing::Line.encode(b"a"), b"a\n".to_vec());
    assert_eq!(Framing::Delimiter(b"\r\n".to_vec()).encode(b"a"), b"a\r\n".to_vec());
    assert_eq!(Framing::Raw.encode(b"a"), b"a".to_vec());
}

/// The two table forms are alternatives, not a combination: a table naming both has no
/// defensible reading, and silently preferring one would frame every message the other way.
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
        assert!(
            got.unwrap_err().to_string().contains("1..=8"),
            "…naming the range"
        );
    }
    // An empty delimiter would match at every position, so it is refused too.
    assert!(framing_from(&l, |t| t.set("delimiter", "").unwrap()).is_err());
}

/// Absent framing is RAW — the escape hatch where `send` writes verbatim — and the only string
/// spelling is "line". An unknown one names the alternatives rather than falling back, since a
/// silent fallback to raw would send every message unframed.
#[test]
fn absent_framing_is_raw_and_an_unknown_name_is_refused() {
    let l = lua();
    assert!(Framing::parse(None).unwrap().is_raw());
    assert!(Framing::parse(Some(Value::Nil)).unwrap().is_raw());

    let line = Framing::parse(Some(Value::String(l.create_string("line").unwrap())));
    assert!(matches!(line.unwrap(), Framing::Line));

    let bogus = Framing::parse(Some(Value::String(l.create_string("lines").unwrap())));
    let err = bogus.unwrap_err().to_string();
    assert!(err.contains("length_prefixed"), "the alternatives are named: {err}");
    assert!(Framing::parse(Some(Value::Integer(4))).is_err(), "a number is not a framing");
}

/// The address is the one string that decides which transport is used at all.
#[test]
fn an_address_names_its_transport_or_is_refused() {
    assert!(matches!(parse_addr("tcp://127.0.0.1:8080").unwrap(), Addr::Tcp(hp) if hp == "127.0.0.1:8080"));
    #[cfg(unix)]
    assert!(matches!(parse_addr("unix:///tmp/s.sock").unwrap(), Addr::Unix(_)));

    for bad in ["127.0.0.1:8080", "http://x", "", "tcp:/oops"] {
        match parse_addr(bad) {
            Ok(_) => panic!("{bad:?} should not parse as an address"),
            Err(e) => assert!(
                e.to_string().contains("tcp://host:port"),
                "{bad:?} is refused with the shape it wanted: {e}"
            ),
        }
    }
}
