#!/bin/bash
set -e

REPO="bifrost-proxy/bifrost"
BINARY_NAME="bifrost"
INSTALL_DIR="${BIFROST_INSTALL_DIR:-${HOME:-/tmp}/.local/bin}"

DEFAULT_GITHUB_MIRROR_URLS=(
    "https://github.com"
    "https://ghfast.top/https://github.com"
    "https://github.moeyy.xyz/https://github.com"
)

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

print_banner() {
    printf "%b" "${CYAN}"
    echo "╔═══════════════════════════════════════════════════════════════╗"
    echo "║                                                               ║"
    echo "║    ____  _  __               _                                ║"
    echo "║   | __ )(_)/ _|_ __ ___  ___| |_                              ║"
    echo "║   |  _ \| | |_| '__/ _ \/ __| __|                             ║"
    echo "║   | |_) | |  _| | | (_) \__ \ |_                              ║"
    echo "║   |____/|_|_| |_|  \___/|___/\__|                             ║"
    echo "║                                                               ║"
    echo "║    High-performance HTTP/HTTPS/SOCKS5 Proxy Server            ║"
    echo "║                                                               ║"
    echo "╚═══════════════════════════════════════════════════════════════╝"
    printf "%b\n" "${NC}"
}

print_step() {
    printf "%b==>%b %s\n" "${BLUE}" "${NC}" "$1"
}

print_success() {
    printf "%b✓%b %s\n" "${GREEN}" "${NC}" "$1"
}

print_warning() {
    printf "%b!%b %s\n" "${YELLOW}" "${NC}" "$1" >&2
}

print_error() {
    printf "%b✗%b %s\n" "${RED}" "${NC}" "$1" >&2
}

detect_os() {
    local os
    os="$(uname -s)"
    case "$os" in
        Linux*)  echo "linux" ;;
        Darwin*) echo "darwin" ;;
        MINGW*|MSYS*|CYGWIN*) echo "windows" ;;
        *)       echo "unknown" ;;
    esac
}

detect_arch() {
    local arch
    arch="$(uname -m)"
    case "$arch" in
        x86_64|amd64)     echo "x86_64" ;;
        aarch64|arm64)    echo "aarch64" ;;
        armv7l|armv7)     echo "armv7" ;;
        *)                echo "unknown" ;;
    esac
}

detect_libc() {
    if [ "$(detect_os)" != "linux" ]; then
        echo "gnu"
        return
    fi

    if command -v ldd &> /dev/null; then
        local ldd_output
        ldd_output=$(ldd --version 2>&1 || true)
        if echo "$ldd_output" | grep -qi "musl"; then
            echo "musl"
            return
        fi
        if echo "$ldd_output" | grep -qiE "GLIBC|GNU libc"; then
            echo "gnu"
            return
        fi
    fi

    if [ -f /lib/ld-musl-x86_64.so.1 ] || \
       [ -f /lib/ld-musl-aarch64.so.1 ] || \
       [ -f /lib/ld-musl-armhf.so.1 ]; then
        echo "musl"
        return
    fi

    echo "gnu"
}

MIN_GLIBC_VERSION="2.39"

get_glibc_version() {
    if [ "$(detect_os)" != "linux" ]; then
        return 1
    fi

    if ! command -v ldd >/dev/null 2>&1; then
        return 1
    fi

    local out version
    out=$(ldd --version 2>&1 || true)

    if ! echo "$out" | grep -qiE "GLIBC|GNU libc"; then
        return 1
    fi

    version=$(echo "$out" | head -1 | grep -oE '[0-9]+\.[0-9]+' | head -1)
    if [ -n "$version" ]; then
        echo "$version"
        return 0
    fi

    return 1
}

version_lt() {
    if [[ "$1" == "$2" ]]; then
        return 1
    fi
    local IFS=.
    local i a=($1) b=($2)
    for ((i = 0; i < ${#a[@]} || i < ${#b[@]}; i++)); do
        local ai="${a[i]:-0}" bi="${b[i]:-0}"
        if ((ai < bi)); then
            return 0
        elif ((ai > bi)); then
            return 1
        fi
    done
    return 1
}

verify_binary_runs() {
    local bin="$1"
    "$bin" --version >/dev/null 2>&1
}

get_linux_target() {
    local arch="$1"
    local libc="$2"
    local gnu_target musl_target

    case "$arch" in
        x86_64)
            gnu_target="x86_64-unknown-linux-gnu"
            musl_target="x86_64-unknown-linux-musl"
            ;;
        aarch64)
            gnu_target="aarch64-unknown-linux-gnu"
            musl_target="aarch64-unknown-linux-musl"
            ;;
        armv7)
            echo "armv7-unknown-linux-gnueabihf"
            return
            ;;
        *)
            return 1
            ;;
    esac

    if [ "$libc" = "musl" ]; then
        echo "$musl_target"
        return
    fi

    local glibc_ver
    glibc_ver=$(get_glibc_version || true)

    if [ -n "$glibc_ver" ]; then
        if version_lt "$glibc_ver" "$MIN_GLIBC_VERSION"; then
            print_warning "Detected glibc $glibc_ver (< $MIN_GLIBC_VERSION), falling back to musl build for compatibility"
            echo "$musl_target"
            return
        fi
        echo "$gnu_target"
    else
        print_warning "Could not detect glibc version, using musl build for maximum compatibility"
        echo "$musl_target"
    fi
}

get_target() {
    local os="$1"
    local arch="$2"

    case "$os" in
        linux)
            local libc
            libc=$(detect_libc)
            get_linux_target "$arch" "$libc"
            ;;
        darwin)
            case "$arch" in
                x86_64)  echo "x86_64-apple-darwin" ;;
                aarch64) echo "aarch64-apple-darwin" ;;
                *)       return 1 ;;
            esac
            ;;
        windows)
            case "$arch" in
                x86_64)  echo "x86_64-pc-windows-msvc" ;;
                aarch64) echo "aarch64-pc-windows-msvc" ;;
                *)       return 1 ;;
            esac
            ;;
        *)
            return 1
            ;;
    esac
}

tar_supports_xz() {
    if ! has_command tar; then
        return 1
    fi
    if [[ "${BIFROST_DISABLE_XZ_ARCHIVE:-0}" == "1" ]]; then
        return 1
    fi
    tar --help 2>/dev/null | grep -q -- '-J'
}

get_archive_ext_candidates() {
    local os="$1"
    case "$os" in
        windows)
            echo "zip"
            ;;
        *)
            if tar_supports_xz; then
                echo "tar.xz"
            fi
            echo "tar.gz"
            ;;
    esac
}

has_command() {
    command -v "$1" >/dev/null 2>&1
}

