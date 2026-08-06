#!/bin/bash
set -euo pipefail

: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
: "${BIFROST_DISABLE_TRAY:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
export BIFROST_DISABLE_TRAY

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
BIFROST_BIN="${BIFROST_BIN:-$ROOT_DIR/target/release/bifrost}"
TEST_ROOT="$(mktemp -d)"

cleanup() {
  rm -rf "$TEST_ROOT"
}
trap cleanup EXIT

if [[ ! -x "$BIFROST_BIN" ]]; then
  cargo build --release --bin bifrost --manifest-path "$ROOT_DIR/Cargo.toml"
fi

export BIFROST_DATA_DIR="$TEST_ROOT/data"

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

assert_contains() {
  local file="$1"
  local expected="$2"
  grep -Fq -- "$expected" "$file" || fail "$file does not contain: $expected"
}

assert_not_contains() {
  local file="$1"
  local unexpected="$2"
  if [[ -f "$file" ]] && grep -Fq -- "$unexpected" "$file"; then
    fail "$file unexpectedly contains: $unexpected"
  fi
}

profile_paths() {
  local shell="$1"
  local home="$2"
  case "$shell" in
    bash) printf '%s\n' "$home/.bashrc" "$home/.bash_profile" ;;
    zsh) printf '%s\n' "$home/.zshrc" "$home/.zprofile" ;;
    fish) printf '%s\n' "$home/.config/fish/config.fish" ;;
    powershell)
      case "$(uname -s)" in
        MINGW*|MSYS*|CYGWIN*)
          printf '%s\n' \
            "$home/Documents/PowerShell/Microsoft.PowerShell_profile.ps1" \
            "$home/Documents/WindowsPowerShell/Microsoft.PowerShell_profile.ps1"
          ;;
        *) printf '%s\n' "$home/.config/powershell/Microsoft.PowerShell_profile.ps1" ;;
      esac
      ;;
    *) fail "unsupported test shell: $shell" ;;
  esac
}

test_shell() {
  local shell="$1"
  local home="$TEST_ROOT/home-$shell"
  local first_profile
  mkdir -p "$home"
  first_profile="$(profile_paths "$shell" "$home" | head -n 1)"
  mkdir -p "$(dirname "$first_profile")"
  printf '%s\n' "# user setting for $shell" > "$first_profile"

  HOME="$home" "$BIFROST_BIN" cli-proxy enable \
    --shell "$shell" \
    --host 127.0.0.1 \
    --port 18888 \
    --no-proxy "localhost,127.0.0.1,example.local"
  HOME="$home" "$BIFROST_BIN" cli-proxy enable \
    --shell "$shell" \
    --host 127.0.0.1 \
    --port 18888 \
    --no-proxy "localhost,127.0.0.1,example.local"

  while IFS= read -r profile; do
    [[ -f "$profile" ]] || fail "$shell profile was not created: $profile"
    [[ "$(grep -Fc '# >>> Bifrost CLI proxy environment start >>>' "$profile")" == "1" ]] \
      || fail "$shell profile is not idempotent: $profile"
    assert_contains "$profile" "Bifrost CLI proxy environment end"
  done < <(profile_paths "$shell" "$home")

  case "$shell" in
    bash|zsh)
      assert_contains "$first_profile" "export HTTPS_PROXY='http://127.0.0.1:18888'"
      assert_contains "$first_profile" "export NODE_EXTRA_CA_CERTS='"
      assert_contains "$first_profile" "export REQUESTS_CA_BUNDLE='"
      assert_contains "$first_profile" "export CARGO_HTTP_CAINFO='"
      assert_contains "$first_profile" "export DENO_CERT='"
      assert_contains "$first_profile" "export SSL_CERT_DIR='"
      ;;
    fish)
      assert_contains "$first_profile" "set -gx HTTPS_PROXY 'http://127.0.0.1:18888'"
      assert_contains "$first_profile" "set -gx NODE_EXTRA_CA_CERTS '"
      assert_contains "$first_profile" "set -gx REQUESTS_CA_BUNDLE '"
      ;;
    powershell)
      assert_contains "$first_profile" "\$env:HTTPS_PROXY = 'http://127.0.0.1:18888'"
      assert_contains "$first_profile" "\$env:NODE_EXTRA_CA_CERTS = '"
      assert_contains "$first_profile" "\$env:REQUESTS_CA_BUNDLE = '"
      ;;
  esac

  HOME="$home" "$BIFROST_BIN" cli-proxy disable --shell "$shell"
  while IFS= read -r profile; do
    assert_not_contains "$profile" "Bifrost CLI proxy environment"
  done < <(profile_paths "$shell" "$home")
  assert_contains "$first_profile" "# user setting for $shell"
}

test_automatic_shell_detection() {
  local home="$TEST_ROOT/home-auto"
  mkdir -p "$home"
  # This script is running under bash. Parent-process detection must win over a stale login-shell
  # variable so nested shells update the profile that is actually executing the command.
  HOME="$home" SHELL=/bin/zsh "$BIFROST_BIN" cli-proxy enable --port 18889
  [[ -f "$home/.bashrc" ]] || fail "current bash parent detection did not write .bashrc"
  [[ ! -e "$home/.zshrc" ]] || fail "stale SHELL overrode the current bash parent"
  HOME="$home" SHELL=/bin/zsh "$BIFROST_BIN" cli-proxy disable
}

