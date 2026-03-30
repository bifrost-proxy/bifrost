#!/bin/bash
# Mock 服务器管理脚本

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../test_utils/process.sh"

HTTP_PORT=${HTTP_PORT:-3000}
HTTPS_PORT=${HTTPS_PORT:-3443}
WS_PORT=${WS_PORT:-3020}
WSS_PORT=${WSS_PORT:-3021}
SSE_PORT=${SSE_PORT:-3003}
PROXY_PORT=${MOCK_ECHO_PROXY_PORT:-${PROXY_PORT:-9999}}
SERVER_LOG_DIR=${SERVER_LOG_DIR:-"$SCRIPT_DIR/.logs"}
DETACHED_MODE=false

declare -a PIDS=()

log() {
    echo "[$(date '+%Y-%m-%d %H:%M:%S')] $1"
}

run_python_server() {
    PYTHONUTF8=1 PYTHONIOENCODING=utf-8 python3 -X utf8 "$@"
}

start_server_process() {
    local log_name=$1
    shift

    if [ "$DETACHED_MODE" = true ]; then
        mkdir -p "$SERVER_LOG_DIR"
        local log_file="$SERVER_LOG_DIR/${log_name}.log"
        PYTHONUTF8=1 PYTHONIOENCODING=utf-8 python3 -c '
import os
import subprocess
import sys

log_path = sys.argv[1]
cmd = [sys.executable, "-X", "utf8", *sys.argv[2:]]
env = os.environ.copy()

log_fh = open(log_path, "ab", buffering=0)
kwargs = dict(
    stdin=subprocess.DEVNULL,
    stdout=log_fh,
    stderr=log_fh,
    env=env,
)

if sys.platform == "win32":
    CREATE_NEW_PROCESS_GROUP = 0x00000200
    CREATE_BREAKAWAY_FROM_JOB = 0x01000000
    flags_attempts = [
        CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB,
        CREATE_NEW_PROCESS_GROUP,
        0,
    ]
    last_err = None
    for flags in flags_attempts:
        try:
            kw = dict(kwargs)
            if flags:
                kw["creationflags"] = flags
            subprocess.Popen(cmd, **kw)
            last_err = None
            break
        except (PermissionError, OSError) as exc:
            last_err = exc
    if last_err is not None:
        sys.stderr.write(f"Failed to spawn {cmd}: {last_err}\n")
        sys.exit(1)
else:
    kwargs["close_fds"] = True
    kwargs["start_new_session"] = True
    subprocess.Popen(cmd, **kwargs)
' "$log_file" "$@" &
    else
        run_python_server "$@" &
    fi
    PIDS+=($!)
}

check_tcp_port() {
    local host=$1
    local port=$2
    if command -v nc &>/dev/null; then
        nc -z "$host" "$port" >/dev/null 2>&1
    else
        (echo > /dev/tcp/"$host"/"$port") >/dev/null 2>&1
    fi
}

wait_for_port_closed() {
    local host=$1
    local port=$2
    local service_name=$3
    local max_attempts=${4:-30}
    local attempt=0

    while [ $attempt -lt $max_attempts ]; do
        if ! check_tcp_port "$host" "$port"; then
            return 0
        fi
        sleep 1
        ((attempt++))
    done

    log "$service_name port $port is still busy after $max_attempts attempts"
    return 1
}

check_http_health() {
    local port=$1
    curl -sf --connect-timeout 2 --max-time 5 "http://127.0.0.1:${port}/health" >/dev/null 2>&1
}

check_https_health() {
    local port=$1
    curl -skf --connect-timeout 2 --max-time 5 "https://127.0.0.1:${port}/health" >/dev/null 2>&1
}

cleanup() {
    log "Stopping all servers..."
    for pid in "${PIDS[@]}"; do
        if kill -0 "$pid" 2>/dev/null; then
            safe_cleanup_proxy "$pid"
            log "Stopped server with PID $pid"
        fi
    done
    exit 0
}

trap cleanup SIGINT SIGTERM

start_http() {
    log "Starting HTTP Echo Server on port $HTTP_PORT..."
    start_server_process "http_echo_server" "$SCRIPT_DIR/http_echo_server.py" "$HTTP_PORT"
}

start_https() {
    log "Starting HTTPS Echo Server on port $HTTPS_PORT..."
    start_server_process "https_echo_server" "$SCRIPT_DIR/https_echo_server.py" "$HTTPS_PORT"
}

