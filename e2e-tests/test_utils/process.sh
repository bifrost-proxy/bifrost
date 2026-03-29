#!/bin/bash

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
    taskkill.exe //F //PID "$pid" >/dev/null 2>&1 || true
}

_win_find_pid_on_port() {
    local port=$1
    netstat.exe -ano 2>/dev/null \
        | awk -v p=":${port}" '$1 == "TCP" && $2 ~ p"$" && $4 == "LISTENING" { print $5; exit }' \
        | tr -d '\r'
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
        taskkill.exe //F //T //PID "$pid" >/dev/null 2>&1 || true
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
        win_pid=$(_win_find_pid_on_port "$port")
        if [ -n "$win_pid" ]; then
            _win_stop_process "$win_pid"
            local wait_count=0
            while [[ $wait_count -lt 30 ]]; do
                win_pid=$(_win_find_pid_on_port "$port")
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
        local target_pid=""
        if command -v lsof &>/dev/null; then
            target_pid=$(lsof -ti :"$port" 2>/dev/null | head -n 1)
        elif command -v fuser &>/dev/null; then
            target_pid=$(fuser "$port"/tcp 2>/dev/null | awk '{print $1}')
        fi
        if [ -n "$target_pid" ]; then
            kill -9 "$target_pid" 2>/dev/null || true
        fi
    fi
}

win_wait_port_free() {
    local port=$1
    local max_wait=${2:-20}
    local waited=0
    while [[ $waited -lt $max_wait ]]; do
        local pid
        pid=$(_win_find_pid_on_port "$port")
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
        taskkill.exe //F //IM bifrost.exe >/dev/null 2>&1 || true
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
    if is_windows; then
        local timeout=30
        local elapsed=0
        while kill -0 "$pid" 2>/dev/null; do
            sleep 0.2
            elapsed=$((elapsed + 1))
            if [ "$elapsed" -ge "$((timeout * 5))" ]; then
                return 1
            fi
        done
        return 0
    else
        wait "$pid" 2>/dev/null || true
    fi
}

python_cmd() {
    if command -v python3 &>/dev/null; then
        echo "python3"
    else
        echo "python"
    fi
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
