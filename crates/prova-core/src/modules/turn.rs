//! The shared TURN model — the substrate every stream transport reads its conversation through
//! (docs/plans/stdio-transport.md §5, docs/design/mocks-proxies-drivers.md).
//!
//! Three orthogonal questions, three types, so no transport has to answer any of them twice:
//!
//! - **`Framing`** — how bytes become TURNS. A raw byte stream has no natural "request" unit, so
//!   matching, transcripts and cassettes all need one imposed: a newline, a delimiter, a length
//!   prefix, or an LSP-shaped `Content-Length` header.
//! - **`Codec`** — how a turn becomes a VALUE. `bytes` (the default) hands the payload over as a
//!   Lua string; `json` decodes it, which is what lets a caller select a turn by its *fields*.
//! - **`Selector`** — how a caller says WHICH turn it wants. The same structural-subset matcher as
//!   `:matches`, `:on` and `received(filter)` (api-freeze §3), so a proof learns one grammar and
//!   spends it everywhere.
//!
//! The split matters most for the third: `recv{ where = { id = 3 } }` on a JSON-RPC stream is only
//! expressible because framing and codec are separate dials. Fuse them and every consumer writes
//! its own read-until-the-id-matches loop — which is exactly what drove
//! `agent-ergonomics.md#stdio-cannot-drive-a-conversational-sut` into a Python co-process.

use mlua::{Lua, Table, Value};
use tokio::io::{AsyncRead, AsyncReadExt};

/// What turns bytes into matchable TURNS. `Raw` is the absence of framing: `send` writes bytes
/// verbatim and `recv(n)` reads exact counts — the driver-level escape hatch.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Framing {
    Raw,
    Line,
    LengthPrefixed(usize),
    Delimiter(Vec<u8>),
    /// `Content-Length: N\r\n\r\n<body>` — LSP and DAP. Header names are matched
    /// case-insensitively and any other headers in the block are skipped, because a real language
    /// server sends `Content-Type` too.
    ContentLength,
}

/// The `\r\n\r\n` that ends a `Content-Length` header block.
const HEADER_END: &[u8] = b"\r\n\r\n";

impl Framing {
    pub(super) fn parse(site: &str, v: Option<Value>) -> mlua::Result<Framing> {
        match v {
            None | Some(Value::Nil) => Ok(Framing::Raw),
            Some(Value::String(s)) => match s.to_string_lossy().as_ref() {
                "line" => Ok(Framing::Line),
                "content_length" => Ok(Framing::ContentLength),
                other => Err(err(format!(
                    "{site}: unknown framing {other:?} (string framings are \"line\" and \
                     \"content_length\"; tables are {{ length_prefixed = n }} or \
                     {{ delimiter = \"…\" }})"
                ))),
            },
            Some(Value::Table(t)) => {
                let lp = t.get::<Option<usize>>("length_prefixed")?;
                let delim = t.get::<Option<mlua::String>>("delimiter")?;
                match (lp, delim) {
                    (Some(n), None) if (1..=8).contains(&n) => Ok(Framing::LengthPrefixed(n)),
                    (Some(n), None) => Err(err(format!(
                        "{site}: length_prefixed must be 1..=8 bytes, got {n}"
                    ))),
                    (None, Some(d)) if !d.as_bytes().is_empty() => {
                        Ok(Framing::Delimiter(d.as_bytes().to_vec()))
                    }
                    (None, Some(_)) => Err(err(format!("{site}: delimiter must be non-empty"))),
                    _ => Err(err(format!(
                        "{site}: framing table is {{ length_prefixed = n }} OR {{ delimiter = \"…\" }}"
                    ))),
                }
            }
            Some(other) => Err(err(format!(
                "{site}: framing must be a string or table, got a {}",
                other.type_name()
            ))),
        }
    }

    pub(super) fn is_raw(&self) -> bool {
        matches!(self, Framing::Raw)
    }

    /// Wrap one payload into its on-wire form.
    pub(super) fn encode(&self, payload: &[u8]) -> Vec<u8> {
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
            Framing::ContentLength => {
                let mut v = format!("Content-Length: {}\r\n\r\n", payload.len()).into_bytes();
                v.extend_from_slice(payload);
                v
            }
        }
    }

    /// Take the first COMPLETE turn out of `buf`, draining its wire bytes; `None` if the buffer
    /// does not hold a whole one yet.
    ///
    /// The single frame scanner. Both the blocking reader ([`read_frame`]) and the proxy's
    /// chunk pump go through here — they used to carry a scanner each, which is how
    /// `Framing::ContentLength` would otherwise have had to be implemented twice and could then
    /// disagree with itself about where a frame ends.
    pub(super) fn take_frame(&self, buf: &mut Vec<u8>) -> Option<Vec<u8>> {
        match self {
            Framing::Raw => None,
            Framing::Line => split_at_needle(buf, b"\n"),
            Framing::Delimiter(d) => split_at_needle(buf, d),
            Framing::LengthPrefixed(n) => {
                if buf.len() < *n {
                    return None;
                }
                let mut len: u64 = 0;
                for b in buf.iter().take(*n) {
                    len = (len << 8) | *b as u64;
                }
                let total = n + len as usize;
                if buf.len() < total {
                    return None;
                }
                let payload = buf[*n..total].to_vec();
                buf.drain(..total);
                Some(payload)
            }
            Framing::ContentLength => {
                let head_end = buf
                    .windows(HEADER_END.len())
                    .position(|w| w == HEADER_END)?;
                let len = content_length_of(&buf[..head_end])?;
                let total = head_end + HEADER_END.len() + len;
                if buf.len() < total {
                    return None;
                }
                let payload = buf[head_end + HEADER_END.len()..total].to_vec();
                buf.drain(..total);
                Some(payload)
            }
        }
    }
}

