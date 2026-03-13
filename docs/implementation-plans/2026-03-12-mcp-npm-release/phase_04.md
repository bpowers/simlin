# MCP npm Release -- Phase 4: Manual Verification and npm Trusted Publisher Setup

**Goal:** End-to-end validation with real npm publishes and OIDC configuration.

**Architecture:** Manual steps: create npm org if needed, bootstrap packages with placeholder publishes, configure Trusted Publisher for each package, then verify the full CI pipeline with a pre-release tag.

**Tech Stack:** npm CLI, npmjs.com web UI, GitHub Actions, git tags

**Scope:** Phase 4 of 4 from original design

**Codebase verified:** 2026-03-12

---

## Acceptance Criteria Coverage

This phase verifies all acceptance criteria end-to-end. No new code is written -- this phase validates the infrastructure from Phases 1-3.

### mcp-npm-release.AC1: CI workflow builds all 4 platforms
- **mcp-npm-release.AC1.1 Success:** `mcp-v*` tag push triggers the workflow

### mcp-npm-release.AC2: npm packages publish correctly
- **mcp-npm-release.AC2.1 Success:** 4 platform packages publish before the wrapper package
- **mcp-npm-release.AC2.2 Success:** Wrapper `@simlin/mcp` publishes with correct `optionalDependencies` versions
- **mcp-npm-release.AC2.3 Success:** All packages publish with provenance attestation
- **mcp-npm-release.AC2.4 Success:** Authentication uses OIDC (no stored NPM_TOKEN)

---

## External Dependency Findings

- First version of each package MUST be published manually before OIDC can be configured (npmjs.com hard requirement).
- No CLI or API for Trusted Publisher configuration -- web UI only at `https://www.npmjs.com/package/<name>/access`.
- The `@simlin` npm organization must exist on npmjs.com before scoped packages can be published.
- Trusted Publisher fields: owner (`bpowers`), repository (`simlin`), workflow filename (`mcp-release.yml`), environment (leave blank).
- The workflow filename is the bare filename only, not the full `.github/workflows/` path.
- `--access public` is required on first publish of scoped packages (they default to restricted/private).

---

<!-- START_TASK_1 -->
### Task 1: Ensure @simlin npm organization exists

**Verifies:** None (prerequisite)

**Steps:**

1. Check if the `@simlin` scope exists on npm:
   ```bash
   npm org ls simlin 2>/dev/null || echo "org does not exist"
   ```

2. If the organization does not exist, create it:
   - Go to `https://www.npmjs.com/org/create`
   - Create organization named `simlin`
   - This is free for public packages

3. Verify:
   ```bash
   npm org ls simlin
   ```
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Bootstrap packages with initial manual publish

**Verifies:** None (prerequisite for OIDC configuration)

**Steps:**

The first version of each package must be published manually so that OIDC Trusted Publisher can be configured. Use a pre-release version (`0.0.1-bootstrap.1`) to avoid permanently creating a broken `0.1.0` on npm (published packages are immutable).

1. Authenticate to npm:
   ```bash
   npm login
   ```

2. Ensure all changes from Phases 1-3 are committed and the code is on a branch where `build-npm-packages.sh` produces correct output.

3. Temporarily set `Cargo.toml` version to the bootstrap version:
   ```bash
   cd src/simlin-mcp
   # Save original version
   ORIG_VERSION=$(grep '^version = ' Cargo.toml | head -1 | sed 's/version = "\(.*\)"/\1/')
   sed -i 's/^version = ".*"/version = "0.0.1-bootstrap.1"/' Cargo.toml
   ```

4. Generate platform packages with the bootstrap version:
   ```bash
   bash build-npm-packages.sh
   ```

5. Publish each platform package (placeholder -- no real binary needed for this step):
   ```bash
   cd npm/@simlin/mcp-darwin-arm64 && npm publish --access public && cd -
   cd npm/@simlin/mcp-linux-arm64 && npm publish --access public && cd -
   cd npm/@simlin/mcp-linux-x64 && npm publish --access public && cd -
   cd npm/@simlin/mcp-win32-x64 && npm publish --access public && cd -
   ```

6. Update wrapper `package.json` version and publish:
   ```bash
   cd ..  # back to src/simlin-mcp
   jq '.version = "0.0.1-bootstrap.1" | .optionalDependencies = (.optionalDependencies | to_entries | map(.value = "0.0.1-bootstrap.1") | from_entries)' package.json > package.json.tmp && mv package.json.tmp package.json
   npm publish --access public
   ```

7. Restore original version:
   ```bash
   sed -i "s/^version = \".*\"/version = \"$ORIG_VERSION\"/" Cargo.toml
   git checkout package.json  # restore committed wrapper version
   ```

