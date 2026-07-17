#!/usr/bin/env bash
# Run a prova suite INSIDE a Parallels Linux VM — the honest native-Linux proving ground.
#
# Why this exists: some proofs (C2, `docs/plans/mocks.md`) only mean something where prova, the Docker
# daemon, and the container share one native-Linux kernel. On Docker Desktop the daemon's proxy hides
# the very behaviour under test. So the suite must run next to a *native* daemon — this drives a
# Parallels Linux VM to be that place. It is the general shape of "run prova next to the substrate":
# swap the launcher and the same move targets a kind cluster or a remote host.
#
# Idempotent: provisions Docker + a current Rust toolchain only if missing, syncs the working tree,
# builds, and runs. Gated on `prlctl` — a no-op with a clear message on a machine without Parallels
# (the `requires = { "parallels" }` idea, at the shell layer).
#
# Usage: scripts/vm-linux-proof.sh [suite.lua ...]   (default: the C2 end-to-end proof)
set -euo pipefail

VM="${PROVA_VM:-Ubuntu 24.04 ARM64}"
SUITES=("${@:-crates/prova-core/testdata/c2_e2e.lua}")
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"

if ! command -v prlctl >/dev/null 2>&1; then
  echo "skip: prlctl not found — this proof requires Parallels Desktop (runs on demand only)." >&2
  exit 0
fi

echo "[host] starting VM: $VM"
prlctl start "$VM" >/dev/null 2>&1 || true
echo "[host] waiting for guest exec..."
until prlctl exec "$VM" true 2>/dev/null; do sleep 3; done

echo "[host] provisioning (idempotent: docker + rust)..."
prlctl exec "$VM" "bash -lc '
  set -e
  export DEBIAN_FRONTEND=noninteractive
  command -v docker >/dev/null 2>&1 || { apt-get update -qq && apt-get install -y -qq docker.io >/dev/null; }
  systemctl enable --now docker >/dev/null 2>&1 || service docker start || true
  # Ubuntus apt cargo (1.75) cannot read a v4 Cargo.lock; a current stable via rustup can.
  [ -x \$HOME/.cargo/bin/cargo ] || curl -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable --profile minimal >/dev/null 2>&1
'"

echo "[host] syncing working tree into the guest..."
prlctl exec "$VM" "rm -rf /root/prova && mkdir -p /root/prova"
tar czf - --exclude=target --exclude=.jj --exclude=.git --exclude='*.output' -C "$REPO_ROOT" . 2>/dev/null \
  | prlctl exec "$VM" "tar xzf - -C /root/prova" 2>/dev/null

echo "[host] building prova in the guest (detached; first build is slow)..."
prlctl exec "$VM" "bash -lc 'cd /root/prova && . \$HOME/.cargo/env && rm -f /root/BUILD_OK /root/BUILD_FAILED && nohup sh -c \"cargo build -p prova-cli > /root/build.log 2>&1 && touch /root/BUILD_OK || touch /root/BUILD_FAILED\" >/dev/null 2>&1 & echo detached'" >/dev/null
until prlctl exec "$VM" "test -f /root/BUILD_OK -o -f /root/BUILD_FAILED" 2>/dev/null; do sleep 15; done
if prlctl exec "$VM" "test -f /root/BUILD_FAILED" 2>/dev/null; then
  echo "[host] BUILD FAILED:" >&2
  prlctl exec "$VM" "tail -30 /root/build.log" >&2
  exit 1
fi
echo "[host] build ok."

status=0
for suite in "${SUITES[@]}"; do
  echo "[host] === running $suite (native linux/$(prlctl exec "$VM" 'uname -m' 2>/dev/null | tr -d '\r')) ==="
  prlctl exec "$VM" "bash -lc 'cd /root/prova && ./target/debug/prova $suite 2>&1'" || status=1
done
exit $status
