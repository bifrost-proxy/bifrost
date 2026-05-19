#!/bin/bash
# Performance test script for chatgpt_web adapter
# Runs 10 rounds of conversation and measures timing for each phase.

set -euo pipefail
export PATH="/usr/bin:/usr/local/bin:/usr/sbin:/sbin:/bin:/opt/homebrew/bin:$PATH"

BASE_URL="${BASE_URL:-http://127.0.0.1:18880/_bifrost/api/im-gateway/chat}"
SESSION_KEY="perf-test-$$"
RESULTS_FILE="/tmp/chatgpt-perf-results-$$.txt"

echo "=== ChatGPT Web Performance Test ===" | tee "$RESULTS_FILE"
echo "Session: $SESSION_KEY" | tee -a "$RESULTS_FILE"
echo "Started: $(date)" | tee -a "$RESULTS_FILE"
echo "" | tee -a "$RESULTS_FILE"

run_test() {
    local test_num="$1"
    local message="$2"
    local desc="${3:-}"

    echo "--- Test #${test_num}: ${desc} ---" | tee -a "$RESULTS_FILE"
    echo "  Message: ${message}" | tee -a "$RESULTS_FILE"

    local start_ts
    start_ts=$(python3 -c "import time; print(f'{time.time():.3f}')")

    local response http_code time_total time_ttfb
    response=$(curl -s -w "\n__CURL__%{http_code}|%{time_total}|%{time_starttransfer}" \
        --max-time 300 \
        -X POST "$BASE_URL" \
        -H "Content-Type: application/json" \
        -d "{
            \"message\": \"$message\",
            \"operation\": \"ask\",
            \"sessionKey\": \"$SESSION_KEY\",
            \"adapter\": \"chatgpt_web\"
        }" 2>&1) || true

    local end_ts
    end_ts=$(python3 -c "import time; print(f'{time.time():.3f}')")

    local curl_meta
    curl_meta=$(echo "$response" | grep "^__CURL__" | sed 's/__CURL__//')
    http_code=$(echo "$curl_meta" | cut -d'|' -f1)
    time_total=$(echo "$curl_meta" | cut -d'|' -f2)
    time_ttfb=$(echo "$curl_meta" | cut -d'|' -f3)

    local body
    body=$(echo "$response" | sed '/__CURL__/d')
    local body_len=${#body}

    local wall_time
    wall_time=$(python3 -c "print(f'{$end_ts - $start_ts:.2f}')")

    # Extract response preview
    local preview
    preview=$(echo "$body" | python3 -c "
import sys, json
try:
    data = json.load(sys.stdin)
    r = data.get('response','') or data.get('stdout','') or ''
    print(r[:120].replace('\n',' '))
except:
    raw = sys.stdin.read()
    print(raw[:120].replace('\n',' '))
" 2>/dev/null || echo "(parse error)")

    echo "  HTTP:     $http_code" | tee -a "$RESULTS_FILE"
    echo "  Wall:     ${wall_time}s" | tee -a "$RESULTS_FILE"
    echo "  TTFB:     ${time_ttfb}s" | tee -a "$RESULTS_FILE"
    echo "  Total:    ${time_total}s" | tee -a "$RESULTS_FILE"
    echo "  Body:     ${body_len} bytes" | tee -a "$RESULTS_FILE"
    echo "  Preview:  ${preview:0:120}" | tee -a "$RESULTS_FILE"
    echo "" | tee -a "$RESULTS_FILE"

    # Pause between tests to avoid rate limiting
    sleep 2
}

# ── Round 1-5: Same conversation (append mode) ──────────────────────
run_test 1 "Hi" "new conversation - simple greeting"
run_test 2 "What is 2+2?" "append - simple math"
run_test 3 "What is the capital of France?" "append - factual"
run_test 4 "List 3 programming languages" "append - short list"
run_test 5 "Thanks!" "append - short reply"

# ── Clear session for fresh conversation ─────────────────────────────
echo "--- Clearing session for new conversation ---" | tee -a "$RESULTS_FILE"
SESSION_KEY="perf-test-new-$$"

# ── Round 6-10: New conversation ─────────────────────────────────────
run_test 6  "Hello" "new conversation 2 - greeting"
run_test 7  "What is REST?" "append - medium explanation"
run_test 8  "What is 10 * 15?" "append - quick math"
run_test 9  "Name 3 colors" "append - trivial list"
run_test 10 "Bye!" "append - farewell"

echo "=== All tests completed at $(date) ===" | tee -a "$RESULTS_FILE"
echo ""
echo "Results saved to: $RESULTS_FILE"
