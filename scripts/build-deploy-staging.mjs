#!/usr/bin/env node
//
// Assemble a self-contained server deploy staging directory.
//
// App Engine standard's Node buildpack ALWAYS runs `pnpm install` on the
// instance from the deployed package.json + lockfile, and offers no vendored-
// node_modules escape hatch. Deploying the workspace root therefore makes the
// instance install every workspace package's dependency closure (rspress,
// vite, slate, jest, @rsbuild/*, ...) -- ~590 MB / 1171 packages -- none of
// which the server needs at runtime. This builds a directory whose
// package.json is exactly the server's prod closure (~80 MB / 230 packages),
// with @simlin/core and @simlin/engine vendored as file: deps (they aren't
// published to npm, so a registry install would 404).
//
// Pre-req: `pnpm build` and `pnpm --filter @simlin/app run deploy:assemble`
// have already run, so the compiled lib/, the full server-side wasm, and the
// assembled public/ exist. This script does NOT build; it stages.
//
// Usage: node scripts/build-deploy-staging.mjs [stagingDir] [prodYaml]
//   stagingDir default: <repo>/deploy-staging
//   prodYaml   default: <repo>/.app.prod.yaml

import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { parse as parseYaml, stringify as stringifyYaml } from 'yaml';

import {
  buildStagingServerManifest,
  rewriteVendoredManifest,
  assertNoWorkspaceProtocol,
  stagingGcloudignore,
  seedLockfileFromRoot,
  untestedPackages,
} from './deploy-staging-manifests.mjs';

const SCRIPT_DIR = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(SCRIPT_DIR, '..');

const PNG_EXPORT = 'simlin_project_render_png';

function die(msg) {
  console.error(`ERROR: ${msg}`);
  process.exit(1);
}

function readJson(file) {
  return JSON.parse(fs.readFileSync(file, 'utf8'));
}

function writeJson(file, obj) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, JSON.stringify(obj, null, 2) + '\n');
}

// Copy a directory tree. Fails loud if the source is missing -- a missing
// source here means a build step was skipped, which must not silently ship.
function copyDir(src, dest, label) {
  if (!fs.existsSync(src)) {
    die(`${label} not found at ${src} -- did 'pnpm build' (+ deploy:assemble) run?`);
  }
  fs.mkdirSync(path.dirname(dest), { recursive: true });
  fs.cpSync(src, dest, { recursive: true });
}

function copyFile(src, dest, label) {
  if (!fs.existsSync(src)) {
    die(`${label} not found at ${src} -- did 'pnpm build' run?`);
  }
  fs.mkdirSync(path.dirname(dest), { recursive: true });
  fs.copyFileSync(src, dest);
}

function wasmHasExport(file, name) {
  // The export name appears as a literal string in the wasm export section.
  return fs.readFileSync(file).includes(Buffer.from(name, 'latin1'));
}

function dirSize(dir) {
  let total = 0;
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      total += dirSize(p);
    } else if (entry.isFile()) {
      total += fs.statSync(p).size;
    }
  }
  return total;
}

// The staged layout is flat: the compiled server lives at ./lib and is run by
// the staging package.json `start` script (`node lib/index.js`), which GAE
// invokes when app.yaml has no `entrypoint`. A repo-root-style entrypoint
// (e.g. `node src/server/lib`, which is correct for the fallback deploy-web.sh)
// would point at a path that doesn't exist on the instance under this layout.
// Catch that mismatch before the deploy rather than crash-looping the instance.
function assertCompatibleEntrypoint(prodYaml) {
  const m = fs.readFileSync(prodYaml, 'utf8').match(/^entrypoint:[ \t]*(.+?)[ \t]*$/m);
  if (!m) {
    return; // no entrypoint -> GAE runs the package.json start script. Correct.
  }
  const entrypoint = m[1].trim();
  if (/src\/server/.test(entrypoint) || !/\blib\/index\.js\b/.test(entrypoint)) {
    die(
      `${prodYaml} sets entrypoint: "${entrypoint}", which will not resolve in the flat staging ` +
        `layout. Remove the entrypoint (the staging package.json start script runs ` +
        `"node lib/index.js") or set it to "node lib/index.js".`,
    );
  }
}

