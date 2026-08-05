#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

source "${PROJECT_DIR}/e2e-tests/test_utils/process.sh"

allocate_port() {
	python3 - <<'PY'
import socket

with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
    sock.bind(("127.0.0.1", 0))
    print(sock.getsockname()[1])
PY
}

BIFROST_BIN="${BIFROST_BIN:-${PROJECT_DIR}/target/debug/bifrost}"
PROXY_PORT="${PROXY_PORT:-$(allocate_port)}"
TEST_DATA_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-videos-removed.XXXXXX")"
PROXY_PID=""

mark_e2e_data_root "$TEST_DATA_DIR"

cleanup() {
	safe_cleanup_proxy "$PROXY_PID"
	if [[ -n "$TEST_DATA_DIR" && -d "$TEST_DATA_DIR" ]]; then
		rm -rf "$TEST_DATA_DIR"
	fi
}
trap cleanup EXIT

if [[ ! -x "$BIFROST_BIN" || "${SKIP_BUILD:-false}" != "true" ]]; then
	(cd "$PROJECT_DIR" && SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost)
fi

BIFROST_DATA_DIR="$TEST_DATA_DIR" \
	BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
	BIFROST_DISABLE_TRAY=1 \
	"$BIFROST_BIN" start \
	-p "$PROXY_PORT" \
	--host 127.0.0.1 \
	--access-mode allow_all \
	--skip-cert-check \
	--no-system-proxy \
	--no-intercept \
	-y >"$TEST_DATA_DIR/proxy.log" 2>&1 &
PROXY_PID=$!

for _ in {1..120}; do
	if curl -fsS "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/proxy/address" >/dev/null 2>&1; then
		break
	fi
	if ! kill -0 "$PROXY_PID" 2>/dev/null; then
		echo "Bifrost exited before the Admin API became ready" >&2
		sed -n '1,200p' "$TEST_DATA_DIR/proxy.log" >&2
		exit 1
	fi
	sleep 0.25
done

curl -fsS "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/proxy/address" >/dev/null
lsof -nP -a -p "$PROXY_PID" -iTCP:"$PROXY_PORT" -sTCP:LISTEN >/dev/null

STATUS="$(curl -sS \
	-o "$TEST_DATA_DIR/videos-response.json" \
	-w '%{http_code}' \
	"http://127.0.0.1:${PROXY_PORT}/_bifrost/api/videos/defaults")"

if [[ "$STATUS" != "404" ]]; then
	echo "Expected removed Videos API to return 404, got $STATUS" >&2
	cat "$TEST_DATA_DIR/videos-response.json" >&2
	exit 1
fi

python3 - "$TEST_DATA_DIR/videos-response.json" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as source:
    payload = json.load(source)

assert payload == {"error": "API endpoint not found", "status": 404}, payload
PY

echo "Videos tool removal E2E passed: retired API returns the shared 404 response"
