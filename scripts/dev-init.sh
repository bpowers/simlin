#!/usr/bin/env bash
#
# Development environment initialization script.
# Sets up git hooks, checks toolchain dependencies, installs pnpm
# packages, and configures AI tools for the pre-commit hook.
#
# This script is idempotent and fast -- run it at the start of every
# session (agent or human) to ensure the environment is ready.
#
set -euo pipefail

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Determine repository root
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

cd "$REPO_ROOT"

AI_CONFIG_FILE="$REPO_ROOT/.ai-tool-config"
NODE_DEPS_STAMP_FILE="$REPO_ROOT/node_modules/.simlin-dev-init-stamp"
INSTALL_CODEX="${SIMLIN_INIT_INSTALL_CODEX:-0}"
PROBE_AI_TOOLS="${SIMLIN_INIT_AI_PROBE:-1}"
REFRESH_AI_CONFIG="${SIMLIN_INIT_REFRESH_AI_CONFIG:-0}"

run_with_timeout() {
    local timeout_secs="$1"
    shift

    if command -v timeout >/dev/null 2>&1; then
        timeout -k 2 "$timeout_secs" "$@"
    elif command -v gtimeout >/dev/null 2>&1; then
        gtimeout -k 2 "$timeout_secs" "$@"
    else
        "$@"
    fi
}

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

is_valid_ai_tool() {
    case "$1" in
        claude|codex|none)
            return 0
            ;;
        *)
            return 1
            ;;
    esac
}

codex_is_usable() {
    if ! command -v codex >/dev/null 2>&1; then
        return 1
    fi

    if [ -n "${OPENAI_API_KEY:-}" ]; then
        return 0
    fi

    codex login status >/dev/null 2>&1
}

ai_tool_is_usable() {
    local tool="$1"

    case "$tool" in
        claude)
            command -v claude >/dev/null 2>&1
            ;;
        codex)
            codex_is_usable
            ;;
        none)
            return 0
            ;;
        *)
            return 1
            ;;
    esac
}

probe_claude() {
    local output
    output="$(mktemp)"

    if run_with_timeout 6 claude -p "respond with the single word: yes" >"$output" 2>&1; then
        if grep -qi "yes" "$output"; then
            rm -f "$output"
            return 0
        fi
    fi

    rm -f "$output"
    return 1
}

probe_codex() {
    local output
    output="$(mktemp)"

    if run_with_timeout 12 codex exec -m gpt-5.3-codex -c 'model_reasoning_effort="high"' "respond with the single word: yes" >"$output" 2>&1; then
        if grep -qi "yes" "$output"; then
            rm -f "$output"
            return 0
        fi
    fi

    rm -f "$output"
    return 1
}

build_node_deps_stamp() {
    local lock_hash
    lock_hash="$(hash_file "$REPO_ROOT/pnpm-lock.yaml")"
    printf 'lock=%s node=%s pnpm=%s\n' "$lock_hash" "${NODE_VERSION:-missing}" "${PNPM_VERSION:-missing}"
}

echo "Setting up Simlin development environment..."
echo ""

# 1. Install git hooks
echo -n "Installing git hooks... "
if HOOK_PATH="$(git rev-parse --git-path hooks/pre-commit 2>/dev/null)"; then
    HOOK_DIR="$(dirname "$HOOK_PATH")"
    DESIRED_HOOK_TARGET="$REPO_ROOT/scripts/pre-commit"

    mkdir -p "$HOOK_DIR"

    if [ -L "$HOOK_PATH" ] && [ "$(readlink "$HOOK_PATH")" = "$DESIRED_HOOK_TARGET" ]; then
        echo -e "${GREEN}already installed${NC}"
    else
        rm -f "$HOOK_PATH"
        ln -s "$DESIRED_HOOK_TARGET" "$HOOK_PATH"
        echo -e "${GREEN}done${NC}"
    fi
else
    echo -e "${YELLOW}skipped (not in a git checkout)${NC}"
fi

# 2. Check for required tools
echo ""
echo "Checking required tools..."

# Check Rust
echo -n "  Rust: "
if command -v rustc >/dev/null 2>&1; then
    RUST_VERSION="$(rustc --version 2>/dev/null | cut -d' ' -f2)"
    echo -e "${GREEN}$RUST_VERSION${NC}"
else
    echo -e "${RED}not found${NC}"
    echo -e "    ${YELLOW}Install Rust: https://rustup.rs/${NC}"