function main() {
  const stagingDir = path.resolve(process.argv[2] ?? path.join(REPO_ROOT, 'deploy-staging'));
  const prodYaml = path.resolve(process.argv[3] ?? path.join(REPO_ROOT, '.app.prod.yaml'));

  if (stagingDir === REPO_ROOT) {
    die('refusing to use the repo root as the staging directory');
  }
  if (!fs.existsSync(prodYaml)) {
    die(`production app.yaml not found at ${prodYaml} (gitignored; see docs/dev/deploy.md)`);
  }
  assertCompatibleEntrypoint(prodYaml);

  const rootPkg = readJson(path.join(REPO_ROOT, 'package.json'));
  const serverPkg = readJson(path.join(REPO_ROOT, 'src/server/package.json'));
  const enginePkg = readJson(path.join(REPO_ROOT, 'src/engine/package.json'));
  const corePkg = readJson(path.join(REPO_ROOT, 'src/core/package.json'));

  console.log(`==> Assembling server deploy staging dir: ${stagingDir}`);

  // 1. Clean.
  fs.rmSync(stagingDir, { recursive: true, force: true });
  fs.mkdirSync(stagingDir, { recursive: true });

  // 2. Server compiled output + runtime assets (resolved relative to CWD at
  //    runtime: ./config, ./default_projects, ./public).
  copyDir(path.join(REPO_ROOT, 'src/server/lib'), path.join(stagingDir, 'lib'), 'server lib');
  copyDir(path.join(REPO_ROOT, 'config'), path.join(stagingDir, 'config'), 'config');
  copyDir(path.join(REPO_ROOT, 'default_projects'), path.join(stagingDir, 'default_projects'), 'default_projects');
  copyDir(path.join(REPO_ROOT, 'public'), path.join(stagingDir, 'public'), 'assembled public/');

  // 3. Vendor @simlin/engine: the Node runtime resolves its exports to lib/
  //    (the `node` condition) and loads core/libsimlin.wasm via __dirname. The
  //    browser variants (lib.browser, libsimlin-browser.wasm) are never
  //    selected by plain `require`, so they are deliberately NOT vendored --
  //    that keeps the 4.8 MB slim wasm off the server image.
  const engineDir = path.join(stagingDir, 'vendor/engine');
  copyDir(path.join(REPO_ROOT, 'src/engine/lib'), path.join(engineDir, 'lib'), 'engine lib');
  copyFile(
    path.join(REPO_ROOT, 'src/engine/core/libsimlin.wasm'),
    path.join(engineDir, 'core/libsimlin.wasm'),
    'engine wasm',
  );
  writeJson(path.join(engineDir, 'package.json'), rewriteVendoredManifest(enginePkg));

  // 4. Vendor @simlin/core: lib/ + manifest with its @simlin/engine dep
  //    rewritten to file:../engine.
  const coreDir = path.join(stagingDir, 'vendor/core');
  copyDir(path.join(REPO_ROOT, 'src/core/lib'), path.join(coreDir, 'lib'), 'core lib');
  const vendoredCore = rewriteVendoredManifest(corePkg);
  assertNoWorkspaceProtocol(vendoredCore.dependencies ?? {}, 'vendored @simlin/core');
  writeJson(path.join(coreDir, 'package.json'), vendoredCore);

  // 5. Staging package.json (server prod closure, workspace deps -> file:).
  const manifest = buildStagingServerManifest(serverPkg, {
    packageManager: rootPkg.packageManager,
  });
  writeJson(path.join(stagingDir, 'package.json'), manifest);

  // 6. .gcloudignore (exclude node_modules; GAE rebuilds it) + the deploy yaml.
  fs.writeFileSync(path.join(stagingDir, '.gcloudignore'), stagingGcloudignore());
  fs.copyFileSync(prodYaml, path.join(stagingDir, 'app.yaml'));

  // 7. Lockfile: SEED it from the committed root lockfile, then prune to the
  //    staging closure. The seed (root `packages`/`snapshots` with `importers`
  //    dropped) makes pnpm reuse the already-locked, CI-tested versions for the
  //    server's deps and resolve only what is new (the file: vendored
  //    packages); without it, `pnpm install` would resolve the semver ranges
  //    fresh against the registry at deploy time and could pin a newer,
  //    untested version than the workspace built and tested against.
  //
  //    --ignore-workspace detaches from the repo's pnpm-workspace.yaml even
  //    though the staging dir sits under the repo root. The peer-resolution
  //    settings are pinned explicitly so the generated lockfile's `settings`
  //    block does NOT depend on the operator's ambient ~/.npmrc; they match
  //    pnpm's defaults, which is what the staging dir (it ships no .npmrc) gets
  //    on the instance.
  const rootLockPath = path.join(REPO_ROOT, 'pnpm-lock.yaml');
  if (!fs.existsSync(rootLockPath)) {
    die(`root pnpm-lock.yaml not found at ${rootLockPath}; the staged deploy derives its pinned versions from it`);
  }
  const rootLock = parseYaml(fs.readFileSync(rootLockPath, 'utf8'));
  fs.writeFileSync(path.join(stagingDir, 'pnpm-lock.yaml'), stringifyYaml(seedLockfileFromRoot(rootLock)));
  console.log('==> Generating pnpm-lock.yaml (seeded from the committed root lockfile)');
  execFileSync(
    'pnpm',
    [
      'install',
      '--lockfile-only',
      '--ignore-workspace',
      '--config.auto-install-peers=true',
      '--config.strict-peer-dependencies=false',
    ],
    { cwd: stagingDir, stdio: 'inherit' },
  );

  // 7b. Guarantee nothing drifted past the tested workspace lockfile. With the
  //     seed this is empty; the check is the explicit safety net that the
  //     deploy only ships versions the workspace install actually resolved.
  const stagingLock = parseYaml(fs.readFileSync(path.join(stagingDir, 'pnpm-lock.yaml'), 'utf8'));
  const drifted = untestedPackages(stagingLock, rootLock);
  if (drifted.length > 0) {
    die(
      `staging lockfile resolved ${drifted.length} package version(s) absent from the committed ` +
        `root pnpm-lock.yaml:\n${drifted.map((d) => `  - ${d}`).join('\n')}\n` +
        `These were never installed/tested by the workspace. Run \`pnpm install\` at the repo ` +
        `root, commit the updated pnpm-lock.yaml, rebuild, and redeploy.`,
    );
  }

  // 8. Self-verify the assembled artifact before anyone deploys it.
  verify(stagingDir);

  const payloadBytes = dirSize(stagingDir) - safeDirSize(path.join(stagingDir, 'node_modules'));
  console.log(`==> Staging dir ready: ${stagingDir}`);
  console.log(`    Upload payload (excl. node_modules): ${(payloadBytes / 1e6).toFixed(1)} MB`);
}

