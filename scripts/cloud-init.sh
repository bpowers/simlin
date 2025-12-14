#!/bin/bash
#
# Cloud initialization script for Claude Code on the web and Codex Web.
# This script sets up the development environment for Simlin, ensuring
# that git submodules are initialized, git hooks are installed, and
# basic dependencies are checked.
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

# 1. Initialize git submodules
echo -n "Initializing git submodules... "
if git submodule update --init --recursive >/dev/null 2>&1; then
    echo -e "${GREEN}done${NC}"
else
    echo -e "${RED}failed${NC}"
    echo -e "${YELLOW}Warning: Could not initialize submodules. Some tests may not work.${NC}"
fi

# 2. Check if test-models submodule has content
if [ -d "test/test-models" ] && [ -n "$(ls -A test/test-models 2>/dev/null)" ]; then
    echo -e "  ${GREEN}✓${NC} test/test-models submodule initialized"
else
    echo -e "  ${YELLOW}!${NC} test/test-models submodule is empty"
    echo -e "    Run: git submodule update --init --recursive"
fi

# 3. Install git hooks
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

# 4. Check for required tools
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

# Check yarn
echo -n "  Yarn: "
if command -v yarn >/dev/null 2>&1; then
    YARN_VERSION=$(yarn --version 2>/dev/null)
    echo -e "${GREEN}$YARN_VERSION${NC}"
else
    echo -e "${RED}not found${NC}"
    echo -e "    ${YELLOW}Install Yarn: npm install -g yarn${NC}"
fi

# Check protoc (protobuf compiler)
echo -n "  Protoc: "
if command -v protoc >/dev/null 2>&1; then
    PROTOC_VERSION=$(protoc --version 2>/dev/null | cut -d' ' -f2)
    echo -e "${GREEN}$PROTOC_VERSION${NC}"
else
    echo -e "${RED}not found${NC}"
    echo -e "    ${YELLOW}Install protobuf-compiler: apt-get install protobuf-compiler${NC}"
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

# 5. Install yarn dependencies if node_modules doesn't exist
echo ""
if [ ! -d "node_modules" ]; then
    echo -n "Installing yarn dependencies... "
    if command -v yarn >/dev/null 2>&1; then
        if yarn install --frozen-lockfile >/dev/null 2>&1; then
            echo -e "${GREEN}done${NC}"
        else
            echo -e "${YELLOW}failed (non-critical)${NC}"
            echo -e "    Try running: yarn install"
        fi
    else
        echo -e "${YELLOW}skipped (yarn not available)${NC}"
    fi
else
    echo -e "Yarn dependencies: ${GREEN}already installed${NC}"
fi

echo ""
echo -e "${GREEN}Environment setup complete!${NC}"
echo ""
echo "Next steps:"
echo "  - Run 'cargo test' to verify Rust tests pass"
echo "  - Run 'yarn build' to build the TypeScript/WASM components"
echo ""
