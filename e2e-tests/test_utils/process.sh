#!/bin/bash

# E2E scripts should never open the Sync login browser unless the test is
# explicitly exercising startup login preflight. Keep this default in the
# shared process helper so direct Bifrost launches inherit it.
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT

is_windows() {
    local uname_out
    uname_out="$(uname -s 2>/dev/null)"
    case "$uname_out" in
        MINGW*|MSYS*|CYGWIN*)
            return 0
            ;;
        *)
            return 1
            ;;
    esac
}

_win_stop_process() {
    local pid=$1
    # Use canonical taskkill flags; `//F` can be rejected in some environments.
    taskkill.exe /F /PID "$pid" >/dev/null 2>&1 || true
}

_win_find_pid_on_port() {
    local port=$1
    # Best-effort: avoid aborting the whole suite under `set -e -o pipefail`.
    netstat.exe -ano 2>/dev/null \
        | awk -v p=":${port}" '$1 == "TCP" && $2 ~ p"$" && $4 == "LISTENING" { print $5; exit }' \
        | tr -d '\r' \
        || true
}

kill_pid() {
    local pid=$1
    if [ -z "$pid" ]; then
        return 0
    fi
    if is_windows; then
        kill "$pid" 2>/dev/null || _win_stop_process "$pid"
    else
        kill "$pid" 2>/dev/null || true
    fi
}

kill_pid_force() {
    local pid=$1
    if [ -z "$pid" ]; then
        return 0
    fi
    if is_windows; then
        _win_stop_process "$pid"
    else
        kill -9 "$pid" 2>/dev/null || true
    fi
}

kill_process_tree() {
    local pid=$1
    if [ -z "$pid" ]; then
        return 0
    fi
    if is_windows; then
        taskkill.exe /F /T /PID "$pid" >/dev/null 2>&1 || true
    else
        kill -- -"$pid" 2>/dev/null || kill -9 "$pid" 2>/dev/null || true
    fi
}

kill_bifrost_on_port() {
    local port=$1
    if [ -z "$port" ]; then
        return 0
    fi
    if is_windows; then
        local win_pid
        win_pid="$(_win_find_pid_on_port "$port" || true)"
        if [ -n "$win_pid" ]; then
            _win_stop_process "$win_pid"
            local wait_count=0
            while [[ $wait_count -lt 30 ]]; do
                win_pid="$(_win_find_pid_on_port "$port" || true)"
                if [[ -z "$win_pid" ]]; then
                    break
                fi
                if [[ $((wait_count % 10)) -eq 9 ]]; then
                    _win_stop_process "$win_pid"
                fi
                sleep 0.5
                wait_count=$((wait_count + 1))
            done
        fi
    else
        local pids=""
        if command -v lsof &>/dev/null; then
            pids="$(lsof -ti :"$port" 2>/dev/null || true)"
        fi
        if [[ -z "$pids" ]] && command -v ss &>/dev/null; then
            pids="$(ss -tlnp "sport = :$port" 2>/dev/null \
                | grep -oP 'pid=\K[0-9]+' 2>/dev/null || true)"
        fi
        if [[ -z "$pids" ]] && command -v fuser &>/dev/null; then
            pids="$(fuser "$port"/tcp 2>/dev/null | tr -s ' ' '\n' || true)"
        fi
        if [ -n "$pids" ]; then
            echo "$pids" | while IFS= read -r pid; do
                pid="$(echo "$pid" | tr -d '[:space:]')"
                [[ -n "$pid" ]] && kill -9 "$pid" 2>/dev/null || true
            done
        fi
    fi
}

win_wait_port_free() {
    local port=$1
    local max_wait=${2:-20}
    local waited=0
    while [[ $waited -lt $max_wait ]]; do
        local pid
        pid="$(_win_find_pid_on_port "$port" || true)"
        if [[ -z "$pid" ]]; then
            return 0
        fi
        sleep 0.5
        waited=$((waited + 1))
    done
    return 1
}

win_find_pid_on_port() {
    _win_find_pid_on_port "$@"
}

kill_all_bifrost() {
    if is_windows; then
        taskkill.exe /F /IM bifrost.exe >/dev/null 2>&1 || true
        sleep 2
    else
        pkill -f bifrost 2>/dev/null || killall bifrost 2>/dev/null || true
    fi
}

wait_pid() {
    local pid=$1
    if [ -z "$pid" ]; then
        return 0
    fi

    local timeout="${BIFROST_E2E_WAIT_PID_TIMEOUT:-30}"
    local elapsed=0
    while kill -0 "$pid" 2>/dev/null; do
        sleep 0.2
        elapsed=$((elapsed + 1))
        if [ "$elapsed" -ge "$((timeout * 5))" ]; then
            kill_pid_force "$pid"
            return 1
        fi
    done

    if ! is_windows; then
        wait "$pid" 2>/dev/null || true
    fi
    return 0
}

python_cmd() {
    # Backwards-compatible alias.
    python3_cmd
}

python3_cmd() {
    # Prefer python3, but also allow `python` if it is Python 3.
    # Cache the resolved command in BIFROST_E2E_PYTHON_BIN to keep logs stable.
    if [[ -n "${BIFROST_E2E_PYTHON_BIN:-}" ]]; then
        echo "$BIFROST_E2E_PYTHON_BIN"
        return 0
    fi

    if command -v python3 &>/dev/null; then
        if python3 -c 'import sys; raise SystemExit(0 if sys.version_info[0] >= 3 else 1)' >/dev/null 2>&1; then
            export BIFROST_E2E_PYTHON_BIN="python3"
            echo "$BIFROST_E2E_PYTHON_BIN"
            return 0
        fi
    fi

    if command -v python &>/dev/null; then
        if python -c 'import sys; raise SystemExit(0 if sys.version_info[0] >= 3 else 1)' >/dev/null 2>&1; then
            export BIFROST_E2E_PYTHON_BIN="python"
            echo "$BIFROST_E2E_PYTHON_BIN"
            return 0
        fi
    fi

    return 1
}

