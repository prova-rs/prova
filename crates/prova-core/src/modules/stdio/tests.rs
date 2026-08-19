//! `stdio.spawn` against real child processes — the parts that are cheaper to prove here than
//! from a suite, because they need a program that misbehaves in a specific way.
//!
//! The black-box proofs live in `proofs/spec/stdio/`; these cover the failure diagnostics, which a
//! proof can only assert the SHAPE of, and the option gate.

use super::*;

/// A LocalSet-backed runtime, because everything here is `spawn_local`'d single-thread alongside
/// the Lua state — the same harness shape every transport's unit tests use.
fn run<F: std::future::Future<Output = ()>>(f: impl FnOnce() -> F) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, f());
}

/// Take the child out from under its `RefCell` BEFORE awaiting on it. The borrow guard lives to
/// the end of the statement, so `this.child.borrow_mut().take().unwrap().kill().await` holds it
/// across the await — which on a single-threaded runtime is how you deadlock a session against
/// itself. The production paths take-then-await for the same reason.
fn take_child(this: &Session) -> tokio::process::Child {
    this.child.borrow_mut().take().expect("the child is still held by the session")
}

/// A `Session` over a real child, without going through Lua (the constructor needs a ctx).
fn session(program: &str, framing: Framing, codec: Codec) -> Session {
    let mut command = CommandSpec::Shell(program.to_string()).build();
    command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().unwrap();
    let stderr_buf: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    if let Some(e) = child.stderr.take() {
        super::super::shell::spawn_output_reader(e, stderr_buf.clone());
    }
    let pid = child.id();
    Session {
        stdin: Rc::new(RefCell::new(child.stdin.take())),
        stdout: Rc::new(RefCell::new(child.stdout.take())),
        child: Rc::new(RefCell::new(Some(child))),
        buf: Rc::new(RefCell::new(Vec::new())),
        stderr: stderr_buf,
        transcript: Rc::new(RefCell::new(Vec::new())),
        framing,
        codec,
        pid,
        lease: RefCell::new(None),
        label: "test-child".to_string(),
    }
}

/// The three facts a silent SUT has to volunteer. Without them a wedged server reports as a bare
/// timeout, which cannot distinguish "never started" from "started and said nothing" from "wrote
/// to the other stream and died" — and telling those apart by hand costs an hour, which is the
/// bug report this whole diagnostic came from.
#[test]
fn a_read_failure_carries_stderr_the_child_status_and_the_label() {
    run(|| async {
        // Writes a complaint to stderr and then sits there saying nothing on stdout.
        let this = session(
            "echo 'FATAL: config missing' >&2; sleep 30",
            Framing::Line,
            Codec::Bytes,
        );
        // Let the reader task pick the stderr line up before we ask for it.
        tokio::time::sleep(Duration::from_millis(200)).await;

        let msg = this.diagnose("recv: timed out", 0, " after 10ms").to_string();
        assert!(msg.contains("FATAL: config missing"), "stderr is in the message: {msg}");
        assert!(msg.contains("still running"), "the child's state is named: {msg}");
        assert!(msg.contains("test-child"), "and WHICH process it was: {msg}");
        assert!(msg.contains("0 turns read"), "and how far the conversation got: {msg}");

        let _ = take_child(&this).kill().await;
    });
}

/// The negative control for the test above: a silent stderr must READ as silent rather than as an
/// empty section, because "the server said nothing" and "we failed to capture what it said" are
/// different diagnoses and the message is the only place they are distinguishable.
#[test]
fn a_silent_stderr_says_so_rather_than_showing_an_empty_section() {
    run(|| async {
        let this = session("sleep 30", Framing::Line, Codec::Bytes);
        tokio::time::sleep(Duration::from_millis(100)).await;
        let msg = this.diagnose("recv: timed out", 0, "").to_string();
        assert!(msg.contains("stderr: (silent)"), "silence is stated: {msg}");
        let _ = take_child(&this).kill().await;
    });
}

/// An exited child is the OTHER half of the same question, and the status has to change to match.
/// A process that died is not "still running", and reporting it as such sends the reader looking
/// for a deadlock that is not there.
#[test]
fn an_exited_child_is_reported_as_exited() {
    run(|| async {
        let this = session("exit 3", Framing::Line, Codec::Bytes);
        // Reap it the way `:wait()` would, so try_wait has a status to report.
        let mut child = take_child(&this);
        let status = child.wait().await.unwrap();
        assert_eq!(status.code(), Some(3));
        *this.child.borrow_mut() = Some(child);

        let msg = this.diagnose("recv: timed out", 0, "").to_string();
        assert!(msg.contains("exited"), "the exit is named, not 'still running': {msg}");
        assert!(!msg.contains("still running"), "{msg}");
    });
}

/// A framed round trip against a real process: write a turn, read the answer back. `cat` is the
/// honest minimum — it proves the pipe is wired both ways and the frame survives it, with no
/// protocol in between to hide a mistake.
#[test]
fn a_line_framed_session_round_trips_through_a_real_child() {
    run(|| async {
        let this = session("cat", Framing::Line, Codec::Bytes);
        {
            // Same rule on the write half: own it across the await, then hand it back.
            let mut w = this.stdin.borrow_mut().take().expect("stdin is piped");
            w.write_all(&Framing::Line.encode(b"ping")).await.unwrap();
            w.flush().await.unwrap();
            *this.stdin.borrow_mut() = Some(w);
        }
        let (mut out, mut buf) = checkout(&this, "recv").unwrap();
        let got = tokio::time::timeout(
            Duration::from_secs(5),
            super::super::turn::read_until(&mut out, &mut buf, &Framing::Line, |_| Ok(true)),
        )
        .await;
        restore(&this, out, buf);
        assert_eq!(got.unwrap().unwrap().as_deref(), Some(&b"ping"[..]));

        this.stdin.borrow_mut().take(); // EOF — `cat` exits
        let code = take_child(&this).wait().await.unwrap();
        assert_eq!(code.code(), Some(0), "the child exits cleanly on stdin EOF");
    });
}

/// `where` needs turns to select BETWEEN. On an unframed session it could only ever be a no-op,
/// and a no-op filter reads as configured — the exact silent drop the closed-opts doctrine exists
/// to refuse.
#[test]
fn where_on_an_unframed_session_is_refused_naming_the_cure() {
    run(|| async {
        let lua = Lua::new();
        let this = session("sleep 30", Framing::Raw, Codec::Bytes);
        let f: mlua::Function = lua.load("function() return true end").eval().unwrap();
        let opts = lua.create_table().unwrap();
        opts.set("where", f).unwrap();

        let e = read_args(&this, &Some(opts)).unwrap_err().to_string();
        assert!(e.contains("set framing"), "the cure is named: {e}");

        // …and the accepted shape still passes, so the refusal above is measuring the guard
        // rather than a gate that refuses everything.
        let opts = lua.create_table().unwrap();
        opts.set("timeout", "5s").unwrap();
        let (dur, sel) = read_args(&this, &Some(opts)).unwrap();
        assert_eq!(dur, Duration::from_secs(5));
        assert!(sel.is_any());

        let _ = take_child(&this).kill().await;
    });
}
