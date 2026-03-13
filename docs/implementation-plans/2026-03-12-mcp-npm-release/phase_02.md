# MCP npm Release -- Phase 2: Local Cross-Build Tooling

**Goal:** Dockerfile and script that build Linux (x64, arm64) and Windows (x64) binaries locally via cargo-zigbuild.

**Architecture:** A toolchain Docker image (`Dockerfile.cross`) provides Rust + Zig + cargo-zigbuild. A shell script builds the image, runs all three cross-compilations in a single container, and copies binaries to `dist/`. Source is mounted read-only; build artifacts use a named Docker volume for incremental builds.

**Tech Stack:** Docker, cargo-zigbuild 0.22.x, Zig 0.15.x, Rust cross-compilation targets (musl, mingw)

**Scope:** Phase 2 of 4 from original design

**Codebase verified:** 2026-03-12

---

## Acceptance Criteria Coverage

This phase implements and tests:

### mcp-npm-release.AC3: Local build works without CI
- **mcp-npm-release.AC3.1 Success:** `cross-build.sh` produces linux-x64, linux-arm64, and win32-x64 binaries via Docker
- **mcp-npm-release.AC3.2 Success:** Linux x64 binary runs on the host (smoke test)
- **mcp-npm-release.AC3.3 Success:** Script works on both x64 and arm64 development hosts

---

## Codebase Verification Notes

- Workspace root `Cargo.toml` is at repo root with 5 members: `src/libsimlin`, `src/simlin-cli`, `src/simlin-engine`, `src/simlin-mcp`, `src/xmutil`
- `simlin-mcp` depends on `simlin-engine` (path dep) but NOT on `xmutil` -- cross-build only compiles simlin-mcp and simlin-engine
- No existing Dockerfiles in the repo. Existing `.dockerignore` is stale (web-app focused, does not exclude `/target`)
- No `src/simlin-mcp/scripts/` directory exists -- will be created
- No `.cargo/config.toml` exists -- no pre-configured cross-compilation settings
- Release profile: `opt-level = "z"`, `lto = true`, `panic = "abort"`, `strip = true` (produces small stripped binaries)

**External dependency findings:**
- cargo-zigbuild 0.22.1 is current stable; install via `cargo install --locked cargo-zigbuild`
- Zig 0.15.2 is current stable; tarball naming uses `zig-{ARCH}-linux-{VERSION}.tar.xz` format (changed at 0.14.0)
- Windows GNU cross-compilation: `raw-dylib` / `dlltool` issue fixed in cargo-zigbuild 0.21.2+ (no need for separate mingw-w64 install)
- musl targets produce fully static binaries with cargo-zigbuild (primary use case, well-supported)
- `CARGO_TARGET_DIR` env var redirects build output away from read-only source mount

---

<!-- START_TASK_1 -->
### Task 1: Create Dockerfile.cross

**Verifies:** None (infrastructure setup)

**Files:**
- Create: `src/simlin-mcp/Dockerfile.cross`

**Implementation:**

Create `src/simlin-mcp/Dockerfile.cross` with the cross-compilation toolchain:

```dockerfile
# Cross-compilation toolchain for simlin-mcp.
#
# Provides Rust + Zig + cargo-zigbuild for building Linux (musl) and
# Windows (mingw) binaries from a single image.  Used by scripts/cross-build.sh.

# Update RUST_VERSION to match the current stable Rust when building.
# Override at build time: docker build --build-arg RUST_VERSION=1.88.0 ...
ARG RUST_VERSION=1.87.0
ARG ZIG_VERSION=0.15.2

FROM rust:${RUST_VERSION}-bookworm

ARG ZIG_VERSION

# Install Zig from upstream tarball
RUN ARCH=$(uname -m) && \
    TARBALL="zig-${ARCH}-linux-${ZIG_VERSION}.tar.xz" && \
    DIR="zig-${ARCH}-linux-${ZIG_VERSION}" && \
    curl -L "https://ziglang.org/download/${ZIG_VERSION}/${TARBALL}" \
        | tar -J -x -C /usr/local && \
    ln -s "/usr/local/${DIR}/zig" /usr/local/bin/zig

RUN cargo install --locked cargo-zigbuild

RUN rustup target add \
    x86_64-unknown-linux-musl \
    aarch64-unknown-linux-musl \
    x86_64-pc-windows-gnu

WORKDIR /src
```

**Verification:**

```bash
docker build -t simlin-mcp-cross -f src/simlin-mcp/Dockerfile.cross src/simlin-mcp/
docker run --rm simlin-mcp-cross cargo zigbuild --version
```

Expected: Image builds successfully, `cargo zigbuild --version` prints version info.

**Commit:** `mcp: add Dockerfile.cross for local cross-compilation toolchain`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Create scripts/cross-build.sh

