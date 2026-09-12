// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Frame timing of canvas drags on the largest model in the repo: C-LEARN v77
 * (~4000 view elements), converted from its `.mdl` by the engine in Node and
 * mounted through the static harness (support.ts), real Editor over real wasm.
 * It drags the stock the most flow ends attach to, then a ~50-element
 * rubber-band selection, each for about two seconds of pointer moves sent as
 * fast as the page accepts them, and writes `perf.json` (canvas-probe.ts
 * `outDir`): requestAnimationFrame deltas and long animation frames while the
 * pointer moves, and from each release the time until the widget sends the
 * saved project and the longest frame meanwhile.
 *
 * What it asserts: the edits land (the stock where the pointer left it, a
 * snapshot per release). What it reports but does NOT assert: any frame
 * budget. The numbers come from headless Chromium on the machine running the
 * test with synthetic CDP input, so they compare runs on one machine; they do
 * not predict user hardware, touch input, or the app host (which runs the
 * engine in a worker, where the notebook widget runs it on the main thread).
 */

import * as fs from 'node:fs';
import * as path from 'node:path';

import { test, expect, type Page } from '@playwright/test';
import { CloudWidth, StockHeight, StockWidth } from '@simlin/diagram/drawing/default';
import { Project } from '@simlin/engine';

import {
  hitAt,
  installProbe,
  outDir,
  settle,
  shot,
  toClient,
  type ProbeWindow,
  type Rect,
  type XY,
} from './canvas-probe';
import { mountWidget, repoRoot, serveHarness, widgetState } from './support';

// Playwright's trace recording snapshots the page on every action; over a
// ~4000-element view that slows the very input pipeline this file measures.
test.use({ trace: 'off' });

interface ClearnElement {
  type: string;
  uid: number;
  x?: number;
  y?: number;
  points?: Array<XY & { attachedToUid?: number }>;
}

interface ClearnView {
  elements: ClearnElement[];
  viewBox?: Rect;
  zoom?: number;
}

interface ClearnProject {
  models: Array<{ views: ClearnView[] }>;
}

// In-page recorder state. Times are performance.now() milliseconds.
interface FrameRecorder {
  /** requestAnimationFrame timestamps. */
  frames: number[];
  running: boolean;
  longFrames: Array<{ start: number; duration: number }>;
  observers: PerformanceObserver[];
  /** The release's event timestamp. */
  pointerUpAt: number | undefined;
  /** When the widget handed each snapshot to the kernel (the model's `send`). */
  snapshotAt: number[];
  restore: () => void;
}

const deltasOf = (times: readonly number[]): number[] => times.slice(1).map((t, i) => t - times[i]);

function frameStats(xs: readonly number[]): { n: number; mean: number; p95: number; max: number } {
  if (xs.length === 0) {
    return { n: 0, mean: 0, p95: 0, max: 0 };
  }
  const sorted = [...xs].sort((a, b) => a - b);
  const r = (v: number): number => Math.round(v * 10) / 10;
  return {
    n: xs.length,
    mean: r(xs.reduce((a, b) => a + b, 0) / xs.length),
    p95: r(sorted[Math.ceil(0.95 * sorted.length) - 1]),
    max: r(sorted[sorted.length - 1]),
  };
}

/**
 * Press at `from`, move along a 1.75-turn arc (radius 120 px) for `durationMs`
 * with each move sent as soon as the previous one was accepted (Chromium acks a
 * dispatched mouse event once the page has handled it), finish on the arc's
 * end point, and release. Frames are split at the release: `drag` covers the
 * moves; `release` runs from the pointerup event to the first frame after the
 * widget hands the saved project to the kernel, with a long animation frame
 * that spans the release counted there. Both are stamped in the page, so the
 * test's own polling adds nothing. Chromium's Event Timing does not report
 * pointermove, so frame times are the per-move measure.
 */