install_binary_atomically() {
    local source_path="$1"
    local dest_path="$2"
    local tmp_path="${dest_path}.tmp.$$"

    rm -f "$tmp_path"
    cp "$source_path" "$tmp_path"
    chmod +x "$tmp_path"
    clear_xattr "$tmp_path"
    mv -f "$tmp_path" "$dest_path"
}

build_mirror_url_list() {
    local preferred="${BIFROST_GITHUB_MIRROR:-}"
    local -a mirrors=()
    local mirror

    if [[ -n "$preferred" ]]; then
        mirrors+=("$preferred")
    fi

    for mirror in "${DEFAULT_GITHUB_MIRROR_URLS[@]}"; do
        if [[ "$mirror" != "$preferred" ]]; then
            mirrors+=("$mirror")
        fi
    done

    printf '%s\n' "${mirrors[@]}"
}

mirror_display_name() {
    echo "$1" | sed 's|https\{0,1\}://||;s|/.*||'
}

probe_github_url() {
    local url="$1"
    local connect_timeout="${BIFROST_DOWNLOAD_CONNECT_TIMEOUT:-5}"
    local probe_timeout="${BIFROST_MIRROR_PROBE_TIMEOUT:-5}"

    if has_command curl; then
        curl -fsSIL \
            --connect-timeout "$connect_timeout" \
            --max-time "$probe_timeout" \
            -o /dev/null \
            "$url" >/dev/null 2>&1 && return 0
    fi

    if has_command wget; then
        wget -q --spider \
            --max-redirect=5 \
            --connect-timeout="$connect_timeout" \
            --timeout="$probe_timeout" \
            "$url" >/dev/null 2>&1 && return 0
    fi

    return 1
}

select_fastest_github_base() {
    local github_path="$1"
    local -a mirrors=()
    local base_url

    while IFS= read -r base_url; do
        [[ -n "$base_url" ]] && mirrors+=("$base_url")
    done < <(build_mirror_url_list)

    if [[ ${#mirrors[@]} -eq 0 ]]; then
        return 1
    fi

    if [[ ${#mirrors[@]} -eq 1 ]]; then
        echo "${mirrors[0]}"
        return 0
    fi

    if ! has_command curl && ! has_command wget; then
        return 1
    fi

    local probe_dir
    probe_dir=$(mktemp -d)
    local -a pids=()
    local index=0

    for base_url in "${mirrors[@]}"; do
        (
            local full_url="${base_url%/}/${github_path}"
            if probe_github_url "$full_url"; then
                printf '%s\n' "$base_url" > "${probe_dir}/${index}.ok"
            fi
        ) &
        pids+=($!)
        index=$((index + 1))
    done

    local winner_index=""
    local probe_deadline=$(( $(date +%s) + ${BIFROST_MIRROR_PROBE_TIMEOUT:-5} + ${BIFROST_DOWNLOAD_CONNECT_TIMEOUT:-5} + 2 ))
    while true; do
        for ((i = 0; i < index; i++)); do
            if [[ -f "${probe_dir}/${i}.ok" ]]; then
                winner_index=$i
                break 2
            fi
        done

        local any_alive=false
        for pid in "${pids[@]}"; do
            if kill -0 "$pid" 2>/dev/null; then
                any_alive=true
                break
            fi
        done
        if ! $any_alive; then
            for ((i = 0; i < index; i++)); do
                if [[ -f "${probe_dir}/${i}.ok" ]]; then
                    winner_index=$i
                    break 2
                fi
            done
            break
        fi
        if [[ $(date +%s) -ge $probe_deadline ]]; then
            break
        fi

        sleep 0.2
    done

    for pid in "${pids[@]}"; do
        kill "$pid" 2>/dev/null || true
        wait "$pid" 2>/dev/null || true
    done

    if [[ -n "$winner_index" ]]; then
        cat "${probe_dir}/${winner_index}.ok"
        rm -rf "$probe_dir"
        return 0
    fi

    rm -rf "$probe_dir"
    return 1
}

build_downloader_list() {
    local preferred="${BIFROST_DOWNLOADER:-auto}"
    local -a candidates=()
    local candidate

    case "$preferred" in
        ""|auto)
            if [[ "$(detect_os)" == "linux" ]]; then
                candidates=(aria2c axel wget curl)
            else
                candidates=(aria2c axel curl wget)
            fi
            ;;
        aria2c)
            candidates=(aria2c axel wget curl)
            ;;
        axel)
            candidates=(axel aria2c wget curl)
            ;;
        wget)
            candidates=(wget aria2c axel curl)
            ;;
        curl)
            candidates=(curl aria2c axel wget)
            ;;
        *)
            print_warning "Unknown BIFROST_DOWNLOADER: $preferred (using auto)"
            if [[ "$(detect_os)" == "linux" ]]; then
                candidates=(aria2c axel wget curl)
            else
                candidates=(aria2c axel curl wget)
            fi
            ;;
    esac

    for candidate in "${candidates[@]}"; do
        if has_command "$candidate"; then
            echo "$candidate"
        fi
    done
}

download_with_tool() {
    local tool="$1"
    local url="$2"
    local output="$3"
    local output_dir output_name
    local show_progress="${BIFROST_DOWNLOAD_PROGRESS:-1}"

    output_dir="$(cd "$(dirname "$output")" && pwd)"
    output_name="$(basename "$output")"

    case "$tool" in
        aria2c)
            local aria2_summary_arg=()
            if [[ "$show_progress" == "0" ]]; then
                aria2_summary_arg=(--summary-interval=0 --console-log-level=warn)
            else
                aria2_summary_arg=(--summary-interval=1 --console-log-level=notice)
            fi
            aria2c --no-conf -x 16 -s 16 --max-connection-per-server=16 \
                   --min-split-size=1M --allow-overwrite=true \
                   "${aria2_summary_arg[@]}" \
                   --connect-timeout="${BIFROST_DOWNLOAD_CONNECT_TIMEOUT:-10}" \
                   --timeout="${BIFROST_DOWNLOAD_TIMEOUT:-120}" \
                   --max-tries="${BIFROST_DOWNLOAD_TRIES:-2}" \
                   -d "$output_dir" -o "$output_name" "$url"
            ;;
        axel)
            local axel_timeout="${BIFROST_DOWNLOAD_TIMEOUT:-120}"
            local axel_args=(-n 16 -o "$output")
            if [[ "$show_progress" == "0" ]]; then
                axel_args=(-q "${axel_args[@]}")
            fi
            if has_command timeout; then
                timeout "$axel_timeout" axel "${axel_args[@]}" "$url"
            else
                axel "${axel_args[@]}" "$url"
            fi
            ;;
        wget)
            local wget_progress_args=(-q)
            if [[ "$show_progress" != "0" ]]; then
                wget_progress_args=(--show-progress --progress=bar:force:noscroll)
            fi
            wget "${wget_progress_args[@]}" \
                --connect-timeout="${BIFROST_DOWNLOAD_CONNECT_TIMEOUT:-10}" \
                --timeout="${BIFROST_DOWNLOAD_TIMEOUT:-120}" \
                --tries="${BIFROST_DOWNLOAD_TRIES:-2}" \
                --waitretry=1 \
                "$url" -O "$output"
            ;;
        curl)
            local curl_retries="${BIFROST_DOWNLOAD_TRIES:-2}"
            curl_retries=$((curl_retries > 0 ? curl_retries - 1 : 0))
            local curl_progress_args=(-fsSL)
            if [[ "$show_progress" != "0" ]]; then
                curl_progress_args=(-fL --progress-bar)
            fi
            curl "${curl_progress_args[@]}" \
                --connect-timeout "${BIFROST_DOWNLOAD_CONNECT_TIMEOUT:-10}" \
                --max-time "${BIFROST_DOWNLOAD_TIMEOUT:-120}" \
                --retry "$curl_retries" \
                --retry-delay 1 \
                "$url" -o "$output"
            ;;
        *)
            return 1
            ;;
    esac
}

