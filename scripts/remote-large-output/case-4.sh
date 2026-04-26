#!/usr/bin/env bash
# Case 4: --output-file local write + SHA-256 digest.
#
# Verifies that `bifrost remote command exec --stream --output-file <path>`
# writes the byte stream directly to the local path with matching SHA-256
# to the producer and does not duplicate/drop bytes across retries.

set -euo pipefail

: "${BIFROST_BIN:=bifrost}"
PAYLOAD_BYTES=$((16 * 1024 * 1024))  # 16 MiB smoke

TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

OUT="$TMPDIR/out.bin"
SRC="$TMPDIR/src.sh"
cat > "$SRC" <<EOF
#!/usr/bin/env bash
set -e
openssl enc -aes-256-ctr -pass pass:bifrost-pr8-case4 -in /dev/zero -nosalt 2>/dev/null \
  | head -c $PAYLOAD_BYTES
EOF
chmod +x "$SRC"

EXPECTED_SHA="$(bash "$SRC" | shasum -a 256 | awk '{print $1}')"

"$BIFROST_BIN" remote command exec \
  --stream \
  --output-file "$OUT" \
  --timeout-ms 600000 \
  --shell-text "bash $SRC"

SIZE=$(wc -c < "$OUT" | tr -d ' ')
SHA=$(shasum -a 256 "$OUT" | awk '{print $1}')

if [[ "$SIZE" != "$PAYLOAD_BYTES" || "$SHA" != "$EXPECTED_SHA" ]]; then
  echo "[case-4] FAIL expected=$PAYLOAD_BYTES/$EXPECTED_SHA got=$SIZE/$SHA" >&2
  exit 2
fi

echo "[case-4] OK bytes=$SIZE sha256=$SHA"