8. Verify all 5 packages exist:
   ```bash
   npm view @simlin/mcp version
   npm view @simlin/mcp-darwin-arm64 version
   npm view @simlin/mcp-linux-arm64 version
   npm view @simlin/mcp-linux-x64 version
   npm view @simlin/mcp-win32-x64 version
   ```

9. Deprecate the bootstrap versions so users don't accidentally install them:
   ```bash
   npm deprecate @simlin/mcp@0.0.1-bootstrap.1 "Bootstrap placeholder -- use latest release"
   npm deprecate @simlin/mcp-darwin-arm64@0.0.1-bootstrap.1 "Bootstrap placeholder"
   npm deprecate @simlin/mcp-linux-arm64@0.0.1-bootstrap.1 "Bootstrap placeholder"
   npm deprecate @simlin/mcp-linux-x64@0.0.1-bootstrap.1 "Bootstrap placeholder"
   npm deprecate @simlin/mcp-win32-x64@0.0.1-bootstrap.1 "Bootstrap placeholder"
   ```

**Note:** These bootstrap versions are placeholders that exist solely to enable OIDC configuration. They are deprecated immediately. The first real release (via CI) publishes the actual version with binaries.
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Configure Trusted Publisher for all 5 packages

**Verifies:** mcp-npm-release.AC2.4 (OIDC authentication), mcp-npm-release.AC5.3 (no stored tokens)

**Steps:**

For each of the 5 packages, configure Trusted Publisher on npmjs.com:

1. Navigate to the package's access settings page. URLs:
   - `https://www.npmjs.com/package/@simlin/mcp/access`
   - `https://www.npmjs.com/package/@simlin/mcp-darwin-arm64/access`
   - `https://www.npmjs.com/package/@simlin/mcp-linux-arm64/access`
   - `https://www.npmjs.com/package/@simlin/mcp-linux-x64/access`
   - `https://www.npmjs.com/package/@simlin/mcp-win32-x64/access`

2. In the **Trusted Publisher** section, click **GitHub Actions**.

3. Fill in the fields (identical for all 5 packages):
   - **Organization or user:** `bpowers`
   - **Repository:** `simlin`
   - **Workflow filename:** `mcp-release.yml`
   - **Environment:** (leave blank)

4. Click **Set up connection** (or equivalent save button).

5. Repeat for all 5 packages.

**Verify:** Each package's access page shows the configured Trusted Publisher.
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Test the full pipeline with a pre-release tag

**Verifies:** mcp-npm-release.AC1.1, mcp-npm-release.AC2.1, mcp-npm-release.AC2.2, mcp-npm-release.AC2.3, mcp-npm-release.AC2.4

**Steps:**

1. First, bump the version in `Cargo.toml` to a pre-release version:
   ```bash
   # In src/simlin-mcp/Cargo.toml, change version to "0.1.1-rc.1" (or next available)
   ```

2. Commit the version bump:
   ```bash
   git add src/simlin-mcp/Cargo.toml
   git commit -m "mcp: bump version to 0.1.1-rc.1 for release pipeline test"
   ```

3. Push the branch and create a tag:
   ```bash
   git push origin mcp-npm-release
   git tag mcp-v0.1.1-rc.1
   git push origin mcp-v0.1.1-rc.1
   ```

4. Monitor the GitHub Actions workflow:
   - Go to `https://github.com/bpowers/simlin/actions`
   - Find the "MCP npm Release" workflow run triggered by the tag
   - Verify all build matrix jobs succeed (4 platform binaries built)
   - Verify `publish-platform` job publishes all 4 platform packages
   - Verify `publish-wrapper` job publishes `@simlin/mcp`

5. Verify published packages:
   ```bash
   npm view @simlin/mcp@0.1.1-rc.1
   npm view @simlin/mcp-linux-x64@0.1.1-rc.1
   ```

6. Check provenance attestation is present:
   - Visit `https://www.npmjs.com/package/@simlin/mcp/v/0.1.1-rc.1`
   - Look for the "Provenance" badge/section
<!-- END_TASK_4 -->

<!-- START_TASK_5 -->
### Task 5: Verify end-to-end installation and execution

**Verifies:** All ACs (end-to-end validation)

**Steps:**

1. Install the package globally:
   ```bash
   npm install -g @simlin/mcp@0.1.1-rc.1
   ```

2. Verify the correct platform binary was installed:
   ```bash
   which simlin-mcp
   file $(which simlin-mcp)
   ```

3. Verify the binary runs (simlin-mcp is a stdio MCP server, so it will wait for input):
   ```bash
   # Send an empty input and check it starts
   echo '' | timeout 2 simlin-mcp || true
   # A non-zero exit is fine -- it proves the binary loaded and executed
   ```

4. Clean up:
   ```bash
   npm uninstall -g @simlin/mcp
   ```

**Done when:** `npm install -g @simlin/mcp` installs correctly and the binary runs on at least one platform.
<!-- END_TASK_5 -->
