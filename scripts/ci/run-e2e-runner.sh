#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SYNC_SERVER_PID=""
SYNC_SERVER_DATA_DIR=""

cleanup() {
  if [[ -n "$SYNC_SERVER_PID" ]] && kill -0 "$SYNC_SERVER_PID" 2>/dev/null; then
    kill "$SYNC_SERVER_PID" 2>/dev/null || true
    wait "$SYNC_SERVER_PID" 2>/dev/null || true
  fi
  if [[ -n "$SYNC_SERVER_DATA_DIR" && -d "$SYNC_SERVER_DATA_DIR" ]]; then
    for _ in $(seq 1 5); do
      rm -rf "$SYNC_SERVER_DATA_DIR" 2>/dev/null && return 0
      sleep 1
    done
    echo "warning: failed to remove sync-server data dir: $SYNC_SERVER_DATA_DIR" >&2
  fi
}
trap cleanup EXIT

pick_unused_port() {
  python3 - <<'PY'
import socket
with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
    sock.bind(("127.0.0.1", 0))
    print(sock.getsockname()[1])
PY
}

start_e2e_sync_server() {
  # Rust E2E group-rule tests exercise sync-backed paths. Keep them on the
  # repo-local test sync server instead of the production default URL.
  # shellcheck source=/dev/null
  source "$ROOT_DIR/e2e-tests/test_utils/sync_server.sh"

  local sync_server_dir
  sync_server_dir="$(sync_server_dir)"
  if [[ ! -d "$sync_server_dir/node_modules" ]]; then
    (cd "$sync_server_dir" && pnpm install --frozen-lockfile)
  fi
  if ! sync_server_has_fresh_dist_entry "$sync_server_dir"; then
    (cd "$sync_server_dir" && pnpm run build)
  fi

  local sync_port sync_url sync_exec sync_log
  sync_port="$(pick_unused_port)"
  sync_url="http://127.0.0.1:${sync_port}"
  SYNC_SERVER_DATA_DIR="$(mktemp -d)"
  sync_log="${SYNC_SERVER_DATA_DIR}/sync-server.log"
  sync_exec="$(sync_server_exec "$sync_server_dir")"

  (
    cd "$sync_server_dir"
    eval "$sync_exec" -H 127.0.0.1 -p "$sync_port" -d "$SYNC_SERVER_DATA_DIR" --enable-remote-invoke
  ) >"$sync_log" 2>&1 &
  SYNC_SERVER_PID=$!

  for _ in $(seq 1 60); do
    if ! kill -0 "$SYNC_SERVER_PID" 2>/dev/null; then
      cat "$sync_log" >&2 || true
      echo "sync-server exited before becoming ready" >&2
      return 1
    fi
    if curl -s --connect-timeout 1 --max-time 2 "${sync_url}/v4/sso/check" >/dev/null 2>&1; then
      break
    fi
    sleep 1
  done

  if ! curl -s --connect-timeout 1 --max-time 2 "${sync_url}/v4/sso/check" >/dev/null 2>&1; then
    cat "$sync_log" >&2 || true
    echo "sync-server did not become ready" >&2
    return 1
  fi

  local register_body register_response sync_token
  register_body='{"user_id":"e2e_runner_user","password":"e2e_runner_123","nickname":"E2E Runner"}'
  register_response="$(curl -sS --connect-timeout 2 --max-time 10 \
    -H 'Content-Type: application/json' \
    -d "$register_body" \
    "${sync_url}/v4/sso/register")"
  sync_token="$(python3 -c 'import json,sys; print(json.load(sys.stdin).get("data",{}).get("token",""))' <<<"$register_response")"
  if [[ -z "$sync_token" ]]; then
    echo "failed to register sync-server E2E user: $register_response" >&2
    return 1
  fi

  export BIFROST_E2E_SYNC_BASE_URL="$sync_url"
  export BIFROST_E2E_SYNC_TOKEN="$sync_token"
  echo "Started local sync-server for Rust E2E at ${sync_url}"
}

cd "$ROOT_DIR"
export BIFROST_E2E=1
start_e2e_sync_server
bash scripts/run_all_e2e.sh --ci --skip-rules --skip-shell --skip-ui --skip-build
