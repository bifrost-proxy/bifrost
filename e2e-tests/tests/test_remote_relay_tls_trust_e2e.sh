#!/bin/bash
set -uo pipefail

: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
: "${BIFROST_DISABLE_TRAY:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
export BIFROST_DISABLE_TRAY

unset http_proxy https_proxy HTTP_PROXY HTTPS_PROXY all_proxy ALL_PROXY no_proxy NO_PROXY

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

source "$ROOT_DIR/e2e-tests/test_utils/assert.sh"

pick_free_port() {
    python3 - <<'PY'
import socket
s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
}

require_cmd() {
    command -v "$1" >/dev/null 2>&1 || {
        echo "Missing required command: $1" >&2
        exit 1
    }
}

require_cmd python3
require_cmd cargo
require_cmd openssl

TEST_ROOT=""
SERVER_PID=""

cleanup() {
    if [[ -n "$SERVER_PID" ]] && kill -0 "$SERVER_PID" 2>/dev/null; then
        kill "$SERVER_PID" 2>/dev/null || true
        wait "$SERVER_PID" 2>/dev/null || true
    fi
    [[ -n "$TEST_ROOT" ]] && rm -rf "$TEST_ROOT" 2>/dev/null || true
}
trap cleanup EXIT

log() { echo "[remote-relay-tls-trust-e2e] $*"; }

wait_for_server() {
    local url="$1"
    local log_file="$2"
    local waited=0
    while [[ $waited -lt 50 ]]; do
        if curl -skS "${url}/healthz" >/dev/null 2>&1; then
            return 0
        fi
        if [[ -n "$SERVER_PID" ]] && ! kill -0 "$SERVER_PID" 2>/dev/null; then
            echo "mock HTTPS relay exited early: $url" >&2
            [[ -f "$log_file" ]] && cat "$log_file" >&2
            exit 1
        fi
        sleep 0.2
        waited=$((waited + 1))
    done
    echo "mock HTTPS relay did not become ready: $url" >&2
    [[ -f "$log_file" ]] && cat "$log_file" >&2
    exit 1
}

generate_private_ca_and_server_cert() {
    local cert_dir="$1"
    mkdir -p "$cert_dir"
    openssl req -x509 -newkey rsa:2048 -nodes \
        -keyout "$cert_dir/ca.key" \
        -out "$cert_dir/ca.pem" \
        -subj "/CN=Bifrost Remote Relay Test Root" \
        -days 1 >/dev/null 2>&1

    cat >"$cert_dir/server.cnf" <<'EOF'
[req]
distinguished_name = dn
req_extensions = v3_req
prompt = no

[dn]
CN = 127.0.0.1

[v3_req]
subjectAltName = @alt_names

[alt_names]
IP.1 = 127.0.0.1
DNS.1 = localhost
EOF

    openssl req -newkey rsa:2048 -nodes \
        -keyout "$cert_dir/server.key" \
        -out "$cert_dir/server.csr" \
        -config "$cert_dir/server.cnf" >/dev/null 2>&1
    openssl x509 -req \
        -in "$cert_dir/server.csr" \
        -CA "$cert_dir/ca.pem" \
        -CAkey "$cert_dir/ca.key" \
        -CAcreateserial \
        -out "$cert_dir/server.pem" \
        -days 1 \
        -sha256 \
        -extfile "$cert_dir/server.cnf" \
        -extensions v3_req >/dev/null 2>&1
}

