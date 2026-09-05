// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell

/**
 * Pins build.sh's orchestration contract: the two wasm artifacts build
 * concurrently, a failure in either one fails the script, and the cargo
 * profile follows the gate/shipping decision that DISABLE_WASM_OPT already
 * makes for wasm-opt.
 *
 * A copy of build.sh runs in a throwaway tree with stub `cargo`, `pnpm` and
 * `wasm-opt` executables ahead of the real ones on PATH. The stubs record
 * their arguments and timestamps, so the assertions are about what build.sh
 * asked for, not about rustc. Nothing here compiles anything.
 */

import { describe, expect, it } from '@rstest/core';

import { spawnSync } from 'node:child_process';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';

const REAL_BUILD_SH = path.join(__dirname, '..', 'build.sh');

// Long enough that two serial invocations cannot overlap by accident, short
// enough that the whole file stays well inside the per-test budget.
const STUB_CARGO_HOLD_MS = 400;

const STUB_CARGO = `#!/usr/bin/env node
const fs = require('node:fs');
const path = require('node:path');
const args = process.argv.slice(2);
const log = process.env.STUB_LOG;
const name = args.includes('--no-default-features') ? 'browser' : 'full';
const profileIdx = args.indexOf('--profile');
// Mirror cargo's output-directory rule so staging finds the blob whichever
// way build.sh selects the profile.
const profile = profileIdx >= 0 ? args[profileIdx + 1] : args.includes('--release') ? 'release' : 'debug';
const targetDir = args[args.indexOf('--target-dir') + 1];
fs.writeFileSync(path.join(log, name + '.start'), String(Date.now()));
fs.writeFileSync(path.join(log, name + '.args'), args.join(' '));
Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ${STUB_CARGO_HOLD_MS});
if ((process.env.STUB_FAIL || '') === name) {
  console.error('stub cargo: failing the ' + name + ' build');
  process.exit(101);
}
const out = path.join(targetDir, 'wasm32-unknown-unknown', profile, 'simlin.wasm');
fs.mkdirSync(path.dirname(out), { recursive: true });
fs.writeFileSync(out, 'fake ' + name + ' ' + profile);
fs.writeFileSync(path.join(log, name + '.end'), String(Date.now()));
`;

// wasm-opt IN -o OUT [flags...]: a transform that visibly changes the bytes,
// so the staged blob and its .raw sibling differ the way they do for real.
const STUB_WASM_OPT = `#!/usr/bin/env node
const fs = require('node:fs');
const args = process.argv.slice(2);
fs.writeFileSync(args[args.indexOf('-o') + 1], fs.readFileSync(args[0]) + ' optimized');
`;

const STUB_PNPM = '#!/bin/bash\nexit 0\n';

interface BuildRun {
  status: number | null;
  output: string;
  engineDir: string;
  logDir: string;
}

function writeExecutable(file: string, contents: string): void {
  fs.writeFileSync(file, contents);
  fs.chmodSync(file, 0o755);
}

/**
 * Lay out `<root>/src/engine/build.sh` plus the sibling script it resolves the
 * target directory through, put the stubs first on PATH, and run it.
 */
function runBuild(extraEnv: Record<string, string>, stubWasmOpt: boolean): BuildRun {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'simlin-build-sh-'));
  const engineDir = path.join(root, 'src', 'engine');
  const binDir = path.join(root, 'bin');
  const logDir = path.join(root, 'log');
  const scriptsDir = path.join(root, 'scripts');
  for (const dir of [engineDir, binDir, logDir, scriptsDir]) {
    fs.mkdirSync(dir, { recursive: true });
  }
  fs.copyFileSync(REAL_BUILD_SH, path.join(engineDir, 'build.sh'));
  writeExecutable(path.join(scriptsDir, 'cargo-target-dir.sh'), `#!/bin/bash\necho "${path.join(root, 'target')}"\n`);
  writeExecutable(path.join(binDir, 'cargo'), STUB_CARGO);
  writeExecutable(path.join(binDir, 'pnpm'), STUB_PNPM);
  if (stubWasmOpt) {
    writeExecutable(path.join(binDir, 'wasm-opt'), STUB_WASM_OPT);
  }

  const env: Record<string, string> = {
    PATH: `${binDir}:${process.env.PATH ?? ''}`,
    HOME: process.env.HOME ?? root,
    STUB_LOG: logDir,
    ...extraEnv,
  };
  // Without the stub, a real wasm-opt on the developer's PATH would be asked
  // to optimize a fake blob; the mode decision is what is under test, not
  // binaryen.
  if (!stubWasmOpt && !('DISABLE_WASM_OPT' in extraEnv)) {
    throw new Error('runs without a stub wasm-opt must set DISABLE_WASM_OPT');
  }
  const result = spawnSync('bash', [path.join(engineDir, 'build.sh')], {
    cwd: root,
    env,
    encoding: 'utf8',
  });
  return {
    status: result.status,
    output: `${result.stdout}${result.stderr}`,
    engineDir,
    logDir,
  };
}

