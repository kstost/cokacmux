#!/bin/bash
#
# Normalize line endings across the repository.
# - Text files (.rs, .py, .md, .sh, ...): convert CRLF -> LF
# - Windows-only scripts (.ps1, .cmd, .bat): keep CRLF (do not touch)
# - Skipped: target/, .git/, dist_beta/, vendor/, builder/tools/
#
# Idempotent: re-running on already-clean files is a no-op.
#
# Usage:
#   bash tools/normalize_eol.sh           # convert and report
#   bash tools/normalize_eol.sh --check   # report only, do not write

set -euo pipefail

cd "$(dirname "$0")/.."

CHECK_ONLY=false
if [ "${1:-}" = "--check" ]; then
    CHECK_ONLY=true
fi

# Patterns to normalize to LF
LF_PATTERNS=(
    "*.rs"
    "*.toml"
    "*.lock"
    "*.md"
    "*.sh"
    "*.py"
    "*.json"
    "*.yml"
    "*.yaml"
    "*.txt"
    "*.cfg"
    "CNAME"
    "Makefile"
    ".gitignore"
    ".gitattributes"
    ".editorconfig"
)

# Directories to skip entirely
EXCLUDE_DIRS=(
    "./target"
    "./.git"
    "./dist_beta"
    "./vendor"
    "./builder/tools"
)

# Build find arguments
name_args=()
first=true
for pat in "${LF_PATTERNS[@]}"; do
    if $first; then
        name_args+=( -name "$pat" )
        first=false
    else
        name_args+=( -o -name "$pat" )
    fi
done

prune_args=()
first=true
for dir in "${EXCLUDE_DIRS[@]}"; do
    if $first; then
        prune_args+=( -path "$dir" )
        first=false
    else
        prune_args+=( -o -path "$dir" )
    fi
done

# Collect candidate files
mapfile -t files < <(
    find . \( "${prune_args[@]}" \) -prune -o \
        -type f \( "${name_args[@]}" \) -print
)

converted=0
already_lf=0
errors=0

for f in "${files[@]}"; do
    # Quick check: does file contain a CR?
    if ! grep -q $'\r' "$f" 2>/dev/null; then
        already_lf=$((already_lf + 1))
        continue
    fi

    if $CHECK_ONLY; then
        echo "CRLF: $f"
        converted=$((converted + 1))
        continue
    fi

    # Rewrite in place. Using a temp file + truncate-and-copy preserves
    # the original inode and mode (sed -i can fail on vmhgfs mounts that
    # do not allow preserving permissions on a renamed file).
    tmp=$(mktemp) || { errors=$((errors + 1)); continue; }
    tr -d '\r' < "$f" > "$tmp"
    if cat "$tmp" > "$f"; then
        converted=$((converted + 1))
        echo "fixed: $f"
    else
        errors=$((errors + 1))
        echo "ERROR: $f" >&2
    fi
    rm -f "$tmp"
done

echo ""
if $CHECK_ONLY; then
    echo "check mode: $converted files would be converted, $already_lf already LF, $errors errors"
else
    echo "done: $converted converted, $already_lf already LF, $errors errors"
fi
