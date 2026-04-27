#!/usr/bin/env bash
# Case 3: 5× forced kill + resume with SHA-256 verification.
#
# Launches a streaming consumer that downloads a deterministic 64 MiB
# payload; after ~N% of expected bytes, SIGKILL the caller process, then
# restart it with --resume-call-id <id> --resume-relay-token <tok> picked
# from the prior run's stderr (or saved marker file). Repeat 5 times,
# appending bytes into the same output file. At the end verify the SHA-256
# matches the deterministic producer digest.

set -euo pipefail

: "${BIFROST_BIN:=bifrost}"
: "${CASE_3_ITERS:=5}"

TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

SRC_SCRIPT="$TMPDIR/produce.sh"
OUT_FILE="$TMPDIR/out.bin"

PAYLOAD_BYTES=$((64 * 1024 * 1024))  # 64 MiB, scaled for fast iteration
cat > "$SRC_SCRIPT" <<EOF
#!/usr/bin/env bash
set -e
openssl enc -aes-256-ctr -pass pass:bifrost-pr8-case3 -in /dev/zero -nosalt 2>/dev/null \
  | head -c $PAYLOAD_BYTES
EOF
chmod +x "$SRC_SCRIPT"

EXPECTED_SHA="$(bash "$SRC_SCRIPT" | shasum -a 256 | awk '{print $1}')"
echo "[case-3] payload=$PAYLOAD_BYTES expected_sha=$EXPECTED_SHA iters=$CASE_3_ITERS"

: > "$OUT_FILE"

CALL_ID=""
RELAY_TOKEN=""

for ((i=1; i<=CASE_3_ITERS; i++)); do
  echo "[case-3] iter $i/$CASE_3_ITERS  current_size=$(wc -c < "$OUT_FILE" | tr -d ' ')"
  LOG="$TMPDIR/iter-$i.log"
  ARGS=(remote exec --stream --output-file "$OUT_FILE" --timeout-ms 600000
        --shell-text "bash $SRC_SCRIPT")
  if [[ -n "$CALL_ID" ]]; then
    ARGS=(remote exec --stream --output-file "$OUT_FILE" --timeout-ms 600000
          --resume-call-id "$CALL_ID" --resume-relay-token "$RELAY_TOKEN"
          --shell-text "bash $SRC_SCRIPT")
  fi

  # Run in background; kill after a fraction of expected bytes in.
  "$BIFROST_BIN" "${ARGS[@]}" > "$LOG.stdout" 2> "$LOG" &
  PID=$!

  # Wait until stream is clearly underway then kill, unless this is the final
  # iteration, in which case we let it finish naturally.
  if (( i < CASE_3_ITERS )); then
    # Wait up to 10 s for the output file to grow.
    for _ in $(seq 1 100); do
      cur=$(wc -c < "$OUT_FILE" | tr -d ' ')
      (( cur > 0 )) && break
      sleep 0.1
    done
    # Let it ingest a bit more.
    sleep 1
    kill -KILL "$PID" 2>/dev/null || true
    wait "$PID" 2>/dev/null || true

    # Extract call_id / relay_token for resume. CLI must emit these on stderr
    # during --stream so resume is possible; pattern: "resume-call-id=XXX" etc.
    CALL_ID="$(grep -m1 -oE 'resume-call-id=[^ ]+' "$LOG" | cut -d= -f2 || true)"
    RELAY_TOKEN="$(grep -m1 -oE 'resume-relay-token=[^ ]+' "$LOG" | cut -d= -f2 || true)"
    if [[ -z "$CALL_ID" || -z "$RELAY_TOKEN" ]]; then
      echo "[case-3] FAIL iter $i: could not extract resume creds" >&2
      tail -40 "$LOG" >&2 || true
      exit 2
    fi
    echo "[case-3] iter $i killed; resume call_id=$CALL_ID token=${RELAY_TOKEN:0:8}..."
  else
    if ! wait "$PID"; then
      echo "[case-3] FAIL iter $i: exec failed" >&2
      tail -40 "$LOG" >&2 || true
      exit 3
    fi
  fi
done

ACTUAL_SIZE=$(wc -c < "$OUT_FILE" | tr -d ' ')
ACTUAL_SHA="$(shasum -a 256 "$OUT_FILE" | awk '{print $1}')"
if [[ "$ACTUAL_SIZE" != "$PAYLOAD_BYTES" || "$ACTUAL_SHA" != "$EXPECTED_SHA" ]]; then
  echo "[case-3] FAIL final mismatch" >&2
  echo "  expected_bytes=$PAYLOAD_BYTES actual=$ACTUAL_SIZE" >&2
  echo "  expected_sha=$EXPECTED_SHA actual=$ACTUAL_SHA" >&2
  exit 4
fi

echo "[case-3] OK bytes=$ACTUAL_SIZE sha256=$ACTUAL_SHA iters=$CASE_3_ITERS"