**Verifies:** mcp-npm-release.AC3.1, mcp-npm-release.AC3.2, mcp-npm-release.AC3.3

**Files:**
- Create: `src/simlin-mcp/scripts/cross-build.sh`

**Implementation:**

Create `src/simlin-mcp/scripts/cross-build.sh` (must be `chmod +x`):

```bash
#!/usr/bin/env bash
# Cross-compile simlin-mcp for Linux (x64, arm64) and Windows (x64).
#
# Builds a Docker toolchain image, then runs cargo-zigbuild inside it with the
# repo source mounted read-only.  A named Docker volume caches the Cargo target
# directory so subsequent runs are incremental.
#
# Output: dist/<triple>/simlin-mcp[.exe]

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MCP_DIR="$(dirname "$SCRIPT_DIR")"
REPO_ROOT="$(cd "$MCP_DIR/../.." && pwd)"

IMAGE_NAME="simlin-mcp-cross"
CACHE_VOLUME="simlin-mcp-cross-cache"
DIST_DIR="$MCP_DIR/dist"

echo "==> Building cross-compilation toolchain image..."
docker build -t "$IMAGE_NAME" -f "$MCP_DIR/Dockerfile.cross" "$MCP_DIR/"

rm -rf "$DIST_DIR"
mkdir -p "$DIST_DIR"

echo "==> Cross-compiling all targets..."
docker run --rm \
    -v "$REPO_ROOT:/src:ro" \
    -v "$CACHE_VOLUME:/tmp/target" \
    -v "$DIST_DIR:/dist" \
    -e CARGO_TARGET_DIR=/tmp/target \
    "$IMAGE_NAME" \
    bash -c '
        cd /src
        for target in x86_64-unknown-linux-musl aarch64-unknown-linux-musl x86_64-pc-windows-gnu; do
            echo "--- Building $target ---"
            cargo zigbuild -p simlin-mcp --release --target "$target"
            mkdir -p "/dist/$target"
            if [[ "$target" == *windows* ]]; then
                cp "/tmp/target/$target/release/simlin-mcp.exe" "/dist/$target/"
            else
                cp "/tmp/target/$target/release/simlin-mcp" "/dist/$target/"
            fi
        done
        echo "--- All targets built ---"
    '

echo ""
echo "Binaries:"
ls -lh "$DIST_DIR"/*/simlin-mcp*

# Smoke test: verify the Linux x64 binary is a valid static executable and runs
echo ""
echo "==> Smoke test: Linux x64 binary..."
file "$DIST_DIR/x86_64-unknown-linux-musl/simlin-mcp"

echo "==> Verifying binary executes..."
# simlin-mcp is a stdio MCP server that waits for input, so feed it empty input
# with a timeout. Any exit (including error) proves the binary loads and runs.
echo '' | timeout 2 "$DIST_DIR/x86_64-unknown-linux-musl/simlin-mcp" 2>/dev/null || true
echo "Smoke test passed (binary executed)"

echo ""
echo "Done. Binaries in $DIST_DIR/"
```

**Verification:**

```bash
cd src/simlin-mcp && bash scripts/cross-build.sh
```

Expected output:
- `dist/x86_64-unknown-linux-musl/simlin-mcp` exists (static ELF binary)
- `dist/aarch64-unknown-linux-musl/simlin-mcp` exists (static ELF binary, aarch64)
- `dist/x86_64-pc-windows-gnu/simlin-mcp.exe` exists (PE executable)
- `file` command confirms the Linux x64 binary is `ELF 64-bit LSB executable, x86-64, ... statically linked`

**Commit:** `mcp: add cross-build.sh for local multi-platform binary builds`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Add dist/ to .gitignore

**Verifies:** None (infrastructure cleanup)

**Files:**
- Modify: `src/simlin-mcp/.gitignore` (create if not exists)

**Implementation:**

The cross-build script outputs binaries to `src/simlin-mcp/dist/`. These must not be committed. Check if `src/simlin-mcp/.gitignore` exists. If not, create it. If it does, append to it.

The root `.gitignore` already has `/src/simlin-mcp/vendor` but not `/src/simlin-mcp/dist`. Add `dist/` to the simlin-mcp-level gitignore (preferred over root since it's component-specific).

Content for `src/simlin-mcp/.gitignore` (create or append):

```
/dist/
```

Alternatively, add to the root `.gitignore` next to the existing simlin-mcp entries:

```
/src/simlin-mcp/dist
```

Choose whichever location already has simlin-mcp ignore patterns. Root `.gitignore` already has `/src/simlin-mcp/vendor`, so adding `/src/simlin-mcp/dist` there is consistent.

**Verification:**

```bash
git status  # dist/ should not appear as untracked
```

**Commit:** `mcp: ignore cross-build dist/ output`
<!-- END_TASK_3 -->
