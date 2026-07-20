#!/usr/bin/env bash
#
# Bifrost upgrade GitHub TLS trust E2E.
# Verifies release archive downloads through a private-CA HTTPS mirror.

set -euo pipefail

: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
: "${BIFROST_DISABLE_TRAY:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
export BIFROST_DISABLE_TRAY

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"
if [[ -n "${BIFROST_BIN:-}" ]]; then
    SHOULD_BUILD=0
else
    BIFROST_BIN="${PROJECT_DIR}/target/release/bifrost"
    SHOULD_BUILD=1
fi
if [[ ! -x "$BIFROST_BIN" && -f "${BIFROST_BIN}.exe" ]]; then
    BIFROST_BIN="${BIFROST_BIN}.exe"
fi

PASSED=0
FAILED=0
TMP_DIR=""
SERVER_PID=""
SERVER_LOG=""

pass() {
    echo "  ✓ $1"
    PASSED=$((PASSED + 1))
}

fail() {
    echo "  ✗ $1"
    FAILED=$((FAILED + 1))
}

cleanup() {
    if [[ -n "$SERVER_PID" ]]; then
        kill "$SERVER_PID" >/dev/null 2>&1 || true
        wait "$SERVER_PID" >/dev/null 2>&1 || true
    fi
    if [[ -n "$TMP_DIR" && -d "$TMP_DIR" ]]; then
        rm -rf "$TMP_DIR"
    fi
}
trap cleanup EXIT

require_tool() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "missing required tool: $1" >&2
        exit 1
    fi
}

target_triple() {
    local host
    host="$(rustc -vV | awk '/^host:/ {print $2}')"
    case "$host" in
        x86_64-unknown-linux-gnu)
            if ldd --version 2>&1 | python3 -c 'import re,sys; text=sys.stdin.read(); m=re.search(r"(\d+)\.(\d+)", text); sys.exit(0 if (not m or (int(m.group(1)), int(m.group(2))) < (2,39)) else 1)'; then
                echo "x86_64-unknown-linux-musl"
            else
                echo "$host"
            fi
            ;;
        aarch64-unknown-linux-gnu)
            if ldd --version 2>&1 | python3 -c 'import re,sys; text=sys.stdin.read(); m=re.search(r"(\d+)\.(\d+)", text); sys.exit(0 if (not m or (int(m.group(1)), int(m.group(2))) < (2,39)) else 1)'; then
                echo "aarch64-unknown-linux-musl"
            else
                echo "$host"
            fi
            ;;
        *)
            echo "$host"
            ;;
    esac
}

start_https_server() {
    local root="$1"
    local cert="$2"
    local key="$3"
    local port_file="$4"
    local server_log="$5"
    python3 - "$root" "$cert" "$key" "$port_file" >"$server_log" 2>&1 <<'PY' &
import functools
import http.server
import pathlib
import ssl
import sys

root, cert, key, port_file = sys.argv[1:5]
handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=root)
httpd = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
context.load_cert_chain(certfile=cert, keyfile=key)
httpd.socket = context.wrap_socket(httpd.socket, server_side=True)
pathlib.Path(port_file).write_text(str(httpd.server_address[1]))
httpd.serve_forever()
PY
    SERVER_PID=$!
}

run_upgrade() {
    local bin="$1"
    shift
    env \
        BIFROST_DATA_DIR="$TMP_DIR/data" \
        BIFROST_GITHUB_MIRROR="$MIRROR_URL" \
        BIFROST_UPGRADE_TEST_ALLOW_RELEASE_OVERRIDES=1 \
        BIFROST_UPGRADE_TEST_LATEST_VERSION="$TEST_VERSION" \
        BIFROST_DOWNLOAD_TRIES=1 \
        BIFROST_DOWNLOAD_CONNECT_TIMEOUT=2 \
        BIFROST_DOWNLOAD_TIMEOUT=8 \
        BIFROST_MIRROR_PROBE_TIMEOUT=2 \
        "$@" \
        "$bin" upgrade
}

