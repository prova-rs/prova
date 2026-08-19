//! The shared byte-turn cassette (docs/design/mocks-proxies-drivers.md) — the storage format and
//! replay discipline common to the transports whose turn is *bytes in → bytes out*: `socket`
//! (framed request turn → response turn) and `shell.proxy` (argv+stdin → stdout+exit). http and
//! grpc keep their own richer formats (structured request keys, descriptors); this is the plain
//! one those two do not fit.
//!
//! One recording is an ordered list of `{ key, response }` entries. `key` selects a response the
//! way an inbound turn does — repeated identical keys replay in recorded order (consumed once
//! each), so a create→read-back sequence reproduces instead of collapsing to its first answer. A
//! replay miss is the caller's to make loud; this module only reports it (`None`).

use std::cell::RefCell;

/// A recorded turn. `key` and `response` are opaque strings — a transport encodes its turn into
/// them (socket: the framed request/response bytes as lossless latin-1; shell: a joined argv+stdin
/// key and the stdout). Both are UTF-8-safe because we base64 arbitrary bytes at the edges.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct Turn {
    pub key: String,
    pub response: String,
    /// Optional structured extras a transport needs on replay (shell's exit code). Absent for
    /// socket, whose response is the whole story.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<i64>,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct Cassette {
    pub version: u32,
    /// What recorded it. Names the TURN MODEL, and a reader accepts any kind sharing its own:
    /// `socket` and `stdio` are byte-turn recordings with an identical format, so a session
    /// captured through one replays through the other — which is a property of the decomposition
    /// (a stdio proxy IS a socket proxy reached by spawn), not a coincidence to paper over.
    /// `shell` is a genuinely different model (argv+stdin → stdout+exit) and does not interchange.
    pub kind: String,
    pub turns: Vec<Turn>,
}

/// The sentinel a redacted secret becomes on disk — shared with the http facet's spelling.
pub(crate) const REDACTION: &str = "REDACTED";

/// Scrub literal secrets from a serialized cassette before it is written — the cross-transport
/// redaction floor (docs/design/mocks-proxies-drivers.md). Recording real traffic writes real
/// traffic to a file someone will commit, so `redact = { "secret" }` guarantees the string never
/// hits disk, whatever the cassette format. Longest-first so a secret that contains another does
/// not leave a partial behind.
pub(crate) fn scrub(mut text: String, redactions: &[String]) -> String {
    let mut ordered: Vec<&String> = redactions.iter().filter(|s| !s.is_empty()).collect();
    ordered.sort_by_key(|s| std::cmp::Reverse(s.len()));
    for secret in ordered {
        text = text.replace(secret.as_str(), REDACTION);
    }
    text
}

/// Record side: append turns as they are observed, flush once at close.
pub(crate) struct Recorder {
    path: String,
    kind: &'static str,
    turns: RefCell<Vec<Turn>>,
    redact: Vec<String>,
}

impl Recorder {
    pub fn new(path: String, kind: &'static str) -> Recorder {
        Recorder {
            path,
            kind,
            turns: RefCell::new(Vec::new()),
            redact: Vec::new(),
        }
    }
    /// Literal strings scrubbed from the serialized cassette at flush time (record-time redaction).
    pub fn with_redactions(mut self, redact: Vec<String>) -> Recorder {
        self.redact = redact;
        self
    }
    pub fn record(&self, key: String, response: String, code: Option<i64>) {
        self.turns.borrow_mut().push(Turn {
            key,
            response,
            code,
        });
    }
    /// Flush the cassette to disk (the close/scope-exit flush point). A recorder that saw nothing
    /// still writes an empty cassette — the file's existence is what `auto` mode keys on next run.
    pub fn flush(&self) -> std::io::Result<()> {
        let cas = Cassette {
            version: 1,
            kind: self.kind.to_string(),
            turns: self.turns.borrow().clone(),
        };
        let text = serde_json::to_string_pretty(&cas)
            .map_err(|e| std::io::Error::other(format!("encoding cassette: {e}")))?;
        std::fs::write(&self.path, scrub(text, &self.redact))
    }
}

/// Replay side: load a cassette and answer keys in recorded order, consuming each entry once.
pub(crate) struct Player {
    /// The recorded turn model, for the reader's compatibility check.
    kind: Option<String>,
    turns: Vec<Turn>,
    consumed: Vec<bool>,
}

impl Player {
    /// Load a cassette, refusing one recorded under a turn model this reader cannot replay.
    ///
    /// The `kind` field advertised itself as "a sanity check" from the start and nothing checked
    /// it, so a socket proxy pointed at a `shell` cassette replayed argv-keyed turns as if they
    /// were framed ones and every answer missed — loud, but for a reason the message never named.
    pub fn load_of(path: &str, accepted: &[&str]) -> std::io::Result<Player> {
        let p = Player::load(path)?;
        if let Some(kind) = &p.kind {
            if !accepted.contains(&kind.as_str()) {
                return Err(std::io::Error::other(format!(
                    "cassette {path:?} was recorded by `{kind}`, which this replay cannot read \
                     (it reads: {}) — a cassette carries its turn model, and the models do not \
                     interchange",
                    accepted.join(", ")
                )));
            }
        }
        Ok(p)
    }

