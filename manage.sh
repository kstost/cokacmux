#!/bin/bash
# Usage: curl -fsSL https://cokacmux.cokac.com/manage.sh | bash

set -e

app="cokacmux"
base="${COKACMUX_BASE_URL:-https://raw.githubusercontent.com/kstost/cokacmux/refs/heads/main/dist_beta}"

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

tmp="$(mktemp)"
trap "rm -f '$tmp'" EXIT

url="$base/$app-$os-$arch"
echo "Downloading $app ($os-$arch)..."
if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$url" -o "$tmp"
elif command -v wget >/dev/null 2>&1; then
    wget -q "$url" -O "$tmp"
else
    echo "curl or wget is required" >&2
    exit 1
fi

[ -s "$tmp" ] || { echo "Download produced an empty file" >&2; exit 1; }

if [ -n "${COKACMUX_INSTALL_DIR:-}" ]; then
    dir="$COKACMUX_INSTALL_DIR"
elif [ -d /usr/local/bin ] && { [ -w /usr/local/bin ] || command -v sudo >/dev/null 2>&1; }; then
    dir="/usr/local/bin"
else
    dir="$HOME/.local/bin"
fi

mkdir -p "$dir" 2>/dev/null || true
dest="$dir/$app"

if [ -w "$dir" ]; then
    install -m 0755 "$tmp" "$dest"
elif command -v sudo >/dev/null 2>&1; then
    sudo install -m 0755 "$tmp" "$dest"
else
    echo "Cannot write to $dir" >&2
    exit 1
fi

"$dest" --version >/dev/null 2>&1 || { echo "Installed file did not run" >&2; exit 1; }

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

echo "Installed to $dest"
echo "Run 'cokacmux' to start."
