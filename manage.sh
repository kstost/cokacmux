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
cokacdir_tmp=""
app_staged=""
cokacdir_staged=""
cleanup() {
    rm -f "$tmp" "$cokacdir_tmp"
    if [ -n "$app_staged" ]; then
        if declare -F target_remove >/dev/null 2>&1 && [ -n "${app_use_sudo:-}" ]; then
            target_remove "$app_staged" "$app_use_sudo" 2>/dev/null || true
        else
            rm -f "$app_staged"
        fi
    fi
    if [ -n "$cokacdir_staged" ]; then
        if declare -F target_remove >/dev/null 2>&1 && [ -n "${cokacdir_use_sudo:-}" ]; then
            target_remove "$cokacdir_staged" "$cokacdir_use_sudo" 2>/dev/null || true
        else
            rm -f "$cokacdir_staged"
        fi
    fi
}
trap cleanup EXIT
cokacdir_tmp="$(mktemp)"

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

validate_binary() {
    name="$1"
    path="$2"
    if ! version_output="$("$path" --version 2>&1)"; then
        echo "$name download is not a runnable binary for this platform" >&2
        return 1
    fi
    case "$version_output" in
        "$name "*) ;;
        *) echo "$name download returned an unexpected version: $version_output" >&2; return 1 ;;
    esac
}

# Validate both downloads before replacing either installed program.  A 200
# response containing an HTML error page must never destroy a working install.
chmod 0700 "$tmp" "$cokacdir_tmp"
validate_binary "$app" "$tmp" || exit 1
validate_binary "$cokacdir_app" "$cokacdir_tmp" || exit 1

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

target_uses_sudo() {
    target_dir="$1"
    if [ -w "$target_dir" ]; then
        printf '0\n'
    elif command -v sudo >/dev/null 2>&1; then
        printf '1\n'
    else
        echo "Cannot write to $target_dir" >&2
        return 1
    fi
}

target_exists() {
    path="$1"
    use_sudo="$2"
    if [ "$use_sudo" -eq 1 ]; then
        sudo test -e "$path" || sudo test -L "$path"
    else
        [ -e "$path" ] || [ -L "$path" ]
    fi
}

target_is_replaceable_file() {
    path="$1"
    use_sudo="$2"
    if [ "$use_sudo" -eq 1 ]; then
        sudo test -f "$path" || sudo test -L "$path"
    else
        [ -f "$path" ] || [ -L "$path" ]
    fi
}

target_remove() {
    path="$1"
    use_sudo="$2"
    if [ "$use_sudo" -eq 1 ]; then
        sudo rm -f "$path"
    else
        rm -f "$path"
    fi
}

target_move() {
    source_path="$1"
    target_path="$2"
    use_sudo="$3"
    if [ "$use_sudo" -eq 1 ]; then
        sudo mv -f "$source_path" "$target_path"
    else
        mv -f "$source_path" "$target_path"
    fi
}

stage_binary() {
    src="$1"
    target="$2"
    use_sudo="$3"
    target_dir="$(dirname "$target")"
    template="$target_dir/.$(basename "$target").XXXXXX"
    staged=""

    if [ "$use_sudo" -eq 0 ]; then
        staged="$(mktemp "$template")"
        if ! install -m 0755 "$src" "$staged"; then
            rm -f "$staged"
            return 1
        fi
    else
        staged="$(sudo mktemp "$template")"
        if ! sudo install -m 0755 "$src" "$staged"; then
            sudo rm -f "$staged" 2>/dev/null || true
            return 1
        fi
    fi
    printf '%s\n' "$staged"
}

reserve_backup_path() {
    target="$1"
    use_sudo="$2"
    target_dir="$(dirname "$target")"
    template="$target_dir/.$(basename "$target").backup.XXXXXX"
    if [ "$use_sudo" -eq 1 ]; then
        backup="$(sudo mktemp "$template")" || return 1
        sudo rm -f "$backup" || return 1
    else
        backup="$(mktemp "$template")" || return 1
        rm -f "$backup" || return 1
    fi
    printf '%s\n' "$backup"
}

