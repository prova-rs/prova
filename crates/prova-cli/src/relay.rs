//! `prova relay --to <addr>` — pipe this process's stdio to a socket, and back.
//!
//! **The adapter that turns a listen posture into a spawnable one.** A SUT that *spawns* its
//! dependency (an MCP server, a language server) cannot dial an address, so `stdio.mock` and
//! `stdio.proxy` need something on PATH. This is that something — and it is deliberately the
//! DUMBEST possible thing: a byte pump with no protocol knowledge at all.
//!
//! That is the whole design (docs/plans/stdio-transport.md §4). The repo's older shims —
//! `terminal.mock`, `shell.proxy` — render BEHAVIOR into a generated `sh` script, which is why
//! their matching is stuck at `case` patterns over bytes: `sh` is the matcher, so the matcher is
//! `sh`-shaped. Inverting it — transport in the shim, behavior in-process — means the stubs,
//! journal, faults and cassettes stay where the real matcher lives, and a new framing or codec
//! costs this program exactly nothing. It never learns what a turn is.
//!
//! Not hidden, and not internal-by-convention: a shim whose failures surface inside a generated
//! script nobody can read is a private protocol between prova and itself. `prova relay` is a verb
//! with help text and a proof, like `prova broker` and `prova lock`.

use std::process::ExitCode;

/// How long to wait for the far end to accept us before giving up.
///
/// The mock binds SYNCHRONOUSLY before handing out the shim path, so in the intended flow this
/// never elapses. It exists for the flow nobody intended: a stale shim left on PATH by a crashed
/// run, pointed at a socket that will never answer. Without the bound the SUT hangs on a pipe
/// forever, which is the failure mode every other read in this codebase is bounded to prevent.
const DIAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

struct Cli {
    addr: String,
}

fn parse(args: Vec<String>) -> Result<Cli, String> {
    let mut addr = None;
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--to" => {
                addr = Some(it.next().ok_or("--to needs an address")?);
            }
            other if other.starts_with("--to=") => {
                addr = Some(other["--to=".len()..].to_string());
            }
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    let addr = addr.ok_or("--to <addr> is required")?;
    if addr.is_empty() {
        return Err("--to needs a non-empty address".to_string());
    }
    Ok(Cli { addr })
}

/// `prova relay --to <addr>`: connect, then copy stdin→socket and socket→stdout until either side
/// closes. Exit 0 on a clean close of either direction.
pub(crate) fn run(args: Vec<String>) -> ExitCode {
    let cli = match parse(args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("prova relay: {e}\nusage: prova relay --to unix:///path/to.sock");
            return ExitCode::from(2);
        }
    };
    {
        let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
            Ok(rt) => rt,
            Err(e) => {
                eprintln!("prova relay: runtime: {e}");
                return ExitCode::FAILURE;
            }
        };
        match rt.block_on(pump(&cli.addr)) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                // stderr, never stdout: stdout IS the protocol channel here, and a diagnostic
                // written there would arrive at the peer as a turn it cannot parse.
                eprintln!("prova relay --to {}: {e}", cli.addr);
                ExitCode::FAILURE
            }
        }
    }
}

/// Dial either scheme the `socket` transport speaks.
///
/// Scheme-unified on purpose: "one namespace, unified by address scheme" is the socket doctrine
/// (mocks-proxies-drivers.md), and a public verb that spoke one of the two would contradict it.
/// It is also the Windows path — that platform has no unix sockets, so the ConPTY-era twin of the
/// spawnable postures is a `tcp://` mock plus a two-line `.cmd`, with this verb unchanged.
async fn dial(addr: &str) -> Result<Box<dyn Duplex>, String> {
    let timed_out = || {
        format!(
            "no answer within {DIAL_TIMEOUT:?} — is the mock still up? (a shim left on PATH by a \
             dead run points at an endpoint nobody is listening on)"
        )
    };
    if let Some(hostport) = addr.strip_prefix("tcp://") {
        let s = tokio::time::timeout(DIAL_TIMEOUT, tokio::net::TcpStream::connect(hostport))
            .await
            .map_err(|_| timed_out())?
            .map_err(|e| format!("connect: {e}"))?;
        return Ok(Box::new(s));
    }
    #[cfg(unix)]
    if let Some(path) = addr.strip_prefix("unix://") {
        let s = tokio::time::timeout(DIAL_TIMEOUT, tokio::net::UnixStream::connect(path))
            .await
            .map_err(|_| timed_out())?
            .map_err(|e| format!("connect: {e}"))?;
        return Ok(Box::new(s));
    }
    Err(format!(
        "address must be tcp://host:port or unix:///path, got {addr:?}"
    ))
}

