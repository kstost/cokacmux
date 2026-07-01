#!/bin/bash
# Usage: curl -fsSL https://cokacmux.cokac.com/manage.sh | bash

set -e

app="cokacmux"
base="${COKACMUX_BASE_URL:-https://raw.githubusercontent.com/kstost/cokacmux/refs/heads/main/dist_beta}"
cokacdir_app="cokacdir"
cokacdir_base="${COKACDIR_BASE_URL:-https://raw.githubusercontent.com/kstost/cokacdir/main/dist}"

case "${1:-install}" in
    install|update) ;;
    -h|--help|help) echo "Usage: manage.sh [install|update]"; exit 0 ;;
    *) echo "Only install/update is supported by this installer." >&2; exit 1 ;;
esac

case "$(uname -s)" in
    Linux*) os="linux" ;;
    Darwin*) os="macos" ;;
    *) echo "Unsupported OS: $(uname -s)" >&2; exit 1 ;;
esac

case "$(uname -m)" in
    x86_64|amd64) arch="x86_64" ;;
    aarch64|arm64) arch="aarch64" ;;
    *) echo "Unsupported architecture: $(uname -m)" >&2; exit 1 ;;
esac

if [ -z "${HOME:-}" ]; then
    echo "HOME is not set. Cannot choose cokacdir install directory." >&2
    exit 1
fi

tmp="$(mktemp)"
cokacdir_tmp="$(mktemp)"
trap "rm -f '$tmp' '$cokacdir_tmp'" EXIT

download_binary() {
    name="$1"
    url="$2"
    output="$3"

    echo "Downloading $name..."
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL "$url" -o "$output"
    elif command -v wget >/dev/null 2>&1; then
        wget -q "$url" -O "$output"
    else
        echo "curl or wget is required" >&2
        exit 1
    fi

    [ -s "$output" ] || { echo "$name download produced an empty file" >&2; exit 1; }
}

url="$base/$app-$os-$arch"
cokacdir_url="$cokacdir_base/$cokacdir_app-$os-$arch"
download_binary "$app ($os-$arch)" "$url" "$tmp"
download_binary "$cokacdir_app ($os-$arch)" "$cokacdir_url" "$cokacdir_tmp"

if [ -n "${COKACMUX_INSTALL_DIR:-}" ]; then
    dir="$COKACMUX_INSTALL_DIR"
elif [ -d /usr/local/bin ] && { [ -w /usr/local/bin ] || command -v sudo >/dev/null 2>&1; }; then
    dir="/usr/local/bin"
else
    dir="$HOME/.local/bin"
fi

ensure_install_dir() {
    target_dir="$1"
    if [ -d "$target_dir" ]; then
        return 0
    fi
    if mkdir -p "$target_dir" 2>/dev/null; then
        return 0
    fi
    if command -v sudo >/dev/null 2>&1; then
        sudo mkdir -p "$target_dir"
        return 0
    fi
    echo "Cannot create $target_dir" >&2
    exit 1
}

ensure_install_dir "$dir"

cokacdir_dir="$HOME/.cokacmux/bin"
mkdir -p "$cokacdir_dir"
chmod 700 "$cokacdir_dir" 2>/dev/null || true

dest="$dir/$app"
cokacdir_dest="$cokacdir_dir/$cokacdir_app"

install_binary() {
    src="$1"
    target="$2"
    target_dir="$(dirname "$target")"

    if [ -w "$target_dir" ]; then
        install -m 0755 "$src" "$target"
    elif command -v sudo >/dev/null 2>&1; then
        sudo install -m 0755 "$src" "$target"
    else
        echo "Cannot write to $target_dir" >&2
        exit 1
    fi
}

install_binary "$tmp" "$dest"
install_binary "$cokacdir_tmp" "$cokacdir_dest"

"$dest" --version >/dev/null 2>&1 || { echo "Installed file did not run" >&2; exit 1; }
[ -x "$cokacdir_dest" ] || { echo "Installed cokacdir is not executable" >&2; exit 1; }

if [ "$dir" = "$HOME/.local/bin" ]; then
    rc=""
    case "$(basename "${SHELL:-}")" in
        zsh) rc="$HOME/.zshrc" ;;
        bash) [ "$(uname -s)" = "Darwin" ] && rc="$HOME/.bash_profile" || rc="$HOME/.bashrc" ;;
    esac
    if [ -n "$rc" ]; then
        touch "$rc" 2>/dev/null || true
        grep -Fq 'export PATH="$HOME/.local/bin:$PATH"' "$rc" 2>/dev/null || {
            printf '\n# cokacmux\nexport PATH="$HOME/.local/bin:$PATH"\n' >> "$rc" || true
        }
    fi
    case ":$PATH:" in *":$dir:"*) ;; *) echo "Open a new terminal so PATH changes take effect." ;; esac
fi

echo "Installed $app to $dest"
echo "Installed $cokacdir_app to $cokacdir_dest"
echo "Run 'cokacmux' to start."