validate_downloaded_file() {
    local file="$1"

    if [[ ! -s "$file" ]]; then
        print_warning "Downloaded file is empty: $file"
        return 1
    fi

    case "$file" in
        *.tar.xz)
            if ! tar -tJf "$file" >/dev/null 2>&1; then
                print_warning "Downloaded tar.xz archive is invalid: $file"
                return 1
            fi
            ;;
        *.tar.gz)
            if ! tar -tzf "$file" >/dev/null 2>&1; then
                print_warning "Downloaded tar.gz archive is invalid: $file"
                return 1
            fi
            ;;
        *.zip)
            if has_command unzip; then
                if ! unzip -tq "$file" >/dev/null 2>&1; then
                    print_warning "Downloaded zip archive is invalid: $file"
                    return 1
                fi
            fi
            ;;
    esac

    return 0
}

github_api_request() {
    local url="$1"
    local result=""

    if command -v curl &> /dev/null; then
        if [[ -n "${GITHUB_TOKEN:-}" ]]; then
            result=$(curl -sL --connect-timeout 10 --max-time 30 -H "Authorization: token ${GITHUB_TOKEN}" "$url" 2>/dev/null) && [[ -n "$result" ]] && { echo "$result"; return 0; }
        else
            result=$(curl -sL --connect-timeout 10 --max-time 30 "$url" 2>/dev/null) && [[ -n "$result" ]] && { echo "$result"; return 0; }
        fi
    fi

    if command -v wget &> /dev/null; then
        if [[ -n "${GITHUB_TOKEN:-}" ]]; then
            result=$(wget -qO- --connect-timeout=10 --timeout=30 --header="Authorization: token ${GITHUB_TOKEN}" "$url" 2>/dev/null) && [[ -n "$result" ]] && { echo "$result"; return 0; }
        else
            result=$(wget -qO- --connect-timeout=10 --timeout=30 "$url" 2>/dev/null) && [[ -n "$result" ]] && { echo "$result"; return 0; }
        fi
    fi

    return 1
}

get_latest_version_via_redirect() {
    local base_url="${1:-https://github.com}"
    local redirect_url="${base_url}/${REPO}/releases/latest"
    local location=""

    if command -v curl &> /dev/null; then
        location=$(curl -sI -o /dev/null -w '%{url_effective}' -L --connect-timeout 10 --max-time 20 "$redirect_url" 2>/dev/null) || location=""
    fi

    if [[ -z "$location" ]] && command -v wget &> /dev/null; then
        location=$(wget --spider -S --max-redirect=5 --connect-timeout=10 --timeout=20 "$redirect_url" 2>&1 | grep -i 'Location:' | tail -1 | sed 's/.*Location:[[:space:]]*//' | sed 's/[[:space:]].*//' | tr -d '\r')
    fi

    if [[ -n "$location" ]]; then
        local version
        version=$(echo "$location" | sed -E 's|.*/tag/([^/?#]+).*|\1|')
        if [[ -n "$version" && "$version" != "$location" ]]; then
            echo "$version"
            return 0
        fi
    fi
    return 1
}