rollback_installed_pair() {
    rollback_failed=0
    if [ "$app_backup_done" -eq 1 ]; then
        target_remove "$dest" "$app_use_sudo" || rollback_failed=1
        if [ "$app_existed" -eq 1 ]; then
            if ! target_move "$app_backup" "$dest" "$app_use_sudo"; then
                echo "Failed to restore $dest from $app_backup" >&2
                rollback_failed=1
            else
                app_backup=""
            fi
        fi
    fi
    if [ "$cokacdir_backup_done" -eq 1 ]; then
        target_remove "$cokacdir_dest" "$cokacdir_use_sudo" || rollback_failed=1
        if [ "$cokacdir_existed" -eq 1 ]; then
            if ! target_move "$cokacdir_backup" "$cokacdir_dest" "$cokacdir_use_sudo"; then
                echo "Failed to restore $cokacdir_dest from $cokacdir_backup" >&2
                rollback_failed=1
            else
                cokacdir_backup=""
            fi
        fi
    fi
    return "$rollback_failed"
}

app_use_sudo="$(target_uses_sudo "$(dirname "$dest")")" || exit 1
cokacdir_use_sudo="$(target_uses_sudo "$(dirname "$cokacdir_dest")")" || exit 1

if target_exists "$dest" "$app_use_sudo" && ! target_is_replaceable_file "$dest" "$app_use_sudo"; then
    echo "Refusing to replace non-file destination: $dest" >&2
    exit 1
fi
if target_exists "$cokacdir_dest" "$cokacdir_use_sudo" && ! target_is_replaceable_file "$cokacdir_dest" "$cokacdir_use_sudo"; then
    echo "Refusing to replace non-file destination: $cokacdir_dest" >&2
    exit 1
fi

# Stage both programs in their destination filesystems before moving either
# installed program. This makes every subsequent rename same-filesystem.
app_staged="$(stage_binary "$tmp" "$dest" "$app_use_sudo")" || {
    echo "Failed to stage $dest" >&2
    exit 1
}
cokacdir_staged="$(stage_binary "$cokacdir_tmp" "$cokacdir_dest" "$cokacdir_use_sudo")" || {
    target_remove "$app_staged" "$app_use_sudo" 2>/dev/null || true
    app_staged=""
    echo "Failed to stage $cokacdir_dest" >&2
    exit 1
}

app_existed=0
cokacdir_existed=0
app_backup_done=0
cokacdir_backup_done=0
app_backup=""
cokacdir_backup=""

if target_exists "$cokacdir_dest" "$cokacdir_use_sudo"; then
    cokacdir_existed=1
    cokacdir_backup="$(reserve_backup_path "$cokacdir_dest" "$cokacdir_use_sudo")" || exit 1
    if ! target_move "$cokacdir_dest" "$cokacdir_backup" "$cokacdir_use_sudo"; then
        echo "Failed to back up $cokacdir_dest" >&2
        exit 1
    fi
fi
cokacdir_backup_done=1
if ! target_move "$cokacdir_staged" "$cokacdir_dest" "$cokacdir_use_sudo"; then
    echo "Failed to install $cokacdir_dest" >&2
    rollback_installed_pair || true
    exit 1
fi
cokacdir_staged=""

if target_exists "$dest" "$app_use_sudo"; then
    app_existed=1
    app_backup="$(reserve_backup_path "$dest" "$app_use_sudo")" || {
        rollback_installed_pair || true
        exit 1
    }
    if ! target_move "$dest" "$app_backup" "$app_use_sudo"; then
        echo "Failed to back up $dest" >&2
        rollback_installed_pair || true
        exit 1
    fi
fi
app_backup_done=1
if ! target_move "$app_staged" "$dest" "$app_use_sudo"; then
    echo "Failed to install $dest" >&2
    rollback_installed_pair || true
    exit 1
fi
app_staged=""

if ! validate_binary "$app" "$dest" || ! validate_binary "$cokacdir_app" "$cokacdir_dest"; then
    echo "Installed pair validation failed; restoring previous versions" >&2
    rollback_installed_pair || true
    exit 1
fi

if [ -n "$app_backup" ]; then
    if target_remove "$app_backup" "$app_use_sudo"; then
        app_backup=""
    else
        echo "Installed pair is valid, but old app backup remains at $app_backup" >&2
    fi
fi
if [ -n "$cokacdir_backup" ]; then
    if target_remove "$cokacdir_backup" "$cokacdir_use_sudo"; then
        cokacdir_backup=""
    else
        echo "Installed pair is valid, but old helper backup remains at $cokacdir_backup" >&2
    fi
fi


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
