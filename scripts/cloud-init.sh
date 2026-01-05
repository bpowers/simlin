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

# Pinned protoc version - update this when a new version is needed
# This is the latest stable release as of January 2026
PROTOC_VERSION="33.2"

# Determine repository root
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

cd "$REPO_ROOT"

# Function to install protoc from GitHub releases
install_protoc() {
    local version="$1"
    local install_dir="$HOME/.local"
    local bin_dir="$install_dir/bin"
    local include_dir="$install_dir/include"

    # Detect OS and architecture
    local os=""
    local arch=""

    case "$(uname -s)" in
        Linux)  os="linux" ;;
        Darwin) os="osx" ;;
        *)
            echo -e "${RED}Unsupported OS: $(uname -s)${NC}"
            return 1
            ;;
    esac

    case "$(uname -m)" in
        x86_64)  arch="x86_64" ;;
        aarch64) arch="aarch_64" ;;
        arm64)   arch="aarch_64" ;;  # macOS ARM
        *)
            echo -e "${RED}Unsupported architecture: $(uname -m)${NC}"
            return 1
            ;;
    esac

    local filename="protoc-${version}-${os}-${arch}.zip"
    local url="https://github.com/protocolbuffers/protobuf/releases/download/v${version}/${filename}"

    echo -n "  Downloading protoc ${version}... "

    # Create temp directory for download
    local tmp_dir
    tmp_dir=$(mktemp -d)
    trap "rm -rf $tmp_dir" EXIT

    # Download with retry logic
    local retry_count=0
    local max_retries=4
    local wait_time=2

    while [ $retry_count -lt $max_retries ]; do
        if curl -fsSL "$url" -o "$tmp_dir/$filename" 2>/dev/null; then
            break
        fi
        retry_count=$((retry_count + 1))
        if [ $retry_count -lt $max_retries ]; then
            sleep $wait_time
            wait_time=$((wait_time * 2))
        fi
    done

    if [ ! -f "$tmp_dir/$filename" ]; then
        echo -e "${RED}failed to download${NC}"
        return 1
    fi
    echo -e "${GREEN}done${NC}"

    echo -n "  Installing protoc to $bin_dir... "

    # Create installation directories
    mkdir -p "$bin_dir" "$include_dir"

    # Extract the zip file
    if ! unzip -q "$tmp_dir/$filename" -d "$tmp_dir/protoc" 2>/dev/null; then
        echo -e "${RED}failed to extract${NC}"
        return 1
    fi

    # Install binary and includes
    cp "$tmp_dir/protoc/bin/protoc" "$bin_dir/"
    chmod +x "$bin_dir/protoc"
    cp -r "$tmp_dir/protoc/include/"* "$include_dir/" 2>/dev/null || true

    echo -e "${GREEN}done${NC}"

    # Add to PATH for current session if not already there
    if [[ ":$PATH:" != *":$bin_dir:"* ]]; then
        export PATH="$bin_dir:$PATH"
        echo -e "  ${YELLOW}Note: Added $bin_dir to PATH for this session${NC}"
        echo -e "  ${YELLOW}Add 'export PATH=\"$bin_dir:\$PATH\"' to your shell profile for persistence${NC}"
    fi

    return 0
}

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

# Check protoc (protobuf compiler) - install if not found
echo -n "  Protoc: "
if command -v protoc >/dev/null 2>&1; then
    INSTALLED_PROTOC_VERSION=$(protoc --version 2>/dev/null | cut -d' ' -f2)
    echo -e "${GREEN}$INSTALLED_PROTOC_VERSION${NC}"
else
    echo -e "${YELLOW}not found - installing...${NC}"
    if install_protoc "$PROTOC_VERSION"; then
        # Verify installation
        if command -v protoc >/dev/null 2>&1; then
            INSTALLED_PROTOC_VERSION=$(protoc --version 2>/dev/null | cut -d' ' -f2)
            echo -e "  Protoc: ${GREEN}$INSTALLED_PROTOC_VERSION${NC} (newly installed)"
        fi
    else
        echo -e "  ${RED}Failed to install protoc automatically${NC}"
        echo -e "  ${YELLOW}Manual installation: apt-get install protobuf-compiler${NC}"
        echo -e "  ${YELLOW}Or download from: https://github.com/protocolbuffers/protobuf/releases${NC}"
    fi
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

# 6. Install and configure AI tools for pre-commit hook
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
echo "  - Run 'yarn build' to build the TypeScript/WASM components"
echo ""
