#!/usr/bin/env bash
#
# Development environment initialization.
# Idempotent and fast -- run at the start of every session.
# Silent on success; verbose on failure.
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_ROOT"

NODE_DEPS_STAMP_FILE="$REPO_ROOT/node_modules/.simlin-dev-init-stamp"

errors=()

hash_file() {
    local file="$1"
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$file" | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$file" | awk '{print $1}'
    elif command -v openssl >/dev/null 2>&1; then
        openssl dgst -sha256 "$file" | awk '{print $NF}'
    else
        cksum "$file" | awk '{print $1 ":" $2}'
    fi
}

build_node_deps_stamp() {
    local lock_hash
    lock_hash="$(hash_file "$REPO_ROOT/pnpm-lock.yaml")"
    printf 'lock=%s node=%s pnpm=%s\n' "$lock_hash" "$NODE_VERSION" "$PNPM_VERSION"
}

# 1. Git hooks
if HOOK_PATH="$(git rev-parse --git-path hooks/pre-commit 2>/dev/null)"; then
    DESIRED="$REPO_ROOT/scripts/pre-commit"
    if ! { [ -L "$HOOK_PATH" ] && [ "$(readlink "$HOOK_PATH")" = "$DESIRED" ]; }; then
        mkdir -p "$(dirname "$HOOK_PATH")"
        rm -f "$HOOK_PATH"
        ln -s "$DESIRED" "$HOOK_PATH"
    fi
fi

# 2. Required tools
missing=()
command -v rustc >/dev/null 2>&1 || missing+=("rustc")
command -v cargo >/dev/null 2>&1 || missing+=("cargo")
command -v node  >/dev/null 2>&1 || missing+=("node")
command -v pnpm  >/dev/null 2>&1 || missing+=("pnpm")

if [ ${#missing[@]} -gt 0 ]; then
    errors+=("Missing required tools: ${missing[*]}")
    errors+=("  rustc/cargo: https://rustup.rs/")
    errors+=("  node:        https://nodejs.org/")
    errors+=("  pnpm:        npm install -g pnpm")
fi

# cbindgen (auto-install if cargo is available)
if ! command -v cbindgen >/dev/null 2>&1; then
    if command -v cargo >/dev/null 2>&1; then
        if ! cargo install cbindgen >/dev/null 2>&1; then
            errors+=("Failed to install cbindgen. Run 'cargo install cbindgen' manually.")
        fi
    fi
fi

# 3. pnpm install (skip if pnpm missing -- already reported above)
NODE_VERSION="$(node --version 2>/dev/null || echo missing)"
PNPM_VERSION="$(pnpm --version 2>/dev/null || echo missing)"

if command -v pnpm >/dev/null 2>&1; then
    CURRENT_STAMP="$(build_node_deps_stamp)"
    NEEDS_INSTALL=true

    if [ -d node_modules ] && [ -f "$NODE_DEPS_STAMP_FILE" ]; then
        SAVED="$(cat "$NODE_DEPS_STAMP_FILE" 2>/dev/null || true)"
        [ "$SAVED" = "$CURRENT_STAMP" ] && NEEDS_INSTALL=false
    fi

    if [ "$NEEDS_INSTALL" = true ]; then
        if pnpm install --frozen-lockfile --prefer-offline >/dev/null 2>&1; then
            mkdir -p node_modules
            printf '%s\n' "$CURRENT_STAMP" >"$NODE_DEPS_STAMP_FILE"
        else
            errors+=("pnpm install failed. Run 'pnpm install' for details.")
        fi
    fi
fi

# Report
if [ ${#errors[@]} -gt 0 ]; then
    printf 'dev-init: %d problem(s):\n' "${#errors[@]}" >&2
    for err in "${errors[@]}"; do
        printf '  %s\n' "$err" >&2
    done
    exit 1
fi
