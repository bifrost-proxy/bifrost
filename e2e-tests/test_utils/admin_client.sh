#!/bin/bash
# Bifrost Admin API 客户端工具

_ADMIN_CLIENT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$_ADMIN_CLIENT_DIR/process.sh"

ADMIN_HOST="${ADMIN_HOST-}"
ADMIN_PORT="${ADMIN_PORT-}"
ADMIN_PATH_PREFIX="${ADMIN_PATH_PREFIX-}"
ADMIN_BASE_URL_OVERRIDE="${ADMIN_BASE_URL-}"

# ---------------------------------------------------------------------------
# Test helper: ensure an admin-capable bifrost instance is running.
#
# Many Admin API test scripts only exercise endpoints and assume the server is
# already started. To make each script self-contained (and to avoid flaky
# timeouts due to `cargo run` debug builds), we provide a lightweight starter
# that:
# - prefers `target/release/bifrost` if present
# - otherwise falls back to `cargo run --release --bin bifrost`
# - writes logs to a temp file and prints tail on failure
#
# NOTE: Only cleans up the process/data-dir if it was started by this helper.
# ---------------------------------------------------------------------------

ADMIN_CLIENT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ADMIN_CLIENT_REPO_DIR="$(cd "$ADMIN_CLIENT_DIR/../.." && pwd)"

ADMIN_CLIENT_STARTED_BIFROST=0
ADMIN_CLIENT_BIFROST_PID=""
ADMIN_CLIENT_BIFROST_DATA_DIR=""
ADMIN_CLIENT_BIFROST_LOG_FILE=""
ADMIN_CLIENT_HOME_DIR=""
ADMIN_CLIENT_XDG_CONFIG_HOME=""
ADMIN_CLIENT_XDG_DATA_HOME=""

admin_log_info() { echo "[INFO] $*"; }
admin_log_fail() { echo "[FAIL] $*"; }

admin_host() {
    echo "${ADMIN_HOST:-127.0.0.1}"
}

admin_port() {
    echo "${ADMIN_PORT:-9900}"
}

admin_path_prefix() {
    echo "${ADMIN_PATH_PREFIX:-/_bifrost}"
}

admin_base_url() {
    if [[ -n "${ADMIN_BASE_URL_OVERRIDE:-}" ]]; then
        echo "${ADMIN_BASE_URL_OVERRIDE}"
    else
        echo "http://$(admin_host):$(admin_port)$(admin_path_prefix)"
    fi
}

admin_wait_for_admin_ready() {
    local timeout="${1:-60}"
    local waited=0
    local admin_url
    admin_url="$(admin_base_url)"

    while [[ $waited -lt $timeout ]]; do
        if curl -s "${admin_url}/api/system" >/dev/null 2>&1 || \
           curl -s "${admin_url}/api/system/status" >/dev/null 2>&1; then
            return 0
        fi

        if [[ -n "$ADMIN_CLIENT_BIFROST_PID" ]] && ! kill -0 "$ADMIN_CLIENT_BIFROST_PID" 2>/dev/null; then
            return 2
        fi

        sleep 1
        waited=$((waited + 1))
    done

    return 1
}

admin_wait_for_proxy_ready() {
    local timeout="${1:-30}"
    local waited=0
    local proxy_ready_url="${ADMIN_PROXY_READY_URL:-}"
    local proxy_url
    proxy_url="http://$(admin_host):$(admin_port)"

    if [[ -z "$proxy_ready_url" ]]; then
        return 0
    fi

    while [[ $waited -lt $timeout ]]; do
        if curl -fsS --max-time 5 --proxy "$proxy_url" "$proxy_ready_url" >/dev/null 2>&1; then
            return 0
        fi

        sleep 1
        waited=$((waited + 1))
    done

    admin_log_fail "Timeout waiting for proxy forwarding via ${proxy_url} to ${proxy_ready_url}"
    return 1
}

