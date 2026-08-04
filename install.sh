#!/bin/bash

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

print_step() {
    echo -e "${BLUE}==>${NC} $1"
}

print_success() {
    echo -e "${GREEN}✓${NC} $1"
}

print_warning() {
    echo -e "${YELLOW}!${NC} $1"
}

print_error() {
    echo -e "${RED}✗${NC} $1"
}

detect_os() {
    case "$(uname -s)" in
        Darwin*) echo "darwin" ;;
        Linux*) echo "linux" ;;
        *) echo "unknown" ;;
    esac
}

OS="$(detect_os)"
INSTALL_DIR="${BIFROST_INSTALL_DIR:-$HOME/.local/bin}"
DEFAULT_APP_INSTALL_DIR="/Applications"
APP_INSTALL_DIR="${BIFROST_APP_INSTALL_DIR:-$DEFAULT_APP_INSTALL_DIR}"
INSTALL_CLI=true
INSTALL_DESKTOP=true

show_help() {
    echo "Bifrost Installation Script"
    echo ""
    echo "Usage: $0 [OPTIONS]"
    echo ""
    echo "Options:"
    echo "  --dir <path>         Custom CLI installation directory (default: ~/.local/bin)"
    echo "  --app-dir <path>     Custom desktop app installation directory (default: /Applications)"
    echo "  --cli-only           Install CLI only"
    echo "  --desktop-only       Install desktop app only"
    echo "  --no-desktop         Skip desktop app build and installation"
    echo "  --help               Show this help message"
    echo ""
    echo "Environment variables:"
    echo "  BIFROST_INSTALL_DIR      Custom CLI installation directory"
    echo "  BIFROST_APP_INSTALL_DIR  Custom desktop app installation directory"
    echo ""
    echo "Examples:"
    echo "  $0"
    echo "  $0 --cli-only"
    echo "  $0 --app-dir ~/Applications"
}

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
            INSTALL_CLI=true
            INSTALL_DESKTOP=false
            shift
            ;;
        --desktop-only)
            INSTALL_CLI=false
            INSTALL_DESKTOP=true
            shift
            ;;
        --no-desktop)
            INSTALL_DESKTOP=false
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

if [[ "$INSTALL_CLI" == "false" && "$INSTALL_DESKTOP" == "false" ]]; then
    print_error "Nothing to install"
    exit 1
fi

if ! command -v cargo &> /dev/null; then
    print_error "Rust toolchain not found. Please install Rust first:"
    echo "  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
    exit 1
fi

if [[ "$INSTALL_DESKTOP" == "true" && "$OS" != "darwin" ]]; then
    print_warning "Desktop app install is currently supported by this script on macOS only"
    INSTALL_DESKTOP=false
fi

if [[ "$INSTALL_CLI" == "true" ]]; then
    print_step "CLI install directory: $INSTALL_DIR"
    mkdir -p "$INSTALL_DIR"
fi

if [[ "$INSTALL_DESKTOP" == "true" ]]; then
    print_step "Desktop app install directory: $APP_INSTALL_DIR"
    mkdir -p "$APP_INSTALL_DIR"
fi

clear_xattr() {
    if [[ "$OS" == "darwin" && -e "$1" ]]; then
        xattr -cr "$1" 2>/dev/null || true
        xattr -d com.apple.provenance "$1" 2>/dev/null || true
        xattr -d com.apple.quarantine "$1" 2>/dev/null || true
    fi
}

ensure_pnpm() {
    if ! command -v pnpm &> /dev/null; then
        print_error "pnpm not found. Please install Node.js 22+ and pnpm first."
        exit 1
    fi
}

ensure_root_deps() {
    ensure_pnpm
    if [[ ! -d "$SCRIPT_DIR/node_modules/@tauri-apps" ]]; then
        print_step "Installing root dependencies..."
        (cd "$SCRIPT_DIR" && pnpm install)
    fi
}

ensure_web_deps() {
    ensure_pnpm
    if [[ ! -d "$SCRIPT_DIR/web/node_modules" ]]; then
        print_step "Installing frontend dependencies..."
        (cd "$SCRIPT_DIR/web" && pnpm install)
    fi
}

ensure_web_dist() {
    local dist_dir="$SCRIPT_DIR/web/dist"
    ensure_web_deps

    if [[ ! -f "$dist_dir/index.html" || ! -d "$dist_dir/assets" ]]; then
        print_step "Building frontend..."
        (cd "$SCRIPT_DIR/web" && pnpm run build)
    fi

    if [[ ! -f "$dist_dir/index.html" || ! -d "$dist_dir/assets" ]]; then
        print_error "Frontend build failed or incomplete"
        exit 1
    fi

    print_success "Frontend ready"
}

ensure_desktop_dist() {
    local dist_dir="$SCRIPT_DIR/web/dist-desktop"
    ensure_root_deps
    ensure_web_deps

    if [[ ! -f "$dist_dir/index.html" || ! -d "$dist_dir/assets" ]]; then
        print_step "Building desktop frontend..."
        (cd "$SCRIPT_DIR/web" && pnpm run build:desktop)
    fi

    if [[ ! -f "$dist_dir/index.html" || ! -d "$dist_dir/assets" ]]; then
        print_error "Desktop frontend build failed or incomplete"
        exit 1
    fi

    print_success "Desktop frontend ready"
}

