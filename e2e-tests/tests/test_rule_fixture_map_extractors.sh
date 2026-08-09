#!/bin/bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../test_rules.sh"

assert_output() {
    local description="$1"
    local expected="$2"
    local actual="$3"

    if [[ "$actual" != "$expected" ]]; then
        printf 'FAIL: %s\nexpected:\n%s\nactual:\n%s\n' "$description" "$expected" "$actual" >&2
        exit 1
    fi
}

template_headers=$(extract_headers_from_value \
    'X-Host=${hostname.replace(no&x-fake=1,replaced)}&X-Mode=active')
assert_output "header templates preserve authored delimiters" \
    $'X-Host|${hostname.replace(no&x-fake=1,replaced)}\nX-Mode|active' \
    "$template_headers"

comma_cookies=$(extract_key_values_from_value '(sid=abc,theme=dark)')
assert_output "inline cookies split comma-separated pairs" \
    $'sid|abc\ntheme|dark' \
    "$comma_cookies"

template_cookies=$(extract_key_values_from_value \
    'sid=${hostname.replace(no,match)}&theme=dark')
assert_output "cookie templates preserve commas" \
    $'sid|${hostname.replace(no,match)}\ntheme|dark' \
    "$template_cookies"

referenced_cookie=$(extract_key_values_from_value \
    'session=safe&injected=yes' '{literal_ampersand_cookie}')
assert_output "value references preserve literal ampersands" \
    'session|safe&injected=yes' \
    "$referenced_cookie"

multiline_cookie=$(extract_key_values_from_value $'session=safe&injected=yes\ntheme=dark')
assert_output "multiline values preserve literal ampersands" \
    $'session|safe&injected=yes\ntheme|dark' \
    "$multiline_cookie"

printf 'PASS: rule fixture map extractors\n'
