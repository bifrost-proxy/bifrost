#!/bin/bash

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

ADMIN_HOST="${ADMIN_HOST:-127.0.0.1}"
ADMIN_PORT="${ADMIN_PORT:-$((19000 + ($$ % 1000)))}"
ADMIN_PATH_PREFIX="${ADMIN_PATH_PREFIX:-/_bifrost}"
export ADMIN_PATH_PREFIX
ADMIN_BASE_URL="http://${ADMIN_HOST}:${ADMIN_PORT}${ADMIN_PATH_PREFIX}"
export ADMIN_HOST ADMIN_PORT ADMIN_BASE_URL

source "$SCRIPT_DIR/../test_utils/admin_client.sh"

TESTS_RUN=0
TESTS_PASSED=0
TESTS_FAILED=0

RULE_NAME="bifrost-file-rule-$$"
VALUE_NAME="bifrost-file-value-$$"
SCRIPT_NAME="syntax-script-$$"

log_info() { echo "[INFO] $*"; }
log_pass() { echo "[PASS] $*"; }
log_fail() { echo "[FAIL] $*"; }

run_test() {
  local test_name="$1"
  local test_func="$2"

  TESTS_RUN=$((TESTS_RUN + 1))
  log_info "Running test: $test_name"

  if $test_func; then
    TESTS_PASSED=$((TESTS_PASSED + 1))
    log_pass "$test_name"
  else
    TESTS_FAILED=$((TESTS_FAILED + 1))
    log_fail "$test_name"
  fi
}

assert_contains() {
  local haystack="$1"
  local needle="$2"
  local message="$3"

  if [[ "$haystack" == *"$needle"* ]]; then
    return 0
  fi

  log_fail "$message: missing '$needle'"
  return 1
}

assert_equals() {
  local expected="$1"
  local actual="$2"
  local message="$3"

  if [[ "$expected" == "$actual" ]]; then
    return 0
  fi

  log_fail "$message: expected '$expected', got '$actual'"
  return 1
}

wait_for_request_script_in_syntax() {
  local attempts="${1:-10}"
  local response=""
  local has_request_script="false"

  for _ in $(seq 1 "$attempts"); do
    response=$(curl -sS "${ADMIN_BASE_URL}/api/syntax") || return 1
    has_request_script=$(echo "$response" | jq -r --arg name "$SCRIPT_NAME" '[.scripts.request_scripts[].name] | index($name) != null')
    if [[ "$has_request_script" == "true" ]]; then
      printf '%s' "$response"
      return 0
    fi
    sleep 0.5
  done

  printf '%s' "$response"
  return 1
}

cleanup_resources() {
  curl -s -X DELETE "${ADMIN_BASE_URL}/api/rules/${RULE_NAME}" >/dev/null 2>&1 || true
  curl -s -X DELETE "${ADMIN_BASE_URL}/api/values/${VALUE_NAME}" >/dev/null 2>&1 || true
  curl -s -X DELETE "${ADMIN_BASE_URL}/api/scripts/request/${SCRIPT_NAME}" >/dev/null 2>&1 || true
}

cleanup() {
  cleanup_resources
  admin_cleanup_bifrost
}

trap cleanup EXIT

create_fixtures() {
  cleanup_resources

  local rule_payload
  rule_payload=$(cat <<EOF
{"name":"${RULE_NAME}","content":"example.com reqHeaders://X-Bifrost-File=1"}
EOF
)

  local value_payload
  value_payload=$(cat <<EOF
{"name":"${VALUE_NAME}","value":"from-bifrost-file"}
EOF
)

  local script_payload
  script_payload=$(cat <<EOF
{"content":"request.headers[\"x-syntax-script\"] = \"enabled\";"}
EOF
)

  curl -fsS -X POST -H "Content-Type: application/json" \
    -d "$rule_payload" \
    "${ADMIN_BASE_URL}/api/rules" >/dev/null || return 1

  curl -fsS -X POST -H "Content-Type: application/json" \
    -d "$value_payload" \
    "${ADMIN_BASE_URL}/api/values" >/dev/null || return 1

  curl -fsS -X PUT -H "Content-Type: application/json" \
    -d "$script_payload" \
    "${ADMIN_BASE_URL}/api/scripts/request/${SCRIPT_NAME}" >/dev/null || return 1
}