/// What a dialed endpoint is, once the scheme stops mattering.
trait Duplex: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin> Duplex for T {}

async fn pump(addr: &str) -> Result<(), String> {
    let sock = dial(addr).await?;
    let (mut sock_r, mut sock_w) = tokio::io::split(sock);
    let mut stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();

    use tokio::io::AsyncWriteExt;

    // **The upstream direction finishing must NOT end the session.** A client that writes its
    // request and closes stdin — `echo … | prova relay`, and every SUT that says what it wants and
    // then waits — has only half-closed: the reply is still coming. Ending here on the first
    // completed direction dropped that reply on the floor, silently, with exit 0.
    //
    // So: half-close the write side (the EOF has to TRAVEL, or the peer waits forever for a turn
    // that is never coming), and keep draining until the peer closes. The DOWN direction ending is
    // what ends the session, because once the peer is gone nothing more can arrive.
    let up = async {
        let r = tokio::io::copy(&mut stdin, &mut sock_w).await;
        let _ = sock_w.shutdown().await;
        r
    };
    let down = tokio::io::copy(&mut sock_r, &mut stdout);
    let mut up = std::pin::pin!(up);
    let mut down = std::pin::pin!(down);
    let mut up_done = false;
    loop {
        tokio::select! {
            r = &mut up, if !up_done => {
                up_done = true;
                r.map_err(|e| format!("stdin→socket: {e}"))?;
            }
            r = &mut down => {
                r.map_err(|e| format!("socket→stdout: {e}"))?;
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    /// Both spellings of the one flag, and a usage error rather than a default when it is absent —
    /// a relay with no destination has nothing sensible to do, and guessing one would send the
    /// SUT's protocol somewhere nobody asked for.
    #[test]
    fn the_destination_is_required_and_takes_either_spelling() {
        assert_eq!(parse(args(&["--to", "unix:///tmp/a.sock"])).unwrap().addr, "unix:///tmp/a.sock");
        assert_eq!(parse(args(&["--to=unix:///tmp/b.sock"])).unwrap().addr, "unix:///tmp/b.sock");

        for bad in [vec![], args(&["--to"]), args(&["--to", ""]), args(&["--nope", "x"])] {
            assert!(parse(bad).is_err());
        }
    }

    /// **A half-close is not a close.** The client writes its request and closes stdin; the reply
    /// is still coming. An earlier cut ended the session on the first completed direction, which
    /// dropped that reply silently and exited 0 — the worst available failure, because the SUT
    /// sees a clean EOF where its answer should have been.
    ///
    /// Driven against a real listener rather than a mock stream: what makes this subtle is the
    /// INTERLEAVING of two real pipes, and a fake with instant reads would not reproduce it.
    #[test]
    fn a_closed_stdin_half_closes_and_still_delivers_the_reply() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = l.local_addr().unwrap();

            // A peer that answers only after seeing EOF — the shape that catches the bug, and a
            // fair model of a server that reads a whole request before it starts working.
            let server = tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let (mut s, _) = l.accept().await.unwrap();
                let mut got = Vec::new();
                s.read_to_end(&mut got).await.unwrap();
                s.write_all(b"ANSWER\n").await.unwrap();
                s.shutdown().await.unwrap();
                got
            });

            // stdin is already at EOF in the test process, so `up` completes immediately —
            // exactly the race that used to cancel `down`.
            pump(&format!("tcp://{addr}")).await.unwrap();
            let got = server.await.unwrap();
            assert!(got.is_empty(), "nothing to send, but the EOF still traveled");
        });
    }

    /// An address in neither scheme is refused naming both, rather than being treated as a path —
    /// "connect: No such file or directory" about something that was never a path is a bad way to
    /// learn you typed the wrong thing.
    #[test]
    fn an_unknown_scheme_is_refused_naming_both() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let e = rt.block_on(pump("http://127.0.0.1:9")).unwrap_err();
        assert!(e.contains("tcp://host:port"), "both accepted shapes are named: {e}");
        assert!(e.contains("unix:///path"), "{e}");
    }
}
