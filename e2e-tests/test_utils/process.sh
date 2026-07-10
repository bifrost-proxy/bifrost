#!/bin/bash

# E2E scripts should never open desktop UI unless the test explicitly covers it.
# Keep these defaults in the shared process helper so direct Bifrost launches
# do not spawn Sync login browser windows or tray helpers.
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
: "${BIFROST_DISABLE_TRAY:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
export BIFROST_DISABLE_TRAY

E2E_OWNERSHIP_MARKER=".bifrost-e2e-owned"

is_production_bifrost_path() {
    local path="${1%/}"
    [[ -n "${HOME:-}" ]] || return 1
    local production_root="${HOME:-}/.bifrost"
    production_root="${production_root%/}"
    [[ -n "$path" && -n "$production_root" \
        && ( "$path" == "$production_root" || "$path" == "$production_root/"* ) ]]
}

mark_e2e_data_root() {
    local root="$1"
    [[ -n "$root" ]] || return 1
    if is_production_bifrost_path "$root" \
        && [[ "${BIFROST_E2E_ALLOW_PRODUCTION_DATA_DIR:-0}" != "1" ]]; then
        echo "REFUSING: E2E ownership marker cannot be created under the production data root: $root" >&2
        return 1
    fi
    mkdir -p "$root"
    : >"$root/$E2E_OWNERSHIP_MARKER"
}

is_e2e_data_root() {
    local root="$1"
    [[ -n "$root" && -f "$root/$E2E_OWNERSHIP_MARKER" ]]
}

# Directly executed E2E scripts used to fall back to ~/.bifrost unless the
# umbrella runner happened to inject BIFROST_DATA_DIR. A parent Bifrost/IM
# process may also export that production directory into the agent shell, so
# merely checking "is the variable set" is insufficient. Refuse the production
# default unless a destructive test explicitly opts in.
_bifrost_data_dir_is_production=0
if is_production_bifrost_path "${BIFROST_DATA_DIR:-}"; then
    _bifrost_data_dir_is_production=1
fi
if [[ -z "${BIFROST_DATA_DIR:-}" \
    || ( "$_bifrost_data_dir_is_production" == "1" \
        && "${BIFROST_E2E_ALLOW_PRODUCTION_DATA_DIR:-0}" != "1" ) ]]; then
    _bifrost_e2e_root="${BIFROST_E2E_SANDBOX_DIR:-${PWD}/.bifrost-e2e-runs/direct}"
    if ! mark_e2e_data_root "$_bifrost_e2e_root"; then
        unset _bifrost_e2e_root _bifrost_data_dir_is_production
        return 1 2>/dev/null || exit 1
    fi
    BIFROST_DATA_DIR="$(mktemp -d "$_bifrost_e2e_root/data-XXXXXX")"
    export BIFROST_DATA_DIR
    unset _bifrost_e2e_root
fi
unset _bifrost_data_dir_is_production

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
    local protected_ports=",${BIFROST_E2E_PROTECTED_PORTS:-9900},"
    if [[ "$protected_ports" == *",${port},"* ]]; then
        echo "REFUSING: E2E cleanup cannot kill protected port ${port}" >&2
        return 1
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
                if [[ -n "$pid" ]]; then
                    kill -INT "$pid" 2>/dev/null || kill "$pid" 2>/dev/null || true
                fi
            done

            local wait_count=0
            while [[ $wait_count -lt 20 ]]; do
                local remaining=""
                if command -v lsof &>/dev/null; then
                    remaining="$(lsof -ti :"$port" 2>/dev/null || true)"
                fi
                if [[ -z "$remaining" ]]; then
                    break
                fi
                sleep 0.25
                wait_count=$((wait_count + 1))
            done

            local remaining=""
            if command -v lsof &>/dev/null; then
                remaining="$(lsof -ti :"$port" 2>/dev/null || true)"
            fi
            if [[ -n "$remaining" ]]; then
                echo "$remaining" | while IFS= read -r pid; do
                    pid="$(echo "$pid" | tr -d '[:space:]')"
                    if [[ -n "$pid" ]]; then
                        kill -9 "$pid" 2>/dev/null || true
                        wait "$pid" 2>/dev/null || true
                    fi
                done
            fi

            echo "$pids" | while IFS= read -r pid; do
                pid="$(echo "$pid" | tr -d '[:space:]')"
                if [[ -n "$pid" ]]; then
                    wait "$pid" 2>/dev/null || true
                fi
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

