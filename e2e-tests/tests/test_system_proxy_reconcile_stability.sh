#!/bin/bash
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1
export BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL=1
export BIFROST_DISABLE_TRAY=1
: "${BIFROST_SYSTEM_PROXY_RECONCILE_SECS:=3}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
export BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL
export BIFROST_SYSTEM_PROXY_RECONCILE_SECS
export BIFROST_SYSTEM_PROXY_DISABLE_LIFECYCLE_HELPER=1
# Count completed read-only inspections as well as transitions, so a stalled
# coordinator cannot satisfy the one-transition convergence assertion.
export RUST_LOG="bifrost::commands::start::system_proxy_reconcile=debug,info"
unset BIFROST_DESKTOP_CORE BIFROST_DESKTOP_APP

set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "SKIP: system proxy reconcile stability requires macOS"
    exit 0
fi

# A direct invocation on a personal Mac must not silently change its proxies.
# This opt-in is supplied only by the approved hosted-macOS CI shard.
require_disposable_macos_ci() {
    [[ "${GITHUB_ACTIONS:-}" == "true" \
        && "${RUNNER_ENVIRONMENT:-}" == "github-hosted" \
        && "${RUNNER_OS:-}" == "macOS" \
        && "${GITHUB_REPOSITORY:-}" == "bifrost-proxy/bifrost" \
        && "${GITHUB_RUN_ID:-}" =~ ^[0-9]+$ \
        && "${BIFROST_NATIVE_PROXY_CI:-}" == "1" ]] || {
        echo "REFUSING: native proxy acceptance requires the explicitly opted-in disposable GitHub macOS runner" >&2
        return 1
    }
}
require_disposable_macos_ci

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
SCRIPT_PATH="$SCRIPT_DIR/test_system_proxy_reconcile_stability.sh"
BIFROST_BIN="${BIFROST_BIN:-$ROOT_DIR/target/release/bifrost}"
PROXY_PORT="${PROXY_PORT:-18889}"
BIFROST_E2E_REPORT_DIR="${BIFROST_E2E_REPORT_DIR:-$ROOT_DIR/.e2e-reports}"
if [[ ! -x "$BIFROST_BIN" ]]; then
    echo "REFUSING: native CI requires its already-built binary" >&2
    exit 1