admin_start_bifrost() {
    local host
    local port
    host="$(admin_host)"
    port="$(admin_port)"

    admin_log_info "Starting Bifrost (admin) on ${host}:${port}..."

    ADMIN_CLIENT_STARTED_BIFROST=1

    if [[ -z "${BIFROST_DATA_DIR:-}" ]]; then
        ADMIN_CLIENT_BIFROST_DATA_DIR="$(mktemp -d)"
        export BIFROST_DATA_DIR="$ADMIN_CLIENT_BIFROST_DATA_DIR"
    else
        ADMIN_CLIENT_BIFROST_DATA_DIR="${BIFROST_DATA_DIR}"
    fi

    ADMIN_CLIENT_HOME_DIR="${ADMIN_CLIENT_BIFROST_DATA_DIR}/home"
    ADMIN_CLIENT_XDG_CONFIG_HOME="${ADMIN_CLIENT_BIFROST_DATA_DIR}/xdg-config"
    ADMIN_CLIENT_XDG_DATA_HOME="${ADMIN_CLIENT_BIFROST_DATA_DIR}/xdg-data"
    mkdir -p \
        "$ADMIN_CLIENT_HOME_DIR" \
        "$ADMIN_CLIENT_XDG_CONFIG_HOME" \
        "$ADMIN_CLIENT_XDG_DATA_HOME"

    ADMIN_CLIENT_BIFROST_LOG_FILE="$(mktemp)"

    local bifrost_bin=""
    local unix_bin="$ADMIN_CLIENT_REPO_DIR/target/release/bifrost"
    local windows_bin="$ADMIN_CLIENT_REPO_DIR/target/release/bifrost.exe"
    if [[ -x "$unix_bin" ]]; then
        bifrost_bin="$unix_bin"
    elif [[ -f "$windows_bin" ]]; then
        bifrost_bin="$windows_bin"
    fi
    if [[ -n "$bifrost_bin" ]]; then
        SKIP_FRONTEND_BUILD=1 \
            HOME="$ADMIN_CLIENT_HOME_DIR" \
            XDG_CONFIG_HOME="$ADMIN_CLIENT_XDG_CONFIG_HOME" \
            XDG_DATA_HOME="$ADMIN_CLIENT_XDG_DATA_HOME" \
            BIFROST_DATA_DIR="$BIFROST_DATA_DIR" \
            "$bifrost_bin" -H "$host" -p "$port" start --skip-cert-check --unsafe-ssl \
            >"$ADMIN_CLIENT_BIFROST_LOG_FILE" 2>&1 &
    else
        (cd "$ADMIN_CLIENT_REPO_DIR" && \
            SKIP_FRONTEND_BUILD=1 \
            HOME="$ADMIN_CLIENT_HOME_DIR" \
            XDG_CONFIG_HOME="$ADMIN_CLIENT_XDG_CONFIG_HOME" \
            XDG_DATA_HOME="$ADMIN_CLIENT_XDG_DATA_HOME" \
            BIFROST_DATA_DIR="$BIFROST_DATA_DIR" \
            cargo run --release --bin bifrost -- -H "$host" -p "$port" start --skip-cert-check --unsafe-ssl \
        ) >"$ADMIN_CLIENT_BIFROST_LOG_FILE" 2>&1 &
    fi

    ADMIN_CLIENT_BIFROST_PID=$!

    local rc
    admin_wait_for_admin_ready 90
    rc=$?
    if [[ $rc -eq 0 ]]; then
        if ! admin_wait_for_proxy_ready 30; then
            rc=1
        else
            admin_log_info "Bifrost started (PID: $ADMIN_CLIENT_BIFROST_PID)"
            return 0
        fi
    fi

    if [[ $rc -eq 2 ]]; then
        admin_log_fail "Bifrost process exited early (PID: $ADMIN_CLIENT_BIFROST_PID)"
    else
        admin_log_fail "Timeout waiting for admin server at ${admin_url}"
    fi

    if [[ -n "$ADMIN_CLIENT_BIFROST_LOG_FILE" ]]; then
        echo "Last log (tail -200):" >&2
        tail -200 "$ADMIN_CLIENT_BIFROST_LOG_FILE" 2>/dev/null >&2 || true
    fi
    return 1
}