fi

# Check cargo
echo -n "  Cargo: "
if command -v cargo >/dev/null 2>&1; then
    CARGO_VERSION="$(cargo --version 2>/dev/null | cut -d' ' -f2)"
    echo -e "${GREEN}$CARGO_VERSION${NC}"
else
    echo -e "${RED}not found${NC}"
fi

# Check Node.js
NODE_VERSION="missing"
echo -n "  Node.js: "
if command -v node >/dev/null 2>&1; then
    NODE_VERSION="$(node --version 2>/dev/null)"
    echo -e "${GREEN}$NODE_VERSION${NC}"
else
    echo -e "${RED}not found${NC}"
    echo -e "    ${YELLOW}Install Node.js: https://nodejs.org/${NC}"
fi

# Check pnpm
PNPM_VERSION="missing"
echo -n "  pnpm: "
if command -v pnpm >/dev/null 2>&1; then
    PNPM_VERSION="$(pnpm --version 2>/dev/null)"
    echo -e "${GREEN}$PNPM_VERSION${NC}"
else
    echo -e "${RED}not found${NC}"
    echo -e "    ${YELLOW}Install pnpm: npm install -g pnpm${NC}"
fi

# Check cbindgen (for simlin.h generation)
echo -n "  cbindgen: "
if command -v cbindgen >/dev/null 2>&1; then
    CBINDGEN_VERSION="$(cbindgen --version 2>/dev/null | cut -d' ' -f2)"
    echo -e "${GREEN}$CBINDGEN_VERSION${NC}"
else
    echo -n "installing... "
    if cargo install cbindgen >/dev/null 2>&1; then
        CBINDGEN_VERSION="$(cbindgen --version 2>/dev/null | cut -d' ' -f2)"
        echo -e "${GREEN}$CBINDGEN_VERSION${NC}"
    else
        echo -e "${RED}failed to install${NC}"
        echo -e "    ${YELLOW}Run 'cargo install cbindgen' manually${NC}"
    fi
fi

# Check Python (optional, for pysimlin)
echo -n "  Python: "
if command -v python3 >/dev/null 2>&1; then
    PY_VERSION="$(python3 --version 2>/dev/null | cut -d' ' -f2)"
    echo -e "${GREEN}$PY_VERSION${NC}"
    # Check if Python is 3.11+
    PY_MAJOR="$(echo "$PY_VERSION" | cut -d. -f1)"
    PY_MINOR="$(echo "$PY_VERSION" | cut -d. -f2)"
    if [ "$PY_MAJOR" -gt 3 ] || { [ "$PY_MAJOR" -eq 3 ] && [ "$PY_MINOR" -ge 11 ]; }; then
        echo -e "    ${GREEN}✓${NC} Python 3.11+ available for pysimlin tests"
    else
        echo -e "    ${YELLOW}!${NC} Python 3.11+ required for pysimlin tests (found $PY_VERSION)"
    fi
else
    echo -e "${YELLOW}not found (optional, needed for pysimlin)${NC}"
fi

# 3. Install pnpm dependencies only when lockfile/runtime changed
echo ""
if command -v pnpm >/dev/null 2>&1; then
    CURRENT_NODE_DEPS_STAMP="$(build_node_deps_stamp)"
    NEEDS_PNPM_INSTALL=true

    if [ -d "node_modules" ] && [ -f "$NODE_DEPS_STAMP_FILE" ]; then
        SAVED_NODE_DEPS_STAMP="$(cat "$NODE_DEPS_STAMP_FILE" 2>/dev/null || true)"
        if [ "$SAVED_NODE_DEPS_STAMP" = "$CURRENT_NODE_DEPS_STAMP" ]; then
            NEEDS_PNPM_INSTALL=false
        fi
    fi

    if [ "$NEEDS_PNPM_INSTALL" = "true" ]; then
        echo -n "Installing pnpm dependencies... "
        if pnpm install --frozen-lockfile --prefer-offline >/dev/null 2>&1; then
            mkdir -p node_modules
            printf '%s\n' "$CURRENT_NODE_DEPS_STAMP" >"$NODE_DEPS_STAMP_FILE"
            echo -e "${GREEN}done${NC}"
        else
            echo -e "${YELLOW}failed (non-critical)${NC}"
            echo -e "    Try running: pnpm install"
        fi
    else
        echo -e "pnpm dependencies: ${GREEN}already up to date${NC}"
    fi