fi
BIFROST_BIN="$(cd "$(dirname "$BIFROST_BIN")" && pwd)/$(basename "$BIFROST_BIN")"
resolve_native_python() {
    # The E2E runner prepends a PATH-dependent python3 shim. Capture the actual
    # interpreter before env -i removes its toolcache PATH, not the shim path.
    local candidate="${BIFROST_NATIVE_PYTHON:-python3}" resolved
    resolved="$("$candidate" -I -c 'import sys; print(sys.executable)')" || {
        echo "REFUSING: native CI could not resolve its Python interpreter" >&2
        return 1
    }
    [[ "$resolved" == /* && -f "$resolved" && -x "$resolved" ]] || {
        echo "REFUSING: native CI requires an absolute executable Python interpreter" >&2
        return 1
    }
    # Validate Python 3.11+ stdlib support with the same clean environment used
    # after elevation. Missing tomllib must fail before any privileged work.
    /usr/bin/env -i PATH=/usr/bin:/bin:/usr/sbin:/sbin LC_ALL=C \
        PYTHONNOUSERSITE=1 PYTHONDONTWRITEBYTECODE=1 \
        "$resolved" -I -c 'import tomllib' || {
        echo "REFUSING: native CI requires Python 3.11+ with tomllib in its clean environment" >&2
        return 1
    }
    printf '%s\n' "$resolved"
}
BIFROST_NATIVE_PYTHON="$(resolve_native_python)"
require_native_test_port() {
    # Canonical, length-bounded decimal prevents octal parsing and overflow.
    [[ "$PROXY_PORT" =~ ^[1-9][0-9]{3,4}$ \
        && "$PROXY_PORT" -ge 1024 && "$PROXY_PORT" -le 65535 \
        && "$PROXY_PORT" != 9900 ]] || {
        echo "REFUSING: native CI requires an unprivileged non-production test port" >&2
        return 1
    }
}
require_native_test_port
if LC_ALL=C grep -aFq 'BIFROST_PROXY_TEST_IO_CAPABILITY_V1' "$BIFROST_BIN"; then
    echo "REFUSING: native acceptance cannot use the fixture-only proxy binary" >&2
    exit 1
fi

fixture_effective_uid() {
    /usr/bin/id -u
}

elevate_native_fixture() {
    require_disposable_macos_ci || return 1
    local uid
    uid="$(fixture_effective_uid)" || return 1
    if [[ "${BIFROST_NATIVE_PROXY_MODE:-direct-exact}" == nonroot-recovery ]]; then
        [[ "$uid" != 0 && "${BIFROST_NATIVE_PROXY_ELEVATED:-0}" != 1 ]] || {
            echo "REFUSING: non-root acceptance must run as the ordinary CI user" >&2
            return 1
        }
        return 0
    fi
    [[ "${BIFROST_NATIVE_PROXY_MODE:-direct-exact}" == direct-exact ]] || return 1
    if [[ "$uid" == 0 && "${BIFROST_NATIVE_PROXY_ELEVATED:-0}" == 1 ]]; then return 0; fi
    if [[ "${BIFROST_NATIVE_PROXY_ELEVATED:-0}" == 1 ]]; then
        echo "REFUSING: scoped native fixture did not obtain Direct root privilege" >&2
        return 1
    fi
    # Elevate only this audited script, never the full E2E runner. No password
    # prompts or persistent privilege configuration; unavailable sudo fails.
    # env -i drops credentials, shell startup hooks, user HOME/data and proxy
    # overrides. Python and the release artifact retain their selected paths.
    local command=(/usr/bin/env -i \
        PATH=/usr/bin:/bin:/usr/sbin:/sbin \
        LC_ALL=C PYTHONNOUSERSITE=1 PYTHONDONTWRITEBYTECODE=1 \
        GITHUB_ACTIONS=true RUNNER_ENVIRONMENT=github-hosted RUNNER_OS=macOS \
        "GITHUB_REPOSITORY=$GITHUB_REPOSITORY" "GITHUB_RUN_ID=$GITHUB_RUN_ID" \
        BIFROST_NATIVE_PROXY_CI=1 BIFROST_NATIVE_PROXY_ELEVATED=1 \
        "BIFROST_BIN=$BIFROST_BIN" "BIFROST_NATIVE_PYTHON=$BIFROST_NATIVE_PYTHON" \
        "PROXY_PORT=$PROXY_PORT" "BIFROST_E2E_REPORT_DIR=$BIFROST_E2E_REPORT_DIR" \
        /bin/bash "$SCRIPT_PATH")
    if [[ "$uid" == 0 ]]; then
        # An already-root caller also gets a clean environment before any work.
        exec "${command[@]}"
    else
        exec /usr/bin/sudo -n -- "${command[@]}"
    fi
}
elevate_native_fixture
NATIVE_MODE="${BIFROST_NATIVE_PROXY_MODE:-direct-exact}"
# Both the shell and every child now have the same UID. Existing PPID/start
# identity checks remain valid; all HOME/config/data are created below.
umask 022
TEST_ROOT="$(mktemp -d)"
export BIFROST_DATA_DIR="$TEST_ROOT/data"
SNAPSHOT_FILE="$TEST_ROOT/macos-proxy-before.tsv"
PROXY_LOG="$TEST_ROOT/proxy.log"
PROXY_PID=""
HELPER_PID=""
CORE_IDENTITY=""
HELPER_IDENTITY=""
PROXY_PAUSED=0
FIXTURE_MUTATED=0
NONROOT_CLEANED=0
READBACK="$ROOT_DIR/e2e-tests/test_utils/macos_proxy_acceptance.py"
NATIVE_SNAPSHOT="$TEST_ROOT/macos-proxy-before.json"
EVIDENCE_DIR="${BIFROST_E2E_REPORT_DIR:-$TEST_ROOT}/native-proxy-recovery-$NATIVE_MODE"
mkdir -p "$BIFROST_DATA_DIR" "$EVIDENCE_DIR"
# Only temporary shell/profile state is available to this fixture.
export HOME="$TEST_ROOT/home"
export XDG_CONFIG_HOME="$HOME/.config"
export XDG_DATA_HOME="$HOME/.local/share"
export SHELL=/bin/fish
mkdir -p "$HOME"
cat >"$BIFROST_DATA_DIR/config.toml" <<'EOF'
[system_proxy]
enabled = true
intent_revision = 1
recovery_mode = "fail_open"
recovery_grace_secs = 3
[sync]
enabled = false
auto_sync = false
[tray]
enabled = false
EOF
"$BIFROST_NATIVE_PYTHON" "$READBACK" intent "$BIFROST_DATA_DIR/config.toml" "$TEST_ROOT/intent-before.json"

network_services() {
    networksetup -listallnetworkservices 2>/dev/null | sed '1d' | sed '/^\*/d'
}

proxy_field() {
    local kind="$1"
    local service="$2"
    local field="$3"
    networksetup "-get${kind}proxy" "$service" 2>/dev/null \
        | awk -F': ' -v field="$field" '$1 == field { print $2; exit }'
}

save_proxy_snapshot() {
    local destination="${1:-$SNAPSHOT_FILE}"
    : >"$destination"
    while IFS= read -r service; do
        printf '%s|%s|%s|%s|%s|%s|%s|%s|%s\n' \
            "$service" \
            "$(proxy_field web "$service" Enabled)" \
            "$(proxy_field web "$service" Server)" \
            "$(proxy_field web "$service" Port)" \
            "$(proxy_field web "$service" 'Authenticated Proxy Enabled')" \
            "$(proxy_field secureweb "$service" Enabled)" \
            "$(proxy_field secureweb "$service" Server)" \
            "$(proxy_field secureweb "$service" Port)" \
            "$(proxy_field secureweb "$service" 'Authenticated Proxy Enabled')" \
            >>"$destination"
    done < <(network_services)
}

restore_proxy_snapshot() {
    [[ -f "$SNAPSHOT_FILE" ]] || return 0
    while IFS='|' read -r service web_enabled web_host web_port _web_auth secure_enabled secure_host secure_port _secure_auth; do
        if [[ -n "$web_host" && "$web_port" =~ ^[0-9]+$ ]]; then
            networksetup -setwebproxy "$service" "$web_host" "$web_port" >/dev/null 2>&1 || true
        fi
        if [[ "$web_enabled" == "Yes" ]]; then
            networksetup -setwebproxystate "$service" on >/dev/null 2>&1 || true
        else
            networksetup -setwebproxystate "$service" off >/dev/null 2>&1 || true
        fi
        if [[ -n "$secure_host" && "$secure_port" =~ ^[0-9]+$ ]]; then
            networksetup -setsecurewebproxy "$service" "$secure_host" "$secure_port" >/dev/null 2>&1 || true
        fi
        if [[ "$secure_enabled" == "Yes" ]]; then
            networksetup -setsecurewebproxystate "$service" on >/dev/null 2>&1 || true
        else
            networksetup -setsecurewebproxystate "$service" off >/dev/null 2>&1 || true
        fi
    done <"$SNAPSHOT_FILE"
}

child_identity() {
    local pid="$1" identity
    [[ "$pid" =~ ^[0-9]+$ ]] || return 1
    identity="$(LC_ALL=C ps -p "$pid" -o ppid= -o lstart= 2>/dev/null | awk '{$1=$1; print}')"
    [[ "${identity%% *}" == "$$" && -n "${identity#* }" ]] || return 1
    printf '%s\n' "$identity"
}

signal_owned_child() {
    local pid="$1" expected="$2" signal="$3" current
    [[ -n "$expected" ]] || return 1
    current="$(child_identity "$pid")" || return 1
    [[ "$current" == "$expected" ]] || return 1
    kill -"$signal" "$pid"
}

wait_owned_child_exit() {
    local pid="$1" expected="$2" deadline=$((SECONDS + $3)) current
    while current="$(child_identity "$pid")" && [[ "$current" == "$expected" ]]; do
        [[ "$SECONDS" -lt "$deadline" ]] || return 1
        sleep 0.2
    done
}

save_fixture_evidence() {
    if [[ "$EVIDENCE_DIR" != "$TEST_ROOT/"* ]]; then
        cp "$TEST_ROOT"/*.json "$TEST_ROOT"/*.tsv "$TEST_ROOT"/*.log "$EVIDENCE_DIR/" 2>/dev/null || true
        cp "$BIFROST_DATA_DIR"/system_proxy_incomplete_restores.json \
            "$BIFROST_DATA_DIR"/logs/system_proxy_events.jsonl "$BIFROST_DATA_DIR"/proxy_state.json \
            "$BIFROST_DATA_DIR"/proxy_backup.json "$EVIDENCE_DIR/" 2>/dev/null || true
    fi
}

privileged_fixture_cleanup_command() {
    require_disposable_macos_ci || return 1
    command /usr/bin/sudo -n -- /usr/bin/env -i PATH=/usr/bin:/bin:/usr/sbin:/sbin LC_ALL=C \
        "HOME=$HOME" "XDG_CONFIG_HOME=$XDG_CONFIG_HOME" "XDG_DATA_HOME=$XDG_DATA_HOME" \
        "BIFROST_DATA_DIR=$BIFROST_DATA_DIR" BIFROST_DISABLE_TRAY=1 \
        BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL=1 \
        BIFROST_SYSTEM_PROXY_DISABLE_LIFECYCLE_HELPER=1 \
        "$BIFROST_BIN" --log-output console,file system-proxy cleanup --data-dir "$BIFROST_DATA_DIR"
}

cleanup_nonroot_fixture() {
    [[ "$FIXTURE_MUTATED" == 1 && "$NONROOT_CLEANED" == 0 ]] || return 0
    if [[ -n "$PROXY_PID" ]]; then
        "$BIFROST_NATIVE_PYTHON" "$READBACK" retire-core "$BIFROST_DATA_DIR" "$PROXY_PID" "$CORE_IDENTITY" || return 1
        wait "$PROXY_PID" 2>/dev/null || true
        PROXY_PID=""
    fi
    # This is privileged FIXTURE CLEANUP after ordinary-user recovery assertions,
    # never evidence that the product's non-root stop can clear empty metadata.
    privileged_fixture_cleanup_command >"$TEST_ROOT/privileged-fixture-cleanup.log" 2>&1 || return 1
    "$BIFROST_NATIVE_PYTHON" "$READBACK" capture "$TEST_ROOT/fixture-cleaned.json" || return 1
    "$BIFROST_NATIVE_PYTHON" "$READBACK" disabled "$NATIVE_SNAPSHOT" "$TEST_ROOT/fixture-cleaned.json" || return 1
    "$BIFROST_NATIVE_PYTHON" "$READBACK" check-intent "$BIFROST_DATA_DIR/config.toml" "$TEST_ROOT/intent-before.json" || return 1
    NONROOT_CLEANED=1
    echo "PASS: privileged fixture cleanup restored exact baseline after non-root product recovery assertions"
}

cleanup() {
    local result=$?
    trap - EXIT
    set +e
    save_fixture_evidence
    # Revalidate direct-child PPID/start identity immediately before each signal.
    # Never search by process name, and never signal a reused PID.
    if [[ "$PROXY_PAUSED" == 1 && -n "$PROXY_PID" ]]; then
        signal_owned_child "$PROXY_PID" "$CORE_IDENTITY" CONT 2>/dev/null
        PROXY_PAUSED=0
    fi
    if [[ -n "$HELPER_PID" ]]; then
        # A still-monitoring helper enters its existing recovery path; a helper
        # already recovering completes the same bounded transaction. Killing it
        # could orphan a networksetup setter, so never SIGKILL this process.
        signal_owned_child "$HELPER_PID" "$HELPER_IDENTITY" TERM 2>/dev/null
        if ! wait_owned_child_exit "$HELPER_PID" "$HELPER_IDENTITY" 180; then
            echo "FAIL: native helper did not quiesce; refusing racing fallback writes; snapshots retained at $TEST_ROOT" >&2
            save_fixture_evidence
            exit 1
        fi
        wait "$HELPER_PID" 2>/dev/null
    fi
    if [[ "$NATIVE_MODE" == nonroot-recovery ]]; then
        if ! cleanup_nonroot_fixture; then
            echo "FAIL: privileged non-root fixture cleanup incomplete; retaining journal and snapshots at $TEST_ROOT" >&2
            save_fixture_evidence
            exit 1
        fi
    elif [[ -n "$PROXY_PID" ]]; then
        signal_owned_child "$PROXY_PID" "$CORE_IDENTITY" TERM 2>/dev/null
        if ! wait_owned_child_exit "$PROXY_PID" "$CORE_IDENTITY" 180; then
            echo "FAIL: native core did not quiesce; refusing racing fallback writes; snapshots retained at $TEST_ROOT" >&2
            save_fixture_evidence
            exit 1
        fi
        wait "$PROXY_PID" 2>/dev/null
    fi
    if [[ "$FIXTURE_MUTATED" == 1 && "$NATIVE_MODE" == direct-exact ]]; then
        restore_proxy_snapshot
        if [[ -f "$NATIVE_SNAPSHOT" ]]; then
            "$BIFROST_NATIVE_PYTHON" "$READBACK" restore-bypass "$NATIVE_SNAPSHOT" || result=1
        fi
    fi
    save_fixture_evidence
    rm -rf -- "$TEST_ROOT"
    exit "$result"
}
trap cleanup EXIT

save_proxy_snapshot
"$BIFROST_NATIVE_PYTHON" "$READBACK" capture "$NATIVE_SNAPSHOT"
if awk -F'|' '$2 == "Yes" || $5 == "1" || $6 == "Yes" || $9 == "1" { found = 1 } END { exit !found }' "$SNAPSHOT_FILE"; then
    echo "SKIP: existing enabled or authenticated system proxy must not be replaced by this test"
    exit 0
fi
if lsof -nP -iTCP:"$PROXY_PORT" -sTCP:LISTEN >/dev/null 2>&1; then
    echo "proxy port $PROXY_PORT is already in use"
    exit 1
fi

canary_ready() {
    [[ "$(curl --silent --show-error --max-time 2 --noproxy '' \
        --proxy "http://127.0.0.1:$PROXY_PORT" -o /dev/null -w '%{http_code}' \
        'http://bifrost-runtime-canary.invalid/__bifrost_runtime_canary')" == 204 ]]
}

wait_stable_canary() {
    local deadline=$((SECONDS + 30)) successes=0 first=0
    while [[ "$SECONDS" -lt "$deadline" ]]; do
        kill -0 "$PROXY_PID" || return 1
        if canary_ready; then
            if [[ "$successes" == 0 ]]; then first=$SECONDS; fi
            successes=$((successes + 1))
            if [[ "$successes" -ge 3 && "$((SECONDS - first))" -ge 2 ]]; then return 0; fi
        else
            successes=0
        fi
        sleep 0.5
    done
    echo "stable data-plane readiness was not established" >&2
    return 1
}

exercise_native_fail_open_resume() {
    wait_stable_canary
    "$BIFROST_NATIVE_PYTHON" "$READBACK" capture "$TEST_ROOT/product-active.json"
    "$BIFROST_NATIVE_PYTHON" "$READBACK" active "$NATIVE_SNAPSHOT" "$TEST_ROOT/product-active.json" "$PROXY_PORT"
    "$BIFROST_NATIVE_PYTHON" "$READBACK" lease "$BIFROST_DATA_DIR/proxy_state.json" "$TEST_ROOT/lease-before.json"
    local started_at
    started_at="$("$BIFROST_NATIVE_PYTHON" - "$BIFROST_DATA_DIR/runtime.json" "$PROXY_PID" "$PROXY_PORT" <<'PYID'
import json, sys
runtime = json.load(open(sys.argv[1]))
assert runtime["pid"] == int(sys.argv[2]), "runtime PID is not this fixture's child"
assert runtime["port"] == int(sys.argv[3]), "runtime port escaped the fixture"
assert isinstance(runtime.get("started_at_ms"), int) and runtime["started_at_ms"] > 0
print(runtime["started_at_ms"])
PYID
)"
    # The existing helper is identity- and generation-fenced. Its normal signal
    # recovery path probes the live-but-paused core; it must not replace it.
    "$BIFROST_BIN" --log-output console,file system-proxy lifecycle-helper --data-dir "$BIFROST_DATA_DIR" \
        --parent-pid "$PROXY_PID" --parent-started-at-ms "$started_at" --poll-secs 1 \
        >"$TEST_ROOT/recovery-helper.log" 2>&1 &
    HELPER_PID=$!
    HELPER_IDENTITY="$(child_identity "$HELPER_PID")"
    local deadline=$((SECONDS + 15))
    until grep -q 'system proxy lifecycle helper started;' "$TEST_ROOT/recovery-helper.log"; do
        kill -0 "$HELPER_PID" || return 1
        [[ "$SECONDS" -lt "$deadline" ]] || return 1
        sleep 0.2
    done
    # A changed heartbeat proves the helper has entered its event loop AFTER
    # installing signal handlers; its earlier startup log alone cannot do that.
    "$BIFROST_NATIVE_PYTHON" - "$BIFROST_DATA_DIR/system_proxy_owner_state.json" "$HELPER_PID" <<'PYHELPER'
import json, sys, time
from pathlib import Path
path, pid = Path(sys.argv[1]), int(sys.argv[2])
def heartbeat():
    state = json.loads(path.read_text())
    assert state["helper_pid"] == pid, "unexpected lifecycle helper identity"
    return state["helper_last_heartbeat_at"]
first = heartbeat()
deadline = time.monotonic() + 12
while time.monotonic() < deadline:
    if heartbeat() != first:
        break
    time.sleep(0.1)
else:
    raise AssertionError("helper signal loop did not become ready")
PYHELPER
    # Deterministic pause only AFTER the ownership writer becomes idle. This
    # does not claim recovery can bypass a live core frozen while holding its
    # journal lock; contention/partial-operation safety has separate unit tests.
    echo "INFO: native pause is fenced after ownership-lock quiescence"
    PROXY_PAUSED=1
    "$BIFROST_NATIVE_PYTHON" "$READBACK" pause-core "$BIFROST_DATA_DIR" "$PROXY_PID" "$CORE_IDENTITY"
    if canary_ready; then
        echo "paused fixture unexpectedly answered the data-plane canary" >&2
        return 1
    fi
    signal_owned_child "$HELPER_PID" "$HELPER_IDENTITY" TERM
    deadline=$((SECONDS + 25))
    until grep -q 'helper_fail_open_applied' "$BIFROST_DATA_DIR/logs/system_proxy_events.jsonl" 2>/dev/null; do
        [[ "$SECONDS" -lt "$deadline" ]] || {
            echo "native helper did not report successful fail-open suspension" >&2
            return 1
        }
        sleep 0.2
    done
    "$BIFROST_NATIVE_PYTHON" "$READBACK" capture "$TEST_ROOT/product-suspended.json"
    local suspended_assertion=disabled
    if [[ "$NATIVE_MODE" == nonroot-recovery ]]; then suspended_assertion=nonroot-suspended; fi
    "$BIFROST_NATIVE_PYTHON" "$READBACK" "$suspended_assertion" "$NATIVE_SNAPSHOT" "$TEST_ROOT/product-suspended.json"
    "$BIFROST_NATIVE_PYTHON" "$READBACK" check-lease "$BIFROST_DATA_DIR/proxy_state.json" "$TEST_ROOT/lease-before.json" suspended
    "$BIFROST_NATIVE_PYTHON" "$READBACK" check-intent "$BIFROST_DATA_DIR/config.toml" "$TEST_ROOT/intent-before.json"
    echo "PASS: real unhealthy core suspended native routing without changing configured intent"
    signal_owned_child "$PROXY_PID" "$CORE_IDENTITY" CONT
    PROXY_PAUSED=0
    wait_stable_canary
    deadline=$((SECONDS + 30))
    until "$BIFROST_NATIVE_PYTHON" "$READBACK" capture "$TEST_ROOT/product-resumed.json" \
        && "$BIFROST_NATIVE_PYTHON" "$READBACK" active "$NATIVE_SNAPSHOT" "$TEST_ROOT/product-resumed.json" "$PROXY_PORT"; do
        [[ "$SECONDS" -lt "$deadline" ]] || return 1
        sleep 0.5
    done
    "$BIFROST_NATIVE_PYTHON" "$READBACK" check-intent "$BIFROST_DATA_DIR/config.toml" "$TEST_ROOT/intent-before.json"
    "$BIFROST_NATIVE_PYTHON" - "$BIFROST_DATA_DIR/runtime.json" "$PROXY_PID" "$started_at" <<'PYID'
import json, sys
runtime = json.load(open(sys.argv[1]))
assert (runtime["pid"], runtime["started_at_ms"]) == (int(sys.argv[2]), int(sys.argv[3])), "live core was replaced"
PYID
    # Bound the helper's completion so it cannot race later fixture restoration.
    wait_owned_child_exit "$HELPER_PID" "$HELPER_IDENTITY" 35
    wait "$HELPER_PID"
    HELPER_PID=""
    "$BIFROST_NATIVE_PYTHON" "$READBACK" check-lease "$BIFROST_DATA_DIR/proxy_state.json" "$TEST_ROOT/lease-before.json" applied
    echo "PASS: stable healthy core automatically resumed native proxy with the same process identity"
}

FIXTURE_MUTATED=1
"$BIFROST_BIN" --host 127.0.0.1 --port "$PROXY_PORT" start \
    --skip-cert-check --unsafe-ssl --system-proxy \
    --proxy-bypass "localhost,127.0.0.1,::1,*.local" \
    >"$PROXY_LOG" 2>&1 &
PROXY_PID=$!
CORE_IDENTITY="$(child_identity "$PROXY_PID")"

for _ in $(seq 1 90); do
    if curl -sf "http://127.0.0.1:$PROXY_PORT/_bifrost/api/system" >/dev/null 2>&1; then
        break
    fi
    if ! kill -0 "$PROXY_PID" 2>/dev/null; then
        tail -n 160 "$PROXY_LOG"
        exit 1
    fi
    sleep 0.5
done
curl -sf "http://127.0.0.1:$PROXY_PORT/_bifrost/api/system" >/dev/null

for _ in $(seq 1 90); do
    if grep -h -q "system proxy transition verified" \
        "$BIFROST_DATA_DIR"/logs/bifrost.*.log 2>/dev/null; then
        break
    fi
    sleep 0.5
done

# Wait for completed inspections, not just elapsed intervals: native readback
# time grows with the number of network services on the host.
verification_count=0
verification_deadline="$((SECONDS + BIFROST_SYSTEM_PROXY_RECONCILE_SECS * 2 + 30))"
while [[ "$SECONDS" -lt "$verification_deadline" ]]; do
    verification_count="$({ grep -h "system proxy ownership verified without transition" \
        "$BIFROST_DATA_DIR"/logs/bifrost.*.log 2>/dev/null || true; } | wc -l | tr -d ' ')"
    if [[ "$verification_count" -ge 2 ]]; then
        break
    fi
    sleep 0.5
done
verified_transition_count="$({ grep -h "system proxy transition verified" \
    "$BIFROST_DATA_DIR"/logs/bifrost.*.log 2>/dev/null || true; } | wc -l | tr -d ' ')"
if [[ "$verified_transition_count" -ne 1 ]]; then
    echo "expected one verified system proxy transition across two short cycles, got $verified_transition_count"
    tail -n 200 "$PROXY_LOG" "$BIFROST_DATA_DIR"/logs/*.log 2>/dev/null || true
    exit 1
fi

if [[ "$verification_count" -lt 2 ]]; then
    echo "expected at least two read-only system proxy inspections, got $verification_count"
    tail -n 200 "$PROXY_LOG" "$BIFROST_DATA_DIR"/logs/*.log 2>/dev/null || true
    exit 1
fi

status="$(curl -sf "http://127.0.0.1:$PROXY_PORT/_bifrost/api/proxy/system")"
if ! grep -q '"managed_by_bifrost":true' <<<"$status"; then
    echo "system proxy ownership changed unexpectedly: $status"
    exit 1
fi

exercise_native_fail_open_resume

if [[ "$NATIVE_MODE" == nonroot-recovery ]]; then
    echo "PASS: ordinary non-root product suspended and resumed the same native lease; final exact cleanup is a separate privileged fixture action"
    cleanup_nonroot_fixture
else
    signal_owned_child "$PROXY_PID" "$CORE_IDENTITY" TERM
    wait_owned_child_exit "$PROXY_PID" "$CORE_IDENTITY" 30
    wait "$PROXY_PID" 2>/dev/null || true
    PROXY_PID=""
    # Observe the product before the fixture's fallback restoration can hide drift.
    "$BIFROST_NATIVE_PYTHON" "$READBACK" capture "$TEST_ROOT/product-stopped.json"
    "$BIFROST_NATIVE_PYTHON" "$READBACK" disabled "$NATIVE_SNAPSHOT" "$TEST_ROOT/product-stopped.json"
    "$BIFROST_NATIVE_PYTHON" "$READBACK" check-intent "$BIFROST_DATA_DIR/config.toml" "$TEST_ROOT/intent-before.json"
    echo "PASS: product stopped routing and preserved configured enable intent before fixture cleanup"
    restore_proxy_snapshot
    "$BIFROST_NATIVE_PYTHON" "$READBACK" restore-bypass "$NATIVE_SNAPSHOT"
fi
AFTER_SNAPSHOT_FILE="$TEST_ROOT/macos-proxy-after.tsv"
save_proxy_snapshot "$AFTER_SNAPSHOT_FILE"
if ! cmp -s "$SNAPSHOT_FILE" "$AFTER_SNAPSHOT_FILE"; then
    echo "system proxy snapshot was not restored exactly"
    diff -u "$SNAPSHOT_FILE" "$AFTER_SNAPSHOT_FILE" || true
    exit 1
fi

echo "PASS: converged system proxy performed one verified transition across two short cycles"