is_bifrost_process() {
    local pid="$1"
    [[ "$pid" =~ ^[0-9]+$ ]] || return 1
    if is_windows; then
        tasklist.exe /FI "PID eq $pid" /FO CSV /NH 2>/dev/null \
            | tr -d '\r' \
            | grep -Eqi '^"?bifrost\.exe"?,'
        return $?
    fi

    local command_name
    command_name="$(ps -p "$pid" -o comm= 2>/dev/null | awk '{$1=$1; print}' || true)"
    command_name="${command_name##*/}"
    [[ "$command_name" == "bifrost" || "$command_name" == "bifrost.exe" ]]
}

pid_from_runtime_file() {
    local path="$1"
    sed -n 's/.*"pid"[[:space:]]*:[[:space:]]*\([0-9][0-9]*\).*/\1/p' "$path" 2>/dev/null | head -n 1
}

kill_bifrost_in_data_root() {
    local root="$1"
    if is_production_bifrost_path "$root" \
        && [[ "${BIFROST_E2E_ALLOW_PRODUCTION_DATA_DIR:-0}" != "1" ]]; then
        echo "REFUSING: E2E cleanup cannot scan the production data root: ${root:-<empty>}" >&2
        return 1
    fi
    if ! is_e2e_data_root "$root"; then
        echo "REFUSING: E2E cleanup root is not owned by this test run: ${root:-<empty>}" >&2
        return 1
    fi

    local seen=" "
    while IFS= read -r pid_file; do
        local pid=""
        case "${pid_file##*/}" in
            runtime.json) pid="$(pid_from_runtime_file "$pid_file")" ;;
            *) pid="$(cat "$pid_file" 2>/dev/null | tr -cd '0-9' || true)" ;;
        esac
        [[ "$pid" =~ ^[0-9]+$ ]] || continue
        [[ "$seen" != *" $pid "* ]] || continue
        seen+="$pid "
        if is_bifrost_process "$pid"; then
            kill_pid "$pid"
        fi
    done < <(find "$root" -type f \( -name runtime.json -o -name bifrost.pid -o -name tray.pid \) -print 2>/dev/null)

    local waited=0
    while [[ $waited -lt 50 ]]; do
        local alive=0
        local pid
        for pid in $seen; do
            if [[ "$pid" =~ ^[0-9]+$ ]] && is_bifrost_process "$pid"; then
                alive=1
                break
            fi
        done
        [[ "$alive" -eq 0 ]] && return 0
        sleep 0.2
        waited=$((waited + 1))
    done

    local pid
    for pid in $seen; do
        if [[ "$pid" =~ ^[0-9]+$ ]] && is_bifrost_process "$pid"; then
            kill_pid_force "$pid"
        fi
    done
}

kill_all_bifrost() {
    local root="${BIFROST_E2E_SANDBOX_DIR:-${BIFROST_DATA_DIR:-}}"
    kill_bifrost_in_data_root "$root"
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

    if is_windows; then
        kill_pid "$pid"
    else
        kill -INT "$pid" 2>/dev/null || kill_pid "$pid"
    fi

    local timeout="${BIFROST_E2E_CLEANUP_TIMEOUT:-10}"
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
        wait "$pid" 2>/dev/null || true
        sleep 0.5
    else
        wait "$pid" 2>/dev/null || true
    fi

    if is_windows; then
        if kill -0 "$pid" 2>/dev/null; then
            _win_stop_process "$pid"
        fi
    fi
}