async function measureDrag(
  page: Page,
  from: XY,
  durationMs: number,
  modelIndex: number,
): Promise<{
  moves: number;
  dragMs: number;
  drag: { frames: ReturnType<typeof frameStats>; longAnimationFrames: ReturnType<typeof frameStats> };
  release: { toSnapshotMs: number; longestFrameMs: number; longAnimationFrames: ReturnType<typeof frameStats> };
  delta: XY;
}> {
  const radius = 120;
  const along = (fraction: number): XY => {
    const theta = 1.75 * Math.PI * fraction;
    return { x: from.x + radius * Math.sin(theta), y: from.y + (radius * (1 - Math.cos(theta))) / 2 };
  };
  await page.mouse.move(from.x, from.y);
  await page.mouse.down();
  await page.evaluate((index) => {
    const w = window as unknown as ProbeWindow & { __rec: FrameRecorder };
    const model = w.harness.models[index];
    const send = model.send;
    const rec: FrameRecorder = {
      frames: [],
      running: true,
      longFrames: [],
      observers: [],
      pointerUpAt: undefined,
      snapshotAt: [],
      restore: () => {
        model.send = send;
      },
    };
    const tick = (t: number): void => {
      if (rec.running) {
        rec.frames.push(t);
        requestAnimationFrame(tick);
      }
    };
    requestAnimationFrame(tick);
    try {
      const o = new PerformanceObserver((list) => {
        for (const e of list.getEntries()) {
          rec.longFrames.push({ start: e.startTime, duration: e.duration });
        }
      });
      o.observe({ type: 'long-animation-frame' });
      rec.observers.push(o);
    } catch {
      // Long Animation Frames are Chromium-only; the rAF timestamps still stand.
    }
    window.addEventListener(
      'pointerup',
      (e) => {
        rec.pointerUpAt = e.timeStamp;
      },
      { capture: true, once: true },
    );
    model.send = (content, callbacks, buffers) => {
      if ((content as { type?: unknown } | null)?.type === 'snapshot') {
        rec.snapshotAt.push(performance.now());
      }
      send.call(model, content, callbacks, buffers);
    };
    w.__rec = rec;
  }, modelIndex);
  const start = Date.now();
  let moves = 0;
  for (let elapsed = 0; elapsed < durationMs; elapsed = Date.now() - start) {
    const p = along(elapsed / durationMs);
    await page.mouse.move(p.x, p.y);
    moves++;
  }
  const end = along(1);
  await page.mouse.move(end.x, end.y);
  moves++;
  const dragMs = Date.now() - start;
  await page.mouse.up();
  await expect
    .poll(() => page.evaluate(() => (window as unknown as { __rec: FrameRecorder }).__rec.snapshotAt.length), {
      timeout: 120_000,
    })
    .toBeGreaterThan(0);
  await settle(page, 1);
  const recorded = await page.evaluate(() => {
    const rec = (window as unknown as { __rec: FrameRecorder }).__rec;
    rec.running = false;
    rec.restore();
    for (const o of rec.observers) {
      for (const e of o.takeRecords()) {
        rec.longFrames.push({ start: e.startTime, duration: e.duration });
      }
      o.disconnect();
    }
    return { frames: rec.frames, longFrames: rec.longFrames, pointerUpAt: rec.pointerUpAt, snapshotAt: rec.snapshotAt };
  });
  expect(recorded.pointerUpAt, 'the release was observed').toBeDefined();
  const up = recorded.pointerUpAt!;
  const saved = recorded.snapshotAt[0];
  const dragFrames = recorded.frames.filter((t) => t <= up);
  // The last frame before the release through the first frame after the
  // snapshot, so a frame the commit blocks is counted whole.
  const firstAfterSave = recorded.frames.findIndex((t) => t > saved);
  const releaseFrames = recorded.frames.slice(
    Math.max(0, dragFrames.length - 1),
    firstAfterSave < 0 ? recorded.frames.length : firstAfterSave + 1,
  );
  const endsBeforeRelease = (f: { start: number; duration: number }): boolean => f.start + f.duration <= up;
  return {
    moves,
    dragMs,
    drag: {
      frames: frameStats(deltasOf(dragFrames)),
      longAnimationFrames: frameStats(recorded.longFrames.filter(endsBeforeRelease).map((f) => f.duration)),
    },
    release: {
      toSnapshotMs: Math.round((saved - up) * 10) / 10,
      longestFrameMs: frameStats(deltasOf(releaseFrames)).max,
      longAnimationFrames: frameStats(recorded.longFrames.filter((f) => !endsBeforeRelease(f)).map((f) => f.duration)),
    },
    delta: { x: end.x - from.x, y: end.y - from.y },
  };
}

