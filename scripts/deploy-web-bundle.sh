#!/usr/bin/env bash
#
# Copyright 2026 The Simlin Authors. All rights reserved.
# Use of this source code is governed by the Apache License,
# Version 2.0, that can be found in the LICENSE file.
#
# Experimental alternative to `pnpm deploy:web`.
#
# `pnpm deploy:web` (scripts/deploy-web.sh) runs `gcloud app deploy` from the
# monorepo root, so GAE's Node buildpack re-runs `pnpm install` *at the root*
# on the instance -- which installs the production dependencies of every
# workspace package (src/diagram's slate/radix/recharts, website's rspress,
# src/simlin-serve/web's vite, ...) and, because pnpm v10 + NODE_ENV=production
# does not skip devDependencies (GoogleCloudPlatform/buildpacks#591),
# src/app's @rsbuild/*, src/server's firebase-tools, jest, eslint, and so on.
# The result is a multi-hundred-MB install of which the Express server actually
# needs ~100 MB.
#
# This script instead ships a self-contained server bundle: `pnpm deploy`
# produces a directory with @simlin/server's *production* deps only (incl.
# @simlin/core and @simlin/engine materialized, with the WASM), and then
# `gcloud app deploy` runs from that directory. The workspace: dep specifiers
# are rewritten to `file:` refs against a vendored copy, and the bundle ships a
# regenerated `pnpm-lock.yaml` matching the rewritten `package.json`, so GAE's
# instance-side install (which it always runs) is a fast frozen-lockfile no-op.
#
# STATUS: validated locally end to end (the bundle is ~100 MB vs ~800 MB for the
# full workspace install; the server resolves all modules and starts from it;
# `pnpm install --frozen-lockfile` in a copy of the bundle -- standing in for
# GAE's instance-side install -- is a fast no-op). The one thing only a real
# `gcloud app deploy` can confirm is GAE's buildpack behavior on the bundle. So:
# run `pnpm deploy:web:bundle --no-promote` first, smoke-test the version URL,
# and only then promote / make this the default. See docs/dev/deploy.md.

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

bundle_dir="$repo_root/.deploy-bundle"
CLEANUP_DONE=0

if [ ! -f "$repo_root/.app.prod.yaml" ]; then
  echo "error: .app.prod.yaml not found in $repo_root (it is gitignored; you keep it locally)" >&2
  exit 1
fi

cleanup() {
  # Restore the public/ symlinks and tracked index.html that deploy:assemble
  # mutated, and remove build + bundle artifacts. Runs on success, failure,
  # and Ctrl-C so an interrupted deploy leaves a clean working tree. The
  # guard makes it idempotent: on Ctrl-C the INT and EXIT traps both fire.
  if [ "$CLEANUP_DONE" -eq 1 ]; then
    return 0
  fi
  CLEANUP_DONE=1
  pnpm --filter @simlin/app run deploy:clean >/dev/null 2>&1 || true
  rm -rf "$bundle_dir" "$repo_root/src/app/build" "$repo_root/src/app/build-component" >/dev/null 2>&1 || true
}
trap cleanup EXIT INT TERM

export NODE_ENV=production

echo "==> pnpm clean && pnpm build"
pnpm clean
pnpm build

echo "==> assembling SPA static assets into public/"
pnpm --filter @simlin/app run deploy:assemble

echo "==> pnpm deploy --legacy --prod -> $bundle_dir"
rm -rf "$bundle_dir"
# --legacy: pnpm v10 refuses a non-injected deploy by default; --legacy uses
# the pre-v10 implementation, which materializes @simlin/core / @simlin/engine
# (hard-linked from the store) under .deploy-bundle/node_modules/.pnpm/.
pnpm --filter=@simlin/server deploy --legacy --prod "$bundle_dir"