test_syntax_endpoint_exposes_dynamic_data() {
  local response
  response=$(wait_for_request_script_in_syntax) || {
    local scripts_response
    scripts_response=$(curl -sS "${ADMIN_BASE_URL}/api/scripts" 2>/dev/null || true)
    log_fail "syntax endpoint should expose newly created request script" "${SCRIPT_NAME}" "${scripts_response:-unavailable}"
    return 1
  }

  local req_headers_alias
  req_headers_alias=$(echo "$response" | jq -r '.protocol_aliases.pathReplace')
  assert_equals "urlReplace" "$req_headers_alias" "pathReplace alias is exposed" || return 1

  local has_req_headers
  has_req_headers=$(echo "$response" | jq -r '[.protocols[].name] | index("reqHeaders") != null')
  assert_equals "true" "$has_req_headers" "reqHeaders protocol is exposed" || return 1

  local has_builtin_decode
  has_builtin_decode=$(echo "$response" | jq -r '[.scripts.decode_scripts[].name] | index("utf8") != null')
  assert_equals "true" "$has_builtin_decode" "built-in decode script is exposed" || return 1

  local has_request_script
  has_request_script=$(echo "$response" | jq -r --arg name "$SCRIPT_NAME" '[.scripts.request_scripts[].name] | index($name) != null')
  assert_equals "true" "$has_request_script" "user request script is exposed through syntax payload" || return 1
}

test_bifrost_file_rules_roundtrip() {
  local export_payload export_response detected_type import_response imported_name
  export_payload=$(cat <<EOF
{"rule_names":["${RULE_NAME}"],"description":"rule export for e2e"}
EOF
)

  export_response=$(curl -sS -X POST -H "Content-Type: application/json" \
    -d "$export_payload" \
    "${ADMIN_BASE_URL}/api/bifrost-file/export/rules") || return 1

  assert_contains "$export_response" "01 rules" "rules export header" || return 1
  assert_contains "$export_response" "[meta]" "rules export meta" || return 1
  assert_contains "$export_response" "$RULE_NAME" "rules export includes rule name" || return 1

  detected_type=$(printf '%s' "$export_response" | curl -sS -X POST \
    -H "Content-Type: text/plain" \
    --data-binary @- \
    "${ADMIN_BASE_URL}/api/bifrost-file/detect" | jq -r '.file_type')
  assert_equals "rules" "$detected_type" "rules detect endpoint" || return 1

  curl -sS -X DELETE "${ADMIN_BASE_URL}/api/rules/${RULE_NAME}" >/dev/null || return 1

  import_response=$(printf '%s' "$export_response" | curl -sS -X POST \
    -H "Content-Type: text/plain" \
    --data-binary @- \
    "${ADMIN_BASE_URL}/api/bifrost-file/import") || return 1

  imported_name=$(echo "$import_response" | jq -r '.data.rule_names[0]')
  assert_equals "$RULE_NAME" "$imported_name" "rules import restores rule name" || return 1
}

test_bifrost_file_values_roundtrip() {
  local export_payload export_response detected_type import_response imported_name imported_value
  export_payload=$(cat <<EOF
{"value_names":["${VALUE_NAME}"],"description":"value export for e2e"}
EOF
)

  export_response=$(curl -sS -X POST -H "Content-Type: application/json" \
    -d "$export_payload" \
    "${ADMIN_BASE_URL}/api/bifrost-file/export/values") || return 1

  assert_contains "$export_response" "01 values" "values export header" || return 1
  assert_contains "$export_response" "$VALUE_NAME" "values export includes name" || return 1
  assert_contains "$export_response" "from-bifrost-file" "values export includes content" || return 1

  detected_type=$(printf '%s' "$export_response" | curl -sS -X POST \
    -H "Content-Type: text/plain" \
    --data-binary @- \
    "${ADMIN_BASE_URL}/api/bifrost-file/detect" | jq -r '.file_type')
  assert_equals "values" "$detected_type" "values detect endpoint" || return 1

  curl -sS -X DELETE "${ADMIN_BASE_URL}/api/values/${VALUE_NAME}" >/dev/null || return 1

  import_response=$(printf '%s' "$export_response" | curl -sS -X POST \
    -H "Content-Type: text/plain" \
    --data-binary @- \
    "${ADMIN_BASE_URL}/api/bifrost-file/import") || return 1

  imported_name=$(echo "$import_response" | jq -r '.data.value_names[0]')
  assert_equals "$VALUE_NAME" "$imported_name" "values import restores value name" || return 1

  imported_value=$(curl -sS "${ADMIN_BASE_URL}/api/values/${VALUE_NAME}" | jq -r '.value')
  assert_equals "from-bifrost-file" "$imported_value" "values import restores value content" || return 1
}

main() {
  if ! admin_ensure_bifrost; then
    log_fail "Failed to start admin server"
    exit 1
  fi

  if ! create_fixtures; then
    log_fail "Failed to create API fixtures"
    exit 1
  fi

  run_test "syntax endpoint exposes dynamic scripts and aliases" test_syntax_endpoint_exposes_dynamic_data
  run_test "bifrost-file rules export/detect/import roundtrip" test_bifrost_file_rules_roundtrip
  run_test "bifrost-file values export/detect/import roundtrip" test_bifrost_file_values_roundtrip

  echo ""
  echo "Tests run:    $TESTS_RUN"
  echo "Tests passed: $TESTS_PASSED"
  echo "Tests failed: $TESTS_FAILED"

  if [[ "$TESTS_FAILED" -gt 0 ]]; then
    exit 1
  fi
}

main "$@"