get_latest_version_via_redirect_race() {
    local -a mirrors=()
    local base_url

    while IFS= read -r base_url; do
        [[ -n "$base_url" ]] && mirrors+=("$base_url")
    done < <(build_mirror_url_list)

    if [[ ${#mirrors[@]} -eq 0 ]]; then
        return 1
    fi

    if [[ ${#mirrors[@]} -eq 1 ]]; then
        get_latest_version_via_redirect "${mirrors[0]}"
        return
    fi

    local race_dir
    race_dir=$(mktemp -d)
    local -a pids=()
    local index=0

    for base_url in "${mirrors[@]}"; do
        (
            local version
            version=$(get_latest_version_via_redirect "$base_url" 2>/dev/null) || exit 1
            printf '%s\n' "$version" > "${race_dir}/${index}.version"
            touch "${race_dir}/${index}.ok"
        ) &
        pids+=($!)
        index=$((index + 1))
    done

    local winner_index=""
    local version_deadline=$(( $(date +%s) + ${BIFROST_MIRROR_PROBE_TIMEOUT:-5} + ${BIFROST_DOWNLOAD_CONNECT_TIMEOUT:-5} + 2 ))
    while true; do
        for ((i = 0; i < index; i++)); do
            if [[ -f "${race_dir}/${i}.ok" ]]; then
                winner_index=$i
                break 2
            fi
        done

        local any_alive=false
        for pid in "${pids[@]}"; do
            if kill -0 "$pid" 2>/dev/null; then
                any_alive=true
                break
            fi
        done
        if ! $any_alive; then
            for ((i = 0; i < index; i++)); do
                if [[ -f "${race_dir}/${i}.ok" ]]; then
                    winner_index=$i
                    break 2
                fi
            done
            break
        fi
        if [[ $(date +%s) -ge $version_deadline ]]; then
            break
        fi

        sleep 0.2
    done

    for pid in "${pids[@]}"; do
        kill "$pid" 2>/dev/null || true
        wait "$pid" 2>/dev/null || true
    done

    if [[ -n "$winner_index" ]]; then
        cat "${race_dir}/${winner_index}.version"
        rm -rf "$race_dir"
        return 0
    fi

    rm -rf "$race_dir"
    return 1
}

get_latest_version_via_api() {
    local all_releases_url="https://api.github.com/repos/${REPO}/releases?per_page=10"
    local response version

    response=$(github_api_request "$all_releases_url" 2>/dev/null) || return 1

    if echo "$response" | grep -q '"message"[[:space:]]*:[[:space:]]*"API rate limit exceeded'; then
        return 1
    fi

    if echo "$response" | grep -q '"message"[[:space:]]*:[[:space:]]*"Not Found"'; then
        print_error "Repository not found: ${REPO}"
        exit 1
    fi

    version=$(echo "$response" | grep -B5 '"prerelease"[[:space:]]*:[[:space:]]*false' | grep '"tag_name":' | head -1 | sed -E 's/.*"([^"]+)".*/\1/')

    if [[ -n "$version" ]]; then
        echo "$version"
        return 0
    fi

    version=$(echo "$response" | grep '"tag_name":' | head -1 | sed -E 's/.*"([^"]+)".*/\1/')
    if [[ -n "$version" ]]; then
        echo "$version"
        return 0
    fi

    return 1
}

get_latest_version() {
    local version

    version=$(get_latest_version_via_redirect_race 2>/dev/null) && {
        echo "$version"
        return 0
    }
    print_warning "Redirect-based version detection failed on all mirrors, falling back to GitHub API..."

    version=$(get_latest_version_via_api 2>/dev/null) && {
        echo "$version"
        return 0
    }

    print_error "Failed to detect latest version"
    echo "" >&2
    echo "Solutions:" >&2
    echo "  1. Specify a version manually:" >&2
    echo "     curl -fsSL ... | bash -s -- --version v0.2.0" >&2
    echo "  2. Download directly from:" >&2
    echo "     https://github.com/${REPO}/releases" >&2
    exit 1
}

download_file() {
    local url="$1"
    local output="$2"
    local tmp_output="${output}.part"
    local downloader
    local dl_ok=0
    local is_first_attempt=1

    print_step "Downloading from: \`$url\`"

    rm -f "$tmp_output" 2>/dev/null || true

    while IFS= read -r downloader; do
        [[ -z "$downloader" ]] && continue
        rm -f "$tmp_output" 2>/dev/null || true

        if [[ "$is_first_attempt" != "1" ]]; then
            print_warning "Retrying with: $downloader"
        fi
        is_first_attempt=0

        print_step "Using downloader: $downloader"
        if download_with_tool "$downloader" "$url" "$tmp_output" && validate_downloaded_file "$tmp_output"; then
            mv "$tmp_output" "$output"
            dl_ok=1
            break
        fi

        print_warning "$downloader failed"
    done < <(build_downloader_list)

    rm -f "$tmp_output" 2>/dev/null || true

    if [[ "$dl_ok" != "1" ]]; then
        print_error "All download methods failed"
        return 1
    fi

    if [[ ! -f "$output" ]]; then
        print_error "Download appeared to succeed but file not found at: $output"
        return 1
    fi
}

download_github_file_race() {
    local github_path="$1"
    local output="$2"
    local race_dir
    race_dir=$(mktemp -d)

    local -a pids=()
    local -a labels=()
    local index=0

    local -a mirrors=()
    while IFS= read -r m; do
        [[ -n "$m" ]] && mirrors+=("$m")
    done < <(build_mirror_url_list)

    local -a downloaders=()
    while IFS= read -r d; do
        [[ -n "$d" ]] && downloaders+=("$d")
    done < <(build_downloader_list)

    if [[ ${#mirrors[@]} -eq 0 ]]; then
        print_error "No mirrors configured"
        rm -rf "$race_dir"
        return 1
    fi

    if [[ ${#downloaders[@]} -eq 0 ]]; then
        print_error "No download tools available (need curl, wget, aria2c, or axel)"
        rm -rf "$race_dir"
        return 1
    fi

    local total=$(( ${#mirrors[@]} * ${#downloaders[@]} ))
    print_step "Downloading $(basename "$github_path") (racing $total sources)"

    for base_url in "${mirrors[@]}"; do
        local full_url="${base_url}/${github_path}"
        local mirror_short
        mirror_short=$(echo "$base_url" | sed 's|https\{0,1\}://||;s|/.*||')

        for downloader in "${downloaders[@]}"; do
            local race_output="${race_dir}/${index}.data"

            (
                BIFROST_DOWNLOAD_PROGRESS=0 download_with_tool "$downloader" "$full_url" "$race_output" >/dev/null 2>&1 && \
                    validate_downloaded_file "$race_output" 2>/dev/null && \
                    touch "${race_dir}/${index}.ok"
            ) &
            pids+=($!)
            labels+=("${downloader} via ${mirror_short}")
            index=$((index + 1))
        done
    done

    local winner_index=""
    local download_deadline=$(( $(date +%s) + ${BIFROST_DOWNLOAD_TIMEOUT:-120} + ${BIFROST_DOWNLOAD_CONNECT_TIMEOUT:-10} + 5 ))
    while true; do
        for ((i = 0; i < index; i++)); do
            if [[ -f "${race_dir}/${i}.ok" ]]; then
                winner_index=$i
                break 2
            fi
        done

        local any_alive=false
        for pid in "${pids[@]}"; do
            if kill -0 "$pid" 2>/dev/null; then
                any_alive=true
                break
            fi
        done
        if ! $any_alive; then
            for ((i = 0; i < index; i++)); do
                if [[ -f "${race_dir}/${i}.ok" ]]; then
                    winner_index=$i
                    break 2
                fi
            done
            break
        fi
        if [[ $(date +%s) -ge $download_deadline ]]; then
            break
        fi

        sleep 1
    done

    for pid in "${pids[@]}"; do
        kill "$pid" 2>/dev/null || true
        wait "$pid" 2>/dev/null || true
    done

    if [[ -n "$winner_index" ]]; then
        mv "${race_dir}/${winner_index}.data" "$output"
        print_success "Downloaded (${labels[$winner_index]})"
        rm -rf "$race_dir"
        return 0
    fi

    rm -rf "$race_dir"
    print_error "All download attempts failed for: $(basename "$github_path")"
    return 1
}

download_github_file() {
    local github_path="$1"
    local output="$2"
    local selected_base=""
    local selected_label=""
    local selected_url=""

    selected_base=$(select_fastest_github_base "$github_path" 2>/dev/null || true)

    if [[ -n "$selected_base" ]]; then
        selected_label=$(mirror_display_name "$selected_base")
        selected_url="${selected_base%/}/${github_path}"
        print_step "Selected fastest available source: $selected_label"

        if download_file "$selected_url" "$output"; then
            print_success "Downloaded via $selected_label"
            return 0
        fi

        print_warning "Selected source failed during full download, falling back to all mirrors"
    else
        print_warning "Could not probe GitHub mirrors, falling back to all mirrors"
    fi

    download_github_file_race "$github_path" "$output"
}

download_github_optional_file() {
    local github_path="$1"
    local output="$2"
    local selected_base=""
    local selected_label=""
    local selected_url=""

    selected_base=$(select_fastest_github_base "$github_path" 2>/dev/null || true)

    if [[ -z "$selected_base" ]]; then
        print_warning "Optional archive not available: $(basename "$github_path")"
        return 1
    fi

    selected_label=$(mirror_display_name "$selected_base")
    selected_url="${selected_base%/}/${github_path}"
    print_step "Selected fastest available source: $selected_label"

    if download_file "$selected_url" "$output"; then
        print_success "Downloaded via $selected_label"
        return 0
    fi

    print_warning "Optional archive download failed: $(basename "$github_path")"
    return 1
}

verify_checksum() {
    local file="$1"
    local expected="$2"
    local actual

    if command -v sha256sum &> /dev/null; then
        actual=$(sha256sum "$file" | awk '{print $1}')
    elif command -v shasum &> /dev/null; then
        actual=$(shasum -a 256 "$file" | awk '{print $1}')
    else
        print_warning "sha256sum/shasum not found, skipping checksum verification"
        return 0
    fi

    if [[ "$actual" != "$expected" ]]; then
        print_error "Checksum verification failed!"
        print_error "Expected: $expected"
        print_error "Actual:   $actual"
        return 1
    fi

    print_success "Checksum verified"
    return 0
}

extract_archive() {
    local archive="$1"
    local dest="$2"
    local os="$3"

    case "$os" in
        windows)
            if command -v unzip &> /dev/null; then
                unzip -q "$archive" -d "$dest"
            elif command -v powershell.exe &> /dev/null; then
                powershell.exe -Command "Expand-Archive -Path '$archive' -DestinationPath '$dest' -Force"
            elif command -v pwsh &> /dev/null; then
                pwsh -Command "Expand-Archive -Path '$archive' -DestinationPath '$dest' -Force"
            else
                print_error "Neither unzip nor PowerShell found"
                print_warning "Please install unzip or use the PowerShell installer:"
                echo "  irm https://raw.githubusercontent.com/${REPO}/main/install-binary.ps1 | iex"
                exit 1
            fi
            ;;
        *)
            if [[ "$os" == "linux" ]]; then
                if [[ "$archive" == *.tar.xz ]]; then
                    if ! tar --no-same-owner -xJf "$archive" -C "$dest" 2>/dev/null; then
                        tar -xJf "$archive" -C "$dest"
                    fi
                else
                    if ! tar --no-same-owner -xzf "$archive" -C "$dest" 2>/dev/null; then
                        tar -xzf "$archive" -C "$dest"
                    fi
                fi
            else
                if [[ "$archive" == *.tar.xz ]]; then
                    tar -xJf "$archive" -C "$dest"
                else
                    tar -xzf "$archive" -C "$dest"
                fi
            fi
            ;;
    esac
}

clear_xattr() {
    local file="$1"
    if [[ "$(detect_os)" == "darwin" ]]; then
        xattr -c "$file" 2>/dev/null || true
        xattr -d com.apple.provenance "$file" 2>/dev/null || true
        xattr -d com.apple.quarantine "$file" 2>/dev/null || true
    fi
}

detect_user_shell() {
    local shell_name=""

    if [[ -n "${SHELL:-}" ]]; then
        shell_name=$(basename "$SHELL")
    fi

    if [[ -z "$shell_name" ]] && command -v getent &>/dev/null; then
        local passwd_shell
        passwd_shell=$(getent passwd "$(whoami)" 2>/dev/null | cut -d: -f7)
        if [[ -n "$passwd_shell" ]]; then
            shell_name=$(basename "$passwd_shell")
        fi
    fi

    if [[ -z "$shell_name" ]] && [[ -f /etc/passwd ]]; then
        local passwd_shell
        passwd_shell=$(grep "^$(whoami):" /etc/passwd 2>/dev/null | cut -d: -f7)
        if [[ -n "$passwd_shell" ]]; then
            shell_name=$(basename "$passwd_shell")
        fi
    fi

    case "$shell_name" in
        bash|zsh|fish) echo "$shell_name" ;;
        *) echo "bash" ;;
    esac
}

get_shell_config_file() {
    local shell_name="$1"
    local os="$2"

    case "$shell_name" in
        fish)
            echo "${HOME}/.config/fish/config.fish"
            ;;
        zsh)
            echo "${HOME}/.zshrc"
            ;;
        bash)
            if [[ "$os" == "darwin" ]]; then
                if [[ -f "${HOME}/.bash_profile" ]]; then
                    echo "${HOME}/.bash_profile"
                elif [[ -f "${HOME}/.bashrc" ]]; then
                    echo "${HOME}/.bashrc"
                else
                    echo "${HOME}/.bash_profile"
                fi
            else
                echo "${HOME}/.bashrc"
            fi
            ;;
        *)
            echo "${HOME}/.profile"
            ;;
    esac
}

build_path_line() {
    local shell_name="$1"
    local dir="$2"

    case "$shell_name" in
        fish)
            echo "fish_add_path \"$dir\""
            ;;
        *)
            echo "export PATH=\"$dir:\$PATH\""
            ;;
    esac
}

path_already_configured() {
    local config_file="$1"
    local dir="$2"

    [[ ! -f "$config_file" ]] && return 1

    if grep -qF "$dir" "$config_file" 2>/dev/null; then
        return 0
    fi

    return 1
}

add_to_path() {
    local shell_name="$1"
    local config_file="$2"
    local dir="$3"

    if path_already_configured "$config_file" "$dir"; then
        print_success "PATH already configured in $config_file"
        return 0
    fi

    local path_line
    path_line=$(build_path_line "$shell_name" "$dir")

    local config_dir
    config_dir=$(dirname "$config_file")
    if [[ ! -d "$config_dir" ]]; then
        mkdir -p "$config_dir"
    fi

    printf '\n# Added by Bifrost installer\n%s\n' "$path_line" >> "$config_file"
    print_success "Added to $config_file: $path_line"
    return 0
}

path_in_current_process() {
    local dir="$1"
    [[ ":$PATH:" == *":$dir:"* ]]
}

prepend_current_process_path() {
    local dir="$1"
    if ! path_in_current_process "$dir"; then
        export PATH="$dir:$PATH"
    fi
}

windows_path_powershell_command() {
    cat <<'PS'
$dir = $env:BIFROST_WINDOWS_PATH_DIR
if ([string]::IsNullOrWhiteSpace($dir)) {
    Write-Output "missing-dir"
    exit 2
}

$current = [Environment]::GetEnvironmentVariable('Path', 'User')
if ($null -eq $current) {
    $current = ''
}

$normalizedDir = $dir.Trim().TrimEnd('\')
$exists = $false
foreach ($entry in ($current -split ';')) {
    if ($entry.Trim().TrimEnd('\') -ieq $normalizedDir) {
        $exists = $true
        break
    }
}

if ($exists) {
    Write-Output "already"
    exit 0
}

if ([string]::IsNullOrWhiteSpace($current)) {
    $newPath = $dir
} else {
    $newPath = $current.TrimEnd(';') + ';' + $dir
}

[Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
Write-Output "added"
PS
}

add_to_windows_user_path() {
    local win_dir="$1"
    local ps_bin=""
    local result=""

    if has_command powershell.exe; then
        ps_bin="powershell.exe"
    elif has_command pwsh.exe; then
        ps_bin="pwsh.exe"
    elif has_command pwsh; then
        ps_bin="pwsh"
    else
        print_warning "PowerShell not found; cannot update Windows User PATH automatically"
        return 1
    fi

    result=$(BIFROST_WINDOWS_PATH_DIR="$win_dir" "$ps_bin" -NoProfile -ExecutionPolicy Bypass -Command "$(windows_path_powershell_command)" 2>/dev/null | tr -d '\r' | tail -n 1) || {
        print_warning "Failed to update Windows User PATH automatically"
        return 1
    }

    case "$result" in
        added)
            print_success "Added to Windows User PATH: $win_dir"
            ;;
        already)
            print_success "Windows User PATH already contains: $win_dir"
            ;;
        *)
            print_warning "Unexpected Windows User PATH update result: ${result:-empty}"
            return 1
            ;;
    esac
}

configure_install_path() {
    local os="$1"
    local install_dir="$2"
    local current_path_missing=0
    local need_path_hint=0

    if ! path_in_current_process "$install_dir"; then
        current_path_missing=1
    fi

    if [[ "$NO_MODIFY_PATH" == "1" ]]; then
        if [[ "$current_path_missing" == "1" ]]; then
            print_warning "$install_dir is not in your PATH (auto-configuration skipped by --no-modify-path)"
            need_path_hint=1
        fi
    elif [[ "$os" == "windows" ]]; then
        local win_install_dir="$install_dir"
        if command -v cygpath >/dev/null 2>&1; then
            win_install_dir=$(cygpath -w "$install_dir")
        fi

        if [[ "$current_path_missing" == "1" ]]; then
            print_warning "$install_dir is not in your PATH"
            echo ""

            local user_shell
            user_shell=$(detect_user_shell)
            local config_file
            config_file=$(get_shell_config_file "$user_shell" "$os")

            print_step "Detected shell: $user_shell (config: $config_file)"
            add_to_path "$user_shell" "$config_file" "$install_dir"
            prepend_current_process_path "$install_dir"
            need_path_hint=1
        fi

        add_to_windows_user_path "$win_install_dir" || {
            echo ""
            echo "You can add it to Windows PATH manually (PowerShell):"
            echo ""
            echo "  \$currentPath = [Environment]::GetEnvironmentVariable('Path', 'User')"
            echo "  [Environment]::SetEnvironmentVariable('Path', \"\$currentPath;$win_install_dir\", 'User')"
        }
    elif [[ "$current_path_missing" == "1" ]]; then
        print_warning "$install_dir is not in your PATH"
        echo ""

        local user_shell
        user_shell=$(detect_user_shell)
        local config_file
        config_file=$(get_shell_config_file "$user_shell" "$os")

        print_step "Detected shell: $user_shell (config: $config_file)"
        add_to_path "$user_shell" "$config_file" "$install_dir"
        prepend_current_process_path "$install_dir"
        need_path_hint=1
    fi

    if [[ "$need_path_hint" == "1" ]]; then
        echo ""
        print_warning "Restart your terminal or run the following to use bifrost from this shell now:"
        echo ""
        echo "  export PATH=\"$install_dir:\$PATH\""
        if [[ "$os" == "windows" ]]; then
            echo ""
            echo "New PowerShell/CMD/Git Bash windows will inherit the updated Windows User PATH."
            echo "Already-open PowerShell/CMD windows must be restarted to see the new PATH."
        fi
    fi
}

show_help() {
    echo "Bifrost Installation Script"
    echo ""
    echo "Usage: $0 [OPTIONS]"
    echo ""
    echo "Options:"
    echo "  --version <VERSION>   Install a specific version (e.g., v0.1.0)"
    echo "  --dir <PATH>          Installation directory (default: ~/.local/bin)"
    echo "  --target <TRIPLE>     Override target triple (e.g., x86_64-unknown-linux-musl)"
    echo "  --libc <gnu|musl>     Override libc variant on Linux (auto-detected by default)"
    echo "  --no-modify-path      Skip automatic PATH configuration in shell profile"
    echo "  --no-post-install     Skip automatic CA trust, skill install, and service start"
    echo "  --no-install-cert     Skip automatic CA certificate installation/trust"
    echo "  --no-install-skills   Skip automatic Bifrost skill installation"
    echo "  --no-start            Skip automatic Bifrost service startup"
    echo "  --help                Show this help message"
    echo ""
    echo "Environment variables:"
    echo "  BIFROST_INSTALL_DIR   Custom installation directory"
    echo "  BIFROST_DOWNLOADER    Preferred downloader: auto|aria2c|axel|wget|curl"
    echo "  BIFROST_GITHUB_MIRROR Preferred mirror base URL"
    echo "  BIFROST_DOWNLOAD_CONNECT_TIMEOUT  Connection timeout in seconds"
    echo "  BIFROST_MIRROR_PROBE_TIMEOUT      Fast mirror probe timeout in seconds"
    echo "  BIFROST_DOWNLOAD_TIMEOUT          Total timeout per download attempt in seconds"
    echo "  BIFROST_DOWNLOAD_TRIES            Retry count per downloader attempt"
    echo "  BIFROST_INSTALL_POST_INSTALL      Set to 0/false to skip post-install setup"
    echo "  BIFROST_INSTALL_POST_INSTALL_TIMEOUT  Timeout per post-install command in seconds"
    echo "  BIFROST_INSTALL_AUTO_CERT         Set to 0/false to skip CA trust"
    echo "  BIFROST_INSTALL_AUTO_SKILLS       Set to 0/false to skip skill installation"
    echo "  BIFROST_INSTALL_AUTO_START        Set to 0/false to skip service startup"
    echo ""
    echo "Examples:"
    echo "  curl -fsSL https://raw.githubusercontent.com/${REPO}/main/install-binary.sh | bash"
    echo "  curl -fsSL ... | bash -s -- --version v0.0.9-alpha"
    echo "  curl -fsSL ... | bash -s -- --dir /usr/local/bin"
    echo "  curl -fsSL ... | bash -s -- --libc musl"
    echo "  curl -fsSL ... | bash -s -- --target x86_64-unknown-linux-musl"
    echo "  curl -fsSL ... | bash -s -- --no-modify-path"
    echo "  curl -fsSL ... | bash -s -- --no-post-install"
    echo ""
    echo "If raw.githubusercontent.com is unreachable, use a mirror to download this script:"
    echo "  curl -fsSL https://ghfast.top/https://raw.githubusercontent.com/${REPO}/main/install-binary.sh | bash"
    echo "  curl -fsSL https://github.moeyy.xyz/https://raw.githubusercontent.com/${REPO}/main/install-binary.sh | bash"
}

VERSION=""
FORCE_TARGET=""
FORCE_LIBC=""
NO_MODIFY_PATH=0
NO_POST_INSTALL=0
NO_INSTALL_CERT=0
NO_INSTALL_SKILLS=0
NO_START=0

while [[ $# -gt 0 ]]; do
    case $1 in
        --version)
            if [[ -z "${2:-}" || "$2" == --* ]]; then
                print_error "--version requires a value (e.g., --version v0.1.0)"
                exit 1
            fi
            VERSION="$2"
            shift 2
            ;;
        --dir)
            if [[ -z "${2:-}" || "$2" == --* ]]; then
                print_error "--dir requires a value (e.g., --dir /usr/local/bin)"
                exit 1
            fi
            INSTALL_DIR="$2"
            shift 2
            ;;
        --target)
            if [[ -z "${2:-}" || "$2" == --* ]]; then
                print_error "--target requires a value (e.g., --target x86_64-unknown-linux-musl)"
                exit 1
            fi
            FORCE_TARGET="$2"
            shift 2
            ;;
        --libc)
            if [[ -z "${2:-}" || "$2" == --* ]]; then
                print_error "--libc requires a value (gnu or musl)"
                exit 1
            fi
            FORCE_LIBC="$2"
            if [[ "$FORCE_LIBC" != "gnu" && "$FORCE_LIBC" != "musl" ]]; then
                print_error "Invalid --libc value: $FORCE_LIBC (must be 'gnu' or 'musl')"
                exit 1
            fi
            shift 2
            ;;
        --no-modify-path)
            NO_MODIFY_PATH=1
            shift
            ;;
        --no-post-install)
            NO_POST_INSTALL=1
            shift
            ;;
        --no-install-cert)
            NO_INSTALL_CERT=1
            shift
            ;;
        --no-install-skills)
            NO_INSTALL_SKILLS=1
            shift
            ;;
        --no-start)
            NO_START=1
            shift
            ;;
        --help)
            show_help
            exit 0
            ;;
        *)
            print_error "Unknown option: $1"
            show_help
            exit 1
            ;;
    esac
