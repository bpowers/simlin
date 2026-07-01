#!/usr/bin/env bash
#
# Deploy the simlin web app to Google App Engine from a self-contained
# staging directory (instead of the workspace root).
#
# Why a separate script from deploy-web.sh:
# - deploy-web.sh runs `gcloud app deploy` from the repo root, so the GAE
#   instance's `pnpm install` walks the whole pnpm workspace and installs
#   EVERY package's dependency closure (rspress, vite, slate, jest,
#   @rsbuild/*, ...): ~590 MB / 1171 packages, none of which the server
#   needs at runtime. App Engine standard has no vendored-node_modules
#   escape hatch -- the only lever is which package.json + lockfile the
#   deploy points at.
# - This script stages a directory whose package.json is exactly the
#   server's prod closure (~80 MB / 230 packages), with @simlin/core and
#   @simlin/engine vendored as file: deps (they are not published to npm),
#   then deploys THAT. See scripts/build-deploy-staging.mjs and
#   docs/dev/deploy.md.
#
# deploy-web.sh is kept as the proven fallback until this path has been
# validated against a real `gcloud app deploy --no-promote`.
#
# The shape (trap-based cleanup, validate-then-build) mirrors deploy-web.sh.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

cd "$REPO_ROOT"

STAGING_DIR="$REPO_ROOT/deploy-staging"

CLEANUP_DONE=0

cleanup() {
    # Idempotent guard: both the normal-exit and trapped-exit paths fire.
    if [ "$CLEANUP_DONE" -eq 1 ]; then
        return
    fi
    CLEANUP_DONE=1
    echo ""
    echo "==> Restoring tracked symlinks and removing build artifacts (deploy:clean)"
    # deploy:assemble copies build output into the tracked public/ and drops
    # the symlinks; restore them even if a step above failed. The staging dir
    # itself is gitignored and intentionally LEFT in place so a --no-promote
    # smoke test can inspect exactly what was uploaded; the next run rebuilds
    # it from scratch.
    pnpm --filter @simlin/app run deploy:clean || \
        echo "WARNING: deploy:clean failed; you may need to run 'git checkout -- public src/server' and 'rm -rf src/app/build src/app/build-component' manually."
}

trap cleanup EXIT INT TERM

if [ ! -f "$REPO_ROOT/.app.prod.yaml" ]; then
    echo "ERROR: .app.prod.yaml not found in $REPO_ROOT" >&2
    echo "       It is gitignored and lives only on your machine. See docs/dev/deploy.md." >&2
    exit 1
fi

node "$REPO_ROOT/scripts/validate-app-prod-config.mjs" "$REPO_ROOT/.app.prod.yaml"

export NODE_ENV=production

echo "==> pnpm clean"
pnpm clean

echo "==> pnpm build"
pnpm build

echo "==> Staging app build into public/ (pnpm --filter @simlin/app run deploy:assemble)"
pnpm --filter @simlin/app run deploy:assemble

echo "==> Verifying assembled build artifacts (scripts/verify-deploy-build.sh)"
bash "$REPO_ROOT/scripts/verify-deploy-build.sh"

echo "==> Assembling self-contained server staging dir (scripts/build-deploy-staging.mjs)"
node "$REPO_ROOT/scripts/build-deploy-staging.mjs" "$STAGING_DIR" "$REPO_ROOT/.app.prod.yaml"

# The staging dir is bounded by construction (build-deploy-staging.mjs copies
# an explicit file list), so this gate is cheap here -- it exists to catch a
# regression in the staging assembly (e.g. accidentally vendoring a
# node_modules tree) before the upload starts. See issue #695.
echo "==> Checking upload file count against the GAE 10k cap (scripts/check-upload-file-count.sh)"
bash "$REPO_ROOT/scripts/check-upload-file-count.sh" "$STAGING_DIR"

echo "==> gcloud app deploy $STAGING_DIR/app.yaml"
gcloud app deploy "$STAGING_DIR/app.yaml" "$@"

# cleanup runs here via the EXIT trap.
