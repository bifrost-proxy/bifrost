#!/usr/bin/env bash
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT

# Case 2: long-running task with multiple relay reconnects.
#
# Runs a long-duration producer that emits periodic output, during which
# the streaming client is expected to hit the RELAY_RECONNECT_HINT_MS
# boundary at least 4 times (CI-friendly default: 120 s total, ~30 s hint
# cadence => 4 reconnect hints emitted). We verify the final byte count
# and SHA-256 match, and that at least 4 Reconnect hints appeared in the
# streaming log.

set -euo pipefail

: "${BIFROST_BIN:=bifrost}"
: "${CASE_2_DURATION:=120}"

TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

SRC_SCRIPT="$TMPDIR/produce.sh"
OUT_FILE="$TMPDIR/out.bin"
LOG_FILE="$TMPDIR/stream.log"

cat > "$SRC_SCRIPT" <<EOF
#!/usr/bin/env bash
set -e
SECS="\${1:-$CASE_2_DURATION}"
# 1 line per 100 ms => 10 lines/sec => ~\$((SECS*10)) lines total.
for ((i=0; i<SECS*10; i++)); do
  printf "chunk_%08d_%s\\n" "\$i" "\$(date +%N)"
  sleep 0.1
done
EOF
chmod +x "$SRC_SCRIPT"

EXPECTED_SHA="$(bash "$SRC_SCRIPT" "$CASE_2_DURATION" | shasum -a 256 | awk '{print $1}')"
EXPECTED_BYTES="$(bash "$SRC_SCRIPT" "$CASE_2_DURATION" | wc -c | tr -d ' ')"
echo "[case-2] expected sha256=$EXPECTED_SHA bytes=$EXPECTED_BYTES duration=${CASE_2_DURATION}s"

# Use --stream with RUST_LOG=debug so Reconnect frames surface in logs.
RUST_LOG=bifrost_cli=debug,bifrost_admin=debug \
  "$BIFROST_BIN" remote exec \
    --stream \
    --output-file "$OUT_FILE" \
    --timeout-ms "$((CASE_2_DURATION * 2 * 1000))" \
    --shell-text "bash $SRC_SCRIPT $CASE_2_DURATION" \
    2> "$LOG_FILE"

if [[ ! -f "$OUT_FILE" ]]; then
  echo "[case-2] FAIL: output missing" >&2
  exit 2
fi

ACTUAL_SIZE=$(wc -c < "$OUT_FILE" | tr -d ' ')
ACTUAL_SHA="$(shasum -a 256 "$OUT_FILE" | awk '{print $1}')"

if [[ "$ACTUAL_SIZE" != "$EXPECTED_BYTES" || "$ACTUAL_SHA" != "$EXPECTED_SHA" ]]; then
  echo "[case-2] FAIL: bytes/sha mismatch" >&2
  echo "  expected_bytes=$EXPECTED_BYTES actual=$ACTUAL_SIZE" >&2
  echo "  expected_sha=$EXPECTED_SHA actual=$ACTUAL_SHA" >&2
  exit 3
fi

# Count reconnect hints in log. Accept log or structured frame text.
RECONNECTS=$(grep -cE 'Reconnect|reconnect_hint|relay-wall-clock' "$LOG_FILE" || true)
echo "[case-2] reconnect_hints_observed=$RECONNECTS (required >= 4)"
if (( RECONNECTS < 4 )); then
  echo "[case-2] FAIL: expected >=4 reconnect hints, got $RECONNECTS" >&2
  echo "---- streaming log tail ----" >&2
  tail -40 "$LOG_FILE" >&2 || true
  exit 4
fi

echo "[case-2] OK bytes=$ACTUAL_SIZE sha256=$ACTUAL_SHA reconnects=$RECONNECTS"