require_tool rustc
require_tool python3
require_tool openssl
require_tool tar
require_tool curl

if [[ "$SHOULD_BUILD" == "1" || ! -x "$BIFROST_BIN" ]]; then
    echo "building release bifrost binary..."
    (cd "$PROJECT_DIR" && cargo build --release --bin bifrost)
fi

TMP_DIR="$(mktemp -d)"
mkdir -p "$TMP_DIR/bin" "$TMP_DIR/mirror" "$TMP_DIR/data"

TEST_VERSION="99.99.99-test"
TARGET="$(target_triple)"
ARCHIVE_DIR="$TMP_DIR/mirror/bifrost-proxy/bifrost/releases/download/v${TEST_VERSION}/bifrost-v${TEST_VERSION}-${TARGET}"
mkdir -p "$ARCHIVE_DIR"
# The production upgrader verifies the installed binary reports the exact pinned
# target. Build a fixture executable that reports TEST_VERSION for --version and
# delegates every other command to the real release binary.
python3 - "$ARCHIVE_DIR/bifrost" "$BIFROST_BIN" "$TEST_VERSION" <<'PY'
import pathlib
import shlex
import sys

fixture_path, source_binary, version = sys.argv[1:4]
pathlib.Path(fixture_path).write_text(
    "#!/bin/sh\n"
    "if [ \"${1:-}\" = \"--version\" ]; then\n"
    f"    printf '%s\\n' 'bifrost {version}'\n"
    "    exit 0\n"
    "fi\n"
    f"exec {shlex.quote(source_binary)} \"$@\"\n"
)
PY
chmod +x "$ARCHIVE_DIR/bifrost"
tar -C "$(dirname "$ARCHIVE_DIR")" -czf "$(dirname "$ARCHIVE_DIR")/bifrost-v${TEST_VERSION}-${TARGET}.tar.gz" "$(basename "$ARCHIVE_DIR")"

cat > "$TMP_DIR/openssl.cnf" <<'EOF'
[ req ]
distinguished_name = dn
x509_extensions = v3_ca
prompt = no

[ dn ]
CN = Bifrost Upgrade Test CA

[ v3_ca ]
basicConstraints = critical,CA:TRUE
keyUsage = critical,keyCertSign,cRLSign
subjectKeyIdentifier = hash
authorityKeyIdentifier = keyid:always,issuer
EOF

cat > "$TMP_DIR/server.cnf" <<'EOF'
[ req ]
distinguished_name = dn
req_extensions = v3_req
prompt = no

[ dn ]
CN = 127.0.0.1

[ v3_req ]
basicConstraints = CA:FALSE
keyUsage = digitalSignature,keyEncipherment
extendedKeyUsage = serverAuth
subjectAltName = @alt_names

[ alt_names ]
IP.1 = 127.0.0.1
DNS.1 = localhost
EOF

openssl req -x509 -newkey rsa:2048 -days 1 -nodes \
    -keyout "$TMP_DIR/ca.key" \
    -out "$TMP_DIR/ca.pem" \
    -config "$TMP_DIR/openssl.cnf" >/dev/null 2>&1
openssl req -newkey rsa:2048 -nodes \
    -keyout "$TMP_DIR/server.key" \
    -out "$TMP_DIR/server.csr" \
    -config "$TMP_DIR/server.cnf" >/dev/null 2>&1
openssl x509 -req -days 1 \
    -in "$TMP_DIR/server.csr" \
    -CA "$TMP_DIR/ca.pem" \
    -CAkey "$TMP_DIR/ca.key" \
    -CAcreateserial \
    -out "$TMP_DIR/server.pem" \
    -extensions v3_req \
    -extfile "$TMP_DIR/server.cnf" >/dev/null 2>&1