done

is_enabled() {
    local value="$1"
    case "$(printf '%s' "$value" | tr '[:upper:]' '[:lower:]')" in
        0|false|no|off) return 1 ;;
        *) return 0 ;;
    esac
}

run_bifrost_post_install_command() {
    if is_enabled "${BIFROST_INSTALL_POST_INSTALL_DRY_RUN:-0}"; then
        echo "  [dry-run] $*"
        return 0
    fi

    local timeout_seconds="${BIFROST_INSTALL_POST_INSTALL_TIMEOUT:-120}"
    if [[ "$timeout_seconds" =~ ^[0-9]+$ ]] && [[ "$timeout_seconds" -gt 0 ]]; then
        if has_command perl; then
            perl -e '
                my $timeout = shift @ARGV;
                my $pid = fork();
                exit 125 unless defined $pid;
                if ($pid == 0) {
                    exec @ARGV;
                    exit 127;
                }
                $SIG{ALRM} = sub {
                    kill "TERM", $pid;
                    select undef, undef, undef, 1;
                    kill "KILL", $pid;
                    exit 124;
                };
                alarm $timeout;
                waitpid($pid, 0);
                my $status = $?;
                alarm 0;
                exit($status >> 8) if ($status & 127) == 0;
                exit(128 + ($status & 127));
            ' "$timeout_seconds" "$@"
            local rc=$?
            if [[ "$rc" -eq 124 ]]; then
                print_warning "Post-install command timed out after ${timeout_seconds}s: $*"
            fi
            return "$rc"
        fi

        print_warning "perl not found; running post-install command without watchdog timeout"
    fi

    "$@"
}

