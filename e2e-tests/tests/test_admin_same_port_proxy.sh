#!/usr/bin/env bash
set -euo pipefail

export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1
export BIFROST_DISABLE_TRAY=1
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
BIFROST_BIN="${BIFROST_BIN:-$ROOT_DIR/target/debug/bifrost}"
DATA_DIR="$(mktemp -d "$ROOT_DIR/.bifrost-e2e-admin-routing.XXXXXX")"
PROXY_PID=""
TARGET_PID=""
cleanup() {
    for pid in "$PROXY_PID" "$TARGET_PID"; do
        if [[ -n "$pid" ]]; then
            kill "$pid" 2>/dev/null || true
            wait "$pid" 2>/dev/null || true
        fi
    done
    rm -rf "$DATA_DIR"
}
trap cleanup EXIT

free_port() {
    python3 - <<'PY'
import socket
with socket.socket() as sock:
    sock.bind(("127.0.0.1", 0))
    port = sock.getsockname()[1]
    assert port != 9900
    print(port)
PY
}
PROXY_PORT="$(free_port)"
TARGET_PORT="$(free_port)"
mkdir -p "$DATA_DIR/target/_bifrost/api"
printf 'remote-admin-target\n' > "$DATA_DIR/target/_bifrost/api/rules"
python3 -m http.server "$TARGET_PORT" --bind 127.0.0.1 --directory "$DATA_DIR/target" \
    > "$DATA_DIR/target.log" 2>&1 &
TARGET_PID=$!
export BIFROST_ROUTING_TARGET="127.0.0.1:$TARGET_PORT"
BIFROST_DATA_DIR="$DATA_DIR" "$BIFROST_BIN" start \
    --host 0.0.0.0 --port "$PROXY_PORT" --yes --skip-cert-check \
    --no-system-proxy --no-intercept \
    --rules-file "$ROOT_DIR/e2e-tests/rules/forwarding/admin_same_port.txt" \
    > "$DATA_DIR/proxy.log" 2>&1 &
PROXY_PID=$!
for _ in {1..120}; do
    if curl --noproxy '*' -fsS "http://127.0.0.1:$PROXY_PORT/_bifrost/api/auth/status" >/dev/null 2>&1 \
        && curl --noproxy '*' -fsS "http://127.0.0.1:$TARGET_PORT/_bifrost/api/rules" >/dev/null 2>&1; then
        break
    fi
    if ! kill -0 "$PROXY_PID" 2>/dev/null; then
        cat "$DATA_DIR/proxy.log" >&2
        exit 1
    fi
    sleep 0.25
done
curl --noproxy '*' -fsS "http://127.0.0.1:$PROXY_PORT/_bifrost/api/auth/status" >/dev/null
body="$(curl --noproxy '' -fsS --max-time 10 -x "http://127.0.0.1:$PROXY_PORT" \
    "http://192.0.2.80:$PROXY_PORT/_bifrost/api/rules")"
[[ "$body" == 'remote-admin-target' ]]
echo 'PASS: same-port remote admin path reaches the proxy target'

status="$(curl --noproxy '' -sS --max-time 10 -o /dev/null -w '%{http_code}' \
    -x "http://127.0.0.1:$PROXY_PORT" "http://127.0.0.1:$PROXY_PORT/_bifrost/api/rules")"
[[ "$status" == 403 ]]
echo 'PASS: absolute-form local admin request is still rejected'

curl --noproxy '' -fsS --max-time 10 -x "http://127.0.0.1:$PROXY_PORT" \
    "http://bifrost.local:$PROXY_PORT/_bifrost/api/auth/status" >/dev/null
echo 'PASS: bifrost.local still reaches the local admin API'