function safeDirSize(dir) {
  return fs.existsSync(dir) ? dirSize(dir) : 0;
}

function verify(stagingDir) {
  const errors = [];
  const check = (cond, msg) => {
    if (!cond) errors.push(msg);
  };

  check(fs.existsSync(path.join(stagingDir, 'lib/index.js')), 'lib/index.js missing');
  // render.ts spawns this sibling via __dirname at runtime (issue #694); a
  // deploy without it 500s every preview while everything else looks healthy.
  check(
    fs.existsSync(path.join(stagingDir, 'lib/render-worker.js')),
    'lib/render-worker.js missing (preview renders would 500 at runtime)',
  );
  check(fs.existsSync(path.join(stagingDir, 'config/production.json')), 'config/production.json missing');
  check(
    fs.existsSync(path.join(stagingDir, 'default_projects')) &&
      fs.readdirSync(path.join(stagingDir, 'default_projects')).length > 0,
    'default_projects empty/missing',
  );
  check(fs.existsSync(path.join(stagingDir, 'app.yaml')), 'app.yaml missing');
  check(fs.existsSync(path.join(stagingDir, 'pnpm-lock.yaml')), 'pnpm-lock.yaml not generated');

  // public/ assembled SPA.
  const indexHtml = path.join(stagingDir, 'public/index.html');
  if (!fs.existsSync(indexHtml)) {
    errors.push('public/index.html missing');
  } else if (fs.readFileSync(indexHtml, 'utf8').includes('<%= PUBLIC_URL %>')) {
    errors.push('public/index.html still has unsubstituted <%= PUBLIC_URL %> (build skipped?)');
  }
  check(fs.existsSync(path.join(stagingDir, 'public/favicon.ico')), 'public/favicon.ico missing');

  // The full server-side wasm, with the PNG export the preview pipeline needs.
  const wasm = path.join(stagingDir, 'vendor/engine/core/libsimlin.wasm');
  if (!fs.existsSync(wasm)) {
    errors.push('vendor/engine/core/libsimlin.wasm missing');
  } else {
    const size = fs.statSync(wasm).size;
    if (size < 1_000_000) errors.push(`vendor wasm too small (${size} bytes)`);
    if (!wasmHasExport(wasm, PNG_EXPORT)) {
      errors.push(`vendor wasm lacks ${PNG_EXPORT} export (slim browser wasm shipped to server?)`);
    }
  }

  // No residual workspace: protocol anywhere the instance install reads.
  try {
    const m = readJson(path.join(stagingDir, 'package.json'));
    assertNoWorkspaceProtocol(m.dependencies, 'staging package.json');
    check(
      m.dependencies['@simlin/core'] === 'file:./vendor/core' &&
        m.dependencies['@simlin/engine'] === 'file:./vendor/engine',
      'staging package.json workspace deps not rewritten to file:',
    );
    const cm = readJson(path.join(stagingDir, 'vendor/core/package.json'));
    assertNoWorkspaceProtocol(cm.dependencies ?? {}, 'vendored core package.json');
  } catch (e) {
    errors.push(e.message);
  }

  if (errors.length > 0) {
    console.error('build-deploy-staging verification FAILED:');
    for (const e of errors) console.error(`  - ${e}`);
    process.exit(1);
  }
  console.log('==> Staging verification: OK');
}

main();
