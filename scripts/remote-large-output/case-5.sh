#!/usr/bin/env bash
# Case 5: legacy envelope + stream_frame dual-track parallel validation.
#
# PR #6c injected parallel emission of stream_frame alongside the legacy
# call_frame envelope. This test runs the same command in both modes and
# verifies:
#   a) stream mode (stream_frame path) produces the correct bytes/sha.
#   b) legacy mode (no --stream) produces the same bytes/sha (up to the
#      policy inline-preview cap; bytes beyond cap are expected to be
#      clipped but SHA-256 of the full stream is recorded in the response
#      meta, which we extract from JSON output and compare).
#
# This guards against regressions where the two tracks diverge.

set -euo pipefail

: "${BIFROST_BIN:=bifrost}"
PAYLOAD_BYTES=$((4 * 1024 * 1024))   # 4 MiB, small enough for legacy preview

TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

SRC="$TMPDIR/src.sh"
STREAM_OUT="$TMPDIR/stream.bin"
LEGACY_JSON="$TMPDIR/legacy.json"

cat > "$SRC" <<EOF
#!/usr/bin/env bash
set -e
openssl enc -aes-256-ctr -pass pass:bifrost-pr8-case5 -in /dev/zero -nosalt 2>/dev/null \
  | head -c $PAYLOAD_BYTES
EOF
chmod +x "$SRC"

EXPECTED_SHA="$(bash "$SRC" | shasum -a 256 | awk '{print $1}')"
echo "[case-5] expected sha=$EXPECTED_SHA bytes=$PAYLOAD_BYTES"

# Track A: streaming (stream_frame).
"$BIFROST_BIN" remote exec \
  --stream \
  --output-file "$STREAM_OUT" \
  --timeout-ms 600000 \
  --shell-text "bash $SRC"

STREAM_SHA="$(shasum -a 256 "$STREAM_OUT" | awk '{print $1}')"
STREAM_SIZE=$(wc -c < "$STREAM_OUT" | tr -d ' ')
if [[ "$STREAM_SHA" != "$EXPECTED_SHA" || "$STREAM_SIZE" != "$PAYLOAD_BYTES" ]]; then
  echo "[case-5] FAIL stream-path bytes=$STREAM_SIZE sha=$STREAM_SHA" >&2
  exit 2
fi
echo "[case-5] stream track OK"

# Track B: legacy envelope. Use --format json-pretty to get full response
# metadata including stdout_sha256_full_hex / stdout_total_bytes.
"$BIFROST_BIN" remote exec \
  --format json-pretty \
  --timeout-ms 600000 \
  --shell-text "bash $SRC" \
  > "$LEGACY_JSON"

LEGACY_SHA="$(python3 -c "import json,sys; j=json.load(open('$LEGACY_JSON')); \
print(j.get('stdout_sha256_full_hex') or j.get('stdout_sha256') or '')")"
LEGACY_BYTES="$(python3 -c "import json,sys; j=json.load(open('$LEGACY_JSON')); \
print(j.get('stdout_total_bytes') or j.get('stdout_bytes') or 0)")"

if [[ "$LEGACY_SHA" != "$EXPECTED_SHA" || "$LEGACY_BYTES" != "$PAYLOAD_BYTES" ]]; then
  echo "[case-5] FAIL legacy-path bytes=$LEGACY_BYTES sha=$LEGACY_SHA" >&2
  echo "---- json ----" >&2
  head -c 2048 "$LEGACY_JSON" >&2 || true
  exit 3
fi

echo "[case-5] OK legacy_sha=$LEGACY_SHA stream_sha=$STREAM_SHA bytes=$PAYLOAD_BYTES"
