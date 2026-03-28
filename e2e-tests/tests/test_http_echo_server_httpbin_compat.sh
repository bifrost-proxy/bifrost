#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SERVER_SCRIPT="$SCRIPT_DIR/../mock_servers/http_echo_server.py"
PORT="${HTTPBIN_HTTP_MOCK_PORT:-39180}"
BASE_URL="http://127.0.0.1:${PORT}"
TMP_DIR="$(mktemp -d)"
SERVER_PID=""

source "$SCRIPT_DIR/../test_utils/process.sh"

cleanup() {
    if [[ -n "${SERVER_PID}" ]]; then
        kill_pid "${SERVER_PID}"
        wait_pid "${SERVER_PID}"
    fi
    rm -rf "${TMP_DIR}"
}
trap cleanup EXIT

assert_eq() {
    local actual="$1"
    local expected="$2"
    local message="$3"
    if [[ "${actual}" != "${expected}" ]]; then
        echo "[FAIL] ${message}: expected '${expected}', got '${actual}'" >&2
        exit 1
    fi
    echo "[PASS] ${message}"
}

assert_contains() {
    local actual="$1"
    local expected="$2"
    local message="$3"
    if [[ "${actual}" != *"${expected}"* ]]; then
        echo "[FAIL] ${message}: missing '${expected}'" >&2
        echo "Actual: ${actual}" >&2
        exit 1
    fi
    echo "[PASS] ${message}"
}

assert_file_size_gt_zero() {
    local file="$1"
    local message="$2"
    local size
    size="$(wc -c < "${file}" | tr -d ' ')"
    if [[ "${size}" -le 0 ]]; then
        echo "[FAIL] ${message}: file is empty" >&2
        exit 1
    fi
    echo "[PASS] ${message}"
}

json_eval() {
    local file="$1"
    local expr="$2"
    python3 - "$file" "$expr" <<'PY'
import json
import sys

path, expr = sys.argv[1], sys.argv[2]
with open(path, "r", encoding="utf-8") as f:
    data = json.load(f)
value = eval(expr, {"__builtins__": {}}, {"data": data})
if isinstance(value, (dict, list)):
    print(json.dumps(value, ensure_ascii=False))
else:
    print(value)
PY
}

curl_json() {
    local path="$1"
    local output="$2"
    shift 2
    curl -sS "$@" "${BASE_URL}${path}" -o "${output}"
}

python3 "${SERVER_SCRIPT}" "${PORT}" >"${TMP_DIR}/server.log" 2>&1 &
SERVER_PID="$!"

for _ in $(seq 1 50); do
    if curl -fsS "${BASE_URL}/health" >/dev/null 2>&1; then
        break
    fi
    sleep 0.2
done

curl -fsS "${BASE_URL}/health" >/dev/null

GET_JSON="${TMP_DIR}/get.json"
curl_json '/get?foo=bar' "${GET_JSON}" -H 'User-Agent: curl-test/1.0'
assert_eq "$(json_eval "${GET_JSON}" "data['args']['foo']")" "bar" "/get returns query args"
assert_eq "$(json_eval "${GET_JSON}" "data['headers']['User-Agent']")" "curl-test/1.0" "/get returns request headers"

POST_JSON="${TMP_DIR}/post.json"
curl_json '/post?kind=json' "${POST_JSON}" -X POST -H 'Content-Type: application/json' -d '{"hello":"world"}'
assert_eq "$(json_eval "${POST_JSON}" "data['json']['hello']")" "world" "/post parses JSON body"
assert_eq "$(json_eval "${POST_JSON}" "data['method']")" "POST" "/post returns method"

FORM_JSON="${TMP_DIR}/anything.json"
curl_json '/anything/upload?x=1' "${FORM_JSON}" -X POST -H 'Content-Type: application/x-www-form-urlencoded' -d 'alpha=beta&empty='
assert_eq "$(json_eval "${FORM_JSON}" "data['form']['alpha']")" "beta" "/anything parses form data"
assert_eq "$(json_eval "${FORM_JSON}" "data['args']['x']")" "1" "/anything keeps query args"

