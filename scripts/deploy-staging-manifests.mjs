// Pure transforms for assembling the self-contained server deploy staging
// directory (see scripts/build-deploy-staging.mjs and docs/dev/deploy.md).
//
// Why this exists: App Engine's Node buildpack ALWAYS runs `pnpm install` on
// the instance from whatever package.json + lockfile we deploy, and there is
// no vendored-node_modules escape hatch on App Engine standard. Deploying the
// workspace root therefore makes the instance install every workspace
// package's deps (rspress, vite, slate, jest, ...). The fix is to deploy a
// directory whose package.json is just the server's prod closure.
//
// The wrinkle these transforms solve: @simlin/core and @simlin/engine are not
// published to npm, so a plain `pnpm deploy` version-rewrite (workspace:* ->
// 2.0.0) makes the instance try to fetch them from the registry and fail. We
// instead vendor those two packages into the staging dir and point at them
// with file: refs, which the instance install resolves locally.

/** The workspace packages the server depends on, vendored into the staging dir. */
export const WORKSPACE_PACKAGES = Object.freeze(['@simlin/core', '@simlin/engine']);

/** Short directory name a workspace package is vendored under (e.g. @simlin/core -> core). */
function vendorShortName(pkgName) {
  return pkgName.replace(/^@simlin\//, '');
}

function isWorkspaceProtocol(spec) {
  return typeof spec === 'string' && spec.startsWith('workspace:');
}

/**
 * Throw if any dependency still uses the `workspace:` protocol. Used as a
 * final guard: a workspace: ref left in a deployed manifest cannot be resolved
 * outside a pnpm workspace and would abort the instance install.
 */
export function assertNoWorkspaceProtocol(deps, context) {
  const offenders = Object.entries(deps ?? {})
    .filter(([, spec]) => isWorkspaceProtocol(spec))
    .map(([name]) => name);
  if (offenders.length > 0) {
    throw new Error(
      `${context}: unresolved workspace: protocol on ${offenders.join(', ')} -- ` +
        `the App Engine instance install cannot resolve these outside a workspace`,
    );
  }
}

/**
 * Rewrite one dependency block: vendored workspace packages become file: refs
 * via fileRef(name), everything else is copied verbatim. Throws if a
 * workspace: dep is present that is not one of WORKSPACE_PACKAGES (i.e.
 * something we don't know how to vendor -- failing loud beats shipping a
 * manifest the instance can't install). Returns a new object (never mutates).
 */
function rewriteDepBlock(deps, fileRef, context) {
  const out = {};
  for (const [name, spec] of Object.entries(deps ?? {})) {
    if (WORKSPACE_PACKAGES.includes(name)) {
      out[name] = fileRef(name);
    } else if (isWorkspaceProtocol(spec)) {
      throw new Error(
        `unexpected workspace dependency ${name}@${spec} in ${context}: ` +
          `only ${WORKSPACE_PACKAGES.join(', ')} are vendored. Vendor it or move it to devDependencies.`,
      );
    } else {
      out[name] = spec;
    }
  }
  return out;
}

/**
 * Build the package.json for the staging dir from the server's package.json.
 * Carries the prod dependency closure -- `dependencies` and, if present,
 * `optionalDependencies` -- with the two workspace packages rewritten to file:
 * refs. devDependencies are dropped (App Engine ignores them anyway, but we
 * drop them explicitly so the deployed manifest is self-documenting).
 *
 * peerDependencies are rejected, not silently dropped: the staging install
 * does not model peer resolution, so a runtime peer would resolve locally (via
 * the root workspace install) yet be MODULE_NOT_FOUND on GAE. A server app
 * shouldn't declare peers; if it grows one, that must be handled deliberately.
 *
 * @param serverPkg parsed src/server/package.json
 * @param opts.vendorDir relative dir the workspace packages are vendored under (default ./vendor)
 * @param opts.packageManager optional packageManager string to pin (e.g. "pnpm@10.6.0")
 */
export function buildStagingServerManifest(serverPkg, opts = {}) {
  const { vendorDir = './vendor', packageManager } = opts;
  const fileRef = (name) => `file:${vendorDir}/${vendorShortName(name)}`;

  if (serverPkg.peerDependencies && Object.keys(serverPkg.peerDependencies).length > 0) {
    throw new Error(
      `the server manifest declares peerDependencies (${Object.keys(serverPkg.peerDependencies).join(', ')}), ` +
        `which the staging deploy does not handle; move them to dependencies or handle them explicitly`,
    );
  }

  const dependencies = rewriteDepBlock(serverPkg.dependencies, fileRef, 'the server manifest');
  assertNoWorkspaceProtocol(dependencies, 'staging server manifest');

  const manifest = {
    name: 'simlin-server-deploy',
    version: serverPkg.version ?? '1.0.0',
    private: true,
    // GAE runs the `start` script with the staging dir as CWD; the server
    // resolves ./config, ./default_projects and ./public relative to CWD.
    main: 'lib/index.js',
    scripts: { start: 'node lib/index.js' },
    dependencies,
  };

  const optionalDependencies = rewriteDepBlock(serverPkg.optionalDependencies, fileRef, 'the server manifest');
  if (Object.keys(optionalDependencies).length > 0) {
    assertNoWorkspaceProtocol(optionalDependencies, 'staging server manifest (optional)');
    manifest.optionalDependencies = optionalDependencies;
  }

  if (packageManager) {
    manifest.packageManager = packageManager;
  }
  return manifest;
}

/**
 * Rewrite a vendored workspace package's own manifest so any sibling @simlin/*
 * deps (in dependencies or optionalDependencies) point at file:../<short> (the
 * sibling vendor dir), and strip devDependencies + scripts so the instance
 * install neither pulls dev tooling nor runs a build lifecycle for a package
 * that is already compiled.
 *
 * `files` is also dropped. It governs npm-pack inclusion, and the builder
 * copies an explicit minimal subset (e.g. engine's lib/ + the full wasm, but
 * NOT the browser variants) into the vendor dir; an inherited `files` list
 * that names omitted paths is both misleading and a hazard (it could exclude a
 * staged file when pnpm packs the file: dep). With no `files`, the file: dep
 * includes exactly what the builder placed in the dir.
 *
 * Resolution-critical fields (exports, main, type, sideEffects) are preserved.
 */
export function rewriteVendoredManifest(pkg) {
  const out = { ...pkg };
  delete out.devDependencies;
  delete out.scripts;
  delete out.files;
  const fileRef = (name) => `file:../${vendorShortName(name)}`;
  const context = `vendored ${pkg.name ?? 'package'}`;
  if (out.dependencies) {
    out.dependencies = rewriteDepBlock(out.dependencies, fileRef, context);
  }
  if (out.optionalDependencies) {
    out.optionalDependencies = rewriteDepBlock(out.optionalDependencies, fileRef, context);
  }
  return out;
}

/**
 * .gcloudignore for the staging dir. node_modules is excluded because App
 * Engine rebuilds it on the instance from package.json + the lockfile;
 * uploading it is wasted bytes the buildpack ignores.
 */
export function stagingGcloudignore() {
  return ['node_modules', '.DS_Store', ''].join('\n');
}

/**
 * Build a seed `pnpm-lock.yaml` object from the committed root (workspace)
 * lockfile: keep the resolution cache (`packages`/`snapshots`) + `settings` and
 * `lockfileVersion`, but DROP `importers`. The seed is written into the staging
 * dir before `pnpm install --lockfile-only`, so pnpm reuses the already-locked
 * (and CI-tested) versions for the server's deps and only resolves what is new
 * (the `file:` vendored packages), then prunes `packages`/`snapshots` down to
 * the staging closure. Dropping `importers` is what makes the prune happen:
 * leaving the workspace importers in keeps every other package reachable, so
 * the staging lockfile balloons back to the whole-workspace closure.
 *
 * Without this seed the staging lockfile is resolved fresh against the registry
 * at deploy time and could pin a newer, untested version (direct or transitive)
 * than the one the workspace built and tested against.
 */
export function seedLockfileFromRoot(rootLock) {
  const seed = { lockfileVersion: rootLock.lockfileVersion };
  if (rootLock.settings !== undefined) seed.settings = rootLock.settings;
  if (rootLock.packages !== undefined) seed.packages = rootLock.packages;
  if (rootLock.snapshots !== undefined) seed.snapshots = rootLock.snapshots;
  return seed;
}

/**
 * The set of registry `name@version` keys in a parsed pnpm lockfile's
 * `packages` map, with any `(peer...)` suffix stripped and local `file:`/`link:`
 * entries (the vendored workspace packages) excluded -- those are resolved
 * locally and never come from the registry.
 */
export function registryPackageKeys(lock) {
  const out = new Set();
  for (const key of Object.keys(lock?.packages ?? {})) {
    if (key.includes('@file:') || key.includes('@link:')) continue;
    out.add(key.replace(/\(.*$/, ''));
  }
  return out;
}

/**
 * Registry packages resolved in the staging lockfile that are absent from the
 * root lockfile -- i.e. versions that were NOT part of the workspace install
 * the build and tests ran against. A non-empty result means the staging deploy
 * would ship an untested version; the builder fails loud on it. Seeding makes
 * this empty in the normal case; the check is the belt-and-suspenders guarantee.
 */
export function untestedPackages(stagingLock, rootLock) {
  const root = registryPackageKeys(rootLock);
  return [...registryPackageKeys(stagingLock)].filter((key) => !root.has(key)).sort();
}