/**
 * A rubber band in the visible part of the view holding about `want` (within
 * -20%/+30%) of the elements the planner selects (stocks, clouds, flows by
 * valve, modules and aliases by center; auxes by center or by a corner inside
 * the circle), clear of the editor chrome, starting on empty canvas. Bands
 * keep the safe area's aspect ratio, since a view is wider than it is tall,
 * and grow from an empty start point in any of the four directions: a dense
 * view (links and labels everywhere) leaves few empty points, and the band
 * that holds `want` elements can span most of the visible view.
 * Zoom is 1, so model and client pixels agree up to the canvas offset.
 */
async function findRubberBand(
  page: Page,
  view: ClearnView,
  size: { width: number; height: number },
  canvasBox: { x: number; y: number },
  want: number,
): Promise<{ from: XY; to: XY; members: number; pressable: XY[] }> {
  const origin = await toClient(page, { x: 0, y: 0 });
  const toModel = (c: XY): XY => ({ x: c.x - origin.x, y: c.y - origin.y });
  // Clear of the search bar and tool FAB along the top and bottom edges.
  const top = toModel({ x: canvasBox.x + 24, y: canvasBox.y + 96 });
  const bottom = toModel({ x: canvasBox.x + size.width - 24, y: canvasBox.y + size.height - 96 });
  // Only elements in (or an aux radius around) the safe area can be members.
  const selectable = view.elements.filter(
    (e) =>
      ['stock', 'cloud', 'flow', 'module', 'alias', 'aux'].includes(e.type) &&
      e.x! >= top.x - 9 &&
      e.x! <= bottom.x + 9 &&
      e.y! >= top.y - 9 &&
      e.y! <= bottom.y + 9,
  ) as Array<ClearnElement & XY>;
  const membersOf = (l: number, t: number, r: number, b: number): Array<ClearnElement & XY> =>
    selectable.filter((e) => {
      if (e.x >= l && e.x <= r && e.y >= t && e.y <= b) {
        return true;
      }
      return (
        e.type === 'aux' &&
        [
          { x: l, y: t },
          { x: r, y: t },
          { x: l, y: b },
          { x: r, y: b },
        ].some((c) => Math.hypot(c.x - e.x, c.y - e.y) <= 9)
      );
    });
  const aspect = (bottom.x - top.x) / (bottom.y - top.y);
  const starts: XY[] = [];
  for (let y = top.y; y <= bottom.y; y += 24) {
    for (let x = top.x; x <= bottom.x; x += 24) {
      starts.push({ x, y });
    }
  }
  // A press reaches the canvas (and starts a rubber band) on the svg itself or
  // on a group's box, which has no press handler of its own.
  const empty = await page.evaluate(
    ({ points, offset }) =>
      points.map((p) => {
        const el = document.elementFromPoint(p.x + offset.x, p.y + offset.y);
        return el !== null && (el.tagName === 'svg' || el.closest('g.simlin-group') !== null);
      }),
    { points: starts, offset: origin },
  );
  let best = 0;
  for (const [i, start] of starts.entries()) {
    if (!empty[i]) {
      continue;
    }
    for (const [sx, sy] of [
      [1, 1],
      [-1, 1],
      [1, -1],
      [-1, -1],
    ]) {
      for (let span = 60; ; span += 12) {
        const end = { x: start.x + sx * span, y: start.y + (sy * span) / aspect };
        if (end.x < top.x || end.x > bottom.x || end.y < top.y || end.y > bottom.y) {
          break;
        }
        const members = membersOf(
          Math.min(start.x, end.x),
          Math.min(start.y, end.y),
          Math.max(start.x, end.x),
          Math.max(start.y, end.y),
        );
        best = Math.max(best, members.length);
        if (members.length < want * 0.8) {
          continue;
        }
        if (members.length > want * 1.3) {
          break;
        }
        return {
          from: { x: start.x + origin.x, y: start.y + origin.y },
          to: { x: end.x + origin.x, y: end.y + origin.y },
          members: members.length,
          pressable: members.filter((e) => e.type !== 'flow' && e.type !== 'cloud'),
        };
      }
    }
  }
  throw new Error(
    `no rubber band of ~${want} elements: ${empty.filter(Boolean).length} empty start points, best band held ${best}`,
  );
}

