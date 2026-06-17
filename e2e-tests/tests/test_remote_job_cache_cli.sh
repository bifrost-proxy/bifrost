#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

if [[ -z "${BIFROST_BIN:-}" ]]; then
  (cd "$ROOT_DIR" && cargo build -p bifrost-cli --bin bifrost)
  BIFROST_BIN="$ROOT_DIR/target/debug/bifrost"
elif [[ ! -x "$BIFROST_BIN" ]]; then
  echo "BIFROST_BIN is not executable: $BIFROST_BIN" >&2
  exit 1
fi

tmpdir="$(mktemp -d)"
old_token_err="$tmpdir/old-token.err"
missing_err="$tmpdir/missing.err"
trap 'rm -rf "$tmpdir"' EXIT

export BIFROST_DATA_DIR="$tmpdir/data"
mkdir -p "$BIFROST_DATA_DIR"

"$BIFROST_BIN" remote job list | grep -q 'No detached remote jobs remembered locally'

cat >"$BIFROST_DATA_DIR/remote-jobs.json" <<'JSON'
{
  "version": 1,
  "jobs": [
    {
      "call_id": "call-cache-1",
      "relay_token_encrypted": "{\"version\":1,\"nonce\":\"invalid\",\"ciphertext\":\"invalid\"}",
      "relay_url": "https://relay.example.invalid",
      "client_instance_id": "client-1",
      "device_name": "dev-box",
      "command_preview": "cargo test",
      "status": "running",
      "created_at": 1,
      "updated_at": 1
    }
  ]
}
JSON

"$BIFROST_BIN" remote job list | grep -q 'call-cache-1'
"$BIFROST_BIN" remote job list | grep -q 'dev-box'
"$BIFROST_BIN" remote job list | grep -q 'cargo test'

"$BIFROST_BIN" remote job status --help >"$tmpdir/status-help.txt"
if grep -q -- '--relay-token' "$tmpdir/status-help.txt"; then
  echo "remote job status help must not expose --relay-token" >&2
  exit 1
fi

if "$BIFROST_BIN" remote job status call-cache-1 --relay-token tok >/dev/null 2>"$old_token_err"; then
  echo "remote job status unexpectedly accepted --relay-token" >&2
  exit 1
fi
grep -Eq 'unexpected argument|--relay-token' "$old_token_err"

rm -f "$BIFROST_DATA_DIR/remote-jobs.json"
if "$BIFROST_BIN" remote job status call-missing >/dev/null 2>"$missing_err"; then
  echo "remote job status unexpectedly succeeded without a local job cache" >&2
  exit 1
fi
grep -q 'no local remote job cache' "$missing_err"
grep -q 'bifrost remote job list' "$missing_err"
grep -q 'bifrost remote exec --detach' "$missing_err"

echo "remote job cache CLI contract: PASS"
