#!/usr/bin/env bash
#
# Deploy the simlin web app (SPA + Express server) to Google App Engine.
#
# Reasoning for shape:
# - Implemented as a script (rather than a one-line `&&` chain in
#   package.json) so we can install a single `trap` that ALWAYS runs
#   `pnpm --filter @simlin/app run deploy:clean`. The previous one-line
#   chain left build artifacts in `public/` and the symlinks dropped
#   if `gcloud app deploy` was interrupted, because no cleanup ran on
#   the failure / Ctrl-C path.
# - Lives at `scripts/deploy-web.sh` and is invoked via the
#   `deploy:web` script in the root package.json. The script name was
#   moved off `deploy` because pnpm 10 has a built-in `pnpm deploy`
#   subcommand that shadows any same-named script -- typing
#   `pnpm deploy` ran the built-in (which errors with
#   ERR_PNPM_NOTHING_TO_DEPLOY) instead of this pipeline.
# - Still mutates the tracked `public/` directory in place (the
#   underlying `pnpm --filter @simlin/app run deploy:assemble` step
#   copies build output there and drops the symlinks; the
#   `deploy:clean` step restores them). That is the historical shape
#   we have not yet replaced; the cleaner staging-dir approach is
#   tracked as tech debt (see docs/tech-debt.md, "Web deploy mutates
#   tracked public/").

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

cd "$REPO_ROOT"

CLEANUP_DONE=0

cleanup() {
    # Idempotent: a normal exit and a trapped exit both fire, so guard
    # against running deploy:clean twice (the second run would still
    # succeed -- it's `git checkout HEAD --` on tracked paths -- but
    # is noisy in the log).
    if [ "$CLEANUP_DONE" -eq 1 ]; then
        return
    fi
    CLEANUP_DONE=1
    echo ""
    echo "==> Restoring tracked symlinks and removing build artifacts (deploy:clean)"
    # Run cleanup even if it itself partially fails; we don't want a
    # cleanup error to mask the real exit code from the deploy step.
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

# Gate on the real upload set right before the deploy: this deploy uploads
# from the repo root, where the upload set is whatever .gcloudignore leaves
# in -- independent of git status, and including files the build steps above
# just created. Failing here (instead of inside gcloud app deploy) names the
# offending directories and still runs the cleanup trap. See issue #695.
echo "==> Checking upload file count against the GAE 10k cap (scripts/check-upload-file-count.sh)"
bash "$REPO_ROOT/scripts/check-upload-file-count.sh" "$REPO_ROOT"

echo "==> gcloud app deploy ./.app.prod.yaml"
gcloud app deploy "$REPO_ROOT/.app.prod.yaml" "$@"

# cleanup runs here via the EXIT trap.
