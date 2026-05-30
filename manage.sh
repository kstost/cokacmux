#!/bin/bash
#
# COKACMUX installer and updater
# Usage: curl -fsSL https://raw.githubusercontent.com/kstost/cokacmux/refs/heads/main/manage.sh | bash
#

set -euo pipefail

BINARY_NAME="cokacmux"
BASE_URL="${COKACMUX_BASE_URL:-https://raw.githubusercontent.com/kstost/cokacmux/refs/heads/main/dist_beta}"
SYSTEM_INSTALL_DIR="/usr/local/bin"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

info() {
    printf '%b→%b %s\n' "$BLUE" "$NC" "$1"
}

success() {
    printf '%b✓%b %s\n' "$GREEN" "$NC" "$1"
}

warn() {
    printf '%b!%b %s\n' "$YELLOW" "$NC" "$1" >&2
}

error() {
    printf '%b✗%b %s\n' "$RED" "$NC" "$1" >&2
    exit 1
}

usage() {
    cat <<EOF
cokacmux installer

Usage:
  manage.sh [install|update]
  manage.sh uninstall
  manage.sh --help

Examples:
  curl -fsSL https://cokacmux.cokac.com/manage.sh | bash
  curl -fsSL https://cokacmux.cokac.com/manage.sh | bash -s -- uninstall
  COKACMUX_INSTALL_DIR="\$HOME/bin" ./manage.sh

Environment:
  COKACMUX_INSTALL_DIR       Install directory override
  COKACMUX_BASE_URL          Download base URL override
  COKACMUX_REQUIRE_CHECKSUM  Set to 1 to fail when .sha256 is unavailable
EOF
}

require_home() {
    if [ -z "${HOME:-}" ]; then
        error "HOME is not set"
    fi
}

user_install_dir() {
    require_home
    printf '%s\n' "$HOME/.local/bin"
}

# Detect OS
detect_os() {
    local os
    os="$(uname -s)"
    case "$os" in
        Linux*)  echo "linux" ;;
        Darwin*) echo "macos" ;;
        *)       error "Unsupported OS: $os" ;;
    esac
}

# Detect architecture
detect_arch() {
    local arch
    arch="$(uname -m)"
    case "$arch" in
        x86_64|amd64)  echo "x86_64" ;;
        aarch64|arm64) echo "aarch64" ;;
        *)             error "Unsupported architecture: $arch" ;;
    esac
}

root_group() {
    case "$(detect_os)" in
        macos) echo "wheel" ;;
        *)     echo "root" ;;
    esac
}

# Check if command exists
has_cmd() {
    command -v "$1" >/dev/null 2>&1
}

# Download file
download() {
    local url="$1"
    local dest="$2"

    if has_cmd curl; then
        curl -fsSL "$url" -o "$dest"
    elif has_cmd wget; then
        wget -q "$url" -O "$dest"
    else
        error "curl or wget is required"
    fi
}

checksum_of() {
    local file="$1"

    if has_cmd sha256sum; then
        sha256sum "$file" | awk '{print $1}'
    elif has_cmd shasum; then
        shasum -a 256 "$file" | awk '{print $1}'
    else
        return 1
    fi
}

verify_checksum() {
    local url="$1"
    local file="$2"
    local checksum_url="${url}.sha256"
    local checksum_file
    checksum_file="$(mktemp)"

    if ! download "$checksum_url" "$checksum_file" 2>/dev/null; then
        rm -f "$checksum_file"
        if [ "${COKACMUX_REQUIRE_CHECKSUM:-0}" = "1" ]; then
            error "Checksum file is unavailable: $checksum_url"
        fi
        warn "Checksum file is unavailable; continuing without checksum verification."
        return 0
    fi

    local expected actual
    expected="$(awk '{print $1; exit}' "$checksum_file")"
    rm -f "$checksum_file"

    if [ -z "$expected" ]; then
        error "Checksum file is empty: $checksum_url"
    fi

    if ! actual="$(checksum_of "$file")"; then
        if [ "${COKACMUX_REQUIRE_CHECKSUM:-0}" = "1" ]; then
            error "sha256sum or shasum is required for checksum verification"
        fi
        warn "sha256sum/shasum not found; continuing without checksum verification."
        return 0
    fi

    if [ "$expected" != "$actual" ]; then
        error "Checksum mismatch for downloaded binary"
    fi

    success "Checksum verified."
}