else
    echo -e "pnpm dependencies: ${YELLOW}skipped (pnpm not available)${NC}"
fi

# 4. Install and configure AI tools for pre-commit hook
echo ""
echo "Setting up AI tools for pre-commit hook..."

echo -n "  Checking @openai/codex... "
if command -v codex >/dev/null 2>&1; then
    CODEX_VERSION="$(codex --version 2>/dev/null | head -1)"
    echo -e "${GREEN}already installed ($CODEX_VERSION)${NC}"
elif [ "$INSTALL_CODEX" = "1" ]; then
    if npm install -g @openai/codex >/dev/null 2>&1; then
        echo -e "${GREEN}installed${NC}"
    else
        echo -e "${YELLOW}failed (non-critical)${NC}"
    fi
else
    echo -e "${YELLOW}skipped (set SIMLIN_INIT_INSTALL_CODEX=1 to install globally)${NC}"
fi

# Login to codex with API key if available and not already authenticated.
if command -v codex >/dev/null 2>&1 && [ -n "${OPENAI_API_KEY:-}" ]; then
    if codex login status >/dev/null 2>&1; then
        echo -e "  Codex auth: ${GREEN}already configured${NC}"
    else
        echo -n "  Configuring codex with API key... "
        if printenv OPENAI_API_KEY | codex login --with-api-key >/dev/null 2>&1; then
            echo -e "${GREEN}done${NC}"
        else
            echo -e "${YELLOW}failed${NC}"
        fi
    fi
fi

# Select AI tool for pre-commit hook, preferring fast local checks and cache reuse.
echo -n "  Selecting AI tool for pre-commit... "
SELECTED_AI_TOOL=""
SELECTION_REASON=""
AI_TOOL_OVERRIDE="${AI_TOOL:-}"

if [ -n "$AI_TOOL_OVERRIDE" ]; then
    if is_valid_ai_tool "$AI_TOOL_OVERRIDE"; then
        SELECTED_AI_TOOL="$AI_TOOL_OVERRIDE"
        SELECTION_REASON="from AI_TOOL override"
    else
        echo -e "${YELLOW}invalid AI_TOOL='$AI_TOOL_OVERRIDE'; expected claude|codex|none${NC}"
    fi
fi

if [ -z "$SELECTED_AI_TOOL" ] && [ "$REFRESH_AI_CONFIG" != "1" ] && [ -f "$AI_CONFIG_FILE" ]; then
    CACHED_AI_TOOL="$(tr -d '[:space:]' < "$AI_CONFIG_FILE")"
    if is_valid_ai_tool "$CACHED_AI_TOOL" && ai_tool_is_usable "$CACHED_AI_TOOL"; then
        SELECTED_AI_TOOL="$CACHED_AI_TOOL"
        SELECTION_REASON="cached configuration"
    fi
fi

if [ -z "$SELECTED_AI_TOOL" ]; then
    if [ "$PROBE_AI_TOOLS" = "1" ]; then
        if command -v codex >/dev/null 2>&1 && probe_codex; then
            SELECTED_AI_TOOL="codex"
            SELECTION_REASON="codex probe passed"
        elif command -v claude >/dev/null 2>&1 && probe_claude; then
            SELECTED_AI_TOOL="claude"
            SELECTION_REASON="claude probe passed"
        else
            SELECTED_AI_TOOL="none"
            SELECTION_REASON="all probes failed"
        fi
    else
        SELECTED_AI_TOOL="none"
        SELECTION_REASON="probes disabled (SIMLIN_INIT_AI_PROBE=0)"
    fi
fi

printf '%s\n' "$SELECTED_AI_TOOL" > "$AI_CONFIG_FILE"

if [ "$SELECTED_AI_TOOL" = "none" ]; then
    echo -e "${YELLOW}none${NC} ($SELECTION_REASON)"
    echo -e "    ${YELLOW}Pre-commit AI checks will be skipped${NC}"
    echo -e "    ${YELLOW}Set ANTHROPIC_API_KEY or OPENAI_API_KEY and re-run to enable${NC}"
else
    echo -e "${GREEN}$SELECTED_AI_TOOL${NC} ($SELECTION_REASON)"
fi

echo ""
echo -e "${GREEN}Environment setup complete!${NC}"
echo ""
echo "Next steps:"
echo "  - Run 'cargo test' to verify Rust tests pass"
echo "  - Run 'pnpm build' to build the TypeScript/WASM components"
echo ""
