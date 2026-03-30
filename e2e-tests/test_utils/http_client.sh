#!/bin/bash
# HTTP 客户端封装 - 简化测试请求发送
# 支持 HTTP 和 SOCKS5 两种代理模式

_HTTP_CLIENT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

PROXY_HOST=${PROXY_HOST-}
PROXY_PORT=${PROXY_PORT-}
SOCKS5_PORT=${SOCKS5_PORT-}
PROXY_MODE=${PROXY_MODE-}
TIMEOUT=${TIMEOUT-}
BIFROST_E2E_HTTP_RETRIES=${BIFROST_E2E_HTTP_RETRIES-}

_temp_headers_file=""
_temp_body_file=""

_cleanup_temp() {
    [ -f "$_temp_headers_file" ] && rm -f "$_temp_headers_file"
    [ -f "$_temp_body_file" ] && rm -f "$_temp_body_file"
}

http_proxy_host() {
    echo "${PROXY_HOST:-127.0.0.1}"
}

http_proxy_port() {
    echo "${PROXY_PORT:-8080}"
}

http_socks5_port() {
    echo "${SOCKS5_PORT:-}"
}

http_proxy_mode() {
    echo "${PROXY_MODE:-http}"
}

http_timeout() {
    local base="${TIMEOUT:-10}"
    if command -v is_windows &>/dev/null && is_windows && [[ "$base" -lt 15 ]]; then
        echo "15"
    else
        echo "$base"
    fi
}

http_retry_count() {
    local retries="${BIFROST_E2E_HTTP_RETRIES:-0}"
    if [[ "$retries" =~ ^[0-9]+$ ]]; then
        echo "$retries"
    else
        echo "0"
    fi
}

should_retry_request() {
    local curl_exit=$1
    local http_status=$2
    local attempt=$3
    local max_retries=$4

    if [ "$attempt" -ge "$max_retries" ]; then
        return 1
    fi

    if [ "$curl_exit" -ne 0 ]; then
        return 0
    fi

    case "$http_status" in
        000|408|429|500|502|503|504)
            return 0
            ;;
        *)
            return 1
            ;;
    esac
}

perform_curl_with_retries() {
    local attempt=0
    local max_retries
    local curl_exit

    max_retries=$(http_retry_count)

    while true; do
        : > "$_temp_headers_file"
        : > "$_temp_body_file"

        curl_exit=0
        HTTP_STATUS=$(command curl "$@" 2>/dev/null) || curl_exit=$?
        if [ -z "$HTTP_STATUS" ] && [ "$curl_exit" -ne 0 ]; then
            HTTP_STATUS="000"
        fi

        HTTP_HEADERS=$(cat "$_temp_headers_file" | tr -d '\r')
        HTTP_BODY=$(cat "$_temp_body_file")

        if ! should_retry_request "$curl_exit" "$HTTP_STATUS" "$attempt" "$max_retries"; then
            if [ "$curl_exit" -ne 0 ]; then
                return "$curl_exit"
            fi
            return 0
        fi

        attempt=$((attempt + 1))
        echo "[http_client] transient failure status=${HTTP_STATUS:-empty} curl_exit=${curl_exit} retry=${attempt}/${max_retries}" >&2
        sleep 1
    done
}

