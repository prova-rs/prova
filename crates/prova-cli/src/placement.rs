//! The placement client's transport foothold (docs/design/placement.md §Transport).
//!
//! What is wired today: address resolution (`PROVA_PLACEMENT_BROKER` > `[placement] broker` >
//! nothing), the mandatory `hello`, and the loud-error rule — a configured-but-unreachable broker
//! fails the run before any proof loads, never falling back to local. Falling back would turn a
//! broken pool into a suite that quietly stopped distributing, with speed as the only symptom.
//!
//! What is deliberately NOT wired yet: routing `requires` and `resources` through the broker.
//! Those answers only mean something once work can be *placed* on the node that answered — until
//! the exec/materialize planes drive dispatch, "a peer has docker" would green-light a local run
//! on a machine without it, manufacturing exactly the false reds this system exists to prevent.
//! See docs/plans/placement-client.md for the staging and the open decisions.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

/// The protocol this client speaks — kept in lockstep with the conformance suite's.
const PROTOCOL: &str = "1.0";

/// Where the broker address came from — named in errors so the fix is obvious.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Source {
    Env,
    Manifest,
}

impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Source::Env => write!(f, "PROVA_PLACEMENT_BROKER"),
            Source::Manifest => write!(f, "[placement] broker in prova.toml"),
        }
    }
}

/// The configured broker address, if any: the env var wins over the manifest, and an empty or
/// whitespace value is unset (so `PROVA_PLACEMENT_BROKER= prova` disables rather than misdials).
pub fn configured(manifest_broker: Option<&str>) -> Option<(String, Source)> {
    if let Ok(v) = std::env::var("PROVA_PLACEMENT_BROKER") {
        let v = v.trim();
        if !v.is_empty() {
            return Some((v.to_string(), Source::Env));
        }
    }
    manifest_broker
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(|v| (v.to_string(), Source::Manifest))
}

/// What `hello` learned about the broker — enough to announce it and to know which optional
/// planes exist.
#[derive(Debug)]
pub struct BrokerInfo {
    pub broker: String,
    pub protocol: String,
    pub nodes: u64,
    #[allow(dead_code)] // read when the dispatch planes land; negotiated now so hello is honest
    pub features: Vec<String>,
}

