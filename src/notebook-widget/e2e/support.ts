// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * The static-page harness the Playwright journeys share: `dist/widget.js`,
 * the harness page and the engine wasm are served from disk through
 * `page.route()` (no web server), and a widget is mounted the way anywidget
 * mounts one (a fresh blob: URL per instance, a fake `AnyModel` playing the
 * kernel). See `src/notebook-widget/CLAUDE.md`, Files -> `e2e/`.
 */

import * as fs from 'node:fs';
import * as path from 'node:path';

import type { Page } from '@playwright/test';

const here = import.meta.dirname;
export const packageRoot = path.resolve(here, '..');
export const repoRoot = path.resolve(packageRoot, '..', '..');

export const ORIGIN = 'https://simlin-widget.test';

export const harnessFiles: Record<string, { path: string; type: string }> = {
  '/': { path: path.join(here, 'harness', 'index.html'), type: 'text/html' },
  '/harness/fake-anywidget-model.js': {
    path: path.join(here, 'harness', 'fake-anywidget-model.js'),
    type: 'text/javascript',
  },
  '/widget.js': { path: path.join(packageRoot, 'dist', 'widget.js'), type: 'text/javascript' },
  '/libsimlin-browser.wasm': {
    path: path.join(repoRoot, 'src', 'engine', 'core', 'libsimlin-browser.wasm'),
    type: 'application/wasm',
  },
};

export async function serveHarness(page: Page): Promise<void> {
  for (const file of Object.values(harnessFiles)) {
    if (!fs.existsSync(file.path)) {
      throw new Error(`missing ${file.path}; run \`pnpm build\` in src/engine and src/notebook-widget first`);
    }
  }
  await page.route(`${ORIGIN}/**`, async (route) => {
    const url = new URL(route.request().url());
    const file = harnessFiles[url.pathname];
    if (file === undefined) {
      await route.fulfill({ status: 404, body: `no such harness file: ${url.pathname}` });
      return;
    }
    await route.fulfill({ status: 200, contentType: file.type, body: fs.readFileSync(file.path) });
  });
  await page.goto(`${ORIGIN}/`);
  await page.waitForFunction(() => (window as unknown as { harness?: unknown }).harness !== undefined);
}

export interface HarnessModel {
  state: Record<string, unknown>;
  sets: Array<{ key: string; value: unknown }>;
  saveChangesCount: number;
  wasmRequests: number;
  sent: unknown[];
  snapshots: Array<{ id: unknown; base: number; json: string }>;
  kernel: { revision: number; projectJson: string };
  send(content: unknown, callbacks?: unknown, buffers?: unknown): void;
  kernelPush(patch: Record<string, unknown>): void;
  kernelSend(content: unknown): void;
  kernelChange(projectJson: string, notice?: string): void;
  cleanup?: () => void;
}

export interface HarnessWindow {
  harness: {
    loadWidgetModule(inlineWasmBase64?: string): Promise<unknown>;
    mount(mod: unknown, el: HTMLElement, state: Record<string, unknown>): Promise<number>;
    models: HarnessModel[];
  };
}

export function widgetState(projectJson: string, overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    project_json: projectJson,
    revision: 0,
    selection: [],
    height: 520,
    theme: 'light',
    read_only: false,
    ...overrides,
  };
}

// Every mount imports the bundle again through a fresh blob: URL -- a
// separate module instance with its own module-level state -- because that is
// exactly what anywidget does per widget instance (load.ts `loadEsm` creates a
// new object URL for each Runtime). Sharing across instances therefore has to
// go through globalThis.
export async function mountWidget(page: Page, cellId: string, state: Record<string, unknown>): Promise<number> {
  return page.evaluate(
    async ({ cellId, state }) => {
      const w = window as unknown as HarnessWindow;
      const mod = await w.harness.loadWidgetModule();
      const el = document.getElementById(cellId)!;
      return w.harness.mount(mod, el, state);
    },
    { cellId, state },
  );
}