http_request() {
    local url=$1
    local method=${2:-GET}
    local data=${3:-}
    local extra_headers=${4:-}

    _temp_headers_file=$(mktemp)
    _temp_body_file=$(mktemp)

    local curl_args=(
        -s
        -X "$method"
        --max-time "$(http_timeout)"
        -D "$_temp_headers_file"
        -o "$_temp_body_file"
        -w '%{http_code}'
    )

    if [ -n "$TEST_ID" ]; then
        curl_args+=(-H "X-Test-ID: $TEST_ID")
    fi

    local proxy_host proxy_port proxy_mode socks5_port
    proxy_host="$(http_proxy_host)"
    proxy_port="$(http_proxy_port)"
    proxy_mode="$(http_proxy_mode)"
    socks5_port="$(http_socks5_port)"

    if [ -n "$proxy_host" ] && [ -n "$proxy_port" ]; then
        if [ "$proxy_mode" = "socks5" ] && [ -n "$socks5_port" ]; then
            curl_args+=(--socks5-hostname "${proxy_host}:${socks5_port}")
        else
            curl_args+=(--proxy "http://${proxy_host}:${proxy_port}")
        fi
    fi

    local _temp_data_file=""
    if [ -n "$data" ]; then
        if [ "${#data}" -gt 8000 ]; then
            _temp_data_file=$(mktemp)
            printf '%s' "$data" > "$_temp_data_file"
            curl_args+=(--data-binary "@$_temp_data_file")
        else
            curl_args+=(-d "$data")
        fi
    fi

    if [ -n "$extra_headers" ]; then
        while IFS= read -r header; do
            [ -n "$header" ] && curl_args+=(-H "$header")
        done <<< "$extra_headers"
    fi

    curl_args+=("$url")

    perform_curl_with_retries "${curl_args[@]}"

    [ -n "$_temp_data_file" ] && rm -f "$_temp_data_file"
    _cleanup_temp
}

http_get() {
    local url=$1
    local extra_headers=${2:-}
    http_request "$url" "GET" "" "$extra_headers"
}

http_post() {
    local url=$1
    local data=${2:-}
    local extra_headers=${3:-}
    http_request "$url" "POST" "$data" "$extra_headers"
}

http_post_file() {
    local url=$1
    local file_path=$2
    local extra_headers=${3:-}

    _temp_headers_file=$(mktemp)
    _temp_body_file=$(mktemp)

    local curl_args=(
        -s
        -X "POST"
        --max-time "$(http_timeout)"
        -D "$_temp_headers_file"
        -o "$_temp_body_file"
        -w '%{http_code}'
        --data-binary "@$file_path"
    )

    if [ -n "$TEST_ID" ]; then
        curl_args+=(-H "X-Test-ID: $TEST_ID")
    fi

    local proxy_host proxy_port proxy_mode socks5_port
    proxy_host="$(http_proxy_host)"
    proxy_port="$(http_proxy_port)"
    proxy_mode="$(http_proxy_mode)"
    socks5_port="$(http_socks5_port)"

    if [ -n "$proxy_host" ] && [ -n "$proxy_port" ]; then
        if [ "$proxy_mode" = "socks5" ] && [ -n "$socks5_port" ]; then
            curl_args+=(--socks5-hostname "${proxy_host}:${socks5_port}")
        else
            curl_args+=(--proxy "http://${proxy_host}:${proxy_port}")
        fi
    fi

    if [ -n "$extra_headers" ]; then
        while IFS= read -r header; do
            [ -n "$header" ] && curl_args+=(-H "$header")
        done <<< "$extra_headers"
    fi

    curl_args+=("$url")

    perform_curl_with_retries "${curl_args[@]}"

    _cleanup_temp
}

http_put() {
    local url=$1
    local data=${2:-}
    local extra_headers=${3:-}
    http_request "$url" "PUT" "$data" "$extra_headers"
}

http_delete() {
    local url=$1
    local extra_headers=${2:-}
    http_request "$url" "DELETE" "" "$extra_headers"
}

http_request_no_proxy() {
    local url=$1
    local method=${2:-GET}
    local data=${3:-}
    local extra_headers=${4:-}

    _temp_headers_file=$(mktemp)
    _temp_body_file=$(mktemp)

    local curl_args=(
        -s
        -X "$method"
        --max-time "$(http_timeout)"
        -D "$_temp_headers_file"
        -o "$_temp_body_file"
        -w '%{http_code}'
        --noproxy '*'
    )

    local proxy_host proxy_port proxy_mode socks5_port
    proxy_host="$(http_proxy_host)"
    proxy_port="$(http_proxy_port)"
    proxy_mode="$(http_proxy_mode)"
    socks5_port="$(http_socks5_port)"

    if [ -n "$proxy_host" ] && [ -n "$proxy_port" ]; then
        if [ "$proxy_mode" = "socks5" ] && [ -n "$socks5_port" ]; then
            curl_args+=(--socks5-hostname "${proxy_host}:${socks5_port}")
        else
            curl_args+=(--proxy "http://${proxy_host}:${proxy_port}")
        fi
    fi

    local _temp_data_file=""
    if [ -n "$data" ]; then
        if [ "${#data}" -gt 8000 ]; then
            _temp_data_file=$(mktemp)
            printf '%s' "$data" > "$_temp_data_file"
            curl_args+=(--data-binary "@$_temp_data_file")
        else
            curl_args+=(-d "$data")
        fi
    fi

    if [ -n "$extra_headers" ]; then
        while IFS= read -r header; do
            [ -n "$header" ] && curl_args+=(-H "$header")
        done <<< "$extra_headers"
    fi

    curl_args+=("$url")

    perform_curl_with_retries "${curl_args[@]}"

    [ -n "$_temp_data_file" ] && rm -f "$_temp_data_file"
    _cleanup_temp
}

