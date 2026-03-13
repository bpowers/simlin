# MCP npm Release -- Phase 3: GitHub Actions Workflow

**Goal:** CI workflow that builds all 4 platform binaries and publishes the 5 npm packages using OIDC trusted publishing.

**Architecture:** Three-job workflow: `build` (matrix, 4 platforms) produces binary artifacts; `publish-platform` (needs: build) downloads artifacts and publishes 4 platform npm packages; `publish-wrapper` (needs: publish-platform) publishes the `@simlin/mcp` wrapper. OIDC trusted publishing eliminates stored npm tokens.

**Tech Stack:** GitHub Actions, cargo-zigbuild, Zig, npm OIDC trusted publishing, provenance attestation

**Scope:** Phase 3 of 4 from original design

**Codebase verified:** 2026-03-12

---

## Acceptance Criteria Coverage

This phase implements and tests:

### mcp-npm-release.AC1: CI workflow builds all 4 platforms
- **mcp-npm-release.AC1.1 Success:** `mcp-v*` tag push triggers the workflow
- **mcp-npm-release.AC1.2 Success:** Linux x64 musl static binary is produced
- **mcp-npm-release.AC1.3 Success:** Linux arm64 musl static binary is produced
- **mcp-npm-release.AC1.4 Success:** Windows x64 PE binary is produced (mingw cross-compiled)
- **mcp-npm-release.AC1.5 Success:** macOS arm64 binary is produced (native build on macOS runner)
- **mcp-npm-release.AC1.6 Failure:** Workflow does not trigger on non-`mcp-v*` tags

### mcp-npm-release.AC2: npm packages publish correctly
- **mcp-npm-release.AC2.1 Success:** 4 platform packages publish before the wrapper package
- **mcp-npm-release.AC2.2 Success:** Wrapper `@simlin/mcp` publishes with correct `optionalDependencies` versions
- **mcp-npm-release.AC2.3 Success:** All packages publish with provenance attestation
- **mcp-npm-release.AC2.4 Success:** Authentication uses OIDC (no stored NPM_TOKEN)
- **mcp-npm-release.AC2.5 Edge:** Non-Windows binaries have execute permission after artifact download

### mcp-npm-release.AC5: Minimal security surface
- **mcp-npm-release.AC5.1 Success:** Build jobs have only `contents: read` permission
- **mcp-npm-release.AC5.2 Success:** Only publish jobs have `id-token: write` permission
- **mcp-npm-release.AC5.3 Success:** No long-lived npm tokens stored in GitHub secrets

---

## Codebase Verification Notes

- Existing `release.yml` (pysimlin) uses `pysimlin-v*` tag trigger, artifact upload/download v4, and `if: startsWith(github.ref, 'refs/tags/')` guard. Uses stored `PYPI_API_TOKEN` secret (not OIDC). The MCP workflow follows the same tag pattern but uses OIDC instead.
- No reusable workflows or composite actions exist in `.github/` -- the new workflow must be self-contained.
- `ci.yaml` uses `actions/cache@v4` with key `cargo-build-${{ matrix.os }}-${{ hashFiles('**/Cargo.lock') }}`. The MCP workflow should use a distinct prefix.
- Node.js version in CI is 22.
- `macos-latest` runners are Apple Silicon (arm64) -- native `cargo build` for `aarch64-apple-darwin` works without cross-compilation.
- Repository is `bpowers/simlin` (confirmed from git remote).

**External dependency findings:**
- npm OIDC trusted publishing is GA (since July 2025). Requires npm >= 11.5.1 (current: 11.11.1). Node 22 is fine.
- Critical gotcha: `actions/setup-node` with `registry-url` injects `NODE_AUTH_TOKEN` placeholder that breaks OIDC. Fix: omit `registry-url` (npm defaults to registry.npmjs.org anyway).
- First version of each package must be published manually before OIDC can be configured -- Phase 4 handles this.
- `actions/upload-artifact@v4` does NOT preserve Unix file permissions. Must `chmod +x` binaries after download.
- `mlugg/setup-zig@v2` is the recommended GitHub Action for Zig (caches across runs).
- `--provenance` flag should be passed explicitly to `npm publish` as a safeguard.
- Each of the 5 packages needs its own Trusted Publisher configuration on npmjs.com (all pointing to same workflow file).

---

<!-- START_TASK_1 -->
### Task 1: Create .github/workflows/mcp-release.yml