# Get preferred install directory
get_install_dir() {
    if [ -n "${COKACMUX_INSTALL_DIR:-}" ]; then
        mkdir -p "$COKACMUX_INSTALL_DIR"
        echo "$COKACMUX_INSTALL_DIR"
        return
    fi

    if [ -d "$SYSTEM_INSTALL_DIR" ]; then
        if [ -w "$SYSTEM_INSTALL_DIR" ] || has_cmd sudo; then
            echo "$SYSTEM_INSTALL_DIR"
            return
        fi
        warn "$SYSTEM_INSTALL_DIR is not writable and sudo is unavailable; using $(user_install_dir)."
    fi

    mkdir -p "$(user_install_dir)"
    user_install_dir
}

install_file_plain() {
    local src="$1"
    local dest="$2"

    if has_cmd install; then
        install -m 0755 "$src" "$dest"
    else
        cp "$src" "$dest"
        chmod 0755 "$dest"
    fi
}

install_file_sudo() {
    local src="$1"
    local dest="$2"
    local group
    group="$(root_group)"

    if ! has_cmd sudo; then
        return 1
    fi

    if has_cmd install; then
        sudo install -m 0755 -o root -g "$group" "$src" "$dest"
    else
        sudo cp "$src" "$dest"
        sudo chown "root:$group" "$dest"
        sudo chmod 0755 "$dest"
    fi
}

install_file_to_dir() {
    local src="$1"
    local install_dir="$2"
    local dest="${install_dir}/${BINARY_NAME}"

    if [ ! -d "$install_dir" ]; then
        mkdir -p "$install_dir" || return 1
    fi

    if [ -w "$install_dir" ]; then
        install_file_plain "$src" "$dest"
    else
        install_file_sudo "$src" "$dest"
    fi
}

INSTALL_DIR=""
INSTALL_PATH=""
TMPFILE=""

install_binary() {
    local src="$1"
    local preferred_dir
    preferred_dir="$(get_install_dir)"

    if install_file_to_dir "$src" "$preferred_dir"; then
        INSTALL_DIR="$preferred_dir"
        INSTALL_PATH="${preferred_dir}/${BINARY_NAME}"
        return 0
    fi

    local fallback_dir
    fallback_dir="$(user_install_dir)"

    if [ -z "${COKACMUX_INSTALL_DIR:-}" ] && [ "$preferred_dir" != "$fallback_dir" ]; then
        warn "Could not install to $preferred_dir; trying $fallback_dir instead."
        mkdir -p "$fallback_dir"
        if install_file_to_dir "$src" "$fallback_dir"; then
            INSTALL_DIR="$fallback_dir"
            INSTALL_PATH="${fallback_dir}/${BINARY_NAME}"
            return 0
        fi
    fi

    return 1
}

# Get shell config file
get_shell_config() {
    local shell_path="${SHELL:-}"
    local shell_name

    if [ -z "$shell_path" ]; then
        echo ""
        return
    fi

    shell_name="$(basename "$shell_path")"

    case "$shell_name" in
        bash)
            if [ "$(detect_os)" = "macos" ]; then
                echo "$HOME/.bash_profile"
            elif [ -f "$HOME/.bashrc" ]; then
                echo "$HOME/.bashrc"
            elif [ -f "$HOME/.bash_profile" ]; then
                echo "$HOME/.bash_profile"
            else
                echo "$HOME/.bashrc"
            fi
            ;;
        zsh)
            echo "$HOME/.zshrc"
            ;;
        *)
            echo ""
            ;;
    esac
}

path_contains() {
    local dir="$1"

    case ":${PATH:-}:" in
        *":$dir:"*) return 0 ;;
        *)          return 1 ;;
    esac
}

