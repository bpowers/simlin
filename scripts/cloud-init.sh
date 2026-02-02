#!/bin/bash
#
# Cloud initialization script for Claude Code on the web and Codex Web.
# This script sets up the development environment for Simlin, ensuring
# that git hooks are installed and basic dependencies are checked.
#
# This script is idempotent - it can be run multiple times safely.
#
set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Determine repository root
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

cd "$REPO_ROOT"

echo "Setting up Simlin development environment..."
echo ""

# 1. Install git hooks
echo -n "Installing git hooks... "
if [ -f ".git/hooks/pre-commit" ] && [ -L ".git/hooks/pre-commit" ]; then
    # Hook already exists and is a symlink
    LINK_TARGET="$(readlink .git/hooks/pre-commit)"
    if [ "$LINK_TARGET" = "../../scripts/pre-commit" ]; then
        echo -e "${GREEN}already installed${NC}"
    else
        rm -f .git/hooks/pre-commit
        ln -s ../../scripts/pre-commit .git/hooks/pre-commit
        echo -e "${GREEN}updated${NC}"
    fi
else
    # Install the hook
    rm -f .git/hooks/pre-commit
    ln -s ../../scripts/pre-commit .git/hooks/pre-commit
    echo -e "${GREEN}done${NC}"
fi

# 2. Check for required tools
echo ""
echo "Checking required tools..."

# Check Rust
echo -n "  Rust: "
if command -v rustc >/dev/null 2>&1; then
    RUST_VERSION=$(rustc --version 2>/dev/null | cut -d' ' -f2)
    echo -e "${GREEN}$RUST_VERSION${NC}"
else
    echo -e "${RED}not found${NC}"
    echo -e "    ${YELLOW}Install Rust: https://rustup.rs/${NC}"
fi

# Check cargo
echo -n "  Cargo: "
if command -v cargo >/dev/null 2>&1; then
    CARGO_VERSION=$(cargo --version 2>/dev/null | cut -d' ' -f2)
    echo -e "${GREEN}$CARGO_VERSION${NC}"
else
    echo -e "${RED}not found${NC}"
fi

# Check Node.js
echo -n "  Node.js: "
if command -v node >/dev/null 2>&1; then
    NODE_VERSION=$(node --version 2>/dev/null)
    echo -e "${GREEN}$NODE_VERSION${NC}"
else
    echo -e "${RED}not found${NC}"
    echo -e "    ${YELLOW}Install Node.js: https://nodejs.org/${NC}"
fi

# Check pnpm
echo -n "  pnpm: "
if command -v pnpm >/dev/null 2>&1; then
    PNPM_VERSION=$(pnpm --version 2>/dev/null)
    echo -e "${GREEN}$PNPM_VERSION${NC}"
else
    echo -e "${RED}not found${NC}"
    echo -e "    ${YELLOW}Install pnpm: npm install -g pnpm${NC}"
fi

# Check Python (optional, for pysimlin)
echo -n "  Python: "
if command -v python3 >/dev/null 2>&1; then
    PY_VERSION=$(python3 --version 2>/dev/null | cut -d' ' -f2)
    echo -e "${GREEN}$PY_VERSION${NC}"
    # Check if Python is 3.11+
    PY_MAJOR=$(echo "$PY_VERSION" | cut -d. -f1)
    PY_MINOR=$(echo "$PY_VERSION" | cut -d. -f2)
    if [ "$PY_MAJOR" -ge 3 ] && [ "$PY_MINOR" -ge 11 ]; then
        echo -e "    ${GREEN}✓${NC} Python 3.11+ available for pysimlin tests"
    else
        echo -e "    ${YELLOW}!${NC} Python 3.11+ required for pysimlin tests (found $PY_VERSION)"
    fi
else
    echo -e "${YELLOW}not found (optional, needed for pysimlin)${NC}"
fi

# 3. Install pnpm dependencies if node_modules doesn't exist
echo ""
if [ ! -d "node_modules" ]; then
    echo -n "Installing pnpm dependencies... "
    if command -v pnpm >/dev/null 2>&1; then
        if pnpm install --frozen-lockfile >/dev/null 2>&1; then
            echo -e "${GREEN}done${NC}"
        else
            echo -e "${YELLOW}failed (non-critical)${NC}"
            echo -e "    Try running: pnpm install"
        fi
    else
        echo -e "${YELLOW}skipped (pnpm not available)${NC}"
    fi
else
    echo -e "pnpm dependencies: ${GREEN}already installed${NC}"
fi

# 4. Install and configure AI tools for pre-commit hook
echo ""
echo "Setting up AI tools for pre-commit hook..."

# Install codex if not present
echo -n "  Installing @openai/codex... "
if command -v codex >/dev/null 2>&1; then
    CODEX_VERSION=$(codex --version 2>/dev/null | head -1)
    echo -e "${GREEN}already installed ($CODEX_VERSION)${NC}"
else
    if npm install -g @openai/codex >/dev/null 2>&1; then
        echo -e "${GREEN}done${NC}"
    else
        echo -e "${YELLOW}failed (non-critical)${NC}"
    fi
fi

# Login to codex with API key if available
if command -v codex >/dev/null 2>&1 && [ -n "$OPENAI_API_KEY" ]; then
    echo -n "  Configuring codex with API key... "
    if printenv OPENAI_API_KEY | codex login --with-api-key >/dev/null 2>&1; then
        echo -e "${GREEN}done${NC}"
    else
        echo -e "${YELLOW}failed${NC}"
    fi
fi

# Test which AI tool works for pre-commit hook
# We'll save the result to a config file that the pre-commit hook can read
AI_CONFIG_FILE="$REPO_ROOT/.ai-tool-config"
echo -n "  Testing AI tools for pre-commit... "

# First, test Claude CLI (10 second timeout)
CLAUDE_WORKS=false
if command -v claude >/dev/null 2>&1; then
    CLAUDE_OUTPUT=$(mktemp)
    if timeout -k 2 10 claude -p "respond with the single word: yes" > "$CLAUDE_OUTPUT" 2>&1; then
        if grep -qi "yes" "$CLAUDE_OUTPUT"; then
            CLAUDE_WORKS=true
        fi
    fi
    rm -f "$CLAUDE_OUTPUT"
fi

# Test codex if Claude didn't work
CODEX_WORKS=false
if [ "$CLAUDE_WORKS" = "false" ] && command -v codex >/dev/null 2>&1; then
    CODEX_OUTPUT=$(mktemp)
    if timeout -k 2 30 codex exec -m gpt-5.2 "respond with the single word: yes" > "$CODEX_OUTPUT" 2>&1; then
        if grep -qi "yes" "$CODEX_OUTPUT"; then
            CODEX_WORKS=true
        fi
    fi
    rm -f "$CODEX_OUTPUT"
fi

# Save the preferred tool to config file
if [ "$CLAUDE_WORKS" = "true" ]; then
    echo "claude" > "$AI_CONFIG_FILE"
    echo -e "${GREEN}claude${NC}"
elif [ "$CODEX_WORKS" = "true" ]; then
    echo "codex" > "$AI_CONFIG_FILE"
    echo -e "${GREEN}codex${NC}"
else
    echo "none" > "$AI_CONFIG_FILE"
    echo -e "${YELLOW}none available${NC}"
    echo -e "    ${YELLOW}Pre-commit AI checks will be skipped${NC}"
fi

echo ""
echo -e "${GREEN}Environment setup complete!${NC}"
echo ""
echo "Next steps:"
echo "  - Run 'cargo test' to verify Rust tests pass"
echo "  - Run 'pnpm build' to build the TypeScript/WASM components"
echo ""
