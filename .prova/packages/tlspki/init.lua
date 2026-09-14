-- Test PKI + a TLS terminator, for proving prova's TLS clients
-- (docs/design/architecture.md#tls-everywhere).
--
-- **Why a real CA and a real leaf, not one self-signed certificate.** The obvious shortcut —
-- `openssl req -x509` once, then point `ca_cert` at that same file — does not work, and the way it
-- fails is instructive: `-x509` sets `basicConstraints=CA:TRUE`, and webpki correctly refuses a CA
-- certificate presented as an end entity (`CaUsedAsEndEntity`). So the shortcut makes `ca_cert`
-- look broken when it is the *fixture* that is malformed. Minting a CA and signing a leaf with it
-- is also the shape of the thing being proven: a private CA an endpoint's certificate chains to.
--
-- **Why a python TLS terminator rather than a TLS server per transport.** `wss://` and gRPC-over-TLS
-- need a TLS server that speaks websocket and h2 respectively, and prova's own mocks speak both —
-- in plaintext. A terminator that decrypts and forwards raw bytes to a loopback port turns every
-- plaintext mock prova already has into a TLS endpoint, so one piece of infrastructure serves all
-- three transports and no proof has to hand-roll a protocol server. ALPN is settable because h2
-- is negotiated, not assumed: a gRPC client that cannot agree on `h2` fails at the handshake.

local M = {}

--- Mint a CA and a leaf it signed, under `t:tempdir(key)`.
---
--- The leaf carries `subjectAltName` for both `localhost` and `127.0.0.1` so a proof may address
--- the endpoint either way — a certificate valid for only one of them turns a hostname choice into
--- a mysterious verification failure. `CA:FALSE` + `serverAuth` on the leaf keeps it a leaf.
---
--- `opts.san` / `opts.cn` override the leaf's subjectAltName and subject, for the
--- hostname-mismatch case. A mismatch is worth its own fixture because it is the half of TLS that
--- is NOT the chain: a certificate can be perfectly signed by a CA you trust and still be the
--- wrong certificate for the host you dialed. `insecure` has to tolerate that on every transport
--- and `ca_cert` has to keep rejecting it on every transport — and those are different code paths
--- in three different crates.
---
--- Returns `{ dir, ca, ca_key, cert, key }` — paths, all absolute.
function M.mint(t, key, opts)
  opts = opts or {}
  local dir = t:tempdir(key or "tlspki")
  local ca, ca_key = dir .. "/ca.pem", dir .. "/ca-key.pem"
  local cert, cert_key = dir .. "/srv.pem", dir .. "/srv-key.pem"
  local csr, ext = dir .. "/srv.csr", dir .. "/srv.ext"

  fs.write(ext, table.concat({
    "subjectAltName=" .. (opts.san or "DNS:localhost,IP:127.0.0.1"),
    "basicConstraints=critical,CA:FALSE",
    "extendedKeyUsage=serverAuth",
  }, "\n") .. "\n")

  -- `-nodes` (no encryption) because a passphrase-protected key would need one supplied on every
  -- read, and this key exists for the length of one test.
  local function openssl(args)
    local r = shell.run({ "openssl", table.unpack(args) }, { merge_stderr = true, timeout = "60s" })
    if r.code ~= 0 then
      error("tlspki: openssl " .. args[1] .. " failed (" .. r.code .. "): " .. r.stdout, 0)
    end
  end

  openssl({ "req", "-x509", "-newkey", "rsa:2048", "-nodes",
            "-keyout", ca_key, "-out", ca, "-days", "1",
            "-subj", "/CN=Prova Test CA",
            "-addext", "basicConstraints=critical,CA:TRUE" })
  openssl({ "req", "-newkey", "rsa:2048", "-nodes",
            "-keyout", cert_key, "-out", csr,
            "-subj", "/CN=" .. (opts.cn or "localhost") })
  -- `-CAcreateserial` is REQUIRED, not optional tidiness. OpenSSL 3 creates the `.srl` serial file
  -- implicitly; LibreSSL — which is what `/usr/bin/openssl` is on a stock macOS — does not, and
  -- fails with `ca.srl: No such file or directory` several lines into an otherwise successful
  -- signing ("Signature ok" prints first, which is why this reads as a fixture bug rather than a
  -- usage one). Found when `/opt/homebrew/bin` left PATH and openssl resolved to LibreSSL 3.3.6
  -- instead of OpenSSL 3.6.2: the suite had been green only because Homebrew's came first.
  openssl({ "x509", "-req", "-in", csr, "-CA", ca, "-CAkey", ca_key, "-CAcreateserial",
            "-out", cert, "-days", "1", "-extfile", ext })

  return { dir = dir, ca = ca, ca_key = ca_key, cert = cert, key = cert_key }
end

--- A second, unrelated CA — the negative control for `ca_cert`.
---
--- Without it, "`ca_cert` works" is satisfied by an implementation that ignores the option and
--- trusts everything. Pointing at a CA that signed nothing in play must FAIL, and that is the
--- assertion that gives the passing one its meaning.
function M.other_ca(t, key)
  local dir = t:tempdir(key or "tlspki-other")
  local ca = dir .. "/other-ca.pem"
  local r = shell.run({ "openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes",
                        "-keyout", dir .. "/other-ca-key.pem", "-out", ca, "-days", "1",
                        "-subj", "/CN=Prova Unrelated CA",
                        "-addext", "basicConstraints=critical,CA:TRUE" },
                      { merge_stderr = true, timeout = "60s" })
  if r.code ~= 0 then
    error("tlspki: minting the unrelated CA failed: " .. r.stdout, 0)
  end
  return ca
end

-- Decrypt and splice: a TLS listener that forwards raw plaintext both ways to `upstream_port`.
-- Threads rather than asyncio because the whole script has to stay readable inside a proof.
local TERMINATOR_PY = [[
import socket, ssl, sys, threading

listen_port, upstream_port, cert, key = int(sys.argv[1]), int(sys.argv[2]), sys.argv[3], sys.argv[4]
alpn = sys.argv[5] if len(sys.argv) > 5 and sys.argv[5] else None

ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
ctx.load_cert_chain(cert, key)
if alpn:
    ctx.set_alpn_protocols([alpn])

def splice(a, b):
    try:
        while True:
            data = a.recv(65536)
            if not data:
                break
            b.sendall(data)
    except OSError:
        pass
    finally:
        for s in (a, b):
            try:
                s.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass

def serve(tls_sock):
    try:
        up = socket.create_connection(("127.0.0.1", upstream_port))
    except OSError:
        tls_sock.close()
        return
    threading.Thread(target=splice, args=(tls_sock, up), daemon=True).start()
    threading.Thread(target=splice, args=(up, tls_sock), daemon=True).start()

srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
srv.bind(("127.0.0.1", listen_port))
srv.listen(64)
print("listening", flush=True)
while True:
    raw, _ = srv.accept()
    try:
        serve(ctx.wrap_socket(raw, server_side=True))
    except (ssl.SSLError, OSError):
        # A failed handshake is the EXPECTED outcome of the negative-control tests, so it must not
        # take the terminator down with it — the next test connects to the same front door.
        try:
            raw.close()
        except OSError:
            pass
]]

--- Put a TLS front door on a plaintext loopback port. Returns `{ port, host_url, stop }`.
---
--- `opts.upstream_port` (required) is the plaintext port to forward to. `opts.alpn` negotiates a
--- protocol ("h2" for gRPC). `opts.pki` is `mint`'s result.
---
--- Readiness is the process printing `listening`, waited on rather than slept through: a sleep long
--- enough to be safe on a loaded CI runner is long enough to be felt on every local run.
function M.terminate(t, ctx, opts)
  local pki = opts.pki
  local script = pki.dir .. "/terminator.py"
  fs.write(script, TERMINATOR_PY)
  local port = net.free_port()
  local python = package.config:sub(1, 1) == "\\" and "python" or "python3"
  local proc = ctx:manage(shell.spawn({
    python, "-u", script, tostring(port), tostring(opts.upstream_port),
    pki.cert, pki.key, opts.alpn or "",
  }))

  -- Bounded by a poll COUNT rather than a clock: `os.clock` is CPU time in Lua, not wall time, so
  -- a deadline built on it never arrives in a coroutine that spends its life awaiting. 600 × 50ms
  -- is a 30s budget for what normally takes one poll — sized for the slowest honest interpreter
  -- cold start on a loaded CI runner, since the cost of being generous here is zero.
  local listening = false
  for _ = 1, 600 do
    local out = proc:output() or ""
    if out:find("listening", 1, true) then
      listening = true
      break
    end
    if not proc:running() then
      error("tlspki: the TLS terminator exited before listening: " .. out, 0)
    end
    prova.sleep(50)
  end
  if not listening then
    error("tlspki: the TLS terminator never listened. Output: " .. (proc:output() or ""), 0)
  end

  return {
    port = port,
    -- Addressed by NAME, not by IP: hostname verification is half of what TLS does, and a proof
    -- that only ever used 127.0.0.1 would not notice a client that skipped it.
    url = "https://localhost:" .. port,
    ws_url = "wss://localhost:" .. port,
    addr = "localhost:" .. port,
  }
end

return M