install_cli() {
    ensure_web_dist

    print_step "Building and installing bifrost CLI (release mode)..."
    touch "$SCRIPT_DIR/crates/bifrost-admin/build.rs"
    cargo install --locked --path crates/bifrost-cli --root "$INSTALL_DIR" --force
    clear_xattr "$INSTALL_DIR/bin/bifrost"
    mv "$INSTALL_DIR/bin/bifrost" "$INSTALL_DIR/bifrost"
    printf '%s\n' script > "$INSTALL_DIR/bifrost.install-source.tmp.$$"
    mv -f "$INSTALL_DIR/bifrost.install-source.tmp.$$" "$INSTALL_DIR/bifrost.install-source"
    rmdir "$INSTALL_DIR/bin" 2>/dev/null || true
    rm -f "$INSTALL_DIR/.crates.toml" "$INSTALL_DIR/.crates2.json" 2>/dev/null || true
    print_success "bifrost CLI installed successfully"
}

find_desktop_bundle() {
    local bundle_root="$SCRIPT_DIR/desktop/src-tauri/target/release/bundle"
    local app_path=""

    if [[ -d "$bundle_root/macos/Bifrost.app" ]]; then
        app_path="$bundle_root/macos/Bifrost.app"
    else
        app_path="$(find "$bundle_root" -maxdepth 3 -type d -name 'Bifrost.app' 2>/dev/null | head -n 1)"
    fi

    if [[ -z "$app_path" || ! -d "$app_path" ]]; then
        return 1
    fi

    printf '%s\n' "$app_path"
}

copy_app_bundle() {
    local source_app="$1"
    local target_app="$2"

    rm -rf "$target_app"

    if command -v ditto &> /dev/null; then
        ditto "$source_app" "$target_app"
    else
        cp -R "$source_app" "$target_app"
    fi
}

install_desktop() {
    local desktop_bundle_path
    local installed_app_path="$APP_INSTALL_DIR/Bifrost.app"

    ensure_desktop_dist

    print_step "Building desktop app bundle for macOS..."
    (cd "$SCRIPT_DIR" && pnpm run desktop:build:app)

    desktop_bundle_path="$(find_desktop_bundle)" || {
        print_error "Desktop bundle not found after build"
        exit 1
    }

    print_step "Installing desktop app..."
    if [[ ! -w "$APP_INSTALL_DIR" ]]; then
        print_error "No write permission for $APP_INSTALL_DIR"
        echo "  Try re-running with sudo, or pass --app-dir \$HOME/Applications"
        exit 1
    fi
    copy_app_bundle "$desktop_bundle_path" "$installed_app_path"
    clear_xattr "$installed_app_path"
    print_success "Desktop app installed: $installed_app_path"
}

check_path_configured() {
    case "$SHELL" in
        */zsh)
            SHELL_RC="$HOME/.zshrc"
            ;;
        */bash)
            if [[ -f "$HOME/.bash_profile" ]]; then
                SHELL_RC="$HOME/.bash_profile"
            else
                SHELL_RC="$HOME/.bashrc"
            fi
            ;;
        */fish)
            SHELL_RC="$HOME/.config/fish/config.fish"
            ;;
        *)
            SHELL_RC="$HOME/.profile"
            ;;
    esac

    [[ ":$PATH:" == *":$INSTALL_DIR:"* ]]
}

add_to_path() {
    local shell_rc="$1"
    local path_line=""

    case "$SHELL" in
        */fish)
            path_line="set -gx PATH \"$INSTALL_DIR\" \$PATH"
            ;;
        *)
            path_line="export PATH=\"$INSTALL_DIR:\$PATH\""
            ;;
    esac

    if [[ -f "$shell_rc" ]] && grep -q "$INSTALL_DIR" "$shell_rc" 2>/dev/null; then
        print_warning "PATH already configured in $shell_rc"
        return
    fi

    echo "" >> "$shell_rc"
    echo "# Bifrost" >> "$shell_rc"
    echo "$path_line" >> "$shell_rc"
    print_success "Added $INSTALL_DIR to PATH in $shell_rc"
}

if [[ "$INSTALL_CLI" == "true" ]]; then
    install_cli
fi

if [[ "$INSTALL_DESKTOP" == "true" ]]; then
    install_desktop
fi

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
print_success "Installation completed!"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

if [[ "$INSTALL_CLI" == "true" ]]; then
    echo "  bifrost:     $INSTALL_DIR/bifrost"
fi

if [[ "$INSTALL_DESKTOP" == "true" ]]; then
    echo "  desktop:     $APP_INSTALL_DIR/Bifrost.app"
fi

echo ""

if [[ "$INSTALL_CLI" == "true" ]]; then
    if ! check_path_configured; then
        print_warning "$INSTALL_DIR is not in your PATH"
        echo ""

        read -p "Would you like to add it to your shell configuration? [Y/n] " -n 1 -r
        echo ""

        if [[ $REPLY =~ ^[Yy]$ ]] || [[ -z $REPLY ]]; then
            add_to_path "$SHELL_RC"
            echo ""
            print_warning "Please restart your terminal or run:"
            echo "  source $SHELL_RC"
        else
            echo ""
            echo "To add manually, add this line to your shell configuration file:"
            echo ""
            case "$SHELL" in
                */fish)
                    echo "  set -gx PATH \"$INSTALL_DIR\" \$PATH"
                    ;;
                *)
                    echo "  export PATH=\"$INSTALL_DIR:\$PATH\""
                    ;;
            esac
        fi
    else
        print_success "$INSTALL_DIR is already in your PATH"
        echo ""
        echo "You can now run:"
        echo "  bifrost --help"
    fi
fi

if [[ "$INSTALL_DESKTOP" == "true" ]]; then
    echo ""
    echo "You can now launch:"
    echo "  open \"$APP_INSTALL_DIR/Bifrost.app\""
fi

echo ""
