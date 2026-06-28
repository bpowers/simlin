import assert from 'node:assert/strict';
import { describe, it } from 'node:test';

import {
  WORKSPACE_PACKAGES,
  buildStagingServerManifest,
  rewriteVendoredManifest,
  assertNoWorkspaceProtocol,
  stagingGcloudignore,
  seedLockfileFromRoot,
  registryPackageKeys,
  untestedPackages,
} from '../deploy-staging-manifests.mjs';

// A trimmed stand-in for src/server/package.json.
const serverPkg = {
  name: '@simlin/server',
  version: '1.0.0',
  private: true,
  main: 'lib',
  dependencies: {
    '@iarna/toml': '^2.2.5',
    '@simlin/core': 'workspace:*',
    '@simlin/engine': 'workspace:*',
    cors: '^2.8.6',
    express: '^5.2.1',
    'firebase-admin': '^13.6.1',
    'google-protobuf': '^4.0.1',
    helmet: '^8.1.0',
  },
  devDependencies: {
    jest: '^30.2.0',
    typescript: '^5.9.3',
  },
  scripts: { build: 'tsc', test: 'jest', start: 'node lib/index.js' },
};

describe('buildStagingServerManifest', () => {
  it('rewrites workspace packages to file: refs under the vendor dir', () => {
    const out = buildStagingServerManifest(serverPkg, { packageManager: 'pnpm@10.6.0' });
    assert.equal(out.dependencies['@simlin/core'], 'file:./vendor/core');
    assert.equal(out.dependencies['@simlin/engine'], 'file:./vendor/engine');
  });

  it('keeps third-party deps verbatim', () => {
    const out = buildStagingServerManifest(serverPkg);
    assert.equal(out.dependencies['@iarna/toml'], '^2.2.5');
    assert.equal(out.dependencies.express, '^5.2.1');
    assert.equal(out.dependencies['firebase-admin'], '^13.6.1');
    assert.equal(Object.keys(out.dependencies).length, 8);
  });

  it('drops devDependencies entirely', () => {
    const out = buildStagingServerManifest(serverPkg);
    assert.equal(out.devDependencies, undefined);
  });

  it('sets a self-contained start script that runs the compiled entry', () => {
    const out = buildStagingServerManifest(serverPkg);
    assert.equal(out.scripts.start, 'node lib/index.js');
    assert.equal(out.main, 'lib/index.js');
  });

  it('passes through packageManager when provided, omits it otherwise', () => {
    assert.equal(
      buildStagingServerManifest(serverPkg, { packageManager: 'pnpm@10.6.0' }).packageManager,
      'pnpm@10.6.0',
    );
    assert.equal(buildStagingServerManifest(serverPkg).packageManager, undefined);
  });

  it('pins engines.pnpm from the packageManager version (the GAE buildpack reads engines, not corepack)', () => {
    const out = buildStagingServerManifest(serverPkg, { packageManager: 'pnpm@10.6.0' });
    assert.equal(out.engines.pnpm, '10.6.0');
  });

  it('strips the +sha512 build-metadata suffix when deriving engines.pnpm', () => {
    const out = buildStagingServerManifest(serverPkg, {
      packageManager: 'pnpm@10.6.0+sha512.abc123def456',
    });
    assert.equal(out.engines.pnpm, '10.6.0');
    // packageManager itself is preserved verbatim (corepack still wants the hash).
    assert.equal(out.packageManager, 'pnpm@10.6.0+sha512.abc123def456');
  });

  it('adds no engines when no packageManager is provided and the server declares none', () => {
    assert.equal(buildStagingServerManifest(serverPkg).engines, undefined);
  });

  it('preserves the server-declared engines when no packageManager is provided', () => {
    const withEngines = { ...serverPkg, engines: { node: '>=20' } };
    const out = buildStagingServerManifest(withEngines);
    assert.deepEqual(out.engines, { node: '>=20' });
  });

  it('merges the pnpm pin into the server-declared engines, with the pin winning', () => {
    const withEngines = { ...serverPkg, engines: { node: '>=20', pnpm: '>=9' } };
    const out = buildStagingServerManifest(withEngines, { packageManager: 'pnpm@10.6.0' });
    assert.deepEqual(out.engines, { node: '>=20', pnpm: '10.6.0' });
  });

  it('adds no engines.pnpm for a non-pnpm packageManager, but keeps packageManager', () => {
    const out = buildStagingServerManifest(serverPkg, { packageManager: 'yarn@4.1.0' });
    assert.equal(out.packageManager, 'yarn@4.1.0');
    assert.equal(out.engines, undefined);
  });

  it('produces a manifest with no residual workspace: protocol', () => {
    const out = buildStagingServerManifest(serverPkg);
    // Should not throw.
    assertNoWorkspaceProtocol(out.dependencies, 'staging server manifest');
  });

  it('throws if a workspace dep is present that we do not know how to vendor', () => {
    const bad = {
      ...serverPkg,
      dependencies: { ...serverPkg.dependencies, '@simlin/diagram': 'workspace:*' },
    };
    assert.throws(() => buildStagingServerManifest(bad), /@simlin\/diagram/);
  });

  it('carries optionalDependencies, rewriting any workspace ref to file:', () => {
    const withOptional = {
      ...serverPkg,
      optionalDependencies: { fsevents: '^2.3.3', '@simlin/engine': 'workspace:*' },
    };
    const out = buildStagingServerManifest(withOptional);
    assert.equal(out.optionalDependencies.fsevents, '^2.3.3');
    assert.equal(out.optionalDependencies['@simlin/engine'], 'file:./vendor/engine');
  });

  it('omits optionalDependencies when the server has none', () => {
    assert.equal(buildStagingServerManifest(serverPkg).optionalDependencies, undefined);
  });

  it('throws (does not silently drop) when the server declares peerDependencies', () => {
    const withPeer = { ...serverPkg, peerDependencies: { react: '^19.0.0' } };
    assert.throws(() => buildStagingServerManifest(withPeer), /peerDependencies/);
  });

  it('does not mutate the input package object', () => {
    const before = JSON.stringify(serverPkg);
    buildStagingServerManifest(serverPkg);
    assert.equal(JSON.stringify(serverPkg), before);
  });
});

