#!/bin/bash
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT

set -euo pipefail

unset http_proxy https_proxy HTTP_PROXY HTTPS_PROXY all_proxy ALL_PROXY no_proxy NO_PROXY

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

source "$ROOT_DIR/e2e-tests/test_utils/assert.sh"
source "$ROOT_DIR/e2e-tests/test_utils/admin_client.sh"

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

TEST_ROOT="$(mktemp -d)"
export ADMIN_PORT="$(pick_free_port)"
export BIFROST_DATA_DIR="${TEST_ROOT}/bifrost-data"
export BIFROST_BIN="${BIFROST_BIN:-${ROOT_DIR}/target/debug/bifrost}"

cleanup() {
    admin_cleanup_bifrost
    rm -rf "$TEST_ROOT" 2>/dev/null || true
}
trap cleanup EXIT

log() { echo "[sync-github-gist-expired-e2e] $*"; }

mkdir -p "$BIFROST_DATA_DIR"
cat >"${BIFROST_DATA_DIR}/sync-state.json" <<'JSON'
{
  "token": null,
  "user": null,
  "provider_sessions": {
    "github_gist": {
      "token": "ghp_expired_bifrost_e2e_token",
      "remote_base_url": "https://api.github.com/gists",
      "user": {
        "user_id": "github:e2e-expired",
        "nickname": "GitHub E2E Expired",
        "avatar": "",
        "email": ""
      }
    }
  },
  "last_sync_at": null,
  "last_sync_action": null,
  "startup_login_prompt": null,
  "deleted_rules": {},
  "basic_configs": {}
}
JSON

if [[ "${SKIP_BUILD:-false}" == "true" ]]; then
    if [[ ! -x "$BIFROST_BIN" ]]; then
        echo "SKIP_BUILD=true but BIFROST_BIN is not executable: $BIFROST_BIN" >&2
        exit 1
    fi
    log "Using prebuilt Bifrost binary: $BIFROST_BIN"
else
    log "Building current debug bifrost binary"
    (cd "$ROOT_DIR" && cargo build --bin bifrost)
fi

log "Starting Bifrost admin with an expired GitHub Gist provider session"
admin_start_bifrost

status=""
for _ in $(seq 1 40); do
    status="$(admin_get "/api/sync/status")"
    if echo "$status" | jq -e '
      .providers[]
      | select(.id == "github_gist")
      | .connected == true
        and .authorized == false
        and .reason == "error"
        and (.last_error | type == "string")
        and (.last_error | test("GitHub token is invalid|gist scope|does not have access"))
    ' >/dev/null; then
        break
    fi
    sleep 0.5
done

if ! echo "$status" | jq -e '
  .providers[]
  | select(.id == "github_gist")
  | .connected == true
    and .authorized == false
    and .reason == "error"
    and (.last_error | type == "string")
    and (.last_error | test("GitHub token is invalid|gist scope|does not have access"))
' >/dev/null; then
    echo "Last /api/sync/status did not contain the expected expired GitHub Gist provider state:" >&2
    echo "$status" | jq . >&2 || echo "$status" >&2
    exit 1
fi

badge_payload="$(echo "$status" | jq -c '.providers[] | select(.id == "github_gist")')"
assert_body_contains '"connected":true' "$badge_payload" "saved GitHub Gist session should remain available for reconnect/sign-out" || exit 1
assert_body_contains '"authorized":false' "$badge_payload" "expired GitHub Gist token should mark provider unauthorized" || exit 1
assert_body_contains '"reason":"error"' "$badge_payload" "expired GitHub Gist token should mark provider reason=error" || exit 1
assert_body_contains '"last_error"' "$badge_payload" "expired GitHub Gist token should expose provider last_error" || exit 1

log "Expired GitHub Gist provider status was surfaced correctly"