PORT_FILE="$TMP_DIR/https-port"
SERVER_LOG="$TMP_DIR/https-server.log"
start_https_server "$TMP_DIR/mirror" "$TMP_DIR/server.pem" "$TMP_DIR/server.key" "$PORT_FILE" "$SERVER_LOG"
for _ in {1..300}; do
    [[ -s "$PORT_FILE" ]] && break
    if [[ -n "$SERVER_PID" ]] && ! kill -0 "$SERVER_PID" 2>/dev/null; then
        break
    fi
    sleep 0.1
done
if [[ ! -s "$PORT_FILE" ]]; then
    echo "HTTPS mirror did not start" >&2
    cat "$SERVER_LOG" >&2 2>/dev/null || true
    exit 1
fi
MIRROR_URL="https://127.0.0.1:$(cat "$PORT_FILE")"
if ! env NO_PROXY="*" no_proxy="*" curl --cacert "$TMP_DIR/ca.pem" \
    -fsS --connect-timeout 1 --max-time 3 \
    "$MIRROR_URL/" >/dev/null 2>&1; then
    echo "HTTPS mirror bound a port but did not answer requests" >&2
    cat "$SERVER_LOG" >&2 2>/dev/null || true
    exit 1
fi

NO_CA_BIN="$TMP_DIR/bin/bifrost-no-ca"
CA_BIN="$TMP_DIR/bin/bifrost-ca"
UNSAFE_BIN="$TMP_DIR/bin/bifrost-unsafe"
cp "$BIFROST_BIN" "$NO_CA_BIN"
cp "$BIFROST_BIN" "$CA_BIN"
cp "$BIFROST_BIN" "$UNSAFE_BIN"
chmod +x "$NO_CA_BIN" "$CA_BIN" "$UNSAFE_BIN"

echo "Test mirror: $MIRROR_URL"
echo "Target triple: $TARGET"

set +e
NO_CA_OUTPUT="$(run_upgrade "$NO_CA_BIN" 2>&1)"
NO_CA_STATUS=$?
set -e
if [[ $NO_CA_STATUS -ne 0 ]] && echo "$NO_CA_OUTPUT" | grep -Eiq 'certificate|tls|ssl|UnknownIssuer|invalid peer'; then
    pass "upgrade download fails against private-CA mirror without extra trust"
else
    fail "upgrade without CA should fail on certificate trust; status=$NO_CA_STATUS output=$NO_CA_OUTPUT"
fi

set +e
CA_OUTPUT="$(run_upgrade "$CA_BIN" BIFROST_GITHUB_CA_BUNDLE="$TMP_DIR/ca.pem" 2>&1)"
CA_STATUS=$?
set -e
if [[ $CA_STATUS -eq 0 ]] && echo "$CA_OUTPUT" | grep -qi 'Upgrade completed successfully'; then
    pass "BIFROST_GITHUB_CA_BUNDLE lets upgrade download from private-CA mirror"
else
    fail "upgrade with BIFROST_GITHUB_CA_BUNDLE failed; status=$CA_STATUS output=$CA_OUTPUT"
fi

set +e
UNSAFE_OUTPUT="$(run_upgrade "$UNSAFE_BIN" BIFROST_UPGRADE_UNSAFE_SSL=1 2>&1)"
UNSAFE_STATUS=$?
set -e
if [[ $UNSAFE_STATUS -eq 0 ]] && echo "$UNSAFE_OUTPUT" | grep -qi 'Upgrade completed successfully'; then
    pass "BIFROST_UPGRADE_UNSAFE_SSL lets upgrade download from private-CA mirror as fallback"
else
    fail "upgrade with BIFROST_UPGRADE_UNSAFE_SSL failed; status=$UNSAFE_STATUS output=$UNSAFE_OUTPUT"
fi

echo ""
echo "Passed: $PASSED"
echo "Failed: $FAILED"
if [[ $FAILED -ne 0 ]]; then
    exit 1
fi