**Verifies:** mcp-npm-release.AC1.1, mcp-npm-release.AC1.2, mcp-npm-release.AC1.3, mcp-npm-release.AC1.4, mcp-npm-release.AC1.5, mcp-npm-release.AC1.6, mcp-npm-release.AC2.1, mcp-npm-release.AC2.2, mcp-npm-release.AC2.3, mcp-npm-release.AC2.4, mcp-npm-release.AC2.5, mcp-npm-release.AC5.1, mcp-npm-release.AC5.2, mcp-npm-release.AC5.3

**Files:**
- Create: `.github/workflows/mcp-release.yml`

**Implementation:**

Create `.github/workflows/mcp-release.yml` with the complete workflow below. The workflow has three jobs:

1. **`build`** -- matrix of 4 platform targets. Three use `cargo-zigbuild` on `ubuntu-latest`; macOS arm64 uses native `cargo build` on `macos-latest`. Each uploads the binary as a named artifact.

2. **`publish-platform`** -- runs after build completes. Downloads all artifacts, runs `build-npm-packages.sh` to generate platform `package.json` files with the correct version (from `Cargo.toml`), copies binaries into platform package `bin/` directories, `chmod +x` non-Windows binaries, then publishes all 4 platform packages with `npm publish --provenance --access public`.

3. **`publish-wrapper`** -- runs after platform packages are published. Updates the wrapper `package.json` version and `optionalDependencies` versions to match `Cargo.toml`, then publishes with provenance.

Key design decisions:
- Workflow-level `permissions: contents: read` (AC5.1). Only publish jobs add `id-token: write` (AC5.2).
- No `registry-url` in `actions/setup-node` -- avoids `NODE_AUTH_TOKEN` placeholder that conflicts with OIDC (AC2.4).
- `npm install -g npm@latest` ensures npm >= 11.5.1 for OIDC support.
- Tag format validated against `Cargo.toml` version in a separate `validate` job to fail fast.
- `workflow_dispatch` enabled for dry-run testing (build jobs run, publish jobs skip because `if: startsWith(github.ref, 'refs/tags/')` is false).