MULTIPART_JSON="${TMP_DIR}/multipart.json"
curl_json '/post' "${MULTIPART_JSON}" -X POST -F 'note=hello' -F 'upload=@/etc/hosts'
assert_eq "$(json_eval "${MULTIPART_JSON}" "data['form']['note']")" "hello" "/post parses multipart form fields"
assert_contains "$(json_eval "${MULTIPART_JSON}" "data['files']['upload']")" "localhost" "/post parses multipart file uploads"

STATUS_CODE="$(curl -sS -o /dev/null -w '%{http_code}' "${BASE_URL}/status/418")"
assert_eq "${STATUS_CODE}" "418" "/status returns requested status"

BASIC_OK_JSON="${TMP_DIR}/basic_auth_ok.json"
curl_json '/basic-auth/user/passwd' "${BASIC_OK_JSON}" -u 'user:passwd'
assert_eq "$(json_eval "${BASIC_OK_JSON}" "data['authenticated']")" "True" "/basic-auth accepts valid credentials"

BASIC_FAIL_CODE="$(curl -sS -o /dev/null -w '%{http_code}' "${BASE_URL}/basic-auth/user/passwd")"
assert_eq "${BASIC_FAIL_CODE}" "401" "/basic-auth challenges missing credentials"

BEARER_JSON="${TMP_DIR}/bearer.json"
curl_json '/bearer' "${BEARER_JSON}" -H 'Authorization: Bearer test-token'
assert_eq "$(json_eval "${BEARER_JSON}" "data['token']")" "test-token" "/bearer returns bearer token"

BYTES_SIZE="$(curl -sS "${BASE_URL}/bytes/32" | wc -c | tr -d ' ')"
assert_eq "${BYTES_SIZE}" "32" "/bytes returns exact payload size"

STREAM_LINES="$(curl -sS "${BASE_URL}/stream/3" | wc -l | tr -d ' ')"
assert_eq "${STREAM_LINES}" "3" "/stream/<n> returns JSON lines"

STREAM_BYTES_HEADERS="${TMP_DIR}/stream_bytes.headers"
STREAM_BYTES_BODY="${TMP_DIR}/stream_bytes.body"
curl -sS -D "${STREAM_BYTES_HEADERS}" "${BASE_URL}/stream-bytes/64?chunk_size=8" -o "${STREAM_BYTES_BODY}"
assert_contains "$(tr -d '\r' < "${STREAM_BYTES_HEADERS}")" 'Transfer-Encoding: chunked' "/stream-bytes uses chunked transfer"
assert_eq "$(wc -c < "${STREAM_BYTES_BODY}" | tr -d ' ')" "64" "/stream-bytes returns requested byte count"

SSE_BODY="$(curl -sS "${BASE_URL}/sse?count=2&delay=0")"
assert_contains "${SSE_BODY}" '"id": 0' "/sse returns first event"
assert_contains "${SSE_BODY}" '"id": 1' "/sse returns second event"

REDIRECT_CODE="$(curl -sS -o /dev/null -w '%{http_code}' -L "${BASE_URL}/redirect/2")"
assert_eq "${REDIRECT_CODE}" "200" "/redirect chain resolves to /get"

REDIRECT_TO_HEADERS="${TMP_DIR}/redirect_to.headers"
curl -sS -D "${REDIRECT_TO_HEADERS}" -o /dev/null "${BASE_URL}/redirect-to?url=/anything/demo&status_code=307"
assert_contains "$(tr -d '\r' < "${REDIRECT_TO_HEADERS}")" 'Location: /anything/demo' "/redirect-to sets Location header"

GZIP_JSON="${TMP_DIR}/gzip.json"
curl_json '/gzip' "${GZIP_JSON}" --compressed
assert_eq "$(json_eval "${GZIP_JSON}" "data['gzipped']")" "True" "/gzip returns compressed JSON payload"