run_bifrost_post_install_step() {
    local label="$1"
    shift

    print_step "$label"
    if run_bifrost_post_install_command "$@"; then
        print_success "$label completed"
        return 0
    fi

    print_warning "$label failed"
    print_warning "You can retry manually with:"
    echo "  $*"
    return 1
}

run_post_install() {
    local bifrost_bin="$1"
    local failures=0

    if [[ "$NO_POST_INSTALL" == "1" ]] || ! is_enabled "${BIFROST_INSTALL_POST_INSTALL:-1}"; then
        print_warning "Post-install setup skipped"
        return 0
    fi

    echo ""
    print_step "Completing one-click setup..."

    if [[ "$NO_INSTALL_CERT" != "1" ]] && is_enabled "${BIFROST_INSTALL_AUTO_CERT:-1}"; then
        run_bifrost_post_install_step "Installing and trusting CA certificate" \
            "$bifrost_bin" ca install || failures=$((failures + 1))
    else
        print_warning "CA certificate installation skipped"
    fi

    if [[ "$NO_INSTALL_SKILLS" != "1" ]] && is_enabled "${BIFROST_INSTALL_AUTO_SKILLS:-1}"; then
        run_bifrost_post_install_step "Installing Bifrost skills for all supported AI tools" \
            "$bifrost_bin" install-skill --tool all -y || failures=$((failures + 1))
    else
        print_warning "Bifrost skill installation skipped"
    fi

    if [[ "$NO_START" != "1" ]] && is_enabled "${BIFROST_INSTALL_AUTO_START:-1}"; then
        run_bifrost_post_install_step "Starting Bifrost service" \
            "$bifrost_bin" start --daemon --yes || failures=$((failures + 1))
    else
        print_warning "Bifrost service startup skipped"
    fi

    if [[ "$failures" -eq 0 ]]; then
        print_success "One-click setup completed"
    else
        print_warning "One-click setup finished with $failures failed step(s); the CLI binary is installed."
    fi

    return 0
}

