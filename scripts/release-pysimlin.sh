#!/usr/bin/env bash
#
# Tag a pysimlin release.
#
# Updates pysimlin.version, commits, and creates a git tag. Does NOT push
# automatically -- prints instructions so the caller can review first.
#
# Pushing the tag triggers .github/workflows/release.yml, which builds the
# notebook widget's assets once (widget-assets job: pnpm build + wasm-opt +
# src/pysimlin/scripts/stage_widget_assets.py --require-opt), packs that one
# set into every platform wheel via cibuildwheel, and checks each wheel for
# them before publishing. Nothing about the assets needs to happen locally
# for the release itself; the preflight below only proves the widget builds
# on this tree so a broken bundle is found before the tag exists, not by a
# failed workflow after it is pushed. Set SKIP_ASSET_PREFLIGHT=1 to skip it
# (a few minutes: engine wasm + diagram + widget builds).
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

usage() {
  echo "Usage: $0 <version>" >&2
  echo "  version must be semver: MAJOR.MINOR.PATCH (e.g. 1.2.3)" >&2
  exit 1
}

if [ $# -ne 1 ]; then
  usage
fi

VERSION="$1"

if ! [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "error: invalid version '$VERSION' -- must match MAJOR.MINOR.PATCH" >&2
  usage
fi

cd "$REPO_ROOT"

if ! git diff --quiet || ! git diff --cached --quiet; then
  echo "error: working tree is dirty -- commit or stash changes first" >&2
  exit 1
fi

if [ "${SKIP_ASSET_PREFLIGHT:-0}" != "1" ]; then
  echo "Preflight: building and staging the notebook widget assets..."
  if ! python3 src/pysimlin/scripts/stage_widget_assets.py; then
    echo "error: the widget asset build failed; fix it before tagging" >&2
    exit 1
  fi
fi

VERSION_FILE="src/simlin-mcp/pysimlin.version"

# Tag before committing the version-file bump: the pre-commit hook runs
# simlin-mcp's pysimlin_version_matches_latest_tag test, which requires the
# pysimlin-v* tag matching pysimlin.version to already exist. The tag lands
# on the last content commit -- which is also what setuptools-scm derives
# pysimlin's version from; the bump commit only refreshes MCP instructions.
git tag "pysimlin-v$VERSION"

printf '%s\n' "$VERSION" > "$VERSION_FILE"
git add "$VERSION_FILE"
git commit -m "mcp: update pysimlin version reference to $VERSION"

echo "Tagged pysimlin-v$VERSION. Push with: git push origin main pysimlin-v$VERSION"