start_ws() {
    log "Starting WebSocket Echo Server on port $WS_PORT..."
    start_server_process "ws_echo_server" "$SCRIPT_DIR/ws_echo_server.py" "$WS_PORT"
}

start_wss() {
    log "Starting WebSocket Secure Echo Server on port $WSS_PORT..."
    start_server_process "wss_echo_server" "$SCRIPT_DIR/ws_echo_server.py" "$WSS_PORT" --ssl
}

start_sse() {
    log "Starting SSE Echo Server on port $SSE_PORT..."
    start_server_process "sse_echo_server" "$SCRIPT_DIR/sse_echo_server.py" --port "$SSE_PORT"
}

start_proxy() {
    log "Starting HTTP Proxy Echo Server on port $PROXY_PORT..."
    start_server_process "proxy_echo_server" "$SCRIPT_DIR/http_echo_server.py" "$PROXY_PORT"
}

should_start_server() {
    local name=$1
    if [ -z "${MOCK_SERVERS:-}" ]; then
        return 0
    fi
    case ",$MOCK_SERVERS," in
        *",$name,"*) return 0 ;;
        *) return 1 ;;
    esac
}

start_all() {
    ! should_start_server http || start_http
    ! should_start_server https || start_https
    ! should_start_server ws || start_ws
    ! should_start_server wss || start_wss
    ! should_start_server sse || start_sse
    sleep 0.5
    ! should_start_server proxy || start_proxy
}

wait_for_server() {
    local check_cmd=$1
    local service_name=$2
    local max_attempts=${3:-30}
    local attempt=0

    while [ $attempt -lt $max_attempts ]; do
        if eval "$check_cmd"; then
            log "$service_name ready (${attempt}s)"
            return 0
        fi
        sleep 1
        ((attempt++))
    done
    log "$service_name did not become ready after $max_attempts attempts"
    return 1
}

wait_for_all_servers() {
    local failed=0

    ! should_start_server http || { wait_for_server "check_http_health $HTTP_PORT" "HTTP Echo Server" 30 || failed=1; }
    ! should_start_server https || { wait_for_server "check_https_health $HTTPS_PORT" "HTTPS Echo Server" 45 || failed=1; }
    ! should_start_server ws || { wait_for_server "check_tcp_port 127.0.0.1 $WS_PORT" "WebSocket Echo Server" 30 || failed=1; }
    ! should_start_server wss || { wait_for_server "check_tcp_port 127.0.0.1 $WSS_PORT" "WebSocket Secure Echo Server" 45 || failed=1; }
    ! should_start_server sse || { wait_for_server "check_http_health $SSE_PORT" "SSE Echo Server" 30 || failed=1; }
    ! should_start_server proxy || { wait_for_server "check_http_health $PROXY_PORT" "HTTP Proxy Echo Server" 30 || failed=1; }

    return $failed
}

status() {
    echo "Mock Server Status:"
    echo "==================="

    if check_http_health "$HTTP_PORT"; then
        echo "HTTP   (port $HTTP_PORT): ✅ Running"
    else
        echo "HTTP   (port $HTTP_PORT): ❌ Not running"
    fi

    if check_https_health "$HTTPS_PORT"; then
        echo "HTTPS  (port $HTTPS_PORT): ✅ Running"
    else
        echo "HTTPS  (port $HTTPS_PORT): ❌ Not running"
    fi

    if check_tcp_port 127.0.0.1 "$WS_PORT"; then
        echo "WS     (port $WS_PORT): ✅ Running"
    else
        echo "WS     (port $WS_PORT): ❌ Not running"
    fi

    if check_tcp_port 127.0.0.1 "$WSS_PORT"; then
        echo "WSS    (port $WSS_PORT): ✅ Running"
    else
        echo "WSS    (port $WSS_PORT): ❌ Not running"
    fi

    if check_http_health "$SSE_PORT"; then
        echo "SSE    (port $SSE_PORT): ✅ Running"
    else
        echo "SSE    (port $SSE_PORT): ❌ Not running"
    fi

    if check_http_health "$PROXY_PORT"; then
        echo "PROXY  (port $PROXY_PORT): ✅ Running"
    else
        echo "PROXY  (port $PROXY_PORT): ❌ Not running"
    fi
}