install_binary_for_target() {
    local target="$1"
    local version="$2"
    local os="$3"
    local install_dir="$4"
    local tmpdir="$5"

    local cli_archive=""
    local ext
    local checksums_path="${REPO}/releases/download/${version}/bifrost-${version}-checksums.txt"

    while IFS= read -r ext; do
        [[ -n "$ext" ]] || continue
        local candidate_archive="bifrost-${version}-${target}.${ext}"
        local candidate_path="${REPO}/releases/download/${version}/${candidate_archive}"
        local download_ok=1
        if [[ "$ext" != "tar.gz" && "$ext" != "zip" ]]; then
            download_github_optional_file "$candidate_path" "$tmpdir/$candidate_archive" && download_ok=0
        else
            download_github_file "$candidate_path" "$tmpdir/$candidate_archive" && download_ok=0
        fi
        if [[ "$download_ok" == "0" ]]; then
            cli_archive="$candidate_archive"
            break
        fi
        if [[ "$ext" != "tar.gz" && "$ext" != "zip" ]]; then
            print_warning "Could not download ${candidate_archive}, falling back to the compatible archive"
        fi
    done < <(get_archive_ext_candidates "$os")

    if [[ -z "$cli_archive" ]]; then
        return 1
    fi

    if [[ ! -f "$tmpdir/checksums.txt" ]]; then
        download_github_file "$checksums_path" "$tmpdir/checksums.txt" || print_warning "Could not download checksums file, skipping verification"
    fi

    if [[ -f "$tmpdir/checksums.txt" ]]; then
        local expected_checksum
        expected_checksum=$(grep -F "$cli_archive" "$tmpdir/checksums.txt" 2>/dev/null | awk '{print $1}')
        if [[ -n "$expected_checksum" ]]; then
            if ! verify_checksum "$tmpdir/$cli_archive" "$expected_checksum"; then
                print_warning "Checksum mismatch (continuing anyway)"
            fi
        else
            print_warning "Checksum not found for $cli_archive, skipping verification"
        fi
    fi

    print_step "Extracting..."
    local extract_subdir="$tmpdir/extract_${target}"
    mkdir -p "$extract_subdir"
    extract_archive "$tmpdir/$cli_archive" "$extract_subdir" "$os"

    local extracted_dir="bifrost-${version}-${target}"
    local binary_name="bifrost"
    [[ "$os" == "windows" ]] && binary_name="bifrost.exe"

    if [[ -f "$extract_subdir/$extracted_dir/$binary_name" ]]; then
        install_binary_atomically "$extract_subdir/$extracted_dir/$binary_name" "$install_dir/$binary_name"
    elif [[ -f "$extract_subdir/$binary_name" ]]; then
        install_binary_atomically "$extract_subdir/$binary_name" "$install_dir/$binary_name"
    else
        local found_binary
        found_binary=$(find "$extract_subdir" -name "$binary_name" -type f 2>/dev/null | head -1)
        if [[ -n "$found_binary" ]]; then
            print_warning "Binary found at unexpected path: $found_binary"
            install_binary_atomically "$found_binary" "$install_dir/$binary_name"
        else
            print_error "Binary not found in archive"
            ls -laR "$extract_subdir" >&2 2>/dev/null || true
            return 1
        fi
    fi
    return 0
}

