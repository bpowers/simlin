# MCP npm Release CI Design

## Summary

This plan adds automated CI/CD for publishing the `simlin-mcp` binary -- an MCP server that exposes the Simlin simulation engine to AI assistants -- as a set of npm packages covering macOS, Linux, and Windows. The approach uses the platform-specific optional dependency pattern (as used by esbuild, Biome, and Codex): a thin JS wrapper package (`@simlin/mcp`) declares optional dependencies on four platform-specific packages, each containing a single native binary. When a user runs `npm install @simlin/mcp`, npm automatically installs only the package matching their OS and architecture, and the JS launcher spawns that binary.

The implementation proceeds in four phases. First, clean up existing scaffolding by removing the stale `darwin-x64` platform and fixing the Windows target triple. Second, create a Docker-based local cross-build script using `cargo-zigbuild` (which leverages Zig as a cross-linker) so developers can produce Linux and Windows binaries without CI. Third, add a GitHub Actions workflow triggered by `mcp-v*` tags that builds all four platform binaries in a matrix, then publishes the five npm packages using OIDC trusted publishing -- eliminating the need for stored npm tokens. Fourth, manually configure npm's Trusted Publisher settings and verify the full pipeline with a pre-release tag.

## Definition of Done

1. **A GitHub Actions workflow** (`.github/workflows/mcp-release.yml`) triggered by `mcp-v*` tags that builds `simlin-mcp` for 4 platforms (macOS arm64, Linux arm64 musl, Linux x64 musl, Windows x64 mingw) and publishes 5 npm packages (`@simlin/mcp` + 4 platform packages) to npmjs.com.

2. **A local build/test script** (or Dockerfile/docker-compose) that lets you build and verify Linux + Windows binaries without GitHub Actions.

3. **Cleanup of existing scaffolding**: remove `mcp-darwin-x64`, update `simlin-mcp.js` to use `x86_64-pc-windows-gnu` triple, update `build-npm-packages.sh`.

4. **Minimal security surface**: least-privilege permissions on the workflow, no unnecessary secrets exposure.

## Acceptance Criteria

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

### mcp-npm-release.AC3: Local build works without CI
- **mcp-npm-release.AC3.1 Success:** `cross-build.sh` produces linux-x64, linux-arm64, and win32-x64 binaries via Docker
- **mcp-npm-release.AC3.2 Success:** Linux x64 binary runs on the host (smoke test)
- **mcp-npm-release.AC3.3 Success:** Script works on both x64 and arm64 development hosts

### mcp-npm-release.AC4: Scaffolding is clean
- **mcp-npm-release.AC4.1 Success:** `mcp-darwin-x64` package directory is removed
- **mcp-npm-release.AC4.2 Success:** JS launcher maps Windows to `x86_64-pc-windows-gnu` triple
- **mcp-npm-release.AC4.3 Success:** `build-npm-packages.sh` generates exactly 4 platform packages
- **mcp-npm-release.AC4.4 Success:** All 5 package.json files include `publishConfig.access: "public"` and `repository`

### mcp-npm-release.AC5: Minimal security surface
- **mcp-npm-release.AC5.1 Success:** Build jobs have only `contents: read` permission
- **mcp-npm-release.AC5.2 Success:** Only publish jobs have `id-token: write` permission
- **mcp-npm-release.AC5.3 Success:** No long-lived npm tokens stored in GitHub secrets

## Glossary

- **MCP (Model Context Protocol)**: A protocol for exposing tool capabilities to AI assistants via JSON-RPC over stdio. `simlin-mcp` is a server implementing this protocol.
- **musl**: A lightweight, statically-linkable C standard library for Linux. Building against musl produces fully static binaries with no runtime dependency on the host's glibc version.
- **PE binary**: Portable Executable, the standard executable format on Windows (`.exe` files).
- **mingw (MinGW-w64)**: A toolchain that provides Windows API headers and libraries on Linux, enabling cross-compilation of Windows executables. The `x86_64-pc-windows-gnu` Rust target uses this toolchain.
- **cargo-zigbuild**: A Cargo subcommand that uses the Zig compiler's cross-linker to cross-compile Rust code, replacing the need for separate cross-compilation toolchains.
- **optionalDependencies**: An npm `package.json` field listing packages that should be installed if available but whose installation failure is not an error. Platform-specific binary packages use this so npm silently skips packages whose `os`/`cpu` fields do not match the host.
- **OIDC Trusted Publishing**: An authentication mechanism where GitHub Actions requests a short-lived token from GitHub's OIDC provider, and npm validates it against a pre-configured trust relationship (repository + workflow file). Eliminates stored npm access tokens.
- **Provenance attestation**: A cryptographically signed statement generated by `npm publish --provenance` that links a published package version to the exact source commit and CI workflow run that produced it.
- **Target triple**: A string like `x86_64-unknown-linux-musl` that identifies a compilation target by CPU architecture, vendor/OS, and C library.
- **actionlint**: A static analysis tool for GitHub Actions workflow YAML files.