admin_stop_bifrost() {
    if [[ -n "$ADMIN_CLIENT_BIFROST_PID" ]] && kill -0 "$ADMIN_CLIENT_BIFROST_PID" 2>/dev/null; then
        safe_cleanup_proxy "$ADMIN_CLIENT_BIFROST_PID"
    fi
    if is_windows; then
        local _port
        _port="$(admin_port)"
        if [[ -n "$_port" ]]; then
            kill_bifrost_on_port "$_port"
        fi
    fi

    if [[ -n "$ADMIN_CLIENT_BIFROST_LOG_FILE" && -f "$ADMIN_CLIENT_BIFROST_LOG_FILE" ]]; then
        rm -f "$ADMIN_CLIENT_BIFROST_LOG_FILE" 2>/dev/null || true
    fi

    if [[ -n "$ADMIN_CLIENT_BIFROST_DATA_DIR" && -d "$ADMIN_CLIENT_BIFROST_DATA_DIR" ]]; then
        rm -rf "$ADMIN_CLIENT_BIFROST_DATA_DIR" 2>/dev/null || true
    fi

    ADMIN_CLIENT_BIFROST_PID=""
    ADMIN_CLIENT_BIFROST_LOG_FILE=""
    ADMIN_CLIENT_BIFROST_DATA_DIR=""
    ADMIN_CLIENT_HOME_DIR=""
    ADMIN_CLIENT_XDG_CONFIG_HOME=""
    ADMIN_CLIENT_XDG_DATA_HOME=""
    ADMIN_CLIENT_STARTED_BIFROST=0
}

admin_ensure_bifrost() {
    local admin_url
    admin_url="$(admin_base_url)"

    # If admin is already reachable, do nothing.
    if curl -s "${admin_url}/api/system/status" >/dev/null 2>&1 || \
       curl -s "${admin_url}/api/system" >/dev/null 2>&1; then
        return 0
    fi
    admin_start_bifrost
}

admin_cleanup_bifrost() {
    if [[ "$ADMIN_CLIENT_STARTED_BIFROST" == "1" ]]; then
        admin_stop_bifrost
    fi
}

admin_get() {
    local path="$1"
    curl -s "$(admin_base_url)${path}"
}

admin_post() {
    local path="$1"
    local data="$2"
    curl -s -X POST -H "Content-Type: application/json" -d "$data" "$(admin_base_url)${path}"
}

admin_delete() {
    local path="$1"
    curl -s -X DELETE "$(admin_base_url)${path}"
}

get_traffic_list() {
    local arg1="${1:-}"
    local arg2="${2:-}"
    local arg3="${3:-100}"

    if [[ -n "$arg2" ]]; then
        local host="$arg1"
        local port="$arg2"
        local limit="$arg3"
        curl -s "http://${host}:${port}$(admin_path_prefix)/api/traffic?limit=${limit}"
    else
        local limit="${arg1:-100}"
        admin_get "/api/traffic?limit=${limit}"
    fi
}

get_traffic_detail() {
    local id="$1"
    admin_get "/api/traffic/${id}"
}

get_response_body() {
    local id="$1"
    admin_get "/api/traffic/${id}/response-body"
}

get_traffic_by_url() {
    local url_pattern="$1"
    local limit="${2:-10}"

    get_traffic_list "$limit" | jq -r ".records[] | select((.url // .p // \"\") | contains(\"$url_pattern\"))"
}

find_traffic_id_by_url() {
    local host="${1:-}"
    local port="${2:-}"
    local url_pattern="${3:-}"
    local limit="${4:-50}"

    if [[ -z "$url_pattern" ]]; then
        url_pattern="$host"
        limit="${port:-50}"
        get_traffic_list "$limit" | jq -r ".records[] | select((.url // .p // \"\") | contains(\"$url_pattern\")) | .id" | head -1
    else
        curl -s "http://${host}:${port}$(admin_path_prefix)/api/traffic?limit=${limit}" | jq -r ".records[] | select((.url // .p // \"\") | contains(\"$url_pattern\")) | .id" | head -1
    fi
}

get_frames() {
    local arg1="$1"
    local arg2="${2:-}"
    local arg3="${3:-}"
    local arg4="${4:-0}"
    local arg5="${5:-100}"

    if [[ -n "${arg3:-}" ]]; then
        local host="$arg1"
        local port="$arg2"
        local traffic_id="$arg3"
        local after="$arg4"
        local limit="$arg5"
        curl -s "http://${host}:${port}$(admin_path_prefix)/api/traffic/${traffic_id}/frames?after=${after}&limit=${limit}"
    else
        local traffic_id="$arg1"
        local after="${arg2:-0}"
        local limit="${arg3:-100}"
        admin_get "/api/traffic/${traffic_id}/frames?after=${after}&limit=${limit}"
    fi
}

get_frame_detail() {
    local traffic_id="$1"
    local frame_id="$2"

    admin_get "/api/traffic/${traffic_id}/frames/${frame_id}"
}

get_frame_count() {
    local traffic_id="$1"

    get_frames "$traffic_id" | jq -r '.frames | length'
}

