#!/usr/bin/env bash
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT

# Case 1: 1 GiB binary stdout byte-exact verification.
#
# Produces a deterministic 1 GiB byte stream (SHA-256 computed at source),
# runs it through `bifrost remote exec --stream --output-file`, then
# verifies the received file byte-exactly via SHA-256 equality.

set -euo pipefail

: "${BIFROST_BIN:=bifrost}"
: "${CASE_1_BYTES:=1073741824}"

TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

SRC_SCRIPT="$TMPDIR/produce.sh"
OUT_FILE="$TMPDIR/out.bin"

# Deterministic 1 GiB generator via /dev/urandom seeded indirectly:
# Use a fixed pattern so sender SHA-256 is reproducible for verification.
cat > "$SRC_SCRIPT" <<EOF
#!/usr/bin/env bash
set -e
# openssl enc with a fixed key yields a reproducible pseudorandom stream.
# We emit exactly \$1 bytes to stdout.
BYTES="\${1:-$CASE_1_BYTES}"
openssl enc -aes-256-ctr -pass pass:bifrost-pr8-case1 -in /dev/zero -nosalt 2>/dev/null \
  | head -c "\$BYTES"
EOF
chmod +x "$SRC_SCRIPT"

# Compute expected SHA-256 locally.
EXPECTED_SHA="$(bash "$SRC_SCRIPT" "$CASE_1_BYTES" | shasum -a 256 | awk '{print $1}')"
echo "[case-1] expected sha256=$EXPECTED_SHA bytes=$CASE_1_BYTES"

# Drive bifrost remote to execute the same producer on the remote side.
# The remote must have a Shell Access policy allowing bash invocation of
# SRC_SCRIPT. For CI the script is copied to the target via scp or placed
# in a shared path; for local verification the same path works.
"$BIFROST_BIN" remote exec \
  --stream \
  --output-file "$OUT_FILE" \
  --timeout-ms 1800000 \
  --shell-text "bash $SRC_SCRIPT $CASE_1_BYTES"

if [[ ! -f "$OUT_FILE" ]]; then
  echo "[case-1] FAIL: output file missing" >&2
  exit 2
fi

ACTUAL_SIZE=$(wc -c < "$OUT_FILE" | tr -d ' ')
if [[ "$ACTUAL_SIZE" != "$CASE_1_BYTES" ]]; then
  echo "[case-1] FAIL: size mismatch expected=$CASE_1_BYTES actual=$ACTUAL_SIZE" >&2
  exit 3
fi

ACTUAL_SHA="$(shasum -a 256 "$OUT_FILE" | awk '{print $1}')"
if [[ "$ACTUAL_SHA" != "$EXPECTED_SHA" ]]; then
  echo "[case-1] FAIL: sha256 mismatch" >&2
  echo "  expected=$EXPECTED_SHA" >&2
  echo "  actual  =$ACTUAL_SHA" >&2
  exit 4
fi

echo "[case-1] OK bytes=$ACTUAL_SIZE sha256=$ACTUAL_SHA"