test('performance: a stock drag and a ~50-element selection drag on C-LEARN', async ({ page }) => {
  test.setTimeout(300_000);
  const errors: string[] = [];
  fs.mkdirSync(outDir, { recursive: true });
  page.on('pageerror', (err) => errors.push(`pageerror: ${err.message}`));
  await page.addInitScript(installProbe, CloudWidth);
  await serveHarness(page);

  // The canvas a PERF_HEIGHT cell gives, measured on a small mount in the
  // other cell (same width): a view seeded with a viewBox of exactly this size
  // opens at that viewBox with no fit on mount, and one inside the diagram's
  // bounds is not re-centered, so the drag target can be framed without a wheel
  // pan whose deferred viewport commit would land in the middle of a
  // measurement.
  const PERF_HEIGHT = 700;
  const small = fs.readFileSync(path.join(repoRoot, 'test', 'logistic-growth.sd.json'), 'utf8');
  await mountWidget(page, 'cell2', widgetState(small, { height: PERF_HEIGHT }));
  await expect(page.locator('#cell2 svg.simlin-canvas')).toBeVisible({ timeout: 60_000 });
  const size = await page.evaluate(() => {
    const el = document.querySelector('#cell2 div.simlin-canvas') as HTMLElement;
    return { width: el.clientWidth, height: el.clientHeight };
  });
  await page.evaluate(() => (window as unknown as ProbeWindow).harness.models[0].cleanup?.());

  const mdl = fs.readFileSync(path.join(repoRoot, 'test', 'xmutil_test_models', 'C-LEARN v77 for Vensim.mdl'));
  const project = await Project.openVensim(mdl);
  const clearn = JSON.parse(await project.serializeJson()) as ClearnProject;
  await project.dispose();
  const view = clearn.models[0].views[0];
  const byUid = new Map(view.elements.map((e) => [e.uid, e]));
  // The stock the most flow ends attach to: a drag routes every one of them.
  const ends = new Map<number, number>();
  for (const el of view.elements) {
    if (el.type !== 'flow' || el.points === undefined) {
      continue;
    }
    for (const p of [el.points[0], el.points[el.points.length - 1]]) {
      const t = p.attachedToUid === undefined ? undefined : byUid.get(p.attachedToUid);
      if (t?.type === 'stock') {
        ends.set(t.uid, (ends.get(t.uid) ?? 0) + 1);
      }
    }
  }
  const [targetUid, targetEnds] = [...ends.entries()].sort((a, b) => b[1] - a[1])[0];
  const target = byUid.get(targetUid)!;
  view.viewBox = {
    x: size.width / 2 - target.x!,
    y: size.height / 2 - target.y!,
    width: size.width,
    height: size.height,
  };
  view.zoom = 1;

  const openStart = Date.now();
  await mountWidget(page, 'cell1', widgetState(JSON.stringify(clearn), { height: PERF_HEIGHT }));
  await expect(page.locator('#cell1 svg.simlin-canvas')).toBeVisible({ timeout: 120_000 });
  const targetClient = await toClient(page, { x: target.x!, y: target.y! });
  const canvasBox = (await page.locator('#cell1 div.simlin-canvas').boundingBox())!;
  expect(targetClient.x - canvasBox.x, 'the view opened at the seeded viewBox').toBeCloseTo(size.width / 2, 0);
  expect(targetClient.y - canvasBox.y).toBeCloseTo(size.height / 2, 0);
  // The first simulation runs on the main thread after open; wait for its
  // results (the stock sparklines) so it does not land inside a measurement.
  await expect
    .poll(async () => page.locator('#cell1 g.simlin-stock > g[transform^="translate"]').count(), { timeout: 120_000 })
    .toBeGreaterThan(0);
  await settle(page);
  const openMs = Date.now() - openStart;
  await shot(page, 'perf-clearn-open');

  // ---- stock drag -----------------------------------------------------------
  expect((await hitAt(page, targetClient)).group).toContain('simlin-stock');
  const stockDrag = await measureDrag(page, targetClient, 2000, 1);
  // Where the pointer left the stock, within a pixel: the arc's points are
  // fractional client pixels, which the input pipeline need not preserve.
  const expectedRect = {
    x: target.x! - StockWidth / 2 + stockDrag.delta.x,
    y: target.y! - StockHeight / 2 + stockDrag.delta.y,
  };
  const nearestRect = await page.evaluate(
    ({ x, y }) =>
      Math.min(
        ...Array.from(document.querySelectorAll('#cell1 g.simlin-stock rect')).map((r) =>
          Math.hypot(Number(r.getAttribute('x')) - x, Number(r.getAttribute('y')) - y),
        ),
      ),
    expectedRect,
  );
  expect(nearestRect, 'the dragged stock landed where the pointer left it').toBeLessThan(1);
  await shot(page, 'perf-clearn-stock-drag-after');

  // ---- rubber band ~50 elements, then drag them ------------------------------
  const committed = JSON.parse(
    await page.evaluate(() => {
      const m = (window as unknown as ProbeWindow).harness.models[1];
      return m.snapshots[m.snapshots.length - 1].json;
    }),
  ) as ClearnProject;
  const band = await findRubberBand(page, committed.models[0].views[0], size, canvasBox, 50);
  await page.mouse.move(band.from.x, band.from.y);
  await page.mouse.down();
  await page.mouse.move(band.to.x, band.to.y, { steps: 20 });
  await page.mouse.up();
  await settle(page);
  const selectedCount = await page.locator('#cell1 svg.simlin-canvas .simlin-selected').count();
  await shot(page, 'perf-clearn-rubber-band');
  expect(selectedCount, 'the rubber band selected its members').toBeGreaterThan(band.members * 0.5);
  let grab: XY | undefined;
  for (const p of band.pressable) {
    const c = await toClient(page, p);
    if ((await hitAt(page, c)).group.includes('simlin-selected')) {
      grab = c;
      break;
    }
  }
  expect(grab, 'a selected element to grab').toBeDefined();
  const selectionDrag = await measureDrag(page, grab!, 2000, 1);
  await shot(page, 'perf-clearn-selection-drag-after');

  // What rendered the numbers: headless or not, and which GL backend (a
  // software renderer where no GPU is available).
  const browser = await page.evaluate(() => {
    const gl = document.createElement('canvas').getContext('webgl');
    const info = gl?.getExtension('WEBGL_debug_renderer_info');
    return {
      userAgent: navigator.userAgent,
      webglRenderer: gl && info ? String(gl.getParameter(info.UNMASKED_RENDERER_WEBGL)) : 'no webgl',
      devicePixelRatio: window.devicePixelRatio,
    };
  });
  const perf = {
    model: 'C-LEARN v77',
    viewElements: view.elements.length,
    browser,
    canvas: size,
    openMs,
    stockDrag: { stockEnds: targetEnds, ...stockDrag },
    selectionDrag: { rubberBandMembers: band.members, selectedDrawn: selectedCount, ...selectionDrag },
  };
  fs.writeFileSync(path.join(outDir, 'perf.json'), JSON.stringify(perf, null, 2));
  console.log(JSON.stringify(perf, null, 2));
  expect(errors).toEqual([]);
});