/// Split `buf` at the first `needle`, returning what preceded it and consuming both.
fn split_at_needle(buf: &mut Vec<u8>, needle: &[u8]) -> Option<Vec<u8>> {
    let pos = buf.windows(needle.len()).position(|w| w == needle)?;
    let payload = buf[..pos].to_vec();
    buf.drain(..pos + needle.len());
    Some(payload)
}

/// The declared body length from a `Content-Length` header block, case-insensitively. `None` when
/// the block has no such header — a malformed block that would otherwise wedge the reader forever
/// waiting for a length that is never coming.
fn content_length_of(head: &[u8]) -> Option<usize> {
    for line in head.split(|b| *b == b'\n') {
        let line = String::from_utf8_lossy(line);
        let (name, value) = line.split_once(':')?;
        if name.trim().eq_ignore_ascii_case("content-length") {
            return value.trim().parse().ok();
        }
    }
    None
}

/// Read one frame from `stream`, consuming `buf` leftovers first. `Ok(None)` is clean EOF.
pub(super) async fn read_frame<S: AsyncRead + Unpin>(
    stream: &mut S,
    buf: &mut Vec<u8>,
    framing: &Framing,
) -> std::io::Result<Option<Vec<u8>>> {
    if framing.is_raw() {
        return Err(std::io::Error::other(
            "read_frame called without framing (internal)",
        ));
    }
    loop {
        if let Some(payload) = framing.take_frame(buf) {
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

/// Read frames until `accept` says one is the wanted turn. `Ok(None)` is clean EOF.
///
/// `accept` is deliberately **synchronous**: it is where the codec decodes and the selector
/// matches, which means it touches Lua, and a Lua value must not be held across an await on a
/// single-threaded runtime. Keeping it a closure over bytes also means the skipped turns are the
/// caller's to record — they are evidence, not noise
/// (`docs/plans/stdio-transport.md` §3: a driver has a transcript).
pub(super) async fn read_until<S: AsyncRead + Unpin>(
    stream: &mut S,
    buf: &mut Vec<u8>,
    framing: &Framing,
    mut accept: impl FnMut(&[u8]) -> mlua::Result<bool>,
) -> mlua::Result<Option<Vec<u8>>> {
    loop {
        match read_frame(stream, buf, framing).await {
            Err(e) => return Err(err(format!("{e}"))),
            Ok(None) => return Ok(None),
            Ok(Some(payload)) => {
                if accept(&payload)? {
                    return Ok(Some(payload));
                }
            }
        }
    }
}

/// Read exactly `want` bytes (raw mode), consuming leftovers first.
pub(super) async fn read_exact_buffered<S: AsyncRead + Unpin>(
    stream: &mut S,
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

/// How a turn becomes a Lua VALUE — the dial that is deliberately separate from `Framing`.
///
/// `Bytes` is the default and the identity: a turn arrives as the Lua string it was on the wire.
/// `Json` decodes it, which is the whole point — a decoded turn has FIELDS, and fields are what a
/// [`Selector`] matches on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Codec {
    Bytes,
    Json,
}

impl Codec {
    pub(super) fn parse(site: &str, v: Option<Value>) -> mlua::Result<Codec> {
        match v {
            None | Some(Value::Nil) => Ok(Codec::Bytes),
            Some(Value::String(s)) => match s.to_string_lossy().as_ref() {
                "bytes" => Ok(Codec::Bytes),
                "json" => Ok(Codec::Json),
                other => Err(err(format!(
                    "{site}: unknown codec {other:?} (codecs are \"bytes\" and \"json\")"
                ))),
            },
            Some(other) => Err(err(format!(
                "{site}: codec must be a string, got a {}",
                other.type_name()
            ))),
        }
    }

    /// A Lua value on its way to the wire.
    pub(super) fn encode(&self, lua: &Lua, v: &Value) -> mlua::Result<Vec<u8>> {
        match self {
            Codec::Bytes => match v {
                Value::String(s) => Ok(s.as_bytes().to_vec()),
                other => Err(err(format!(
                    "send: with codec \"bytes\" a turn is a string, got a {} — set \
                     codec = \"json\" to send a table",
                    other.type_name()
                ))),
            },
            Codec::Json => {
                let jv = super::formats::lua_value_to_json(lua, v)?;
                serde_json::to_vec(&jv).map_err(|e| err(format!("send: encoding json: {e}")))
            }
        }
    }

    /// A turn off the wire on its way to Lua.
    pub(super) fn decode(&self, lua: &Lua, payload: &[u8]) -> mlua::Result<Value> {
        match self {
            Codec::Bytes => Ok(Value::String(lua.create_string(payload)?)),
            Codec::Json => {
                let jv: serde_json::Value = serde_json::from_slice(payload).map_err(|e| {
                    err(format!(
                        "recv: turn is not json ({e}) — the turn was {:?}",
                        String::from_utf8_lossy(&payload[..payload.len().min(200)])
                    ))
                })?;
                super::formats::json_value_to_lua(lua, &jv)
            }
        }
    }
}

/// WHICH turn a caller wants — the framed analogue of `expect`'s pattern.
///
/// A table is the structural subset match every other prova surface already speaks; a function is
/// an arbitrary predicate over the decoded turn. Turns that do not match are skipped, not
/// discarded: they stay in the transcript, so "the notification that arrived while we waited for
/// the reply" is still evidence.
#[derive(Debug)]
pub(super) enum Selector {
    Any,
    Shape(Table),
    Pred(mlua::Function),
    /// An exact turn, byte for byte. Reachable only as a mock's stub key — a driver's `where`
    /// has no use for it (you cannot ask for "the turn I already know verbatim"), which is why
    /// [`Selector::parse`] does not produce it and [`Selector::parse_stub`] does.
    Bytes(Vec<u8>),
}

impl Selector {
    /// A **stub** key: the same grammar as `where`, plus the literal form a byte mock speaks.
    ///
    /// This is what makes `:on` and `where` one code path. The alternative — a second matcher for
    /// stubs — is two implementations of "does this turn match?" that agree until the day they
    /// don't, and the day they don't is a mock answering the wrong stub while both look right.
    pub(super) fn parse_stub(site: &str, codec: Codec, v: Value) -> mlua::Result<Selector> {
        match v {
            // A string is always the literal turn, under either codec: `m:on('{"a":1}')` means
            // those bytes. Shape matching is what a TABLE asks for.
            Value::String(s) => Ok(Selector::Bytes(s.as_bytes().to_vec())),
            Value::Table(_) if codec == Codec::Bytes => Err(err(format!(
                "{site}: a table stub matches FIELDS, and this mock's turns are bytes — set \
                 codec = \"json\" so turns decode, or stub the exact turn as a string"
            ))),
            other => Selector::parse(site, codec, Some(other)),
        }
    }

    pub(super) fn parse(site: &str, codec: Codec, v: Option<Value>) -> mlua::Result<Selector> {
        match v {
            None | Some(Value::Nil) => Ok(Selector::Any),
            // A byte turn has no fields, so a table filter over one can only ever match nothing.
            // Refusing names the two cures instead of returning an empty-handed timeout.
            Some(Value::Table(_)) if codec == Codec::Bytes => Err(err(format!(
                "{site}: `where` as a table matches FIELDS, and this stream's turns are bytes — \
                 set codec = \"json\" so turns decode, or pass a function predicate over the \
                 raw turn"
            ))),
            Some(Value::Table(t)) => Ok(Selector::Shape(t)),
            Some(Value::Function(f)) => Ok(Selector::Pred(f)),
            Some(other) => Err(err(format!(
                "{site}: `where` must be a table (subset match) or a function (predicate), got a {}",
                other.type_name()
            ))),
        }
    }

    pub(super) fn is_any(&self) -> bool {
        matches!(self, Selector::Any)
    }

    /// Does this decoded turn satisfy the selector?
    pub(super) fn accepts(&self, turn: &Value) -> mlua::Result<bool> {
        match self {
            Selector::Any => Ok(true),
            Selector::Shape(shape) => match turn {
                Value::Table(t) => {
                    Ok(crate::engine::subset_mismatch(shape, t, &mut Vec::new()).is_none())
                }
                // A json stream can legally carry a scalar turn; it just cannot match a shape.
                _ => Ok(false),
            },
            Selector::Pred(f) => {
                let r: Value = f.call(turn.clone())?;
                Ok(!matches!(r, Value::Nil | Value::Boolean(false)))
            }
            // Compared against the raw turn by the caller, which has the bytes; by the time a
            // value has been decoded the literal is the wrong question to ask of it.
            Selector::Bytes(_) => Ok(false),
        }
    }
}

/// How many turns went by while `where` waited — omitted entirely when none did, so the common
/// message stays short.
///
/// It is the difference between the two failures that share one symptom: "nothing arrived" and
/// "things arrived and none was the one asked for" both present as a timeout, and only the second
/// means the selector is wrong.
pub(super) fn waited(skipped: usize) -> String {
    match skipped {
        0 => String::new(),
        1 => " (1 turn arrived and did not match `where`)".to_string(),
        n => format!(" ({n} turns arrived and none matched `where`)"),
    }
}

fn err(msg: impl Into<String>) -> mlua::Error {
    mlua::Error::RuntimeError(msg.into())
}

#[cfg(test)]
mod tests;
