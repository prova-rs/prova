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
    pub kind: String, // "socket" | "shell" — a human reading the file, and a sanity check
    pub turns: Vec<Turn>,
}

/// Record side: append turns as they are observed, flush once at close.
pub(crate) struct Recorder {
    path: String,
    kind: &'static str,
    turns: RefCell<Vec<Turn>>,
}

impl Recorder {
    pub fn new(path: String, kind: &'static str) -> Recorder {
        Recorder {
            path,
            kind,
            turns: RefCell::new(Vec::new()),
        }
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
        std::fs::write(&self.path, text)
    }
}

/// Replay side: load a cassette and answer keys in recorded order, consuming each entry once.
pub(crate) struct Player {
    turns: Vec<Turn>,
    consumed: Vec<bool>,
}

impl Player {
    pub fn load(path: &str) -> std::io::Result<Player> {
        let text = std::fs::read_to_string(path)?;
        let cas: Cassette = serde_json::from_str(&text)
            .map_err(|e| std::io::Error::other(format!("parsing cassette {path:?}: {e}")))?;
        let n = cas.turns.len();
        Ok(Player {
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
        Ok(s) => s.to_string(),
        Err(_) => {
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