get_musl_fallback_target() {
    local target="$1"
    case "$target" in
        x86_64-unknown-linux-gnu)  echo "x86_64-unknown-linux-musl" ;;
        aarch64-unknown-linux-gnu) echo "aarch64-unknown-linux-musl" ;;
        *) echo "" ;;
    esac
}

main() {
    print_banner

    local os arch target ext

    os=$(detect_os)
    arch=$(detect_arch)

    print_step "Detecting system..."
    echo "  OS:           $os"
    echo "  Architecture: $arch"

    if [[ "$os" == "unknown" ]]; then
        print_error "Unsupported operating system"
        exit 1
    fi

    if [[ "$arch" == "unknown" ]]; then
        print_error "Unsupported architecture"
        exit 1
    fi

    if [[ -n "$FORCE_TARGET" ]]; then
        target="$FORCE_TARGET"
        print_step "Using user-specified target: $target"
    elif [[ -n "$FORCE_LIBC" && "$os" == "linux" ]]; then
        case "$arch" in
            x86_64|aarch64) target="${arch}-unknown-linux-${FORCE_LIBC}" ;;
            armv7)          target="armv7-unknown-linux-gnueabihf" ;;
            *)
                print_error "No pre-built binary available for $os-$arch"
                exit 1
                ;;
        esac
        print_step "Using user-specified libc: $FORCE_LIBC -> $target"
    else
        if [[ -n "$FORCE_LIBC" && "$os" != "linux" ]]; then
            print_warning "--libc is only applicable on Linux (detected: $os), ignoring"
        fi
        target=$(get_target "$os" "$arch")
    fi

    if [[ -z "$target" ]]; then
        print_error "No pre-built binary available for $os-$arch"
        print_warning "You can build from source instead:"
        echo "  git clone https://github.com/${REPO}.git"
        echo "  cd bifrost && ./install.sh"
        exit 1
    fi

    if [[ -z "$VERSION" ]]; then
        print_step "Fetching latest version..."
        VERSION=$(get_latest_version)
    fi

    print_success "Installing version: $VERSION"
    echo "  Target: $target"

    mkdir -p "$INSTALL_DIR"

    local tmpdir
    tmpdir=$(mktemp -d)
    trap "rm -rf '$tmpdir'" EXIT INT TERM HUP

    local binary_name="bifrost"
    [[ "$os" == "windows" ]] && binary_name="bifrost.exe"

    print_step "Installing CLI..."
    local install_ok=0
    install_binary_for_target "$target" "$VERSION" "$os" "$INSTALL_DIR" "$tmpdir" && install_ok=1

    local verify_ok=1
    if [[ "$install_ok" == "1" && "$os" == "linux" ]]; then
        verify_binary_runs "$INSTALL_DIR/$binary_name" && verify_ok=1 || verify_ok=0
    fi

    if [[ "$install_ok" != "1" || "$verify_ok" != "1" ]]; then

        local musl_target
        musl_target=$(get_musl_fallback_target "$target")

        if [[ -n "$musl_target" ]]; then
            print_warning "Binary (${target}) not usable, retrying with musl build: $musl_target"
            target="$musl_target"
            install_binary_for_target "$target" "$VERSION" "$os" "$INSTALL_DIR" "$tmpdir"

            if ! verify_binary_runs "$INSTALL_DIR/$binary_name"; then
                print_error "Fallback musl binary also failed to run"
                exit 1
            fi
            print_success "musl fallback succeeded"
        else
            print_error "Installed binary failed to run"
            print_warning "Try reinstalling with: --libc musl  or  --target <triple>"
            exit 1
        fi
    fi

    print_success "CLI installed: $INSTALL_DIR/$binary_name ($target)"

    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    print_success "Installation completed!"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo ""

    configure_install_path "$os" "$INSTALL_DIR"

    run_post_install "$INSTALL_DIR/$binary_name"

    echo ""
    echo "Getting started:"
    echo ""
    echo "  # Check proxy status"
    echo "  bifrost status"
    echo ""
    echo "  # Start with custom port"
    echo "  bifrost -p 8080 start"
    echo ""
    echo "  # Show help"
    echo "  bifrost --help"
    echo ""
    echo "Documentation: https://github.com/${REPO}"
    echo ""
}

if [[ "${BIFROST_INSTALL_BINARY_SKIP_MAIN:-0}" != "1" ]]; then
    main "$@"
fi