wait_for_frames() {
    local traffic_id="$1"
    local expected_count="$2"
    local timeout="${3:-10}"

    local waited=0
    while [[ $waited -lt $((timeout * 10)) ]]; do
        local count=$(get_frame_count "$traffic_id")
        if [[ "$count" -ge "$expected_count" ]]; then
            return 0
        fi
        sleep 0.1
        waited=$((waited + 1))
    done

    echo "Timeout waiting for $expected_count frames (got $(get_frame_count "$traffic_id"))" >&2
    return 1
}

subscribe_frames() {
    local traffic_id="$1"
    local timeout="${2:-10}"

    timeout "$timeout" curl -sN "$(admin_base_url)/api/traffic/${traffic_id}/frames/stream" 2>/dev/null
}

subscribe_frames_bg() {
    local traffic_id="$1"
    local output_file="$2"

    curl -sN "$(admin_base_url)/api/traffic/${traffic_id}/frames/stream" > "$output_file" 2>/dev/null &
    echo $!
}

list_websocket_connections() {
    admin_get "/api/websocket/connections"
}

start_monitoring() {
    local traffic_id="$1"
    admin_post "/api/traffic/${traffic_id}/monitor" '{"action": "start"}'
}

stop_monitoring() {
    local traffic_id="$1"
    admin_post "/api/traffic/${traffic_id}/monitor" '{"action": "stop"}'
}

get_metrics() {
    admin_get "/api/metrics"
}

check_health() {
    admin_get "/api/system/status"
}

wait_for_admin() {
    local timeout="${1:-30}"
    local waited=0
    local admin_url
    admin_url="$(admin_base_url)"

    while [[ $waited -lt $timeout ]]; do
        if curl -s "${admin_url}/api/system/status" >/dev/null 2>&1; then
            return 0
        fi
        sleep 1
        waited=$((waited + 1))
    done

    echo "Timeout waiting for admin server at ${admin_url}" >&2
    return 1
}

is_websocket_traffic() {
    local traffic_id="$1"
    get_traffic_detail "$traffic_id" | jq -r '.is_websocket'
}

is_sse_traffic() {
    local traffic_id="$1"
    get_traffic_detail "$traffic_id" | jq -r '.is_sse'
}

get_frame_types() {
    local traffic_id="$1"
    get_frames "$traffic_id" | jq -r '.frames[].frame_type' | sort | uniq -c
}

get_frame_directions() {
    local traffic_id="$1"
    get_frames "$traffic_id" | jq -r '.frames[].direction' | sort | uniq -c
}

clear_traffic() {
    admin_delete "/api/traffic"
}

admin_put() {
    local path="$1"
    local data="$2"
    curl -s -X PUT -H "Content-Type: application/json" -d "$data" "$(admin_base_url)${path}"
}

admin_delete_with_body() {
    local path="$1"
    local data="$2"
    curl -s -X DELETE -H "Content-Type: application/json" -d "$data" "$(admin_base_url)${path}"
}

list_rules() {
    admin_get "/api/rules"
}

get_rule() {
    local name="$1"
    admin_get "/api/rules/${name}"
}

create_rule() {
    local name="$1"
    local content="$2"
    local enabled="${3:-true}"
    local payload
    payload=$(jq -cn --arg name "$name" --arg content "$content" --argjson enabled "$enabled" \
        '{name:$name, content:$content, enabled:$enabled}')
    admin_post "/api/rules" "$payload"
}

update_rule() {
    local name="$1"
    local content="$2"
    local enabled="${3:-true}"
    local payload
    payload=$(jq -cn --arg content "$content" --argjson enabled "$enabled" \
        '{content:$content, enabled:$enabled}')
    admin_put "/api/rules/${name}" "$payload"
}

delete_rule() {
    local name="$1"
    admin_delete "/api/rules/${name}"
}

enable_rule() {
    local name="$1"
    admin_put "/api/rules/${name}/enable" "{}"
}

disable_rule() {
    local name="$1"
    admin_put "/api/rules/${name}/disable" "{}"
}

list_values() {
    admin_get "/api/values"
}

get_value() {
    local name="$1"
    admin_get "/api/values/${name}"
}

create_value() {
    local name="$1"
    local value="$2"
    admin_post "/api/values" "{\"name\":\"${name}\",\"value\":\"${value}\"}"
}

update_value() {
    local name="$1"
    local value="$2"
    admin_put "/api/values/${name}" "{\"value\":\"${value}\"}"
}

delete_value() {
    local name="$1"
    admin_delete "/api/values/${name}"
}

get_whitelist() {
    admin_get "/api/whitelist"
}