test_old_lifecycle_marker_is_preserved() {
  local home="$TEST_ROOT/home-marker"
  mkdir -p "$home"
  printf '%s\n' \
    '# >>> Bifrost proxy start >>>' \
    'export HTTP_PROXY=old' \
    '# <<< Bifrost proxy end <<<' > "$home/.zshrc"
  HOME="$home" "$BIFROST_BIN" cli-proxy enable --shell zsh
  HOME="$home" "$BIFROST_BIN" cli-proxy disable --shell zsh
  assert_contains "$home/.zshrc" "# >>> Bifrost proxy start >>>"
  assert_contains "$home/.zshrc" "export HTTP_PROXY=old"
}

test_shell_literal_escaping() {
  local home="$TEST_ROOT/home-escaping"
  local sentinel="$TEST_ROOT/should-not-exist"
  mkdir -p "$home"
  HOME="$home" "$BIFROST_BIN" cli-proxy enable \
    --shell bash \
    --no-proxy "one'; touch $sentinel; echo 'two"
  HOME="$home" bash -c 'source "$HOME/.bashrc"'
  [[ ! -e "$sentinel" ]] || fail "shell metacharacters from --no-proxy were executed"
  HOME="$home" "$BIFROST_BIN" cli-proxy disable --shell bash
}

test_manual_fallback_output() {
  local output status
  set +e
  output="$(HOME=/dev/null "$BIFROST_BIN" cli-proxy enable --shell bash 2>&1)"
  status=$?
  set -e
  [[ "$status" -ne 0 ]] || fail "enable with an invalid HOME unexpectedly succeeded"
  [[ "$output" == *"Automatic CLI proxy environment installation failed"* ]] \
    || fail "enable failure did not explain the automatic failure"
  [[ "$output" == *"Manual setup"* ]] || fail "enable failure omitted manual setup"
  [[ "$output" == *"# >>> Bifrost CLI proxy environment start >>>"* ]] \
    || fail "enable failure omitted the complete copyable block"
  [[ "$output" == *"NODE_EXTRA_CA_CERTS"* ]] || fail "enable fallback omitted Node CA"
  [[ "$output" == *"REQUESTS_CA_BUNDLE"* ]] || fail "enable fallback omitted Python CA"
  [[ "$output" == *"CARGO_HTTP_CAINFO"* ]] || fail "enable fallback omitted Cargo CA"
  [[ "$output" == *"DENO_CERT"* ]] || fail "enable fallback omitted Deno CA"

  local disable_home="$TEST_ROOT/home-disable-failure"
  mkdir -p "$disable_home/.bashrc"
  set +e
  output="$(HOME="$disable_home" "$BIFROST_BIN" cli-proxy disable --shell bash 2>&1)"
  status=$?
  set -e
  [[ "$status" -ne 0 ]] || fail "disable with a directory profile unexpectedly succeeded"
  [[ "$output" == *"Automatic CLI proxy environment removal failed"* ]] \
    || fail "disable failure did not explain the automatic failure"
  [[ "$output" == *"Manual removal"* ]] || fail "disable failure omitted manual removal"
  [[ "$output" == *"START: # >>> Bifrost CLI proxy environment start >>>"* ]] \
    || fail "disable failure omitted the start marker"
  [[ "$output" == *"END:   # <<< Bifrost CLI proxy environment end <<<"* ]] \
    || fail "disable failure omitted the end marker"
}

test_incomplete_marker_is_not_overwritten() {
  local home="$TEST_ROOT/home-incomplete-marker"
  local output status
  mkdir -p "$home"
  printf '%s\n' '# user setting' '# >>> Bifrost CLI proxy environment start >>>' > "$home/.bashrc"
  set +e
  output="$(HOME="$home" "$BIFROST_BIN" cli-proxy enable --shell bash 2>&1)"
  status=$?
  set -e
  [[ "$status" -ne 0 ]] || fail "enable unexpectedly accepted an incomplete marker block"
  [[ "$output" == *"incomplete, reversed, or duplicate"* ]] \
    || fail "incomplete marker error was not explained"
  [[ "$(grep -Fc '# >>> Bifrost CLI proxy environment start >>>' "$home/.bashrc")" == "1" ]] \
    || fail "enable duplicated the incomplete marker block"
  assert_contains "$home/.bashrc" "# user setting"

  local unsafe_home="$TEST_ROOT/home-unsafe-value"
  mkdir -p "$unsafe_home"
  set +e
  output="$(HOME="$unsafe_home" "$BIFROST_BIN" cli-proxy enable \
    --shell bash --no-proxy $'localhost\n# >>> Bifrost CLI proxy environment start >>>' 2>&1)"
  status=$?
  set -e
  [[ "$status" -ne 0 ]] || fail "enable unexpectedly accepted marker injection in no-proxy"
  [[ "$output" == *"Cannot render a safe block"* ]] \
    || fail "unsafe manual block was not rejected"
  [[ ! -e "$unsafe_home/.bashrc" ]] || fail "unsafe value created a shell profile"
}

for shell in bash zsh fish powershell; do
  test_shell "$shell"
done
test_automatic_shell_detection
test_old_lifecycle_marker_is_preserved
test_shell_literal_escaping
test_manual_fallback_output
test_incomplete_marker_is_not_overwritten

bundle="$BIFROST_DATA_DIR/certs/cli-proxy-ca-bundle.pem"
[[ -f "$bundle" ]] || fail "combined CA bundle was not generated"
certificate_count="$(grep -Fc -- '-----BEGIN CERTIFICATE-----' "$bundle")"
[[ "$certificate_count" -ge 2 ]] || fail "combined CA bundle lacks system roots"
assert_contains "$bundle" "$(sed -n '2p' "$BIFROST_DATA_DIR/certs/ca.crt")"

echo "PASS: cli-proxy environment enable/disable covers bash, zsh, fish, powershell, CA bundle, detection, idempotency, escaping, lifecycle isolation, and manual failure recovery"
