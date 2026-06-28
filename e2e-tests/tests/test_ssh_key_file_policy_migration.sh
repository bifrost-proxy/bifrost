#!/bin/bash
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
: "${BIFROST_DISABLE_TRAY:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
export BIFROST_DISABLE_TRAY

set -euo pipefail

unset http_proxy https_proxy HTTP_PROXY HTTPS_PROXY all_proxy ALL_PROXY no_proxy NO_PROXY

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

SKIP_BUILD="${SKIP_BUILD:-false}"
if [[ -z "${BIFROST_BIN:-}" ]]; then
    if [[ "$SKIP_BUILD" == "true" && -x "$REPO_DIR/target/release/bifrost" ]]; then
        BIFROST_BIN="$REPO_DIR/target/release/bifrost"
    else
        BIFROST_BIN="$REPO_DIR/target/debug/bifrost"
    fi
fi

TEST_DATA_DIR="$(mktemp -d)"
ADMIN_LOG="$TEST_DATA_DIR/admin.log"
ADMIN_PID=""
ADMIN_PORT=""

cleanup() {
    if [[ -n "${ADMIN_PID:-}" ]] && kill -0 "$ADMIN_PID" 2>/dev/null; then
        kill "$ADMIN_PID" 2>/dev/null || true
        wait "$ADMIN_PID" 2>/dev/null || true
    fi
    rm -rf "$TEST_DATA_DIR" >/dev/null 2>&1 || true
}
trap cleanup EXIT

log() { echo "[ssh-key-file-policy-migration-e2e] $*"; }

require_cmd() {
    command -v "$1" >/dev/null 2>&1 || {
        echo "Missing required command: $1" >&2
        exit 1
    }
}

pick_free_port() {
    python3 - <<'PY'
import socket
s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
}

require_cmd curl
require_cmd jq
require_cmd python3

if [[ "$SKIP_BUILD" != "true" || ! -x "$BIFROST_BIN" ]]; then
    log "Building bifrost debug binary..."
    (cd "$REPO_DIR" && cargo build --bin bifrost)
fi

ADMIN_PORT="$(pick_free_port)"
ADMIN_BASE_URL="http://127.0.0.1:${ADMIN_PORT}/_bifrost"

log "Starting isolated bifrost admin on port ${ADMIN_PORT}"
BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" start \
    -p "$ADMIN_PORT" --unsafe-ssl --no-system-proxy --skip-cert-check >"$ADMIN_LOG" 2>&1 &
ADMIN_PID=$!

ready=0
for _ in $(seq 1 60); do
    if curl -fsS "${ADMIN_BASE_URL}/api/proxy/address" >/dev/null 2>&1; then
        ready=1
        break
    fi
    if ! kill -0 "$ADMIN_PID" 2>/dev/null; then
        echo "bifrost admin exited early" >&2
        cat "$ADMIN_LOG" >&2 || true
        exit 1
    fi
    sleep 0.5
done
if [[ "$ready" != "1" ]]; then
    echo "bifrost admin did not become ready" >&2
    cat "$ADMIN_LOG" >&2 || true
    exit 1
fi

log "Creating SSH key with default Full Trust policy"
curl -fsS -X POST "${ADMIN_BASE_URL}/api/remote-invoke/ssh-key" \
    -H 'content-type: application/json' \
    -d '{"label":"legacy-full-ops","grant_mode":"permanent"}' >/dev/null

fingerprint="$(curl -fsS "${ADMIN_BASE_URL}/api/remote-invoke/ssh-key" | jq -r '.ssh_key_fingerprint // ""')"
if [[ -z "$fingerprint" ]]; then
    echo "failed to read active ssh key fingerprint" >&2
    exit 1
fi

log "Replacing SSH key policy with legacy 12-op Full Trust set"
cat >"$TEST_DATA_DIR/file-access.toml" <<EOF
[[grant]]
match.ssh_fingerprint = "$fingerprint"
name = "ssh-key:legacy-full-ops"
roots = ["/"]
ops = ["read", "list", "stat", "glob", "search", "hash", "write", "edit", "mkdir", "move", "delete", "apply_patch"]
allow_overwrite = true
allow_recursive_delete = false
EOF

log "Triggering file-access config read to run active SSH default policy ensure"
config_json="$(curl -fsS "${ADMIN_BASE_URL}/api/remote-invoke/file-access-config")"

echo "$config_json" | jq -e --arg fp "$fingerprint" '
  .grant[]?
  | select(.match.ssh_fingerprint == $fp)
  | (.ops | index("read_many")) != null and (.ops | index("outline")) != null
' >/dev/null

grep -q 'read_many' "$TEST_DATA_DIR/file-access.toml"
grep -q 'outline' "$TEST_DATA_DIR/file-access.toml"

log "PASS"