function readStamp(run: BuildRun, name: string): number {
  return Number(fs.readFileSync(path.join(run.logDir, name), 'utf8'));
}

function readArgs(run: BuildRun, build: 'full' | 'browser'): string {
  return fs.readFileSync(path.join(run.logDir, `${build}.args`), 'utf8');
}

function staged(run: BuildRun, file: string): string {
  return fs.readFileSync(path.join(run.engineDir, 'core', file), 'utf8');
}

describe('build.sh', () => {
  it('stages both artifacts as raw blobs under DISABLE_WASM_OPT=1', () => {
    const run = runBuild({ DISABLE_WASM_OPT: '1' }, false);
    expect(run.status, run.output).toBe(0);
    expect(staged(run, 'libsimlin.wasm')).toBe(staged(run, 'libsimlin.wasm.raw'));
    expect(staged(run, 'libsimlin-browser.wasm')).toBe(staged(run, 'libsimlin-browser.wasm.raw'));
    expect(staged(run, 'libsimlin.wasm.mode').trim()).toBe('raw');
    expect(staged(run, 'libsimlin-browser.wasm.mode').trim()).toBe('raw');
  });

  it('runs the full and browser cargo builds concurrently', () => {
    const run = runBuild({ DISABLE_WASM_OPT: '1' }, false);
    expect(run.status, run.output).toBe(0);
    const fullStart = readStamp(run, 'full.start');
    const fullEnd = readStamp(run, 'full.end');
    const browserStart = readStamp(run, 'browser.start');
    const browserEnd = readStamp(run, 'browser.end');
    // Two intervals overlap iff each starts before the other ends. Serial
    // execution makes one of these strictly false by the whole hold time.
    expect(fullStart, 'full build started after the browser build finished').toBeLessThan(browserEnd);
    expect(browserStart, 'browser build started after the full build finished').toBeLessThan(fullEnd);
  });

  it('fails when the full build fails, after letting the browser build finish', () => {
    const run = runBuild({ DISABLE_WASM_OPT: '1', STUB_FAIL: 'full' }, false);
    expect(run.status, run.output).not.toBe(0);
    expect(run.output).toContain('libsimlin.wasm');
    expect(fs.existsSync(path.join(run.engineDir, 'core', 'libsimlin.wasm'))).toBe(false);
    expect(fs.existsSync(path.join(run.logDir, 'browser.end'))).toBe(true);
  });

  it('fails when the browser build fails, after letting the full build finish', () => {
    const run = runBuild({ DISABLE_WASM_OPT: '1', STUB_FAIL: 'browser' }, false);
    expect(run.status, run.output).not.toBe(0);
    expect(run.output).toContain('libsimlin-browser.wasm');
    expect(fs.existsSync(path.join(run.engineDir, 'core', 'libsimlin-browser.wasm'))).toBe(false);
    expect(fs.existsSync(path.join(run.logDir, 'full.end'))).toBe(true);
  });

  it('builds with the wasm-gate profile under DISABLE_WASM_OPT=1', () => {
    const run = runBuild({ DISABLE_WASM_OPT: '1' }, false);
    expect(run.status, run.output).toBe(0);
    for (const build of ['full', 'browser'] as const) {
      expect(readArgs(run, build)).toContain('--profile wasm-gate');
      expect(readArgs(run, build)).not.toContain('--release');
    }
  });

  it('builds with the wasm-release profile and runs wasm-opt otherwise', () => {
    const run = runBuild({}, true);
    expect(run.status, run.output).toBe(0);
    for (const build of ['full', 'browser'] as const) {
      expect(readArgs(run, build)).toContain('--profile wasm-release');
    }
    expect(staged(run, 'libsimlin.wasm.mode').trim()).toBe('opt');
    expect(staged(run, 'libsimlin.wasm')).not.toBe(staged(run, 'libsimlin.wasm.raw'));
  });
});
