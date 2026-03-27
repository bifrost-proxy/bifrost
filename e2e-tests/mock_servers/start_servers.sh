#!/bin/bash
# Mock 服务器管理脚本

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

HTTP_PORT=${HTTP_PORT:-3000}
HTTPS_PORT=${HTTPS_PORT:-3443}
WS_PORT=${WS_PORT:-3020}
WSS_PORT=${WSS_PORT:-3021}
SSE_PORT=${SSE_PORT:-3003}
PROXY_PORT=${PROXY_PORT:-9999}

declare -a PIDS=()

log() {
    echo "[$(date '+%Y-%m-%d %H:%M:%S')] $1"
}

run_python_server() {
    PYTHONUTF8=1 PYTHONIOENCODING=utf-8 python3 -X utf8 "$@"
}

cleanup() {
    log "Stopping all servers..."
    for pid in "${PIDS[@]}"; do
        if kill -0 "$pid" 2>/dev/null; then
            kill "$pid" 2>/dev/null
            log "Stopped server with PID $pid"
        fi
    done
    exit 0
}

trap cleanup SIGINT SIGTERM

start_http() {
    log "Starting HTTP Echo Server on port $HTTP_PORT..."
    run_python_server "$SCRIPT_DIR/http_echo_server.py" "$HTTP_PORT" &
    PIDS+=($!)
}

start_https() {
    log "Starting HTTPS Echo Server on port $HTTPS_PORT..."
    run_python_server "$SCRIPT_DIR/https_echo_server.py" "$HTTPS_PORT" &
    PIDS+=($!)
}

start_ws() {
    log "Starting WebSocket Echo Server on port $WS_PORT..."
    run_python_server "$SCRIPT_DIR/ws_echo_server.py" "$WS_PORT" &
    PIDS+=($!)
}

start_wss() {
    log "Starting WebSocket Secure Echo Server on port $WSS_PORT..."
    run_python_server "$SCRIPT_DIR/ws_echo_server.py" "$WSS_PORT" --ssl &
    PIDS+=($!)
}

start_sse() {
    log "Starting SSE Echo Server on port $SSE_PORT..."
    run_python_server "$SCRIPT_DIR/sse_echo_server.py" --port "$SSE_PORT" &
    PIDS+=($!)
}

start_proxy() {
    log "Starting HTTP Proxy Echo Server on port $PROXY_PORT..."
    run_python_server "$SCRIPT_DIR/http_echo_server.py" "$PROXY_PORT" &
    PIDS+=($!)
}

start_all() {
    start_http
    sleep 0.5
    start_https
    sleep 0.5
    start_ws
    sleep 0.5
    start_wss
    sleep 0.5
    start_sse
    sleep 0.5
    start_proxy
}

wait_for_server() {
    local host=$1
    local port=$2
    local max_attempts=${3:-30}
    local attempt=0

    while [ $attempt -lt $max_attempts ]; do
        if nc -z "$host" "$port" 2>/dev/null; then
            return 0
        fi
        sleep 0.1
        ((attempt++))
    done
    return 1
}

status() {
    echo "Mock Server Status:"
    echo "==================="

    if nc -z 127.0.0.1 "$HTTP_PORT" 2>/dev/null; then
        echo "HTTP   (port $HTTP_PORT): ✅ Running"
    else
        echo "HTTP   (port $HTTP_PORT): ❌ Not running"
    fi

    if nc -z 127.0.0.1 "$HTTPS_PORT" 2>/dev/null; then
        echo "HTTPS  (port $HTTPS_PORT): ✅ Running"
    else
        echo "HTTPS  (port $HTTPS_PORT): ❌ Not running"
    fi

    if nc -z 127.0.0.1 "$WS_PORT" 2>/dev/null; then
        echo "WS     (port $WS_PORT): ✅ Running"
    else
        echo "WS     (port $WS_PORT): ❌ Not running"
    fi

    if nc -z 127.0.0.1 "$WSS_PORT" 2>/dev/null; then
        echo "WSS    (port $WSS_PORT): ✅ Running"
    else
        echo "WSS    (port $WSS_PORT): ❌ Not running"
    fi

    if nc -z 127.0.0.1 "$SSE_PORT" 2>/dev/null; then
        echo "SSE    (port $SSE_PORT): ✅ Running"
    else
        echo "SSE    (port $SSE_PORT): ❌ Not running"
    fi

    if nc -z 127.0.0.1 "$PROXY_PORT" 2>/dev/null; then
        echo "PROXY  (port $PROXY_PORT): ✅ Running"
    else
        echo "PROXY  (port $PROXY_PORT): ❌ Not running"
    fi
}

stop_all() {
    log "Stopping all mock servers..."
    pkill -f "http_echo_server.py" 2>/dev/null
    pkill -f "https_echo_server.py" 2>/dev/null
    pkill -f "ws_echo_server.py" 2>/dev/null
    pkill -f "sse_echo_server.py" 2>/dev/null
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
        start_all
        log "All servers started in background."
        sleep 2
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