echo "==> vendoring @simlin/* and rewriting workspace: specifiers"
# pnpm deploy leaves node_modules/@simlin/{core,engine} as symlinks into the
# bundle's .pnpm store and leaves "workspace:*" in package.json -- which an
# install outside a workspace (GAE always re-installs on the instance) can't
# resolve. So: copy the real package contents out to vendor/, drop the .pnpm
# @simlin entries, and rewrite the specifiers to file: refs. -L resolves the
# symlinks to real files.
mkdir -p "$bundle_dir/vendor/@simlin"
cp -RL "$bundle_dir/node_modules/@simlin/core" "$bundle_dir/vendor/@simlin/core"
cp -RL "$bundle_dir/node_modules/@simlin/engine" "$bundle_dir/vendor/@simlin/engine"
rm -rf "$bundle_dir/node_modules/@simlin" \
       "$bundle_dir"/node_modules/.pnpm/@simlin+core* \
       "$bundle_dir"/node_modules/.pnpm/@simlin+engine* \
       "$bundle_dir/vendor/@simlin/core/node_modules/@simlin" \
       "$bundle_dir/vendor/@simlin/engine/node_modules/@simlin" 2>/dev/null || true

# Rewrite the bundle's package.json: workspace: -> file: refs, drop
# devDependencies (the bundle is runtime-only), keep only the `start` script.
node - "$bundle_dir/package.json" <<'EOF'
const fs = require('fs');
const path = process.argv[2];
const pkg = JSON.parse(fs.readFileSync(path, 'utf8'));
pkg.dependencies = pkg.dependencies || {};
pkg.dependencies['@simlin/core'] = 'file:./vendor/@simlin/core';
pkg.dependencies['@simlin/engine'] = 'file:./vendor/@simlin/engine';
delete pkg.devDependencies;
pkg.scripts = { start: 'node lib/index.js' };
fs.writeFileSync(path, JSON.stringify(pkg, null, 2) + '\n');
EOF

# Rewrite the vendored @simlin/* package.json files: drop devDependencies
# (the bundle is runtime-only) and rewrite @simlin/core's @simlin/engine ref
# so the file: dep resolves it relative to vendor/.
node - "$bundle_dir/vendor/@simlin/core/package.json" "$bundle_dir/vendor/@simlin/engine/package.json" <<'EOF'
const fs = require('fs');
for (const p of process.argv.slice(2)) {
  const pkg = JSON.parse(fs.readFileSync(p, 'utf8'));
  if (pkg.dependencies && pkg.dependencies['@simlin/engine']) {
    pkg.dependencies['@simlin/engine'] = 'file:../engine';
  }
  delete pkg.devDependencies;
  fs.writeFileSync(p, JSON.stringify(pkg, null, 2) + '\n');
}
EOF

echo "==> regenerating the bundle's pnpm-lock.yaml"
# Generate a lockfile that matches the rewritten package.json so GAE's
# instance-side install (which it always runs) is a fast frozen-lockfile no-op.
# Remove the lockfile artifacts pnpm deploy left behind first so the
# regenerated one has no stale @simlin/*@file:src/... entries (those monorepo
# paths don't exist in the bundle). --ignore-workspace: the bundle is a
# standalone project, not a member of the monorepo it was copied from.
# --lockfile-only leaves node_modules (the materialized prod tree) untouched.
rm -f "$bundle_dir/pnpm-lock.yaml" "$bundle_dir/node_modules/.modules.yaml" \
      "$bundle_dir/node_modules/.pnpm/lock.yaml"
( cd "$bundle_dir" && pnpm install --lockfile-only --ignore-workspace )

echo "==> copying config/, default_projects/, public/, and app.yaml into the bundle"
cp -RL "$repo_root/config" "$bundle_dir/config"
cp -RL "$repo_root/default_projects" "$bundle_dir/default_projects"
cp -RL "$repo_root/public" "$bundle_dir/public"
cp "$repo_root/.app.prod.yaml" "$bundle_dir/app.yaml"

# pnpm deploy copies @simlin/server's whole directory into the bundle root,
# including the .ts source (lib/ is the compiled output that actually runs),
# tests/, and dev config -- none of it needed at runtime. Exclude it from the
# upload via a .gcloudignore in the bundle. Do NOT exclude node_modules /
# vendor / public / config / default_projects / lib -- those are load-bearing.
cat > "$bundle_dir/.gcloudignore" <<'EOF'
/*.ts
/tests/
/eslint.config.js
/jest.config.js
/tsconfig.json
/CLAUDE.md
/AGENTS.md
EOF

echo "==> gcloud app deploy (from $bundle_dir)"
( cd "$bundle_dir" && gcloud app deploy app.yaml "$@" )

echo "==> done"