# ---------------------------------------------------------------------------
# E2E infra helpers (ports / polling)
# ---------------------------------------------------------------------------

_require_python_for_port_alloc() {
    local py
    py="$(python3_cmd 2>/dev/null || true)"
    if [[ -z "${py:-}" ]]; then
        echo "ERROR: python3 (or python>=3) is required for E2E infrastructure" >&2
        return 1
    fi
    export BIFROST_E2E_PYTHON_BIN="$py"
    return 0
}

allocate_free_port() {
    _require_python_for_port_alloc || return 1
    "$BIFROST_E2E_PYTHON_BIN" - <<'PY'
import socket

s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
s.bind(("0.0.0.0", 0))
print(s.getsockname()[1])
s.close()
PY
}

port_is_available() {
    local port="$1"
    _require_python_for_port_alloc || return 1
    "$BIFROST_E2E_PYTHON_BIN" - "$port" <<'PY'
import socket
import sys

port = int(sys.argv[1])
ok = True
s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
try:
    s.bind(("127.0.0.1", port))
except OSError:
    ok = False
finally:
    try:
        s.close()
    except Exception:
        pass
if ok:
    s2 = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    s2.settimeout(0.2)
    try:
        s2.connect(("127.0.0.1", port))
        ok = False
    except (ConnectionRefusedError, OSError):
        pass
    finally:
        try:
            s2.close()
        except Exception:
            pass
sys.exit(0 if ok else 1)
PY
}

# Pick a base port such that [base, base+span-1] are all available.
# - requested_base_port: 0 means pick a randomized starting point.
# - span: number of consecutive ports needed.
pick_available_base_port() {
    local requested_base_port="${1:-0}"
    local span="${2:-1}"

    _require_python_for_port_alloc || return 1

    "$BIFROST_E2E_PYTHON_BIN" - "$requested_base_port" "$span" <<'PY'
import random
import socket
import sys

requested = int(sys.argv[1])
span = int(sys.argv[2])

def range_ok(base: int, span: int) -> bool:
    sockets = []
    try:
        for p in range(base, base + span):
            s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            s.bind(("127.0.0.1", p))
            sockets.append(s)
            probe = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            probe.settimeout(0.1)
            try:
                probe.connect(("127.0.0.1", p))
                return False
            except (ConnectionRefusedError, OSError):
                pass
            finally:
                try:
                    probe.close()
                except Exception:
                    pass
        return True
    except OSError:
        return False
    finally:
        for s in sockets:
            try:
                s.close()
            except Exception:
                pass

def candidate_bases():
    low, high = 10000, 19999
    high = max(low, high - max(span, 1) - 1)
    if requested > 0:
        yield requested
        for i in range(1, 50):
            yield requested + i * 100
    for _ in range(200):
        yield random.randint(low, high)

for base in candidate_bases():
    if base <= 0:
        continue
    if base + span >= 65535:
        continue
    if range_ok(base, span):
        print(base)
        sys.exit(0)

print(0)
sys.exit(1)
PY
}

wait_for_http_ready() {
    local url="$1"
    local timeout_secs="${2:-30}"
    local interval_secs="${3:-0.2}"

    local start_ts
    start_ts="$(date +%s)"
    while true; do
        if curl -fsS --connect-timeout 2 --max-time 5 "$url" >/dev/null 2>&1; then
            return 0
        fi

        local now_ts
        now_ts="$(date +%s)"
        if (( now_ts - start_ts >= timeout_secs )); then
            return 1
        fi
        sleep "$interval_secs"
    done
}

start_echo_server() {
    local port=$1
    local log_file=${2:-/dev/null}
    local script_dir
    script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
    local server_script="${script_dir}/mock_servers/http_echo_server.py"

    _require_python_for_port_alloc || return 1

    "$BIFROST_E2E_PYTHON_BIN" "${server_script}" "${port}" > >(tee "${log_file}") 2>&1 &
    local pid=$!
    echo "$pid"

    local ready=0
    for _ in $(seq 1 150); do
        if ! kill -0 "${pid}" 2>/dev/null; then
            echo "ERROR: echo server process (PID=${pid}) exited prematurely" >&2
            [[ -f "${log_file}" ]] && cat "${log_file}" >&2
            return 1
        fi
        if grep -q '^READY$' "${log_file}" 2>/dev/null; then
            ready=1
            break
        fi
        sleep 0.2
    done

    if [[ "${ready}" -ne 1 ]]; then
        echo "ERROR: echo server did not become ready in 30s" >&2
        [[ -f "${log_file}" ]] && cat "${log_file}" >&2
        return 1
    fi
    return 0
}

safe_cleanup_proxy() {
    local pid=$1
    if [ -z "$pid" ]; then
        return 0
    fi

    kill_pid "$pid"

    local timeout=5
    local elapsed=0
    while kill -0 "$pid" 2>/dev/null; do
        sleep 0.2
        elapsed=$((elapsed + 1))
        if [ "$elapsed" -ge "$((timeout * 5))" ]; then
            break
        fi
    done

    if kill -0 "$pid" 2>/dev/null; then
        kill_pid_force "$pid"
        sleep 0.5
    fi

    if is_windows; then
        if kill -0 "$pid" 2>/dev/null; then
            _win_stop_process "$pid"
        fi
    fi
}