/// Dial `addr` and perform the mandatory opening turn. Any failure — bad scheme, no socket,
/// refused hello, version mismatch — is an error for the caller to raise LOUDLY: the spec's rule
/// is that a configured broker is load-bearing, and the one forbidden response to its absence is
/// quietly running local.
pub fn hello(addr: &str) -> Result<BrokerInfo, String> {
    let Some(path) = addr.strip_prefix("unix://") else {
        return Err(format!(
            "broker address {addr:?} is not a unix:// socket (prova only dials its local broker; \
             remote reach is the broker's business)"
        ));
    };
    let stream = UnixStream::connect(path)
        .map_err(|e| format!("cannot reach the placement broker at {addr}: {e}"))?;

    let request = serde_json::json!({
        "id": 0,
        "op": "hello",
        "protocol": PROTOCOL,
        "client": format!("prova/{}", env!("CARGO_PKG_VERSION")),
    });
    let mut writer = stream
        .try_clone()
        .map_err(|e| format!("placement broker connection at {addr}: {e}"))?;
    writeln!(writer, "{request}")
        .map_err(|e| format!("cannot speak to the placement broker at {addr}: {e}"))?;

    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .map_err(|e| format!("no hello from the placement broker at {addr}: {e}"))?;
    if line.trim().is_empty() {
        return Err(format!(
            "the placement broker at {addr} closed the connection without answering hello"
        ));
    }
    let frame: serde_json::Value = serde_json::from_str(&line)
        .map_err(|e| format!("malformed hello from the placement broker at {addr}: {e}"))?;

    if !frame.get("ok").and_then(serde_json::Value::as_bool).unwrap_or(false) {
        let message = frame
            .get("message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("hello refused");
        return Err(format!("the placement broker at {addr} refused hello: {message}"));
    }
    let protocol = frame
        .get("protocol")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();
    if !protocol.starts_with("1.") {
        return Err(format!(
            "the placement broker at {addr} speaks protocol {protocol:?}, this prova speaks {PROTOCOL}"
        ));
    }
    Ok(BrokerInfo {
        broker: frame
            .get("broker")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("(unidentified broker)")
            .to_string(),
        protocol,
        nodes: frame.get("nodes").and_then(serde_json::Value::as_u64).unwrap_or(0),
        features: frame
            .get("features")
            .and_then(serde_json::Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_beats_manifest_and_blank_means_unset() {
        // `configured` reads the real environment; these cases only exercise the manifest side and
        // the blank-trimming rule (the env-wins half is pinned black-box in the transport proofs,
        // where the child process's environment is controlled).
        if std::env::var("PROVA_PLACEMENT_BROKER").is_ok() {
            return; // an ambient broker (conformance run) would invert every expectation below
        }
        assert_eq!(
            configured(Some("unix:///tmp/b.sock")).map(|(a, s)| (a, s as u8)),
            Some(("unix:///tmp/b.sock".to_string(), Source::Manifest as u8))
        );
        assert!(configured(Some("   ")).is_none());
        assert!(configured(None).is_none());
    }

    #[test]
    fn non_unix_addresses_are_refused_by_name() {
        let err = hello("tcp://127.0.0.1:9999").unwrap_err();
        assert!(err.contains("unix://"), "{err}");
    }

    /// A one-connection broker stub: accept, optionally read the request line, answer with
    /// `reply` (verbatim JSON line, or nothing for a silent close). Returns the addr and the
    /// join handle carrying the request line the client actually sent.
    fn stub_broker(tag: &str, reply: Option<&'static str>) -> (String, std::thread::JoinHandle<String>) {
        let path = std::env::temp_dir().join(format!("prova-pl-{tag}-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            BufReader::new(stream.try_clone().unwrap()).read_line(&mut request).unwrap();
            if let Some(reply) = reply {
                let mut w = stream;
                writeln!(w, "{reply}").unwrap();
            }
            request
        });
        (format!("unix://{}", path.display()), handle)
    }

    #[test]
    fn hello_negotiates_the_opening_turn() {
        let (addr, broker) = stub_broker(
            "ok",
            Some(r#"{"ok":true,"protocol":"1.0","broker":"stub","nodes":3,"features":["exec"]}"#),
        );
        let info = hello(&addr).unwrap();
        assert_eq!((info.broker.as_str(), info.protocol.as_str(), info.nodes), ("stub", "1.0", 3));
        assert_eq!(info.features, vec!["exec".to_string()]);
        let request = broker.join().unwrap();
        for field in [r#""op":"hello""#, r#""protocol":"1.0""#, r#""client":"prova/"#] {
            assert!(request.contains(field), "request lacks {field}: {request}");
        }
    }

    /// Every refusal is loud and names the broker's own words — the spec's rule that a configured
    /// broker never degrades into a quiet local run.
    #[test]
    fn hello_raises_refusals_mismatches_and_silence_by_name() {
        let (addr, _b) = stub_broker("refuse", Some(r#"{"ok":false,"message":"pool draining"}"#));
        assert!(hello(&addr).unwrap_err().contains("pool draining"));

        let (addr, _b) = stub_broker("proto", Some(r#"{"ok":true,"protocol":"2.0"}"#));
        let err = hello(&addr).unwrap_err();
        assert!(err.contains("protocol \"2.0\""), "{err}");

        let (addr, _b) = stub_broker("mute", None);
        let err = hello(&addr).unwrap_err();
        assert!(err.contains("without answering"), "{err}");

        let err = hello("unix:///nonexistent/broker.sock").unwrap_err();
        assert!(err.contains("cannot reach"), "{err}");
    }
}