## Architecture

Cross-platform binary distribution for `@simlin/mcp` using the wrapper + platform-specific optional dependency pattern (same pattern used by Codex, esbuild, Biome).

### Package Structure

Five npm packages:

| Package | Contents |
|---------|----------|
| `@simlin/mcp` | JS launcher (`bin/simlin-mcp.js`) + `optionalDependencies` on platform packages |
| `@simlin/mcp-linux-x64` | Static musl binary for Linux x86-64 |
| `@simlin/mcp-linux-arm64` | Static musl binary for Linux arm64 |
| `@simlin/mcp-win32-x64` | PE binary for Windows x86-64 (mingw cross-compiled) |
| `@simlin/mcp-darwin-arm64` | Native binary for macOS Apple Silicon |

Platform packages declare `os` and `cpu` fields so npm/pnpm/yarn only install the matching one.

### Build Tooling

`cargo-zigbuild` handles all cross-compilation targets from a single toolchain. Zig provides a drop-in cross-linker with built-in musl and mingw support, eliminating the need for separate `musl-tools`, `aarch64-linux-musl-cross`, and `mingw-w64` packages.

| Target Triple | Platform Package | Runner |
|---------------|-----------------|--------|
| `x86_64-unknown-linux-musl` | `mcp-linux-x64` | `ubuntu-latest` |
| `aarch64-unknown-linux-musl` | `mcp-linux-arm64` | `ubuntu-latest` |
| `x86_64-pc-windows-gnu` | `mcp-win32-x64` | `ubuntu-latest` |
| `aarch64-apple-darwin` | `mcp-darwin-arm64` | `macos-latest` (native `cargo build`) |

macOS arm64 uses native `cargo build` since it cannot be cross-compiled from Linux.

### CI Workflow

`.github/workflows/mcp-release.yml` triggered by `mcp-v*` tag push. Three jobs:

1. **`build`** (matrix, 4 entries) -- compiles release binaries, uploads as artifacts.
2. **`publish-platform`** (needs: build) -- downloads artifacts, copies binaries into platform package `bin/` dirs, publishes 4 platform packages to npmjs.com.
3. **`publish-wrapper`** (needs: publish-platform) -- publishes `@simlin/mcp`. Must go last since `optionalDependencies` reference the platform packages by exact version.

### Authentication

npm Trusted Publishing (OIDC). No `NPM_TOKEN` secret stored in GitHub. The workflow requests a short-lived OIDC token from GitHub, which npm validates against a pre-configured Trusted Publisher for each package. Requires one-time manual setup on npmjs.com for each of the 5 packages, specifying `bpowers/simlin` as the repository and the exact workflow filename.