https_request() {
    local url=$1
    local method=${2:-GET}
    local data=${3:-}
    local extra_headers=${4:-}

    _temp_headers_file=$(mktemp)
    _temp_body_file=$(mktemp)

    local curl_args=(
        -s
        -k  # 允许自签名证书
        -X "$method"
        --max-time "$(http_timeout)"
        -D "$_temp_headers_file"
        -o "$_temp_body_file"
        -w '%{http_code}'
    )

    if [ -n "$TEST_ID" ]; then
        curl_args+=(-H "X-Test-ID: $TEST_ID")
    fi

    local proxy_host proxy_port proxy_mode socks5_port
    proxy_host="$(http_proxy_host)"
    proxy_port="$(http_proxy_port)"
    proxy_mode="$(http_proxy_mode)"
    socks5_port="$(http_socks5_port)"

    if [ -n "$proxy_host" ] && [ -n "$proxy_port" ]; then
        if [ "$proxy_mode" = "socks5" ] && [ -n "$socks5_port" ]; then
            curl_args+=(--socks5-hostname "${proxy_host}:${socks5_port}")
        else
            curl_args+=(--proxy "http://${proxy_host}:${proxy_port}")
        fi
    fi

    local _temp_data_file=""
    if [ -n "$data" ]; then
        if [ "${#data}" -gt 8000 ]; then
            _temp_data_file=$(mktemp)
            printf '%s' "$data" > "$_temp_data_file"
            curl_args+=(--data-binary "@$_temp_data_file")
        else
            curl_args+=(-d "$data")
        fi
    fi

    if [ -n "$extra_headers" ]; then
        while IFS= read -r header; do
            [ -n "$header" ] && curl_args+=(-H "$header")
        done <<< "$extra_headers"
    fi

    curl_args+=("$url")

    perform_curl_with_retries "${curl_args[@]}"

    [ -n "$_temp_data_file" ] && rm -f "$_temp_data_file"
    _cleanup_temp
}

print_request_result() {
    echo "========================================="
    echo "HTTP Status: $HTTP_STATUS"
    echo "========================================="
    echo "Response Headers:"
    echo "$HTTP_HEADERS"
    echo "========================================="
    echo "Response Body:"
    echo "$HTTP_BODY"
    echo "========================================="
}

get_header() {
    local header_name=$1
    echo "$HTTP_HEADERS" | grep -i "^${header_name}:" | head -1 | sed "s/^${header_name}:[[:space:]]*//" | tr -d '\r'
}

get_json_field() {
    local jq_path=$1
    echo "$HTTP_BODY" | jq -r "$jq_path" 2>/dev/null
}