start_https_mock_relay() {
    local port="$1"
    local cert_file="$2"
    local key_file="$3"
    local log_file="$4"

    python3 -u - "$port" "$cert_file" "$key_file" >"$log_file" 2>&1 <<'PY' &
import json
import ssl
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port = int(sys.argv[1])
cert_file = sys.argv[2]
key_file = sys.argv[3]
pairings = {}

class Handler(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        return

    def _write_json(self, status, payload):
        body = json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path == "/healthz":
            self._write_json(200, {"ok": True})
            return

        if self.path.startswith("/v5/remote-invoke/pairings/") and self.path.split("?")[0].endswith("/watch"):
            pairing_id = self.path.split("/")[4]
            meta = pairings.get(pairing_id, {})
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Cache-Control", "no-cache")
            self.end_headers()
            self.wfile.write(b"event: connected\n")
            self.wfile.write(f"data: {json.dumps({'pairing_id': pairing_id})}\n\n".encode())
            self.wfile.flush()
            time.sleep(0.05)
            payload = {
                "status": "approved",
                "claim_token": meta.get("claim_token", "claim-tls"),
                "claim_expires_at": "2099-01-01T00:00:00Z",
                "grant_summary": {
                    "scope": "remote_shell_exec",
                    "mode": "permanent",
                    "file_access": "read_write",
                },
            }
            self.wfile.write(b"event: approved\n")
            self.wfile.write(f"data: {json.dumps(payload)}\n\n".encode())
            self.wfile.flush()
            return

        self._write_json(404, {"code": 404, "message": "not_found", "data": None})

    def do_POST(self):
        if self.path == "/v5/remote-invoke/grants/claim":
            self._write_json(
                200,
                {
                    "code": 0,
                    "message": "ok",
                    "data": {
                        "grant_session_token": "session-tls",
                        "expires_at": "2099-01-01T00:00:00Z",
                        "grant_summary": {
                            "scope": "remote_shell_exec",
                            "mode": "permanent",
                            "file_access": "read_write",
                            "client_ephemeral_pub": "ZRGxH2hR6PqQEaOn6Hxc1eTHFH0CyK2xm8lfajUKXTg=",
                        },
                    },
                },
            )
            return

        if self.path != "/v5/remote-invoke/pairings/start":
            self._write_json(404, {"code": 404, "message": "not_found", "data": None})
            return

        content_length = int(self.headers.get("Content-Length", "0"))
        raw = self.rfile.read(content_length or 0)
        body = json.loads(raw.decode() or "{}")
        pair_code = body.get("pair_code", "")
        pairing_id = f"tls-pairing-{pair_code}"
        pairings[pairing_id] = {"pair_code": pair_code, "claim_token": f"claim-tls-{pair_code}"}
        print(f"relay=tls pair_code={pair_code}", flush=True)
        self._write_json(
            200,
            {
                "code": 0,
                "message": "ok",
                "data": {
                    "pairing_id": pairing_id,
                    "watch_token": "watch-tls",
                    "client_instance_id": "client-tls-123456",
                    "approval_sse_url": f"/v5/remote-invoke/pairings/{pairing_id}/watch",
                },
            },
        )

server = ThreadingHTTPServer(("127.0.0.1", port), Handler)
context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
context.load_cert_chain(certfile=cert_file, keyfile=key_file)
server.socket = context.wrap_socket(server.socket, server_side=True)
server.serve_forever()
PY
    SERVER_PID=$!
    wait_for_server "https://127.0.0.1:${port}" "$log_file"
}

assert_connections_file_has() {
    local data_dir="$1"
    local expected="$2"
    local connections_file="$data_dir/remote-connections.json"
    [[ -f "$connections_file" ]] || {
        echo "missing connections file: $connections_file" >&2
        exit 1
    }
    assert_body_contains "$expected" "$(cat "$connections_file")" "connections file should contain ${expected}" || exit 1
}

run_connect_without_extra_ca() {
    local data_dir="$1"
    shift
    env -u BIFROST_REMOTE_RELAY_CA_BUNDLE \
        -u BIFROST_REMOTE_UNSAFE_SSL \
        -u SSL_CERT_FILE \
        -u REQUESTS_CA_BUNDLE \
        -u CURL_CA_BUNDLE \
        -u NODE_EXTRA_CA_CERTS \
        -u GIT_SSL_CAINFO \
        -u AWS_CA_BUNDLE \
        -u PIP_CERT \
        -u NPM_CONFIG_CAFILE \
        -u npm_config_cafile \
        -u GRPC_DEFAULT_SSL_ROOTS_FILE_PATH \
        -u SSL_CERT_DIR \
        BIFROST_DATA_DIR="$data_dir" "$BIFROST_BIN" "$@" 2>&1
}

run_connect_with_ca_bundle() {
    local data_dir="$1"
    local ca_bundle="$2"
    shift 2
    BIFROST_REMOTE_RELAY_CA_BUNDLE="$ca_bundle" \
        BIFROST_DATA_DIR="$data_dir" "$BIFROST_BIN" "$@" 2>&1
}

run_connect_with_unsafe_ssl() {
    local data_dir="$1"
    shift
    env -u BIFROST_REMOTE_RELAY_CA_BUNDLE \
        -u SSL_CERT_FILE \
        -u REQUESTS_CA_BUNDLE \
        -u CURL_CA_BUNDLE \
        -u NODE_EXTRA_CA_CERTS \
        -u GIT_SSL_CAINFO \
        -u AWS_CA_BUNDLE \
        -u PIP_CERT \
        -u NPM_CONFIG_CAFILE \
        -u npm_config_cafile \
        -u GRPC_DEFAULT_SSL_ROOTS_FILE_PATH \
        -u SSL_CERT_DIR \
        BIFROST_REMOTE_UNSAFE_SSL=1 \
        BIFROST_DATA_DIR="$data_dir" "$BIFROST_BIN" "$@" 2>&1
}

BIFROST_BIN="${BIFROST_BIN:-$ROOT_DIR/target/release/bifrost}"
if [[ "${SKIP_BUILD:-false}" == "true" && -x "$BIFROST_BIN" ]]; then
    log "Using existing bifrost binary: $BIFROST_BIN"
else
    log "Build bifrost (release)..."
    (cd "$ROOT_DIR" && SKIP_FRONTEND_BUILD=1 cargo build --release --bin bifrost >/dev/null 2>&1)
fi

if [[ ! -x "$BIFROST_BIN" ]]; then
    echo "bifrost binary not found at $BIFROST_BIN" >&2
    exit 1
fi

TEST_ROOT="$(mktemp -d "${ROOT_DIR}/.bifrost-e2e-remote-relay-tls.XXXXXX")"
CERT_DIR="$TEST_ROOT/certs"
SERVER_LOG="$TEST_ROOT/https-relay.log"
RELAY_PORT="$(pick_free_port)"
RELAY_URL="https://127.0.0.1:${RELAY_PORT}"

generate_private_ca_and_server_cert "$CERT_DIR"
start_https_mock_relay "$RELAY_PORT" "$CERT_DIR/server.pem" "$CERT_DIR/server.key" "$SERVER_LOG"

FAIL_DIR="$TEST_ROOT/fail"
mkdir -p "$FAIL_DIR"

log "Case 1: private relay CA should fail when no extra trust source is configured"
FAIL_OUTPUT="$(run_connect_without_extra_ca "$FAIL_DIR" remote conn up 882001 --relay-url "$RELAY_URL")"
FAIL_EXIT=$?
if ! assert_status "1" "$FAIL_EXIT" "private relay CA should be rejected without trust config"; then
    echo "$FAIL_OUTPUT" >&2
    exit 1
fi
assert_body_contains "start pairing failed" "$FAIL_OUTPUT" "untrusted private relay should fail before pairing starts" || {
    echo "$FAIL_OUTPUT" >&2
    exit 1
}
assert_body_contains "error sending request" "$FAIL_OUTPUT" "untrusted private relay should fail in the HTTPS request layer" || {
    echo "$FAIL_OUTPUT" >&2
    exit 1
}

PASS_DIR="$TEST_ROOT/pass"
mkdir -p "$PASS_DIR"

log "Case 2: BIFROST_REMOTE_RELAY_CA_BUNDLE should trust the private relay CA"
PASS_OUTPUT="$(run_connect_with_ca_bundle "$PASS_DIR" "$CERT_DIR/ca.pem" remote conn up 882002 --relay-url "$RELAY_URL")"
PASS_EXIT=$?
if ! assert_status "0" "$PASS_EXIT" "private relay CA should be trusted with BIFROST_REMOTE_RELAY_CA_BUNDLE"; then
    echo "$PASS_OUTPUT" >&2
    [[ -f "$SERVER_LOG" ]] && cat "$SERVER_LOG" >&2
    exit 1
fi
assert_body_contains "Connected! Authorization granted" "$PASS_OUTPUT" "trusted private relay should connect successfully" || exit 1
assert_connections_file_has "$PASS_DIR" "$RELAY_URL"
assert_connections_file_has "$PASS_DIR" "client-tls-123456"

UNSAFE_DIR="$TEST_ROOT/unsafe"
mkdir -p "$UNSAFE_DIR"

log "Case 3: BIFROST_REMOTE_UNSAFE_SSL should bypass relay certificate validation as last resort"
UNSAFE_OUTPUT="$(run_connect_with_unsafe_ssl "$UNSAFE_DIR" remote conn up 882003 --relay-url "$RELAY_URL")"
UNSAFE_EXIT=$?
if ! assert_status "0" "$UNSAFE_EXIT" "private relay CA should connect with BIFROST_REMOTE_UNSAFE_SSL"; then
    echo "$UNSAFE_OUTPUT" >&2
    [[ -f "$SERVER_LOG" ]] && cat "$SERVER_LOG" >&2
    exit 1
fi
assert_body_contains "Connected! Authorization granted" "$UNSAFE_OUTPUT" "unsafe remote relay should connect successfully" || exit 1
assert_connections_file_has "$UNSAFE_DIR" "$RELAY_URL"
assert_connections_file_has "$UNSAFE_DIR" "client-tls-123456"

if [[ "$FAILED_ASSERTIONS" -gt 0 ]]; then
    exit 1
fi

echo "All remote relay TLS trust assertions passed."