# Add PATH export to shell config when fallback dir is used
setup_path() {
    local install_dir="$1"

    # Only needed for the fallback dir; /usr/local/bin is normally already in PATH.
    if [ "$install_dir" != "$(user_install_dir)" ]; then
        return 0
    fi

    local config_file
    config_file="$(get_shell_config)"

    if [ -z "$config_file" ]; then
        warn "Could not detect shell config file; add $install_dir to PATH manually."
        return 1
    fi

    if ! touch "$config_file" 2>/dev/null; then
        warn "Could not update $config_file; add $install_dir to PATH manually."
        return 1
    fi

    if grep -Fq "# cokacmux PATH (added by installer)" "$config_file"; then
        return 0
    fi

    if {
        echo ""
        echo "# cokacmux PATH (added by installer)"
        echo 'case ":$PATH:" in'
        echo '    *":$HOME/.local/bin:"*) ;;'
        echo '    *) export PATH="$HOME/.local/bin:$PATH" ;;'
        echo 'esac'
    } >> "$config_file"; then
        success "Added $install_dir to PATH in $config_file."
    else
        warn "Could not update $config_file; add $install_dir to PATH manually."
        return 1
    fi
}

verify_installed() {
    local install_path="$1"

    if [ ! -x "$install_path" ]; then
        error "Installation failed: $install_path is not executable"
    fi

    if ! "$install_path" --version >/dev/null 2>&1; then
        error "Installation failed: '$install_path --version' did not run successfully"
    fi
}

install_main() {
    require_home

    # Detect platform
    local os arch
    os="$(detect_os)"
    arch="$(detect_arch)"

    info "Downloading cokacmux ($os-$arch)..."

    # Build download URL
    local filename="${BINARY_NAME}-${os}-${arch}"
    local url="${BASE_URL}/${filename}"

    # Create temp file
    TMPFILE="$(mktemp)"
    trap 'rm -f "${TMPFILE:-}"' EXIT

    # Download
    if ! download "$url" "$TMPFILE"; then
        error "Download failed: $url"
    fi

    if [ ! -s "$TMPFILE" ]; then
        error "Download produced an empty file: $url"
    fi

    verify_checksum "$url" "$TMPFILE"

    # Install
    if ! install_binary "$TMPFILE"; then
        error "Installation failed"
    fi

    verify_installed "$INSTALL_PATH"

    # Add PATH to shell config if installed under fallback dir. PATH setup
    # failure should not turn a completed binary install into a failed install.
    setup_path "$INSTALL_DIR" || true

    success "Installed to $INSTALL_PATH"

    # Current shell can't have its PATH mutated from a child process; if the
    # fallback dir isn't in this shell's PATH, the user needs a fresh shell.
    if [ "$INSTALL_DIR" = "$(user_install_dir)" ] && ! path_contains "$INSTALL_DIR"; then
        local config_file
        config_file="$(get_shell_config)"
        if [ -n "$config_file" ]; then
            warn "Open a new terminal (or run: source $config_file) to apply PATH."
        else
            warn "Open a new terminal to apply PATH."
        fi
    fi

    success "Run 'cokacmux' to start."
}

remove_file() {
    local path="$1"

    if [ ! -e "$path" ]; then
        return 1
    fi

    if [ -w "$(dirname "$path")" ]; then
        rm -f "$path"
    elif has_cmd sudo; then
        sudo rm -f "$path"
    else
        return 1
    fi
}

uninstall_main() {
    require_home

    local removed=0
    local dirs

    if [ -n "${COKACMUX_INSTALL_DIR:-}" ]; then
        dirs=("$COKACMUX_INSTALL_DIR")
    else
        dirs=("$SYSTEM_INSTALL_DIR" "$(user_install_dir)")
    fi

    local dir path
    for dir in "${dirs[@]}"; do
        path="${dir}/${BINARY_NAME}"
        if remove_file "$path"; then
            success "Removed $path"
            removed=1
        fi
    done

    if [ "$removed" -eq 0 ]; then
        warn "No cokacmux binary found in the usual install directories."
    fi

    warn "Settings and session data under $HOME/.cokacmux were not removed."
}

main() {
    local command="${1:-install}"

    case "$command" in
        install|update)
            install_main
            ;;
        uninstall|remove)
            uninstall_main
            ;;
        -h|--help|help)
            usage
            ;;
        *)
            usage
            error "Unknown command: $command"
            ;;
    esac
}

main "$@"
