// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Feasibility journey for the built bundle: `dist/widget.js` is imported the
 * way anywidget imports it (module text -> blob: URL -> dynamic import), the
 * AFM lifecycle runs against a fake AnyModel that plays the kernel's side of
 * the wasm handshake, the real Editor renders a real project, a variable is
 * added through the UI, and the whole-project snapshot lands on the model.
 *
 * What this establishes: the single-file bundle is loadable with no relative
 * asset, the engine initializes from bytes delivered over the comm, the
 * Editor edits and saves through the anywidget model contract, and a second
 * widget instance on the same page reuses the compiled wasm (no second
 * request). What it does NOT establish: behaviour inside real JupyterLab /
 * Colab / VS Code hosts (Phase 4's JupyterLab journey covers that).
 */

import { createHash } from 'node:crypto';
import * as fs from 'node:fs';
import * as path from 'node:path';

import { test, expect, type Page } from '@playwright/test';

const here = import.meta.dirname;
const packageRoot = path.resolve(here, '..');
const repoRoot = path.resolve(packageRoot, '..', '..');

const ORIGIN = 'https://simlin-widget.test';

const files: Record<string, { path: string; type: string }> = {
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

const projectJson = fs.readFileSync(path.join(repoRoot, 'test', 'logistic-growth.sd.json'), 'utf8');

async function serveHarness(page: Page): Promise<void> {
  for (const [route, file] of Object.entries(files)) {
    if (!fs.existsSync(file.path)) {
      throw new Error(`missing ${file.path}; run \`pnpm build\` in src/engine and src/notebook-widget first`);
    }
    void route;
  }
  await page.route(`${ORIGIN}/**`, async (route) => {
    const url = new URL(route.request().url());
    const file = files[url.pathname];
    if (file === undefined) {
      await route.fulfill({ status: 404, body: `no such harness file: ${url.pathname}` });
      return;
    }
    await route.fulfill({ status: 200, contentType: file.type, body: fs.readFileSync(file.path) });
  });
  await page.goto(`${ORIGIN}/`);
  await page.waitForFunction(() => (window as unknown as { harness?: unknown }).harness !== undefined);
}

interface HarnessWindow {
  harness: {
    loadWidgetModule(inlineWasmBase64?: string): Promise<unknown>;
    mount(mod: unknown, el: HTMLElement, state: Record<string, unknown>): Promise<number>;
    models: Array<{
      state: Record<string, unknown>;
      sets: Array<{ key: string; value: unknown }>;
      saveChangesCount: number;
      wasmRequests: number;
      sent: unknown[];
      snapshots: Array<{ base: number; json: string }>;
      kernel: { revision: number; projectJson: string };
      kernelPush(patch: Record<string, unknown>): void;
      kernelSend(content: unknown): void;
      kernelChange(projectJson: string, notice?: string): void;
      cleanup?: () => void;
    }>;
  };
}

function initialState(overrides: Record<string, unknown> = {}): Record<string, unknown> {
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
// go through globalThis, which is what the second-cell assertions below check.
async function mountWidget(page: Page, cellId: string, state: Record<string, unknown>): Promise<number> {
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

// The drawer sheet slides in with a transform transition; wait until its
// left edge has settled at the widget box's left edge before measuring.
async function settleDrawer(page: Page, cellSelector: string): Promise<void> {
  await expect
    .poll(
      () =>
        page.evaluate((sel) => {
          const wrapper = document.querySelector(`${sel} .simlin-notebook-widget`) as HTMLElement;
          const panel = document.querySelector(`${sel} .simlin-notebook-widget [role="dialog"]`) as HTMLElement;
          return Math.abs(
            panel.getBoundingClientRect().left - (wrapper.getBoundingClientRect().left + wrapper.clientLeft),
          );
        }, cellSelector),
      { timeout: 5_000 },
    )
    .toBeLessThan(1.5);
}

test.describe('notebook widget bundle', () => {
  test('loads from a blob: URL, boots the engine from comm bytes, edits, and saves a snapshot', async ({ page }) => {
    const consoleErrors: string[] = [];
    page.on('pageerror', (err) => consoleErrors.push(`pageerror: ${err.message}`));
    page.on('console', (msg) => {
      if (msg.type() === 'error') {
        consoleErrors.push(`console.error: ${msg.text()}`);
      }
    });
    await serveHarness(page);

    // The only network activity is the harness itself: nothing relative to
    // the (blob:) module may be fetched.
    const requested: string[] = [];
    page.on('request', (req) => requested.push(req.url()));

    const modelIdx = await mountWidget(page, 'cell1', initialState());
    expect(modelIdx).toBe(0);

    // The Editor is up: the diagram canvas (not the first <svg>, which is a
    // search-bar icon) renders once the project has been opened by the
    // (main-thread) engine.
    const cell = page.locator('#cell1');
    const wrapper = cell.locator('[data-lm-suppress-shortcuts]');
    await expect(wrapper).toBeVisible();
    await expect(wrapper).toHaveAttribute('data-theme', 'light');
    await expect(wrapper).toHaveCSS('height', '520px');
    await expect(cell.locator('svg.simlin-canvas')).toBeVisible({ timeout: 60_000 });
    // The stock from the fixture is drawn.
    await expect(cell.getByText('Population', { exact: true }).first()).toBeVisible();

    // Exactly one wasm request went to the kernel.
    const wasmRequests = await page.evaluate(() => (window as unknown as HarnessWindow).harness.models[0].wasmRequests);
    expect(wasmRequests).toBe(1);
    // The compiled module is cached page-wide under a key carrying the sha256
    // of the wasm this bundle was built against (rsbuild.config.ts bakes it
    // in), so bundles from different builds never share a compiled engine.
    const wasmSha256 = createHash('sha256')
      .update(fs.readFileSync(files['/libsimlin-browser.wasm'].path))
      .digest('hex');
    const cacheKeys = await page.evaluate(() =>
      Object.keys(globalThis).filter((k) => k.startsWith('__simlinWidgetWasmModule')),
    );
    expect(cacheKeys).toEqual([`__simlinWidgetWasmModule:${wasmSha256}`]);

    // Add a variable through the UI: open the tool dial, pick "Variable",
    // click on empty canvas, accept the default name with Enter.
    await cell.getByRole('button', { name: 'hide or show editor tools' }).click();
    await cell.getByRole('button', { name: 'Variable', exact: true }).click();
    const svg = cell.locator('svg.simlin-canvas');
    const box = await svg.boundingBox();
    if (box === null) {
      throw new Error('canvas has no bounding box');
    }
    await page.mouse.click(box.x + 120, box.y + 120);
    await page.keyboard.press('Enter');

    // The Editor autosaves the whole project through onSave -> a `snapshot`
    // custom message; the fake kernel accepts it and pushes the traits.
    await expect
      .poll(
        async () =>
          page.evaluate(() => {
            const m = (window as unknown as HarnessWindow).harness.models[0];
            return m.snapshots.length > 0 ? m.snapshots[m.snapshots.length - 1].json : '';
          }),
        { timeout: 30_000 },
      )
      // The engine keeps the display spelling verbatim (canonical matching
      // happens on lookup), so the snapshot carries the name as typed.
      .toContain('"New Variable"');

    const saved = await page.evaluate(() => {
      const m = (window as unknown as HarnessWindow).harness.models[0];
      const last = m.snapshots[m.snapshots.length - 1];
      return {
        setKeys: m.sets.map((s) => s.key),
        json: last.json,
        base: last.base,
        kernel: m.kernel,
        traitJson: m.state.project_json,
        traitRevision: m.state.revision,
      };
    });
    const parsed = JSON.parse(saved.json) as {
      models: Array<{ auxiliaries: Array<{ name: string }>; stocks: Array<{ name: string }> }>;
    };
    expect(parsed.models[0].auxiliaries.map((a) => a.name)).toContain('New Variable');
    expect(parsed.models[0].stocks.map((s) => s.name)).toContain('population');
    expect(saved.base).toBe(0);
    // The widget never writes the kernel-owned traits (only `selection`).
    expect(saved.setKeys.filter((k) => k !== 'selection')).toEqual([]);
    // Accepted: kernel state advanced and its traits carry our exact bytes.
    expect(saved.kernel.revision).toBe(1);
    expect(saved.kernel.projectJson).toBe(saved.json);
    expect(saved.traitJson).toBe(saved.json);
    expect(saved.traitRevision).toBe(1);
    // The Editor was NOT remounted by the accept: the new variable is still
    // on screen from the live Editor (a remount would also show it, so check
    // the editor tools state instead: the dial is still open from our click).
    await expect(cell.getByRole('button', { name: 'Variable', exact: true })).toBeVisible();

    // A second widget instance (a second cell, a second module instance) reuses
    // the page-wide compiled module: it renders without asking the kernel for
    // the wasm again.
    const secondIdx = await mountWidget(page, 'cell2', initialState({ height: 300 }));
    expect(secondIdx).toBe(1);
    const cell2 = page.locator('#cell2');
    await expect(cell2.locator('svg.simlin-canvas')).toBeVisible({ timeout: 60_000 });
    await expect(cell2.locator('[data-lm-suppress-shortcuts]')).toHaveCSS('height', '300px');
    const secondWasmRequests = await page.evaluate(
      () => (window as unknown as HarnessWindow).harness.models[1].wasmRequests,
    );
    expect(secondWasmRequests).toBe(0);
    const secondSent = await page.evaluate(() => (window as unknown as HarnessWindow).harness.models[1].sent);
    expect(secondSent).toEqual([]);

    // Kernel-originated change (e.g. Python edit()) remounts the second
    // widget on the new snapshot: rename the stock kernel-side and expect the
    // new label to appear.
    const renamed = projectJson.replace('"name": "Population"', '"name": "People"');
    expect(renamed).not.toBe(projectJson);
    await page.evaluate((json) => {
      (window as unknown as HarnessWindow).harness.models[1].kernelChange(json, 'Updated on disk');
    }, renamed);
    await expect(cell2.getByText('People', { exact: true }).first()).toBeVisible({ timeout: 30_000 });
    await expect(cell2.getByRole('status')).toHaveText('Updated on disk');

    // The widget's global stylesheets are confined to the widget root: the
    // notebook page's own root element sees none of the diagram tokens, the
    // wrapper does.
    const tokens = await page.evaluate(() => {
      const wrapper = document.querySelector('#cell2 .simlin-notebook-widget') as HTMLElement;
      return {
        page: getComputedStyle(document.documentElement).getPropertyValue('--panel-width-sm').trim(),
        body: getComputedStyle(document.body).getPropertyValue('--panel-width-sm').trim(),
        wrapper: getComputedStyle(wrapper).getPropertyValue('--panel-width-sm').trim(),
      };
    });
    expect(tokens.page).toBe('');
    expect(tokens.body).toBe('');
    expect(tokens.wrapper).toBe('359px');
    // Kept as a visual artifact of the run (gitignored), not an assertion.
    await page.screenshot({ path: path.join(here, '.output', 'two-cells.png'), fullPage: true });

    // Nothing was fetched relative to the module and nothing errored.
    const outside = requested.filter((u) => !u.startsWith(ORIGIN + '/'));
    expect(outside).toEqual([]);
    const relative = requested.filter((u) => u.startsWith(ORIGIN + '/') && !(new URL(u).pathname in files));
    expect(relative).toEqual([]);
    expect(consoleErrors).toEqual([]);

    // Cleanup unmounts React and empties the cell.
    await page.evaluate(() => {
      const w = window as unknown as HarnessWindow;
      w.harness.models[0].cleanup?.();
    });
    await expect(cell.locator('[data-lm-suppress-shortcuts]')).toHaveCount(0);
  });

  // Layout claims need a real layout engine: jsdom cannot tell a max-height
  // that resolves to `none` from one that caps, which is exactly how the
  // details card once grew past a short host box while a stylesheet-text
  // test stayed green. So the box-fitting contract of the right-hand panel
  // (src/diagram/CLAUDE.md, Hosting Requirements) is asserted here, on the
  // built bundle in a 400px cell.
  test('the details panel fits a short host box: it scrolls inside the box and aligns with the search bar', async ({
    page,
  }) => {
    await serveHarness(page);
    await mountWidget(page, 'cell1', initialState({ height: 400 }));
    const cell = page.locator('#cell1');
    await expect(cell.locator('svg.simlin-canvas')).toBeVisible({ timeout: 60_000 });

    // A click (press + release, no drag) on the stock opens its details panel.
    const stock = cell.locator('g.simlin-stock rect').first();
    const stockBox = await stock.boundingBox();
    if (stockBox === null) {
      throw new Error('stock has no bounding box');
    }
    await page.mouse.click(stockBox.x + stockBox.width / 2, stockBox.y + stockBox.height / 2);
    const deleteButton = cell.getByRole('button', { name: 'Delete', exact: true });
    await expect(deleteButton).toBeAttached();

    const layout = await page.evaluate(() => {
      const rect = (el: Element): { top: number; bottom: number; left: number; right: number } => {
        const b = el.getBoundingClientRect();
        return { top: b.top, bottom: b.bottom, left: b.left, right: b.right };
      };
      const root = document.querySelector('#cell1 [data-simlin-editor-root]');
      const searchBar = document.querySelector('#cell1 [class*="searchBar"]');
      // The details slot's only child is the card (Delete is inside it).
      const deleteBtn = Array.from(document.querySelectorAll('#cell1 button')).find(
        (b) => b.textContent?.trim() === 'Delete',
      );
      const card = deleteBtn?.closest('[class*="varDetails"] > *');
      if (!root || !searchBar || !deleteBtn || !card) {
        throw new Error('missing editor root, search bar, or details card');
      }
      return {
        root: rect(root),
        searchBar: rect(searchBar),
        card: rect(card),
        cardScrollHeight: card.scrollHeight,
        cardClientHeight: card.clientHeight,
        cardOverflowY: getComputedStyle(card).overflowY,
      };
    });
    // The card is capped by the box and scrolls its content instead of
    // growing past the editor's (overflow: hidden) bottom edge -- and it is
    // the card that scrolls (overflow-y auto), not the editor root.
    expect(layout.cardScrollHeight).toBeGreaterThan(layout.cardClientHeight);
    expect(['auto', 'scroll']).toContain(layout.cardOverflowY);
    expect(layout.card.bottom).toBeLessThanOrEqual(layout.root.bottom);
    expect(layout.card.top).toBeGreaterThanOrEqual(layout.root.top);
    // Edge-aligned with the search bar that overlays its top (same width
    // clamp, same right anchor).
    expect(layout.card.left).toBeCloseTo(layout.searchBar.left, 0);
    expect(layout.card.right).toBeCloseTo(layout.searchBar.right, 0);
    // The panel's bottom controls are reachable: scrolling the card brings
    // Delete inside the box.
    await deleteButton.scrollIntoViewIfNeeded();
    const deleteBox = await deleteButton.boundingBox();
    if (deleteBox === null) {
      throw new Error('Delete has no bounding box');
    }
    expect(deleteBox.y + deleteBox.height).toBeLessThanOrEqual(layout.root.bottom);
    expect(deleteBox.y).toBeGreaterThanOrEqual(layout.root.top);
    await expect(deleteButton).toBeVisible();
    const scrolled = await page.evaluate(() => {
      const root = document.querySelector('#cell1 [data-simlin-editor-root]') as HTMLElement;
      const deleteBtn = Array.from(document.querySelectorAll('#cell1 button')).find(
        (b) => b.textContent?.trim() === 'Delete',
      );
      const card = deleteBtn?.closest('[class*="varDetails"] > *') as HTMLElement;
      return { cardScrollTop: card.scrollTop, rootScrollTop: root.scrollTop };
    });
    expect(scrolled.cardScrollTop).toBeGreaterThan(0);
    expect(scrolled.rootScrollTop).toBe(0);
  });

  // The Editor's overlay surfaces (the hamburger drawer here) render INSIDE the
  // widget box: portaled to document.body they would sit outside the widget
  // root class, where every scoped `var(--*)` is undefined (a transparent
  // sheet over the diagram) and JupyterLab's shortcut-suppression attribute
  // cannot reach them. Computed styles need a real engine, so this lives here.
  test("the model drawer renders inside the widget box with the widget's tokens, contained to the box, and without an Exit link", async ({
    page,
  }) => {
    await serveHarness(page);
    await mountWidget(page, 'cell1', initialState({ height: 400 }));
    const cell = page.locator('#cell1');
    await expect(cell.locator('svg.simlin-canvas')).toBeVisible({ timeout: 60_000 });

    await cell.getByRole('button', { name: /^menu$/i }).click();
    const drawer = cell.locator('.simlin-notebook-widget [role="dialog"]');
    await expect(drawer).toBeVisible();
    await expect(drawer.getByRole('button', { name: /download model/i })).toBeVisible();
    // No Exit link: the notebook page has no "/" to navigate to.
    await expect(drawer.locator('a[href="/"]')).toHaveCount(0);
    await expect(drawer.getByRole('link', { name: /exit/i })).toHaveCount(0);
    // The sheet slides in (a 225ms transform transition); measure once settled.
    await settleDrawer(page, '#cell1');

    const styles = await page.evaluate(() => {
      const wrapper = document.querySelector('#cell1 .simlin-notebook-widget') as HTMLElement;
      const panel = document.querySelector('#cell1 .simlin-notebook-widget [role="dialog"]') as HTMLElement;
      const backdrop = panel.previousElementSibling as HTMLElement;
      const cs = getComputedStyle(panel);
      const rect = (el: Element): { top: number; bottom: number; left: number; right: number } => {
        const b = el.getBoundingClientRect();
        return { top: b.top, bottom: b.bottom, left: b.left, right: b.right };
      };
      return {
        insideWrapper: wrapper.contains(panel) && panel.parentElement === wrapper,
        insideEditorRoot: panel.closest('[data-simlin-editor-root]') !== null,
        background: cs.backgroundColor,
        fontFamily: cs.fontFamily,
        position: cs.position,
        backdropPosition: getComputedStyle(backdrop).position,
        panel: rect(panel),
        backdrop: rect(backdrop),
        // The wrapper's padding box (inside its 1px border): the containing
        // block the contained surfaces position against.
        wrapper: {
          top: wrapper.getBoundingClientRect().top + wrapper.clientTop,
          left: wrapper.getBoundingClientRect().left + wrapper.clientLeft,
          right: wrapper.getBoundingClientRect().left + wrapper.clientLeft + wrapper.clientWidth,
          bottom: wrapper.getBoundingClientRect().top + wrapper.clientTop + wrapper.clientHeight,
        },
      };
    });
    expect(styles.insideWrapper).toBe(true);
    expect(styles.insideEditorRoot).toBe(false);
    // The scoped tokens resolve: an opaque surface, the Editor's font stack.
    expect(styles.background).not.toBe('rgba(0, 0, 0, 0)');
    expect(styles.background).not.toBe('transparent');
    expect(styles.fontFamily).toContain('Roboto');
    // Contained: absolute inside the box, the sheet spans the box's height
    // from its left edge and the backdrop covers exactly the box.
    expect(styles.position).toBe('absolute');
    expect(styles.backdropPosition).toBe('absolute');
    expect(styles.panel.left).toBeCloseTo(styles.wrapper.left, 0);
    expect(styles.panel.top).toBeGreaterThanOrEqual(styles.wrapper.top);
    expect(styles.panel.bottom).toBeLessThanOrEqual(styles.wrapper.bottom);
    expect(styles.panel.bottom - styles.panel.top).toBeGreaterThan(300);
    expect(styles.backdrop.top).toBeGreaterThanOrEqual(styles.wrapper.top);
    expect(styles.backdrop.bottom).toBeLessThanOrEqual(styles.wrapper.bottom);
    expect(styles.backdrop.right).toBeLessThanOrEqual(styles.wrapper.right);
    // Close returns to the diagram.
    await drawer.getByRole('button', { name: /^close$/i }).click();
    await expect(drawer).toBeHidden();

    // A narrow cell (a phone, or a split view): the sheet leaves a 48px strip
    // of backdrop for tap-to-dismiss and stays inside the box.
    await page.evaluate(() => {
      document.getElementById('cell2')!.style.width = '320px';
    });
    await mountWidget(page, 'cell2', initialState({ height: 400 }));
    const cell2 = page.locator('#cell2');
    await expect(cell2.locator('svg.simlin-canvas')).toBeVisible({ timeout: 60_000 });
    await cell2.getByRole('button', { name: /^menu$/i }).click();
    const drawer2 = cell2.locator('.simlin-notebook-widget [role="dialog"]');
    await expect(drawer2).toBeVisible();
    await settleDrawer(page, '#cell2');
    const narrow = await page.evaluate(() => {
      const wrapper = document.querySelector('#cell2 .simlin-notebook-widget') as HTMLElement;
      const panel = document.querySelector('#cell2 .simlin-notebook-widget [role="dialog"]') as HTMLElement;
      const close = Array.from(panel.querySelectorAll('button')).find((b) => b.getAttribute('aria-label') === 'Close')!;
      return {
        wrapperClientWidth: wrapper.clientWidth,
        wrapperRight: wrapper.getBoundingClientRect().right,
        panelWidth: panel.getBoundingClientRect().width,
        panelRight: panel.getBoundingClientRect().right,
        closeRight: close.getBoundingClientRect().right,
      };
    });
    expect(narrow.wrapperClientWidth).toBeLessThan(375 + 48);
    expect(narrow.panelWidth).toBeCloseTo(narrow.wrapperClientWidth - 48, 0);
    expect(narrow.panelRight).toBeLessThanOrEqual(narrow.wrapperRight);
    expect(narrow.closeRight).toBeLessThanOrEqual(narrow.panelRight);
    // Kept as a visual artifact of the run (gitignored), not an assertion.
    await page.screenshot({ path: path.join(here, '.output', 'drawer-contained.png'), fullPage: true });
  });

  test('SIMLIN_WIDGET_ASSET=inline: the module compiles the engine from the inline global and never asks the kernel', async ({
    page,
  }) => {
    await serveHarness(page);
    const wasmBase64 = fs.readFileSync(files['/libsimlin-browser.wasm'].path).toString('base64');
    const idx = await page.evaluate(
      async ({ state, wasmBase64 }) => {
        const w = window as unknown as HarnessWindow;
        const mod = await w.harness.loadWidgetModule(wasmBase64);
        return w.harness.mount(mod, document.getElementById('cell1')!, state);
      },
      { state: initialState({ height: 300 }), wasmBase64 },
    );
    expect(idx).toBe(0);
    const cell = page.locator('#cell1');
    await expect(cell.locator('svg.simlin-canvas')).toBeVisible({ timeout: 60_000 });
    await expect(cell.getByText('Population', { exact: true }).first()).toBeVisible();
    const after = await page.evaluate(() => {
      const m = (window as unknown as HarnessWindow).harness.models[0];
      return {
        wasmRequests: m.wasmRequests,
        sent: m.sent,
        // Consumed by the module: not left on the page.
        inlineGlobal: (globalThis as Record<string, unknown>).__simlinWidgetInlineWasm,
      };
    });
    expect(after.wasmRequests).toBe(0);
    expect(after.sent).toEqual([]);
    expect(after.inlineGlobal).toBeUndefined();
  });

  test('a kernel that cannot supply the engine shows the error in the cell and a later widget retries', async ({
    page,
  }) => {
    await serveHarness(page);
    // First widget: the kernel replies with an error.
    const idx = await page.evaluate(async () => {
      const w = window as unknown as HarnessWindow & {
        harness: { FakeAnyModel: new (state: Record<string, unknown>, url: string) => unknown };
      };
      const model = new w.harness.FakeAnyModel(
        { project_json: '{}', revision: 0, height: 200, theme: 'light', read_only: false },
        '/libsimlin-browser.wasm',
      ) as unknown as {
        send: (c: unknown) => void;
        trigger: (n: string, ...a: unknown[]) => void;
      } & HarnessWindow['harness']['models'][number];
      model.send = (content: unknown) => {
        model.sent.push(content);
        if ((content as { type?: string }).type === 'wasm') {
          model.wasmRequests += 1;
          setTimeout(() => model.trigger('msg:custom', { type: 'wasm', error: 'asset missing from wheel' }, []), 0);
        }
      };
      w.harness.models.push(model);
      const mod = (await w.harness.loadWidgetModule()) as {
        default: { initialize: (c: unknown) => void; render: (c: unknown) => Promise<() => void> };
      };
      const widget = mod.default;
      widget.initialize({ model });
      await widget.render({ model, el: document.getElementById('cell1') });
      return w.harness.models.length - 1;
    });
    expect(idx).toBe(0);
    await expect(page.locator('#cell1').getByRole('status')).toContainText('asset missing from wheel');

    // Second widget with a healthy kernel: the failed shared promise was
    // dropped, so this one requests the wasm itself and renders.
    const second = await mountWidget(page, 'cell2', initialState({ height: 320 }));
    expect(second).toBe(1);
    await expect(page.locator('#cell2').locator('svg.simlin-canvas')).toBeVisible({ timeout: 60_000 });
    const requests = await page.evaluate(() => (window as unknown as HarnessWindow).harness.models[1].wasmRequests);
    expect(requests).toBe(1);
  });
});