describe('rewriteVendoredManifest', () => {
  const corePkg = {
    name: '@simlin/core',
    version: '1.3.5',
    main: 'lib',
    exports: { '.': { node: './lib/index.js' }, './common': { node: './lib/common.js' } },
    files: ['lib', 'lib.module'],
    dependencies: { '@simlin/engine': 'workspace:^' },
    devDependencies: { typescript: '^5.9.3' },
    scripts: { build: 'tsc', prepublishOnly: 'pnpm build' },
  };

  it('rewrites sibling workspace deps to file: refs at ../<short>', () => {
    const out = rewriteVendoredManifest(corePkg);
    assert.equal(out.dependencies['@simlin/engine'], 'file:../engine');
  });

  it('drops devDependencies and scripts to avoid install-time lifecycle surprises', () => {
    const out = rewriteVendoredManifest(corePkg);
    assert.equal(out.devDependencies, undefined);
    assert.equal(out.scripts, undefined);
  });

  it('preserves resolution-critical exports and main', () => {
    const out = rewriteVendoredManifest(corePkg);
    assert.deepEqual(out.exports, corePkg.exports);
    assert.equal(out.main, 'lib');
  });

  it('drops files (the builder copies an explicit minimal subset into the vendor dir)', () => {
    const out = rewriteVendoredManifest(corePkg);
    assert.equal(out.files, undefined);
  });

  it('rewrites workspace refs in optionalDependencies too', () => {
    const out = rewriteVendoredManifest({
      ...corePkg,
      optionalDependencies: { '@simlin/engine': 'workspace:*', leftpad: '^1.0.0' },
    });
    assert.equal(out.optionalDependencies['@simlin/engine'], 'file:../engine');
    assert.equal(out.optionalDependencies.leftpad, '^1.0.0');
  });

  it('is a no-op on dependencies for a leaf package (engine has none)', () => {
    const enginePkg = { name: '@simlin/engine', version: '2.0.0', main: 'lib/index.js' };
    const out = rewriteVendoredManifest(enginePkg);
    assert.equal(out.dependencies, undefined);
  });

  it('leaves the result free of workspace: protocol', () => {
    const out = rewriteVendoredManifest(corePkg);
    assertNoWorkspaceProtocol(out.dependencies ?? {}, 'vendored core');
  });

  it('does not mutate the input', () => {
    const before = JSON.stringify(corePkg);
    rewriteVendoredManifest(corePkg);
    assert.equal(JSON.stringify(corePkg), before);
  });
});