    pub fn load(path: &str) -> std::io::Result<Player> {
        let text = std::fs::read_to_string(path)?;
        let cas: Cassette = serde_json::from_str(&text)
            .map_err(|e| std::io::Error::other(format!("parsing cassette {path:?}: {e}")))?;
        let n = cas.turns.len();
        Ok(Player {
            kind: Some(cas.kind),
            turns: cas.turns,
            consumed: vec![false; n],
        })
    }
    /// Every recorded key, in order (with duplicates) — for a consumer that renders the whole
    /// cassette up front (the shell replay shim's static `case` arms) rather than answering live.
    pub fn keys(&self) -> Vec<String> {
        self.turns.iter().map(|t| t.key.clone()).collect()
    }

    /// First unconsumed entry matching `key`; `None` is a miss (the caller makes it loud).
    pub fn answer(&mut self, key: &str) -> Option<Turn> {
        for i in 0..self.turns.len() {
            if !self.consumed[i] && self.turns[i].key == key {
                self.consumed[i] = true;
                return Some(self.turns[i].clone());
            }
        }
        None
    }
}

/// Byte-lossless string encoding for a cassette key/value: bytes that are valid UTF-8 stay
/// human-readable; anything else is base64 with a sentinel prefix, so a binary protocol still
/// round-trips and a text protocol stays diff-able.
pub(crate) fn encode_bytes(b: &[u8]) -> String {
    match std::str::from_utf8(b) {
        // Text that happens to START with the sentinel must be base64'd too, or decode would
        // strip the literal prefix and mangle it — a payload must never be able to spoof the
        // encoding's own escape hatch.
        Ok(s) if !s.starts_with("b64:") => s.to_string(),
        _ => {
            use base64::Engine;
            format!("b64:{}", base64::engine::general_purpose::STANDARD.encode(b))
        }
    }
}

pub(crate) fn decode_bytes(s: &str) -> Vec<u8> {
    if let Some(rest) = s.strip_prefix("b64:") {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD
            .decode(rest)
            .unwrap_or_default()
    } else {
        s.as_bytes().to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::make_tempdir;

    /// Text round-trips readable; binary round-trips through the b64 sentinel; and text that
    /// SPOOFS the sentinel round-trips intact rather than decoding to garbage — the edge this
    /// suite exists for (found by writing it).
    #[test]
    fn encode_decode_round_trips() {
        assert_eq!(encode_bytes(b"hello"), "hello");
        assert_eq!(decode_bytes("hello"), b"hello");

        let binary = [0u8, 159, 146, 150];
        let enc = encode_bytes(&binary);
        assert!(enc.starts_with("b64:"));
        assert_eq!(decode_bytes(&enc), binary);

        let spoof = b"b64:not actually base64";
        let enc = encode_bytes(spoof);
        assert_eq!(decode_bytes(&enc), spoof);
    }

    /// Longest-first scrubbing: a secret that contains another leaves no partial behind.
    #[test]
    fn scrub_is_longest_first_and_total() {
        let text = "token=abc123 and also abc".to_string();
        let out = scrub(text, &["abc".to_string(), "abc123".to_string()]);
        assert_eq!(out, format!("token={REDACTION} and also {REDACTION}"));
        // Empty redaction strings are ignored rather than replacing everything.
        assert_eq!(scrub("keep".to_string(), &[String::new()]), "keep");
    }

    /// The record → flush → load → answer loop: repeated identical keys replay in recorded order,
    /// consumed once each, so a create→read-back sequence reproduces instead of collapsing to its
    /// first answer; a miss is `None` (the caller's to make loud).
    #[test]
    fn recorder_player_round_trip_consumes_in_order() {
        let dir = make_tempdir().unwrap();
        let path = dir.join("cas.json").to_string_lossy().into_owned();
        let rec = Recorder::new(path.clone(), "socket");
        rec.record("GET".into(), "first".into(), None);
        rec.record("GET".into(), "second".into(), Some(7));
        rec.flush().unwrap();

        let mut player = Player::load(&path).unwrap();
        assert_eq!(player.keys(), vec!["GET".to_string(), "GET".to_string()]);
        assert_eq!(player.answer("GET").unwrap().response, "first");
        let second = player.answer("GET").unwrap();
        assert_eq!(second.response, "second");
        assert_eq!(second.code, Some(7));
        assert!(player.answer("GET").is_none(), "both entries consumed");
        assert!(player.answer("POST").is_none(), "a miss reports, never invents");
    }

    /// Record-time redaction: the secret never reaches disk, whatever the cassette format.
    #[test]
    fn recorder_scrubs_before_writing() {
        let dir = make_tempdir().unwrap();
        let path = dir.join("cas.json").to_string_lossy().into_owned();
        let rec = Recorder::new(path.clone(), "shell")
            .with_redactions(vec!["hunter2".to_string()]);
        rec.record("login hunter2".into(), "ok hunter2".into(), Some(0));
        rec.flush().unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(!written.contains("hunter2"));
        assert!(written.contains(REDACTION));
    }

    /// A recorder that saw nothing still writes an empty cassette — the file's existence is what
    /// `auto` mode keys on next run.
    #[test]
    fn empty_recorder_still_writes() {
        let dir = make_tempdir().unwrap();
        let path = dir.join("cas.json").to_string_lossy().into_owned();
        Recorder::new(path.clone(), "socket").flush().unwrap();
        let mut player = Player::load(&path).unwrap();
        assert!(player.keys().is_empty());
        assert!(player.answer("anything").is_none());
    }
}