add_whitelist() {
    local ip_or_cidr="$1"
    admin_post "/api/whitelist" "{\"ip_or_cidr\":\"${ip_or_cidr}\"}"
}

remove_whitelist() {
    local ip_or_cidr="$1"
    admin_delete_with_body "/api/whitelist" "{\"ip_or_cidr\":\"${ip_or_cidr}\"}"
}

get_whitelist_mode() {
    admin_get "/api/whitelist/mode"
}

set_whitelist_mode() {
    local mode="$1"
    admin_put "/api/whitelist/mode" "{\"mode\":\"${mode}\"}"
}

get_allow_lan() {
    admin_get "/api/whitelist/allow-lan"
}

set_allow_lan() {
    local allow="$1"
    admin_put "/api/whitelist/allow-lan" "{\"allow_lan\":${allow}}"
}

add_temporary_whitelist() {
    local ip="$1"
    admin_post "/api/whitelist/temporary" "{\"ip\":\"${ip}\"}"
}

remove_temporary_whitelist() {
    local ip="$1"
    admin_delete_with_body "/api/whitelist/temporary" "{\"ip\":\"${ip}\"}"
}

get_pending_authorizations() {
    admin_get "/api/whitelist/pending"
}

approve_authorization() {
    local ip="$1"
    admin_post "/api/whitelist/pending/approve" "{\"ip\":\"${ip}\"}"
}

reject_authorization() {
    local ip="$1"
    admin_post "/api/whitelist/pending/reject" "{\"ip\":\"${ip}\"}"
}

clear_pending_authorizations() {
    admin_delete "/api/whitelist/pending"
}

get_cert_info() {
    admin_get "/api/cert/info"
}

download_cert() {
    local base_url
    base_url="$(admin_base_url)"
    curl -s "${base_url%$(admin_path_prefix)}$(admin_path_prefix)/public/cert"
}

download_cert_absolute_form() {
    local base_url
    base_url="$(admin_base_url)"
    base_url="${base_url%$(admin_path_prefix)}"
    curl -s --proxy "${base_url}" "${base_url}$(admin_path_prefix)/public/cert"
}

get_cert_qrcode() {
    local base_url
    base_url="$(admin_base_url)"
    curl -s "${base_url%$(admin_path_prefix)}$(admin_path_prefix)/public/cert/qrcode"
}

get_system_proxy() {
    admin_get "/api/proxy/system"
}

set_system_proxy() {
    local enabled="$1"
    local bypass="${2:-}"
    if [[ -n "$bypass" ]]; then
        admin_put "/api/proxy/system" "{\"enabled\":${enabled},\"bypass\":\"${bypass}\"}"
    else
        admin_put "/api/proxy/system" "{\"enabled\":${enabled}}"
    fi
}

get_system_proxy_support() {
    admin_get "/api/proxy/system/support"
}

get_system_info() {
    admin_get "/api/system"
}

get_system_overview() {
    admin_get "/api/system/overview"
}

get_metrics_history() {
    local limit="${1:-100}"
    admin_get "/api/metrics/history?limit=${limit}"
}

get_tls_config() {
    admin_get "/api/config/tls"
}

get_server_config() {
    admin_get "/api/config/server"
}

update_server_config() {
    local data="$1"
    admin_put "/api/config/server" "$data"
}

update_tls_config() {
    local data="$1"
    admin_put "/api/config/tls" "$data"
}

set_unsafe_ssl() {
    local unsafe_ssl="$1"
    admin_put "/api/config/tls" "{\"unsafe_ssl\":${unsafe_ssl}}"
}

get_unsafe_ssl() {
    admin_get "/api/config/tls" | jq -r '.unsafe_ssl'
}

list_scripts() {
    admin_get "/api/scripts"
}

get_script() {
    local script_type="$1"
    local name="$2"
    admin_get "/api/scripts/${script_type}/${name}"
}

create_script() {
    local script_type="$1"
    local name="$2"
    local content="$3"
    local description="${4:-}"
    local payload
    if [[ -n "$description" ]]; then
        payload=$(jq -cn --arg content "$content" --arg description "$description" \
            '{content:$content, description:$description}')
    else
        payload=$(jq -cn --arg content "$content" '{content:$content}')
    fi
    admin_put "/api/scripts/${script_type}/${name}" "$payload"
}

delete_script() {
    local script_type="$1"
    local name="$2"
    admin_delete "/api/scripts/${script_type}/${name}"
}
