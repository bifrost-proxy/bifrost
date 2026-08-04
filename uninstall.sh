#!/bin/bash
set -e

BINARY_NAME="bifrost"
APP_NAME="Bifrost.app"
DESKTOP_BUNDLE_ID="com.bifrost.desktop"
DEFAULT_INSTALL_DIR="$HOME/.local/bin"
DEFAULT_APP_INSTALL_DIR="$HOME/Applications"
DEFAULT_DATA_DIR="$HOME/.bifrost"
DEFAULT_CONFIG_DIR="$HOME/.config/bifrost"
DEFAULT_MAC_APP_SUPPORT_DIR="$HOME/Library/Application Support/$DESKTOP_BUNDLE_ID"
DEFAULT_MAC_CACHE_DIR="$HOME/Library/Caches/$DESKTOP_BUNDLE_ID"
DEFAULT_MAC_PREFS_FILE="$HOME/Library/Preferences/$DESKTOP_BUNDLE_ID.plist"
DEFAULT_MAC_SAVED_STATE_DIR="$HOME/Library/Saved Application State/$DESKTOP_BUNDLE_ID.savedState"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

print_banner() {
    printf "%b" "${CYAN}"
    echo "╔═══════════════════════════════════════════════════════════╗"
    echo "║                                                           ║"
    echo "║   Bifrost Uninstaller                                     ║"
    echo "║                                                           ║"
    echo "╚═══════════════════════════════════════════════════════════╝"
    printf "%b\n" "${NC}"
}

print_step() {
    printf "%b==>%b %s\n" "${BLUE}" "${NC}" "$1"
}

print_success() {
    printf "%b✓%b %s\n" "${GREEN}" "${NC}" "$1"
}

print_warning() {
    printf "%b!%b %s\n" "${YELLOW}" "${NC}" "$1"
}

print_error() {
    printf "%b✗%b %s\n" "${RED}" "${NC}" "$1"
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

show_help() {
    echo "Bifrost Uninstallation Script"
    echo ""
    echo "Usage: $0 [OPTIONS]"
    echo ""
    echo "Options:"
    echo "  --dir <PATH>          CLI installation directory (default: ~/.local/bin)"
    echo "  --app-dir <PATH>      Desktop app installation directory (default: ~/Applications)"
    echo "  --cli-only            Uninstall CLI only"
    echo "  --desktop-only        Uninstall desktop app only"
    echo "  --purge               Also remove configuration and data files"
    echo "  --yes, -y             Skip confirmation prompts"
    echo "  --help                Show this help message"
    echo ""
    echo "Examples:"
    echo "  $0"
    echo "  $0 --desktop-only"
    echo "  $0 --yes --purge"
}

INSTALL_DIR="$DEFAULT_INSTALL_DIR"
APP_INSTALL_DIR="${BIFROST_APP_INSTALL_DIR:-$DEFAULT_APP_INSTALL_DIR}"
PURGE=false
YES=false
REMOVE_CLI=true
REMOVE_DESKTOP=true

while [[ $# -gt 0 ]]; do
    case "$1" in
        --dir)
            INSTALL_DIR="$2"
            shift 2
            ;;
        --app-dir)
            APP_INSTALL_DIR="$2"
            shift 2
            ;;
        --cli-only)
            REMOVE_CLI=true
            REMOVE_DESKTOP=false
            shift
            ;;
        --desktop-only)
            REMOVE_CLI=false
            REMOVE_DESKTOP=true
            shift
            ;;
        --purge)
            PURGE=true
            shift
            ;;
        --yes|-y)
            YES=true
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

if [[ "$REMOVE_CLI" == "false" && "$REMOVE_DESKTOP" == "false" ]]; then
    print_error "Nothing to uninstall"
    exit 1
fi

confirm() {
    local prompt="$1"
    if [[ "$YES" == "true" ]]; then
        return 0
    fi

    printf "%s [y/N] " "$prompt"
    read -r response
    case "$response" in
        [yY][eE][sS]|[yY]) return 0 ;;
        *) return 1 ;;
    esac
}

