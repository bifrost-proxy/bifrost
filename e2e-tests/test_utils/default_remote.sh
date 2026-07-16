#!/usr/bin/env bash

BIFROST_DEFAULT_REMOTE_HOST_BASE64="Ymlmcm9zdC5ieXRlZGFuY2UubmV0"

bifrost_decode_base64_string() {
    local encoded="$1"
    local decoded

    if decoded="$(printf '%s' "$encoded" | base64 --decode 2>/dev/null)"; then
        printf '%s' "$decoded"
        return 0
    fi
    if decoded="$(printf '%s' "$encoded" | base64 -D 2>/dev/null)"; then
        printf '%s' "$decoded"
        return 0
    fi

    echo "unable to decode embedded Base64 value" >&2
    return 1
}

bifrost_default_remote_base_url() {
    local host
    host="$(bifrost_decode_base64_string "$BIFROST_DEFAULT_REMOTE_HOST_BASE64")" || return 1
    printf 'https://%s' "$host"
}