```yaml
name: MCP npm Release

on:
  push:
    tags:
      - 'mcp-v*'
  workflow_dispatch:

permissions:
  contents: read

jobs:
  validate:
    name: Validate version
    runs-on: ubuntu-latest
    outputs:
      version: ${{ steps.version.outputs.version }}
    steps:
      - uses: actions/checkout@v4

      - name: Extract and validate version
        id: version
        run: |
          CARGO_VERSION=$(grep '^version = ' src/simlin-mcp/Cargo.toml | head -1 | sed 's/version = "\(.*\)"/\1/')
          echo "Cargo.toml version: $CARGO_VERSION"

          if [[ "$GITHUB_REF" == refs/tags/mcp-v* ]]; then
            TAG_VERSION="${GITHUB_REF_NAME#mcp-v}"
            echo "Tag version: $TAG_VERSION"
            if [ "$TAG_VERSION" != "$CARGO_VERSION" ]; then
              echo "::error::Tag version ($TAG_VERSION) does not match Cargo.toml ($CARGO_VERSION)"
              exit 1
            fi
          fi

          echo "version=$CARGO_VERSION" >> "$GITHUB_OUTPUT"

  build:
    name: Build ${{ matrix.artifact }}
    needs: validate
    runs-on: ${{ matrix.os }}
    strategy:
      fail-fast: false
      matrix:
        include:
          - target: x86_64-unknown-linux-musl
            os: ubuntu-latest
            artifact: mcp-linux-x64
            binary: simlin-mcp
            use-zigbuild: true
          - target: aarch64-unknown-linux-musl
            os: ubuntu-latest
            artifact: mcp-linux-arm64
            binary: simlin-mcp
            use-zigbuild: true
          - target: x86_64-pc-windows-gnu
            os: ubuntu-latest
            artifact: mcp-win32-x64
            binary: simlin-mcp.exe
            use-zigbuild: true
          - target: aarch64-apple-darwin
            os: macos-latest
            artifact: mcp-darwin-arm64
            binary: simlin-mcp
            use-zigbuild: false
    steps:
      - uses: actions/checkout@v4

      - name: Install Rust toolchain
        uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }}

      - name: Cache Cargo artifacts
        uses: actions/cache@v4
        with:
          path: |
            ~/.cargo/bin
            ~/.cargo/registry
            ~/.cargo/git
            target
          key: cargo-mcp-${{ matrix.os }}-${{ matrix.target }}-${{ hashFiles('**/Cargo.lock') }}
          restore-keys: |
            cargo-mcp-${{ matrix.os }}-${{ matrix.target }}-

      - name: Install Zig
        if: matrix.use-zigbuild
        uses: mlugg/setup-zig@v2
        with:
          version: '0.15.2'

      - name: Install cargo-zigbuild
        if: matrix.use-zigbuild
        run: cargo install --locked cargo-zigbuild@0.22

      - name: Build (zigbuild)
        if: matrix.use-zigbuild
        run: cargo zigbuild -p simlin-mcp --release --target ${{ matrix.target }}

      - name: Build (native)
        if: ${{ !matrix.use-zigbuild }}
        run: cargo build -p simlin-mcp --release --target ${{ matrix.target }}

      - name: Upload binary
        uses: actions/upload-artifact@v4
        with:
          name: ${{ matrix.artifact }}
          path: target/${{ matrix.target }}/release/${{ matrix.binary }}
          if-no-files-found: error
          retention-days: 1

  publish-platform:
    name: Publish platform packages
    needs: [validate, build]
    runs-on: ubuntu-latest
    if: startsWith(github.ref, 'refs/tags/')
    permissions:
      contents: read
      id-token: write
    steps:
      - uses: actions/checkout@v4

      - uses: actions/setup-node@v4
        with:
          node-version: '22'

      - run: npm install -g npm@latest

      - uses: actions/download-artifact@v4
        with:
          path: artifacts/

      - name: Prepare platform packages
        working-directory: src/simlin-mcp
        run: |
          bash build-npm-packages.sh

          cp ../../artifacts/mcp-linux-x64/simlin-mcp npm/@simlin/mcp-linux-x64/bin/
          cp ../../artifacts/mcp-linux-arm64/simlin-mcp npm/@simlin/mcp-linux-arm64/bin/
          cp ../../artifacts/mcp-win32-x64/simlin-mcp.exe npm/@simlin/mcp-win32-x64/bin/
          cp ../../artifacts/mcp-darwin-arm64/simlin-mcp npm/@simlin/mcp-darwin-arm64/bin/

          chmod +x npm/@simlin/mcp-linux-x64/bin/simlin-mcp
          chmod +x npm/@simlin/mcp-linux-arm64/bin/simlin-mcp
          chmod +x npm/@simlin/mcp-darwin-arm64/bin/simlin-mcp

      - name: Publish @simlin/mcp-linux-x64
        working-directory: src/simlin-mcp/npm/@simlin/mcp-linux-x64
        run: npm publish --provenance --access public

      - name: Publish @simlin/mcp-linux-arm64
        working-directory: src/simlin-mcp/npm/@simlin/mcp-linux-arm64
        run: npm publish --provenance --access public

      - name: Publish @simlin/mcp-win32-x64
        working-directory: src/simlin-mcp/npm/@simlin/mcp-win32-x64
        run: npm publish --provenance --access public

      - name: Publish @simlin/mcp-darwin-arm64
        working-directory: src/simlin-mcp/npm/@simlin/mcp-darwin-arm64
        run: npm publish --provenance --access public

  publish-wrapper:
    name: Publish @simlin/mcp
    needs: [validate, publish-platform]
    runs-on: ubuntu-latest
    if: startsWith(github.ref, 'refs/tags/')
    permissions:
      contents: read
      id-token: write
    steps:
      - uses: actions/checkout@v4

      - uses: actions/setup-node@v4
        with:
          node-version: '22'

      - run: npm install -g npm@latest

      - name: Update wrapper version
        working-directory: src/simlin-mcp
        run: |
          VERSION="${{ needs.validate.outputs.version }}"
          jq --arg v "$VERSION" '
            .version = $v |
            .optionalDependencies = (
              .optionalDependencies | to_entries | map(.value = $v) | from_entries
            )
          ' package.json > package.json.tmp && mv package.json.tmp package.json

          echo "Updated wrapper package.json:"
          cat package.json

      - name: Publish @simlin/mcp
        working-directory: src/simlin-mcp
        run: npm publish --provenance --access public
```

**Verification:**

The workflow cannot be fully tested without pushing a tag, but verify structural correctness:

```bash
# Check YAML syntax
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/mcp-release.yml'))"

# Verify trigger pattern
grep -A2 'tags:' .github/workflows/mcp-release.yml

# Verify permissions
grep -A1 'permissions:' .github/workflows/mcp-release.yml
```

If `actionlint` is available:
```bash
actionlint .github/workflows/mcp-release.yml
```

**Commit:** `mcp: add GitHub Actions workflow for npm release`
<!-- END_TASK_1 -->