main() {
    print_banner

    local os binary_path source_marker_path app_path
    os=$(detect_os)

    local binary_name="$BINARY_NAME"
    [[ "$os" == "windows" ]] && binary_name="$BINARY_NAME.exe"

    binary_path="$INSTALL_DIR/$binary_name"
    source_marker_path="${binary_path}.install-source"
    app_path="$APP_INSTALL_DIR/$APP_NAME"

    print_step "Checking installation..."

    local found_binary=false
    local found_app=false
    local found_data=false
    local found_config=false
    local found_mac_app_support=false
    local found_mac_cache=false
    local found_mac_prefs=false
    local found_mac_saved_state=false

    if [[ "$REMOVE_CLI" == "true" \
       && ( -f "$binary_path" || -f "$source_marker_path" ) ]]; then
        found_binary=true
        echo "  Binary:  $binary_path"
        [[ -f "$source_marker_path" ]] && echo "  Source:  $source_marker_path"
    fi

    if [[ "$REMOVE_DESKTOP" == "true" && -d "$app_path" ]]; then
        found_app=true
        echo "  App:     $app_path"
    fi

    if [[ "$PURGE" == "true" && -d "$DEFAULT_DATA_DIR" ]]; then
        found_data=true
        echo "  Data:    $DEFAULT_DATA_DIR"
    fi

    if [[ "$PURGE" == "true" && -d "$DEFAULT_CONFIG_DIR" ]]; then
        found_config=true
        echo "  Config:  $DEFAULT_CONFIG_DIR"
    fi

    if [[ "$PURGE" == "true" && "$os" == "darwin" ]]; then
        if [[ -d "$DEFAULT_MAC_APP_SUPPORT_DIR" ]]; then
            found_mac_app_support=true
            echo "  App data: $DEFAULT_MAC_APP_SUPPORT_DIR"
        fi
        if [[ -d "$DEFAULT_MAC_CACHE_DIR" ]]; then
            found_mac_cache=true
            echo "  Cache:    $DEFAULT_MAC_CACHE_DIR"
        fi
        if [[ -f "$DEFAULT_MAC_PREFS_FILE" ]]; then
            found_mac_prefs=true
            echo "  Prefs:    $DEFAULT_MAC_PREFS_FILE"
        fi
        if [[ -d "$DEFAULT_MAC_SAVED_STATE_DIR" ]]; then
            found_mac_saved_state=true
            echo "  State:    $DEFAULT_MAC_SAVED_STATE_DIR"
        fi
    fi

    if [[ "$found_binary" == "false" \
       && "$found_app" == "false" \
       && "$found_data" == "false" \
       && "$found_config" == "false" \
       && "$found_mac_app_support" == "false" \
       && "$found_mac_cache" == "false" \
       && "$found_mac_prefs" == "false" \
       && "$found_mac_saved_state" == "false" ]]; then
        print_warning "Bifrost is not installed or already uninstalled"
        echo ""
        [[ "$REMOVE_CLI" == "true" ]] && echo "  Binary: $binary_path"
        [[ "$REMOVE_DESKTOP" == "true" ]] && echo "  App:    $app_path"
        if [[ "$PURGE" == "true" ]]; then
            echo "  Data:   $DEFAULT_DATA_DIR"
            echo "  Config: $DEFAULT_CONFIG_DIR"
        fi
        exit 0
    fi

    echo ""

    if [[ "$PURGE" == "true" ]]; then
        print_warning "This will remove installed binaries/apps and ALL data/configuration files!"
    fi

    if ! confirm "Do you want to proceed with uninstallation?"; then
        echo "Uninstallation cancelled."
        exit 0
    fi

    echo ""

    if [[ "$found_binary" == "true" ]]; then
        print_step "Removing CLI binary..."
        rm -f "$binary_path" "$source_marker_path"
        print_success "Removed: $binary_path"
    fi

    if [[ "$found_app" == "true" ]]; then
        print_step "Removing desktop app..."
        rm -rf "$app_path"
        print_success "Removed: $app_path"
    fi

    if [[ "$PURGE" == "true" ]]; then
        if [[ "$found_data" == "true" ]]; then
            print_step "Removing data directory..."
            rm -rf "$DEFAULT_DATA_DIR"
            print_success "Removed: $DEFAULT_DATA_DIR"
        fi

        if [[ "$found_config" == "true" ]]; then
            print_step "Removing config directory..."
            rm -rf "$DEFAULT_CONFIG_DIR"
            print_success "Removed: $DEFAULT_CONFIG_DIR"
        fi

        if [[ "$found_mac_app_support" == "true" ]]; then
            print_step "Removing desktop app support data..."
            rm -rf "$DEFAULT_MAC_APP_SUPPORT_DIR"
            print_success "Removed: $DEFAULT_MAC_APP_SUPPORT_DIR"
        fi

        if [[ "$found_mac_cache" == "true" ]]; then
            print_step "Removing desktop cache..."
            rm -rf "$DEFAULT_MAC_CACHE_DIR"
            print_success "Removed: $DEFAULT_MAC_CACHE_DIR"
        fi

        if [[ "$found_mac_prefs" == "true" ]]; then
            print_step "Removing desktop preferences..."
            rm -f "$DEFAULT_MAC_PREFS_FILE"
            print_success "Removed: $DEFAULT_MAC_PREFS_FILE"
        fi

        if [[ "$found_mac_saved_state" == "true" ]]; then
            print_step "Removing desktop saved state..."
            rm -rf "$DEFAULT_MAC_SAVED_STATE_DIR"
            print_success "Removed: $DEFAULT_MAC_SAVED_STATE_DIR"
        fi
    else
        if [[ "$REMOVE_CLI" == "true" && ( -d "$DEFAULT_DATA_DIR" || -d "$DEFAULT_CONFIG_DIR" ) ]]; then
            echo ""
            print_warning "CLI data and config files were preserved."
            echo "  To remove them, run: $0 --purge"
            [[ -d "$DEFAULT_DATA_DIR" ]] && echo "  Data:   $DEFAULT_DATA_DIR"
            [[ -d "$DEFAULT_CONFIG_DIR" ]] && echo "  Config: $DEFAULT_CONFIG_DIR"
        fi

        if [[ "$REMOVE_DESKTOP" == "true" && "$os" == "darwin" \
           && ( -d "$DEFAULT_MAC_APP_SUPPORT_DIR" || -d "$DEFAULT_MAC_CACHE_DIR" || -f "$DEFAULT_MAC_PREFS_FILE" ) ]]; then
            echo ""
            print_warning "Desktop app data was preserved."
            echo "  To remove it, run: $0 --desktop-only --purge"
        fi
    fi

    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    print_success "Uninstallation completed!"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo ""

    if [[ "$REMOVE_CLI" == "true" && ":$PATH:" == *":$INSTALL_DIR:"* ]]; then
        print_warning "You may want to remove $INSTALL_DIR from your PATH"
        echo ""
        echo "Edit your shell configuration file and remove the PATH entry:"
        case "$SHELL" in
            */fish)
                echo "  ~/.config/fish/config.fish"
                ;;
            */zsh)
                echo "  ~/.zshrc"
                ;;
            *)
                echo "  ~/.bashrc"
                ;;
        esac
    fi

    echo ""
}

main "$@"