Workflow permissions: `contents: read` at workflow level. Publish jobs add `id-token: write` at job level (least-privilege -- build jobs don't get the OIDC permission).

### Version Strategy

`build-npm-packages.sh` reads the version from `Cargo.toml` and generates platform `package.json` files with that version. The wrapper `package.json` must also use the same version. The `mcp-v*` tag should match (e.g., `mcp-v0.1.0` for version `0.1.0` in `Cargo.toml`).

## Existing Patterns

The `pysimlin` release workflow (`.github/workflows/release.yml`) provides a reference for multi-platform builds with artifact upload/download, QEMU for arm64 Linux, and tag-triggered publishing. This design follows the same trigger pattern (`mcp-v*` vs `pysimlin-v*`) and artifact staging approach.

The npm package scaffolding already exists in `src/simlin-mcp/`: `bin/simlin-mcp.js` (JS launcher), `build-npm-packages.sh` (platform package generator), `package.json` (wrapper), and `npm/@simlin/` (platform package directories). This design builds on that scaffolding rather than replacing it.

The JS launcher follows the Codex pattern (`~/src/codex/codex-cli/bin/codex.js`): detect OS/arch, resolve platform package via `require.resolve`, fall back to local `vendor/` path for development.

## Implementation Phases

<!-- START_PHASE_1 -->
### Phase 1: Scaffolding Cleanup

**Goal:** Remove stale darwin-x64 support, fix Windows triple, add npm publish configuration.

**Components:**
- Delete `src/simlin-mcp/npm/@simlin/mcp-darwin-x64/` directory
- Update `src/simlin-mcp/build-npm-packages.sh` -- remove `darwin-x64` from `PLATFORMS` array
- Update `src/simlin-mcp/bin/simlin-mcp.js` -- change Windows triple to `x86_64-pc-windows-gnu`, remove `darwin-x64` entry
- Add `publishConfig: { "access": "public" }` and `repository` field to all 5 `package.json` files (wrapper + 4 platform packages)

**Dependencies:** None

**Done when:** `build-npm-packages.sh` runs successfully, produces 4 platform packages (not 5). JS launcher maps to correct triples for all 4 supported platforms.
<!-- END_PHASE_1 -->

<!-- START_PHASE_2 -->
### Phase 2: Local Cross-Build Tooling

**Goal:** Dockerfile and script that build all 3 Linux-hosted targets (linux x64, linux arm64, windows x64) locally.

**Components:**
- `src/simlin-mcp/Dockerfile.cross` -- based on `rust:bookworm`, installs zig + cargo-zigbuild + all 3 Rust targets
- `src/simlin-mcp/scripts/cross-build.sh` -- builds Docker image, runs cross-compilation for all 3 targets, copies binaries to `dist/`

**Dependencies:** Phase 1

**Done when:** Running `./scripts/cross-build.sh` from `src/simlin-mcp/` produces 3 binaries in `dist/`: `x86_64-unknown-linux-musl/simlin-mcp`, `aarch64-unknown-linux-musl/simlin-mcp`, `x86_64-pc-windows-gnu/simlin-mcp.exe`. Linux x64 binary runs and responds to `--help` (or equivalent smoke check).
<!-- END_PHASE_2 -->

<!-- START_PHASE_3 -->
### Phase 3: GitHub Actions Workflow

**Goal:** CI workflow that builds all 4 platform binaries and publishes the 5 npm packages.

**Components:**
- `.github/workflows/mcp-release.yml` -- full workflow with build matrix, publish-platform, and publish-wrapper jobs
- Build job matrix: 3 entries on `ubuntu-latest` (cargo-zigbuild) + 1 entry on `macos-latest` (native cargo)
- Publish jobs with `id-token: write` for OIDC trusted publishing
- `npm publish --provenance` for supply chain attestation

**Dependencies:** Phase 1 (scaffolding), Phase 2 (validates the Dockerfile/cross-build approach, though CI installs tools directly rather than using Docker)

**Done when:** Workflow file passes `actionlint` or equivalent static check. Manual `workflow_dispatch` test (or dry run) confirms the build matrix produces all 4 artifacts.
<!-- END_PHASE_3 -->

<!-- START_PHASE_4 -->
### Phase 4: Manual Verification and npm Trusted Publisher Setup

**Goal:** End-to-end validation with a real tag push and npm publish.

**Components:**
- One-time npmjs.com Trusted Publisher configuration for all 5 packages (manual, documented in this plan)
- Test publish with a pre-release version (e.g., `mcp-v0.1.0-rc.1`) to verify the full pipeline
- Verify installed package works: `npx @simlin/mcp --help` on at least one platform

**Dependencies:** Phase 3

**Done when:** All 5 packages published to npmjs.com. `npm install -g @simlin/mcp` installs correctly and the binary runs on at least one platform.
<!-- END_PHASE_4 -->

## Additional Considerations

**Executable bit preservation:** GitHub Actions artifact upload/download strips the executable bit from files. The publish job must `chmod +x` non-Windows binaries before packaging.

**npm CLI version:** Trusted publishing requires npm >= 11.5.1. The workflow should install a recent npm version explicitly.

**Provenance:** `npm publish --provenance` generates a signed attestation linking the package to the commit and workflow run. This works automatically with OIDC trusted publishing on public repositories.