describe('assertNoWorkspaceProtocol', () => {
  it('throws listing every offending dep', () => {
    assert.throws(
      () => assertNoWorkspaceProtocol({ a: 'workspace:*', b: '^1.0.0', c: 'workspace:^' }, 'ctx'),
      (err) =>
        /ctx/.test(err.message) && /\ba\b/.test(err.message) && /\bc\b/.test(err.message) && !/\bb\b/.test(err.message),
    );
  });

  it('accepts file: and semver refs', () => {
    assert.doesNotThrow(() => assertNoWorkspaceProtocol({ a: 'file:./vendor/x', b: '^1.0.0' }, 'ctx'));
  });
});

describe('stagingGcloudignore', () => {
  it('excludes node_modules so GAE rebuilds rather than uploading the install', () => {
    assert.match(stagingGcloudignore(), /^node_modules$/m);
  });

  it('does not exclude the load-bearing lockfile, manifest, or app.yaml', () => {
    const lines = stagingGcloudignore()
      .split('\n')
      .map((l) => l.trim())
      .filter(Boolean);
    for (const required of ['pnpm-lock.yaml', 'package.json', 'app.yaml']) {
      assert.ok(!lines.includes(required), `${required} must not be in .gcloudignore`);
    }
    // No broad glob that could sweep them up either.
    assert.ok(!lines.includes('*'), 'a bare * would exclude everything');
  });
});

describe('WORKSPACE_PACKAGES', () => {
  it('enumerates exactly the two vendored workspace packages', () => {
    assert.deepEqual([...WORKSPACE_PACKAGES].sort(), ['@simlin/core', '@simlin/engine']);
  });
});

describe('seedLockfileFromRoot', () => {
  const rootLock = {
    lockfileVersion: '9.0',
    settings: { autoInstallPeers: true, excludeLinksFromLockfile: false },
    importers: { '.': {}, 'src/server': { dependencies: {} }, 'src/app': { dependencies: {} } },
    packages: { 'express@5.2.1': {}, 'helmet@8.2.0': {} },
    snapshots: { 'express@5.2.1': {}, 'helmet@8.2.0': {} },
  };

  it('drops importers (so pnpm prunes to the staging closure)', () => {
    const seed = seedLockfileFromRoot(rootLock);
    assert.equal(seed.importers, undefined);
  });

  it('keeps lockfileVersion, settings, packages, and snapshots (the resolution cache)', () => {
    const seed = seedLockfileFromRoot(rootLock);
    assert.equal(seed.lockfileVersion, '9.0');
    assert.deepEqual(seed.settings, rootLock.settings);
    assert.deepEqual(seed.packages, rootLock.packages);
    assert.deepEqual(seed.snapshots, rootLock.snapshots);
  });

  it('does not mutate the root lockfile object', () => {
    const before = JSON.stringify(rootLock);
    seedLockfileFromRoot(rootLock);
    assert.equal(JSON.stringify(rootLock), before);
  });
});

describe('registryPackageKeys', () => {
  it('returns name@version keys, excluding file:/link: vendored packages', () => {
    const keys = registryPackageKeys({
      packages: {
        'express@5.2.1': {},
        '@simlin/core@file:vendor/core': {},
        '@simlin/engine@link:../engine': {},
      },
    });
    assert.deepEqual([...keys].sort(), ['express@5.2.1']);
  });

  it('strips peer-dependency suffixes', () => {
    const keys = registryPackageKeys({ packages: { 'ts-jest@29.4.6(typescript@5.9.3)': {} } });
    assert.ok(keys.has('ts-jest@29.4.6'));
  });

  it('tolerates a lockfile with no packages map', () => {
    assert.equal(registryPackageKeys({}).size, 0);
  });
});

describe('untestedPackages', () => {
  const rootLock = { packages: { 'express@5.2.1': {}, 'helmet@8.2.0': {}, 'cors@2.8.6': {} } };

  it('is empty when every staging version exists in the root lockfile', () => {
    const stagingLock = {
      packages: { 'express@5.2.1': {}, 'helmet@8.2.0': {}, '@simlin/core@file:vendor/core': {} },
    };
    assert.deepEqual(untestedPackages(stagingLock, rootLock), []);
  });

  it('flags a staging version that drifted past the root (tested) lockfile', () => {
    const stagingLock = { packages: { 'express@5.2.1': {}, 'helmet@8.3.0': {} } };
    assert.deepEqual(untestedPackages(stagingLock, rootLock), ['helmet@8.3.0']);
  });
});
