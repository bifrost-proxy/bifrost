#!/bin/bash
# Source after process.sh. All injection is restricted to one test-owned CLI child.
verify_client_response_read() (
    set -euo pipefail
    bin="$1" repo_root="$2" runtime="$3"
    server_pid="" extension="so" library="$runtime/reset-fault.so"
    trap '[[ -z "$server_pid" ]] || { terminate_process_tree "$server_pid" || true; wait "$server_pid" 2>/dev/null || true; }' EXIT
    if [[ "$(uname -s)" == "Darwin" ]]; then
        extension="dylib"
        library="$runtime/reset-fault.$extension"
        cc -dynamiclib -fPIC "$repo_root/e2e-tests/test_data/admin_response_reset_fault.c" -o "$library"
    else
        cc -shared -fPIC "$repo_root/e2e-tests/test_data/admin_response_reset_fault.c" -ldl -pthread -o "$library"
    fi
    python3 "$repo_root/e2e-tests/mock_servers/admin_fragmented_response.py" \
        --port-file "$runtime/response-port" --request-file "$runtime/response-request" &
    server_pid=$!
    attempt=""
    for attempt in $(seq 1 100); do
        [[ ! -s "$runtime/response-port" ]] || break
        kill -0 "$server_pid" 2>/dev/null || { echo "fragmented response fixture exited early" >&2; exit 1; }
        sleep 0.05
    done
    port="" output=""
    port="$(cat "$runtime/response-port")"
    if [[ "$extension" == "dylib" ]]; then
        output="$(BIFROST_E2E_SOCKET_RESET_FAULT=1 DYLD_INSERT_LIBRARIES="$library" \
            BIFROST_E2E_SOCKET_RESET_LIBRARY_LOADED="$runtime/fault-library-loaded" \
            BIFROST_DATA_DIR="$runtime/response-caller" \
            "$bin" -H 127.0.0.1 -p "$port" config show --section server --json)"
    else
        output="$(BIFROST_E2E_SOCKET_RESET_FAULT=1 LD_PRELOAD="$library" \
            BIFROST_E2E_SOCKET_RESET_LIBRARY_LOADED="$runtime/fault-library-loaded" \
            BIFROST_DATA_DIR="$runtime/response-caller" \
            "$bin" -H 127.0.0.1 -p "$port" config show --section server --json)"
    fi
    wait "$server_pid"
    server_pid=""
    [[ -s "$runtime/fault-library-loaded" ]]
    jq -e '.timeout_secs == 30 and .http1_max_header_size == 8192 and .http2_max_header_list_size == 16384 and .websocket_handshake_max_header_size == 4096' <<<"$output" >/dev/null
    grep -Fxq 'GET /_bifrost/api/config/server HTTP/1.1' "$runtime/response-request"
)