if curl --version | grep -qi 'brotli'; then
    BROTLI_JSON="${TMP_DIR}/brotli.json"
    curl_json '/brotli' "${BROTLI_JSON}" --compressed
    assert_eq "$(json_eval "${BROTLI_JSON}" "data['brotli']")" "True" "/brotli returns compressed JSON payload"
else
    echo "[SKIP] /brotli - curl lacks brotli support"
fi

DEFLATE_JSON="${TMP_DIR}/deflate.json"
curl_json '/deflate' "${DEFLATE_JSON}" --compressed
assert_eq "$(json_eval "${DEFLATE_JSON}" "data['deflated']")" "True" "/deflate returns compressed JSON payload"

COOKIE_JAR="${TMP_DIR}/cookies.txt"
curl -sS -c "${COOKIE_JAR}" -o /dev/null "${BASE_URL}/cookies/set?session=abc123"
COOKIES_JSON="${TMP_DIR}/cookies.json"
curl_json '/cookies' "${COOKIES_JSON}" -b "${COOKIE_JAR}"
assert_eq "$(json_eval "${COOKIES_JSON}" "data['cookies']['session']")" "abc123" "/cookies/set and /cookies work together"

RESPONSE_HEADERS="${TMP_DIR}/response_headers.headers"
RESPONSE_HEADERS_JSON="${TMP_DIR}/response_headers.json"
curl -sS -D "${RESPONSE_HEADERS}" "${BASE_URL}/response-headers?X-Test=ok&Content-Type=text/plain" -o "${RESPONSE_HEADERS_JSON}"
assert_contains "$(tr -d '\r' < "${RESPONSE_HEADERS}")" 'X-Test: ok' "/response-headers mirrors custom headers"
assert_eq "$(json_eval "${RESPONSE_HEADERS_JSON}" "data['X-Test']")" "ok" "/response-headers returns header payload"

ETAG_HEADERS="${TMP_DIR}/etag.headers"
curl -sS -D "${ETAG_HEADERS}" -o /dev/null "${BASE_URL}/etag/demo-tag"
assert_contains "$(tr -d '\r' < "${ETAG_HEADERS}")" 'ETag: "demo-tag"' "/etag exposes ETag header"
ETAG_NOT_MODIFIED="$(curl -sS -o /dev/null -w '%{http_code}' -H 'If-None-Match: "demo-tag"' "${BASE_URL}/etag/demo-tag")"
assert_eq "${ETAG_NOT_MODIFIED}" "304" "/etag honors If-None-Match"

RANGE_HEADERS="${TMP_DIR}/range.headers"
RANGE_BODY="${TMP_DIR}/range.body"
curl -sS -D "${RANGE_HEADERS}" -H 'Range: bytes=5-9' "${BASE_URL}/range/26" -o "${RANGE_BODY}"
assert_contains "$(tr -d '\r' < "${RANGE_HEADERS}")" 'HTTP/1.1 206 Partial Content' "/range returns 206 for partial requests"
assert_contains "$(tr -d '\r' < "${RANGE_HEADERS}")" 'Content-Range: bytes 5-9/26' "/range returns content-range header"
assert_eq "$(cat "${RANGE_BODY}")" "fghij" "/range returns requested slice"

BASE64_BODY="$(curl -sS "${BASE_URL}/base64/SGVsbG8=")"
assert_eq "${BASE64_BODY}" "Hello" "/base64 decodes payload"

UUID_JSON="${TMP_DIR}/uuid.json"
curl_json '/uuid' "${UUID_JSON}"
assert_eq "$(json_eval "${UUID_JSON}" "data['uuid']")" "00000000-0000-4000-8000-000000000000" "/uuid returns stable value"

IMAGE_FILE="${TMP_DIR}/image.png"
curl -sS "${BASE_URL}/image/png" -o "${IMAGE_FILE}"
assert_file_size_gt_zero "${IMAGE_FILE}" "/image/png returns binary payload"

echo "HTTP echo server httpbin compatibility checks passed."