stop_all() {
    log "Stopping all mock servers..."
    local port pid
    local ports_to_stop=()
    local names_to_stop=()

    if should_start_server http; then ports_to_stop+=("$HTTP_PORT"); names_to_stop+=("HTTP Echo Server"); fi
    if should_start_server https; then ports_to_stop+=("$HTTPS_PORT"); names_to_stop+=("HTTPS Echo Server"); fi
    if should_start_server ws; then ports_to_stop+=("$WS_PORT"); names_to_stop+=("WebSocket Echo Server"); fi
    if should_start_server wss; then ports_to_stop+=("$WSS_PORT"); names_to_stop+=("WebSocket Secure Echo Server"); fi
    if should_start_server sse; then ports_to_stop+=("$SSE_PORT"); names_to_stop+=("SSE Echo Server"); fi
    if should_start_server proxy; then ports_to_stop+=("$PROXY_PORT"); names_to_stop+=("HTTP Proxy Echo Server"); fi

    for port in "${ports_to_stop[@]}"; do
        if is_windows; then
            pid=$(netstat.exe -ano 2>/dev/null \
                | awk -v p=":${port}" '$1 == "TCP" && $2 ~ p"$" && $4 == "LISTENING" { print $5; exit }' \
                | tr -d '\r')
            if [[ -n "$pid" && "$pid" != "0" ]]; then
                taskkill.exe //F //PID "$pid" >/dev/null 2>&1 || true
            fi
        else
            local target_pid=""
            if command -v lsof &>/dev/null; then
                target_pid=$(lsof -ti :"$port" 2>/dev/null | head -n 1)
            elif command -v fuser &>/dev/null; then
                target_pid=$(fuser "$port"/tcp 2>/dev/null | awk '{print $1}')
            fi
            if [[ -n "$target_pid" ]]; then
                kill -9 "$target_pid" 2>/dev/null || true
            fi
        fi
    done

    local failed=0
    local i=0
    for port in "${ports_to_stop[@]}"; do
        wait_for_port_closed 127.0.0.1 "$port" "${names_to_stop[$i]}" 30 || failed=1
        ((i++))
    done

    if [ $failed -ne 0 ]; then
        log "Some mock server ports did not close cleanly."
        return 1
    fi

    log "All servers stopped"
}

usage() {
    echo "Usage: $0 {start|stop|status|start-http|start-https|start-ws|start-wss|start-sse}"
    echo ""
    echo "Commands:"
    echo "  start       Start all mock servers in foreground"
    echo "  start-bg    Start all mock servers in background"
    echo "  stop        Stop all mock servers"
    echo "  status      Check server status"
    echo "  start-http  Start only HTTP echo server"
    echo "  start-https Start only HTTPS echo server"
    echo "  start-ws    Start only WebSocket echo server"
    echo "  start-wss   Start only WebSocket Secure echo server"
    echo "  start-sse   Start only SSE echo server"
    echo ""
    echo "Environment variables:"
    echo "  HTTP_PORT   HTTP server port (default: 3000)"
    echo "  HTTPS_PORT  HTTPS server port (default: 3443)"
    echo "  WS_PORT     WebSocket server port (default: 3020)"
    echo "  WSS_PORT    WebSocket Secure server port (default: 3021)"
    echo "  SSE_PORT    SSE server port (default: 3003)"
    echo "  PROXY_PORT  Proxy echo server port (default: 9999)"
}

case "$1" in
    start)
        start_all
        log "All servers started. Press Ctrl+C to stop."
        wait
        ;;
    start-bg)
        DETACHED_MODE=true
        start_all
        log "Waiting for mock servers to become ready..."
        if ! wait_for_all_servers; then
            log "Some mock servers failed readiness checks."
            status
            if [ -d "$SERVER_LOG_DIR" ]; then
                log "=== Mock server logs ==="
                for f in "$SERVER_LOG_DIR"/*.log; do
                    [ -f "$f" ] || continue
                    log "--- $(basename "$f") ---"
                    tail -n 30 "$f" 2>/dev/null || true
                done
                log "=== End of mock server logs ==="
            fi
            exit 1
        fi
        log "All servers started in background."
        status
        ;;
    stop)
        stop_all
        ;;
    status)
        status
        ;;
    start-http)
        start_http
        wait
        ;;
    start-https)
        start_https
        wait
        ;;
    start-ws)
        start_ws
        wait
        ;;
    start-wss)
        start_wss
        wait
        ;;
    start-sse)
        start_sse
        wait
        ;;
    *)
        usage
        exit 1
        ;;
esac