generate_large_body() {
    local size=$1
    local marker=$2
    
    local marker_len=${#marker}
    if [ "$size" -lt $((marker_len * 2)) ]; then
        size=$((marker_len * 2))
    fi
    
    local padding_size=$((size - marker_len * 2))
    
    echo -n "$marker"
    head -c "$padding_size" /dev/zero | tr '\0' 'X'
    echo -n "$marker"
}

http_post_large_body() {
    local url=$1
    local size=$2
    local marker=$3
    local extra_headers=${4:-}
    
    _temp_headers_file=$(mktemp)
    _temp_body_file=$(mktemp)
    local _temp_req_body=$(mktemp)
    
    generate_large_body "$size" "$marker" > "$_temp_req_body"
    local actual_size=$(stat -f%z "$_temp_req_body" 2>/dev/null || stat -c%s "$_temp_req_body" 2>/dev/null)
    
    echo "[http_client] Generated request body: size=$actual_size, marker=$marker" >&2
    
    local curl_args=(
        -s
        -X "POST"
        --max-time "${TIMEOUT:-60}"
        -D "$_temp_headers_file"
        -o "$_temp_body_file"
        -w '%{http_code}'
        -H "Content-Type: text/plain"
        --data-binary "@$_temp_req_body"
    )
    
    if [ -n "$TEST_ID" ]; then
        curl_args+=(-H "X-Test-ID: $TEST_ID")
    fi
    
    if [ -n "$PROXY_HOST" ] && [ -n "$PROXY_PORT" ]; then
        if [ "$PROXY_MODE" = "socks5" ] && [ -n "$SOCKS5_PORT" ]; then
            curl_args+=(--socks5-hostname "${PROXY_HOST}:${SOCKS5_PORT}")
        else
            curl_args+=(--proxy "http://${PROXY_HOST}:${PROXY_PORT}")
        fi
    fi
    
    if [ -n "$extra_headers" ]; then
        while IFS= read -r header; do
            [ -n "$header" ] && curl_args+=(-H "$header")
        done <<< "$extra_headers"
    fi
    
    curl_args+=("$url")
    
    HTTP_STATUS=$(curl "${curl_args[@]}")
    HTTP_HEADERS=$(cat "$_temp_headers_file")
    HTTP_BODY=$(cat "$_temp_body_file")
    
    rm -f "$_temp_req_body"
    _cleanup_temp
    
    echo "[http_client] Response: status=$HTTP_STATUS, body_size=${#HTTP_BODY}" >&2
}

https_post_large_body() {
    local url=$1
    local size=$2
    local marker=$3
    local extra_headers=${4:-}
    
    _temp_headers_file=$(mktemp)
    _temp_body_file=$(mktemp)
    local _temp_req_body=$(mktemp)
    
    generate_large_body "$size" "$marker" > "$_temp_req_body"
    local actual_size=$(stat -f%z "$_temp_req_body" 2>/dev/null || stat -c%s "$_temp_req_body" 2>/dev/null)
    
    echo "[http_client] Generated HTTPS request body: size=$actual_size, marker=$marker" >&2
    
    local curl_args=(
        -s
        -k
        -X "POST"
        --max-time "${TIMEOUT:-60}"
        -D "$_temp_headers_file"
        -o "$_temp_body_file"
        -w '%{http_code}'
        -H "Content-Type: text/plain"
        --data-binary "@$_temp_req_body"
    )
    
    if [ -n "$TEST_ID" ]; then
        curl_args+=(-H "X-Test-ID: $TEST_ID")
    fi
    
    if [ -n "$PROXY_HOST" ] && [ -n "$PROXY_PORT" ]; then
        if [ "$PROXY_MODE" = "socks5" ] && [ -n "$SOCKS5_PORT" ]; then
            curl_args+=(--socks5-hostname "${PROXY_HOST}:${SOCKS5_PORT}")
        else
            curl_args+=(--proxy "http://${PROXY_HOST}:${PROXY_PORT}")
        fi
    fi
    
    if [ -n "$extra_headers" ]; then
        while IFS= read -r header; do
            [ -n "$header" ] && curl_args+=(-H "$header")
        done <<< "$extra_headers"
    fi
    
    curl_args+=("$url")
    
    HTTP_STATUS=$(curl "${curl_args[@]}")
    HTTP_HEADERS=$(cat "$_temp_headers_file")
    HTTP_BODY=$(cat "$_temp_body_file")
    
    rm -f "$_temp_req_body"
    _cleanup_temp
    
    echo "[http_client] HTTPS Response: status=$HTTP_STATUS, body_size=${#HTTP_BODY}" >&2
}
