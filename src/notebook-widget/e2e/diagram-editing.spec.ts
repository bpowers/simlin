// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Diagram-editing journey in a real browser: the built widget bundle mounts
 * the real Editor over the real engine wasm (the static harness, see
 * support.ts), and Playwright drives the canvas with real pointer and key
 * events through the gestures the diagram-editing core owns
 * (docs/design-plans/2026-09-10-diagram-editing-core.md). Every step reads the
 * rendered SVG -- flow path `d`, valve circles, cloud transforms, stock rects
 * -- and what the fake kernel received (snapshots carrying the whole project).
 *
 * What this establishes: in Chromium, through pointer capture, React's event
 * scheduling and the controller's executor, a stock drag bends its flows
 * orthogonally with perpendicular face exits; a click on a flow arrowhead or a
 * link arrowhead changes nothing and saves nothing; a flow end detached into
 * empty space previews a cloud at the pointer and commits exactly the last
 * preview frame; reattaching lands on the new stock's face and moves the flow
 * between stock lists in the saved model; a valve dragged perpendicular forms
 * a bracket that follows the pointer and collapses back to straight; Escape
 * cancels a live drag; a drawn flow ends in a cloud at the pointer and takes
 * its typed name; undo and redo restore and reapply geometry; and no routed
 * pipe runs through the body of a stock it was attached to. Frame timing on a
 * large model is diagram-editing-perf.spec.ts.
 *
 * What it does NOT establish: touch, pen or pinch input; Firefox or Safari;
 * zoom levels other than 1; hosts other than this harness (the JupyterLab
 * journey in src/pysimlin/e2e covers a real notebook); and exhaustive geometry
 * (the planner and flow-geometry suites fuzz that).
 */

import * as fs from 'node:fs';
import * as path from 'node:path';

import { test, expect, type Page } from '@playwright/test';
import { CloudWidth, StockWidth } from '@simlin/diagram/drawing/default';
import { CORNER_CLEARANCE } from '@simlin/diagram/flow-geometry/geometry';

import {
  hitAt,
  installProbe,
  outDir,
  probeClouds,
  probeFlow,
  probeStocks,
  settle,
  shot,
  toClient,
  type FlowProbe,
  type ProbeWindow,
  type Rect,
  type XY,
} from './canvas-probe';
import { mountWidget, serveHarness, widgetState } from './support';

interface Geometry {
  flows: Record<string, string>;
  clouds: string[];
  stocks: Record<string, Rect>;
  links: string[];
}

async function geometry(page: Page): Promise<Geometry> {
  return page.evaluate(() => {
    const j = (window as unknown as ProbeWindow).__journey;
    const flows: Record<string, string> = {};
    for (const [name, f] of Object.entries(j.flows())) {
      flows[name] = f.d;
    }
    return {
      flows,
      clouds: j
        .clouds()
        .map((c) => c.transform)
        .sort(),
      stocks: j.stocks(),
      links: j.links(),
    };
  });
}

async function snapshotState(page: Page): Promise<{ count: number; json: string | undefined; kernelJson: string }> {
  return page.evaluate(() => {
    const m = (window as unknown as ProbeWindow).harness.models[0];
    return {
      count: m.snapshots.length,
      json: m.snapshots.length > 0 ? m.snapshots[m.snapshots.length - 1].json : undefined,
      kernelJson: m.kernel.projectJson,
    };
  });
}
interface JsonView {
  elements: Array<{
    type: string;
    uid: number;
    name?: string;
    x?: number;
    y?: number;
    points?: Array<XY & { attachedToUid?: number }>;
  }>;
}

interface JsonProject {
  models: Array<{
    stocks: Array<{ name: string; inflows?: string[]; outflows?: string[] }>;
    flows: Array<{ name: string }>;
    views: JsonView[];
  }>;
}

// The engine matches identifiers canonically; lists may carry either spelling.
const canon = (name: string): string =>
  name
    .trim()
    .toLowerCase()
    .replace(/[\s_]+/g, '_');

function stockLists(project: JsonProject, stock: string): { inflows: string[]; outflows: string[] } {
  const s = project.models[0].stocks.find((x) => canon(x.name) === canon(stock));
  if (s === undefined) {
    throw new Error(`no stock ${stock} in snapshot`);
  }
  return { inflows: (s.inflows ?? []).map(canon), outflows: (s.outflows ?? []).map(canon) };
}

function viewElement(project: JsonProject, name: string): JsonView['elements'][number] {
  const el = project.models[0].views[0].elements.find((e) => e.name !== undefined && canon(e.name) === canon(name));
  if (el === undefined) {
    throw new Error(`no view element named ${name} in snapshot`);
  }
  return el;
}

async function waitForSnapshot(page: Page, what: string, predicate: (p: JsonProject) => boolean): Promise<JsonProject> {
  let found: JsonProject | undefined;
  await expect
    .poll(
      async () => {
        const { json } = await snapshotState(page);
        if (json === undefined) {
          return false;
        }
        const p = JSON.parse(json) as JsonProject;
        if (predicate(p)) {
          found = p;
          return true;
        }
        return false;
      },
      { message: what, timeout: 30_000 },
    )
    .toBe(true);
  return found!;
}

const journeyLog: Array<Record<string, unknown>> = [];

function note(step: string, data: Record<string, unknown>): void {
  journeyLog.push({ step, ...data });
  fs.writeFileSync(path.join(outDir, 'journey.json'), JSON.stringify(journeyLog, null, 2));
}

function unit(from: XY, to: XY): XY {
  const len = Math.hypot(to.x - from.x, to.y - from.y);
  return { x: (to.x - from.x) / len, y: (to.y - from.y) / len };
}

type Face = 'left' | 'right' | 'top' | 'bottom';

const OUTWARD: Record<Face, XY> = {
  left: { x: -1, y: 0 },
  right: { x: 1, y: 0 },
  top: { x: 0, y: -1 },
  bottom: { x: 0, y: 1 },
};

// G4: a stock endpoint lies on a face, at least CORNER_CLEARANCE from its corners.
function faceOf(stock: Rect, p: XY): Face | undefined {
  const eps = 1e-4;
  const alongY = p.y >= stock.y + CORNER_CLEARANCE - eps && p.y <= stock.y + stock.height - CORNER_CLEARANCE + eps;
  const alongX = p.x >= stock.x + CORNER_CLEARANCE - eps && p.x <= stock.x + stock.width - CORNER_CLEARANCE + eps;
  if (Math.abs(p.x - stock.x) <= eps && alongY) {
    return 'left';
  }
  if (Math.abs(p.x - (stock.x + stock.width)) <= eps && alongY) {
    return 'right';
  }
  if (Math.abs(p.y - stock.y) <= eps && alongX) {
    return 'top';
  }
  if (Math.abs(p.y - (stock.y + stock.height)) <= eps && alongX) {
    return 'bottom';
  }
  return undefined;
}

// G2 over the rendered path: every segment axis-aligned and non-degenerate.
function expectOrthogonal(flow: FlowProbe, what: string): void {
  expect(flow.points.length, `${what}: at least two points`).toBeGreaterThanOrEqual(2);
  for (let i = 0; i + 1 < flow.points.length; i++) {
    const a = flow.points[i];
    const b = flow.points[i + 1];
    expect(Math.min(Math.abs(a.x - b.x), Math.abs(a.y - b.y)), `${what}: segment ${i} is axis-aligned`).toBeLessThan(
      1e-5,
    );
    expect(Math.hypot(a.x - b.x, a.y - b.y), `${what}: segment ${i} has length`).toBeGreaterThan(1e-5);
  }
}

// G4 + G5 at the sink: the arrowhead tip (the stored endpoint for a stock sink)
// is on a face, and the final segment enters perpendicular to it.
function expectSinkOnFace(flow: FlowProbe, stock: Rect, what: string): Face {
  const face = faceOf(stock, flow.tip);
  expect(
    face,
    `${what}: sink endpoint ${JSON.stringify(flow.tip)} on a face of ${JSON.stringify(stock)}`,
  ).toBeDefined();
  const n = flow.points.length;
  const u = unit(flow.points[n - 2], flow.points[n - 1]);
  expect(u.x, `${what}: final segment enters the ${face} face perpendicular`).toBeCloseTo(-OUTWARD[face!].x, 6);
  expect(u.y, `${what}: final segment enters the ${face} face perpendicular`).toBeCloseTo(-OUTWARD[face!].y, 6);
  return face!;
}

// G4 + G5 at the source: the first point is on a face and the first segment leaves outward.
function expectSourceOnFace(flow: FlowProbe, stock: Rect, what: string): Face {
  const face = faceOf(stock, flow.points[0]);
  expect(
    face,
    `${what}: source endpoint ${JSON.stringify(flow.points[0])} on a face of ${JSON.stringify(stock)}`,
  ).toBeDefined();
  const u = unit(flow.points[0], flow.points[1]);
  expect(u.x, `${what}: first segment leaves the ${face} face perpendicular`).toBeCloseTo(OUTWARD[face!].x, 6);
  expect(u.y, `${what}: first segment leaves the ${face} face perpendicular`).toBeCloseTo(OUTWARD[face!].y, 6);
  return face!;
}

function near(a: XY, b: XY, tolerance: number): boolean {
  return Math.abs(a.x - b.x) <= tolerance && Math.abs(a.y - b.y) <= tolerance;
}

/**
 * The indices of a rendered flow's segments that pass through the interior of
 * `stock` (a segment running along a face, or leaving it perpendicular, does
 * not). Segments are axis-aligned (G2), so this is an interval overlap test.
 */
function segmentsThrough(flow: FlowProbe, stock: Rect): number[] {
  const eps = 1e-4;
  const crossing: number[] = [];
  for (let i = 0; i + 1 < flow.points.length; i++) {
    const a = flow.points[i];
    const b = flow.points[i + 1];
    const overlapX = Math.min(Math.max(a.x, b.x), stock.x + stock.width) - Math.max(Math.min(a.x, b.x), stock.x);
    const overlapY = Math.min(Math.max(a.y, b.y), stock.y + stock.height) - Math.max(Math.min(a.y, b.y), stock.y);
    const horizontal = Math.abs(a.y - b.y) < eps;
    const inside = horizontal
      ? a.y > stock.y + eps && a.y < stock.y + stock.height - eps && overlapX > eps
      : a.x > stock.x + eps && a.x < stock.x + stock.width - eps && overlapY > eps;
    if (inside) {
      crossing.push(i);
    }
  }
  return crossing;
}

const stockCenter = (r: Rect): XY => ({ x: r.x + r.width / 2, y: r.y + r.height / 2 });

// The journey's project, in the engine's native JSON, loaded through the real
// engine exactly as a kernel seed is. Endpoints are pinned to stock faces the
// way the planner pins them (a face point, perpendicular exit), clouds sit at
// their endpoints (G7), and every stock's lists name the flows attached to it:
// the same shape an Editor-drawn diagram saves.
function journeyProject(): string {
  const halfW = StockWidth / 2;
  const population = { x: 200, y: 150 };
  const inventory = { x: 200, y: 400 };
  const warehouse = { x: 460, y: 400 };
  const reservoir = { x: 640, y: 150 };
  const birthsCloud = { x: 60, y: 150 };
  const stock = (uid: number, name: string, at: XY, labelSide: string) => ({
    type: 'stock',
    uid,
    name,
    x: at.x,
    y: at.y,
    labelSide,
  });
  return JSON.stringify({
    name: 'diagram-editing-journey',
    simSpecs: { startTime: 0, endTime: 10, dt: '1', method: 'euler' },
    dimensions: [],
    units: [],
    models: [
      {
        name: 'main',
        stocks: [
          { name: 'Population', initialEquation: '100', inflows: ['births'], outflows: [] },
          { name: 'Inventory', initialEquation: '100', inflows: [], outflows: ['shipments'] },
          { name: 'Warehouse', initialEquation: '0', inflows: ['shipments'], outflows: [] },
          { name: 'Reservoir', initialEquation: '0', inflows: [], outflows: [] },
        ],
        flows: [
          { name: 'births', equation: 'rate' },
          { name: 'shipments', equation: 'rate' },
        ],
        auxiliaries: [{ name: 'rate', equation: '5' }],
        views: [
          {
            kind: 'stock_flow',
            elements: [
              stock(1, 'Population', population, 'top'),
              {
                type: 'flow',
                uid: 2,
                name: 'births',
                x: (birthsCloud.x + population.x - halfW) / 2,
                y: population.y,
                labelSide: 'bottom',
                points: [
                  { x: birthsCloud.x, y: birthsCloud.y, attachedToUid: 3 },
                  { x: population.x - halfW, y: population.y, attachedToUid: 1 },
                ],
              },
              { type: 'cloud', uid: 3, flowUid: 2, x: birthsCloud.x, y: birthsCloud.y },
              stock(4, 'Inventory', inventory, 'bottom'),
              stock(5, 'Warehouse', warehouse, 'bottom'),
              {
                type: 'flow',
                uid: 6,
                name: 'shipments',
                x: (inventory.x + warehouse.x) / 2,
                y: inventory.y,
                labelSide: 'bottom',
                points: [
                  { x: inventory.x + halfW, y: inventory.y, attachedToUid: 4 },
                  { x: warehouse.x - halfW, y: warehouse.y, attachedToUid: 5 },
                ],
              },
              stock(7, 'Reservoir', reservoir, 'top'),
              { type: 'aux', uid: 8, name: 'rate', x: (inventory.x + warehouse.x) / 2, y: 300, labelSide: 'top' },
              { type: 'link', uid: 9, fromUid: 8, toUid: 6 },
            ],
            viewBox: { x: 0, y: 0, width: 0, height: 0 },
            zoom: 1,
          },
        ],
      },
    ],
  });
}

async function openJourney(page: Page, errors: string[]): Promise<void> {
  fs.mkdirSync(outDir, { recursive: true });
  page.on('pageerror', (err) => errors.push(`pageerror: ${err.message}`));
  page.on('console', (msg) => {
    if (msg.type() === 'error') {
      errors.push(`console.error: ${msg.text()}`);
    }
  });
  await page.addInitScript(installProbe, CloudWidth);
  await serveHarness(page);
  await mountWidget(page, 'cell1', widgetState(journeyProject(), { height: 640 }));
  await expect(page.locator('#cell1 svg.simlin-canvas')).toBeVisible({ timeout: 60_000 });
  await expect
    .poll(async () => Object.keys(await probeStocks(page)).sort())
    .toEqual(['Inventory', 'Population', 'Reservoir', 'Warehouse']);
  await settle(page);
}

test.describe('diagram editing in a real browser', () => {
  test('stock drag, arrowhead clicks, detach and reattach, bracket, drawn flow, undo and redo', async ({ page }) => {
    const errors: string[] = [];
    await openJourney(page, errors);
    const cell = page.locator('#cell1');
    expect((await snapshotState(page)).count, 'opening the project saves nothing').toBe(0);

    // ---- 1. Drag a stock perpendicular to its cloud-ended flow ----------------
    const stocks0 = await probeStocks(page);
    const births0 = (await probeFlow(page, 'births'))!;
    expect(births0.points.length, 'births starts straight').toBe(2);
    await shot(page, '01-stock-drag-before');
    const pop0 = stockCenter(stocks0.Population);
    const popTarget = { x: pop0.x, y: pop0.y + 80 };
    const popPress = await toClient(page, pop0);
    const popRelease = await toClient(page, popTarget);
    expect((await hitAt(page, popPress)).group).toContain('simlin-stock');
    await page.mouse.move(popPress.x, popPress.y);
    await page.mouse.down();
    await page.mouse.move(popRelease.x, popRelease.y, { steps: 12 });
    await expect.poll(async () => (await probeStocks(page)).Population.y).toBeCloseTo(stocks0.Population.y + 80, 6);
    const birthsPreview1 = (await probeFlow(page, 'births'))!;
    await shot(page, '01-stock-drag-preview');
    await page.mouse.up();
    const snap1 = await waitForSnapshot(page, 'the stock move is saved', (p) =>
      near(viewElement(p, 'Population') as XY, popTarget, 1e-6),
    );
    const stocks1 = await probeStocks(page);
    const births1 = (await probeFlow(page, 'births'))!;
    await shot(page, '01-stock-drag-after');
    note('1-stock-drag', {
      before: births0,
      preview: birthsPreview1,
      committed: births1,
      population: stocks1.Population,
    });
    expect(births1.d, 'the committed flow is the last preview frame (E2)').toBe(birthsPreview1.d);
    expect(births1.points.length, 'births bends (an L or a Z)').toBeGreaterThanOrEqual(3);
    expectOrthogonal(births1, 'births after the stock drag');
    expectSinkOnFace(births1, stocks1.Population, 'births after the stock drag');
    expect(segmentsThrough(births1, stocks1.Population), 'no segment runs through Population (G6)').toEqual([]);
    const sourceCloud1 = (await probeClouds(page))[0];
    expect(near(births1.points[0], sourceCloud1.center, 1e-5), 'births still starts at its cloud (G7)').toBe(true);
    expect(stockLists(snap1, 'Population').inflows).toEqual(['births']);

    // ---- 2. Click (no movement) the flow's arrowhead on the stock --------------
    const geometry2 = await geometry(page);
    const count2 = (await snapshotState(page)).count;
    {
      const n = births1.points.length;
      const u = unit(births1.points[n - 2], births1.points[n - 1]);
      // The middle of the arrowhead: back from the tip, off the stock's body.
      const onArrowhead = await toClient(page, { x: births1.tip.x - 4 * u.x, y: births1.tip.y - 4 * u.y });
      const hit = await hitAt(page, onArrowhead);
      expect(hit.cls, 'the press lands on the flow arrowhead').toContain('simlin-arrowhead');
      await shot(page, '02-arrowhead-click-before');
      await page.mouse.click(onArrowhead.x, onArrowhead.y);
      await settle(page);
      await shot(page, '02-arrowhead-click-after');
      const after = await geometry(page);
      note('2-arrowhead-click', { hit, snapshotsBefore: count2, snapshotsAfter: (await snapshotState(page)).count });
      expect(after, 'a click on an attached arrowhead changes nothing').toEqual(geometry2);
      expect((await snapshotState(page)).count, 'and saves nothing').toBe(count2);

      // ---- 3. Drag that end off the stock into empty space -----------------------
      const empty = { x: 380, y: 230 };
      const pressModel = { x: births1.tip.x - 4 * u.x, y: births1.tip.y - 4 * u.y };
      const expectedCloud = {
        x: births1.tip.x + (empty.x - pressModel.x),
        y: births1.tip.y + (empty.y - pressModel.y),
      };
      const emptyClient = await toClient(page, empty);
      expect((await hitAt(page, emptyClient)).tag, 'the drop point is empty canvas').toBe('svg');
      await page.mouse.move(onArrowhead.x, onArrowhead.y);
      await page.mouse.down();
      await page.mouse.move(emptyClient.x, emptyClient.y, { steps: 15 });
      await expect
        .poll(async () => (await probeClouds(page)).some((c) => near(c.center, expectedCloud, 1e-5)), {
          message: 'the preview shows a cloud at the pointer, keeping the grab offset',
        })
        .toBe(true);
      const preview3 = { births: (await probeFlow(page, 'births'))!, clouds: await probeClouds(page) };
      const cloudClient = await toClient(page, expectedCloud);
      await shot(page, '03-detach-preview');
      // The grab offset is the 4px from the arrowhead tip to where it was pressed.
      expect(Math.hypot(cloudClient.x - emptyClient.x, cloudClient.y - emptyClient.y)).toBeCloseTo(4, 4);
      await page.mouse.up();
      await settle(page, 1);
      const released3 = { births: (await probeFlow(page, 'births'))!, clouds: await probeClouds(page) };
      expect(released3, 'the release renders exactly the last preview frame (E2)').toEqual(preview3);
      const snap3 = await waitForSnapshot(
        page,
        'the detach is saved',
        (p) => !stockLists(p, 'Population').inflows.includes('births'),
      );
      await settle(page);
      const committed3 = { births: (await probeFlow(page, 'births'))!, clouds: await probeClouds(page) };
      await shot(page, '03-detach-after');
      note('3-detach', { pointer: emptyClient, expectedCloud, preview: preview3, committed: committed3 });
      expect(committed3, 'after the engine round trip the geometry is still the last preview frame').toEqual(preview3);
      expect(committed3.clouds.length, 'a source cloud and the new sink cloud').toBe(2);
      expectOrthogonal(committed3.births, 'births detached');
      // A pipe through the body of the stock the end just left reads as still
      // attached to it. Soft: the committed state is otherwise valid, so the
      // steps that follow still run and report.
      expect
        .soft(
          segmentsThrough(committed3.births, stocks1.Population),
          'no segment runs through Population, the stock the end detached from',
        )
        .toEqual([]);
      const savedBirths = viewElement(snap3, 'births');
      const sinkUid = savedBirths.points![savedBirths.points!.length - 1].attachedToUid;
      const savedCloud = snap3.models[0].views[0].elements.find((e) => e.uid === sinkUid);
      expect(savedCloud?.type, 'the saved sink is a cloud').toBe('cloud');
      expect(near(savedCloud as XY, expectedCloud, 1e-6), 'the saved cloud is where the preview drew it').toBe(true);
      expect(stockLists(snap3, 'Population').inflows, 'the saved stock no longer lists the flow (M2)').toEqual([]);
    }

    // ---- 4. Drag the end back, onto another stock ------------------------------
    {
      const births = (await probeFlow(page, 'births'))!;
      const clouds = await probeClouds(page);
      const sink = clouds.find((c) => !near(c.center, births.points[0], 1e-5))!;
      const stocks = await probeStocks(page);
      const pressClient = await toClient(page, sink.center);
      expect((await hitAt(page, pressClient)).cls, 'the press lands on the sink cloud').toContain('simlin-cloud');
      const reservoirClient = await toClient(page, stockCenter(stocks.Reservoir));
      await shot(page, '04-reattach-before');
      await page.mouse.move(pressClient.x, pressClient.y);
      await page.mouse.down();
      await page.mouse.move(reservoirClient.x, reservoirClient.y, { steps: 20 });
      await expect
        .poll(async () => faceOf(stocks.Reservoir, (await probeFlow(page, 'births'))!.tip), {
          message: 'the preview attaches births to a face of Reservoir',
        })
        .toBeDefined();
      const preview4 = { births: (await probeFlow(page, 'births'))!, clouds: await probeClouds(page) };
      await shot(page, '04-reattach-preview');
      await page.mouse.up();
      const snap4 = await waitForSnapshot(page, 'the reattach is saved', (p) =>
        stockLists(p, 'Reservoir').inflows.includes('births'),
      );
      await settle(page);
      const committed4 = { births: (await probeFlow(page, 'births'))!, clouds: await probeClouds(page) };
      await shot(page, '04-reattach-after');
      note('4-reattach', { preview: preview4, committed: committed4, reservoir: stocks.Reservoir });
      expect(committed4, 'the committed geometry is the last preview frame').toEqual(preview4);
      expect(committed4.clouds.length, 'the sink cloud is gone').toBe(1);
      expectOrthogonal(committed4.births, 'births reattached');
      expectSinkOnFace(committed4.births, stocks.Reservoir, 'births reattached');
      for (const name of ['Reservoir', 'Population']) {
        expect(
          segmentsThrough(committed4.births, stocks[name]),
          `births reattached: no segment through ${name}`,
        ).toEqual([]);
      }
      expect(stockLists(snap4, 'Reservoir').inflows, 'the saved model lists the flow in the new stock (M2)').toEqual([
        'births',
      ]);
      expect(stockLists(snap4, 'Population').inflows).toEqual([]);
    }

    // ---- 5. Drag a straight flow's valve perpendicular past the faces ----------
    {
      const stocks = await probeStocks(page);
      const shipments = (await probeFlow(page, 'shipments'))!;
      expect(shipments.points.length, 'shipments starts straight').toBe(2);
      const valveClient = await toClient(page, shipments.valve);
      expect((await hitAt(page, valveClient)).group, 'the press lands on the valve').toContain('simlin-flow');
      const bracketY = shipments.valve.y + 80;
      const bracketClient = await toClient(page, { x: shipments.valve.x, y: bracketY });
      await shot(page, '05-bracket-before');
      await page.mouse.move(valveClient.x, valveClient.y);
      await page.mouse.down();
      // The first move past the click threshold is perpendicular, which latches
      // the press into a segment offset rather than a valve slide.
      await page.mouse.move(bracketClient.x, bracketClient.y, { steps: 16 });
      await expect.poll(async () => (await probeFlow(page, 'shipments'))!.valve.y).toBeCloseTo(bracketY, 6);
      const preview5 = (await probeFlow(page, 'shipments'))!;
      await shot(page, '05-bracket-preview');
      await page.mouse.up();
      await waitForSnapshot(page, 'the bracket is saved', (p) => viewElement(p, 'shipments').points!.length >= 4);
      await settle(page);
      const bracket = (await probeFlow(page, 'shipments'))!;
      await shot(page, '05-bracket-after');
      note('5-bracket', { before: shipments, preview: preview5, committed: bracket, pointer: bracketClient });
      expect(bracket, 'the committed bracket is the last preview frame').toEqual(preview5);
      expect(bracket.points.length, 'a bracket: stub, riser, middle, riser, stub').toBeGreaterThanOrEqual(4);
      expectOrthogonal(bracket, 'shipments bracket');
      expectSourceOnFace(bracket, stocks.Inventory, 'shipments bracket');
      expectSinkOnFace(bracket, stocks.Warehouse, 'shipments bracket');
      for (const name of ['Inventory', 'Warehouse']) {
        expect(segmentsThrough(bracket, stocks[name]), `shipments bracket: no segment through ${name}`).toEqual([]);
      }
      // The middle segment follows the pointer: it carries the valve, at the
      // pointer's height.
      const middle = bracket.points.findIndex(
        (p, i) =>
          i + 1 < bracket.points.length &&
          Math.abs(p.y - bracketY) < 1e-6 &&
          Math.abs(bracket.points[i + 1].y - bracketY) < 1e-6,
      );
      expect(middle, 'a segment runs at the pointer height').toBeGreaterThan(0);
      expect((await toClient(page, bracket.valve)).y).toBeCloseTo(bracketClient.y, 4);

      // Drag it back: straight again.
      const backClient = await toClient(page, shipments.valve);
      const bracketValveClient = await toClient(page, bracket.valve);
      await page.mouse.move(bracketValveClient.x, bracketValveClient.y);
      await page.mouse.down();
      await page.mouse.move(backClient.x, backClient.y, { steps: 16 });
      await expect.poll(async () => (await probeFlow(page, 'shipments'))!.points.length).toBe(2);
      const previewBack = (await probeFlow(page, 'shipments'))!;
      await shot(page, '05-straight-preview');
      await page.mouse.up();
      await waitForSnapshot(
        page,
        'the straightened flow is saved',
        (p) => viewElement(p, 'shipments').points!.length === 2,
      );
      await settle(page);
      const straight = (await probeFlow(page, 'shipments'))!;
      await shot(page, '05-straight-after');
      note('5-straight', { preview: previewBack, committed: straight });
      expect(straight, 'the committed straight flow is the last preview frame').toEqual(previewBack);
      expect(straight.d, 'dragging the bracket back restores the original straight pipe').toBe(shipments.d);
      expectSourceOnFace(straight, stocks.Inventory, 'shipments straightened');
      expectSinkOnFace(straight, stocks.Warehouse, 'shipments straightened');
    }

    // ---- 6. Click a link's arrowhead ---------------------------------------------
    {
      const geometry6 = await geometry(page);
      const count6 = (await snapshotState(page)).count;
      expect(geometry6.links.length).toBe(1);
      const head = await page.evaluate(() => (window as unknown as ProbeWindow).__journey.linkArrowheadCenter());
      const hit = await hitAt(page, head);
      expect(hit.cls, 'the press lands on the link arrowhead').toContain('simlin-arrowhead');
      await shot(page, '06-link-arrowhead-before');
      await page.mouse.click(head.x, head.y);
      await settle(page);
      await shot(page, '06-link-arrowhead-after');
      note('6-link-arrowhead-click', {
        hit,
        snapshotsBefore: count6,
        snapshotsAfter: (await snapshotState(page)).count,
      });
      expect(await geometry(page), 'the link still exists, unchanged').toEqual(geometry6);
      expect((await snapshotState(page)).count, 'and nothing is saved').toBe(count6);
    }

    // ---- 8. Draw a new flow from a stock into empty space, then name it -------
    const geometry8Before = await geometry(page);
    {
      const stocks = await probeStocks(page);
      await cell.getByRole('button', { name: 'hide or show editor tools' }).click();
      await cell.getByRole('button', { name: 'Flow', exact: true }).click();
      const pressClient = await toClient(page, stockCenter(stocks.Reservoir));
      expect((await hitAt(page, pressClient)).group).toContain('simlin-stock');
      const target = { x: stockCenter(stocks.Reservoir).x + 120, y: stockCenter(stocks.Reservoir).y + 150 };
      const targetClient = await toClient(page, target);
      expect((await hitAt(page, targetClient)).tag, 'the drop point is empty canvas').toBe('svg');
      await shot(page, '08-draw-flow-before');
      await page.mouse.move(pressClient.x, pressClient.y);
      await page.mouse.down();
      await page.mouse.move(targetClient.x, targetClient.y, { steps: 20 });
      await expect
        .poll(async () => (await probeClouds(page)).some((c) => near(c.center, target, 1e-5)), {
          message: 'the drawn flow previews a cloud at the pointer',
        })
        .toBe(true);
      // While drawn, the flow carries its default name.
      const previewDrain = await probeFlow(page, 'New Flow');
      const previewClouds = await probeClouds(page);
      await shot(page, '08-draw-flow-preview');
      await page.mouse.up();
      const nameEditor = cell.locator('[contenteditable="true"]');
      await expect(nameEditor, 'the drawn flow opens its name editor').toBeVisible();
      await shot(page, '08-draw-flow-naming');
      // The editor opens with its default name selected: typing replaces it.
      await page.keyboard.type('drain');
      await page.keyboard.press('Enter');
      const snap8 = await waitForSnapshot(page, 'the named flow is saved', (p) =>
        p.models[0].flows.some((f) => canon(f.name) === 'drain'),
      );
      await settle(page);
      await expect.poll(async () => (await probeFlow(page, 'drain')) !== null).toBe(true);
      const drain = (await probeFlow(page, 'drain'))!;
      await shot(page, '08-draw-flow-after');
      const clouds8 = await probeClouds(page);
      note('8-draw-flow', { target, targetClient, preview: previewDrain, committed: drain, clouds: clouds8 });
      expect(previewDrain, 'the preview drew the new flow').not.toBeNull();
      expect(drain.d, 'the named flow is the last preview frame').toBe(previewDrain!.d);
      expect(clouds8, 'and so are the clouds').toEqual(previewClouds);
      expectOrthogonal(drain, 'drain');
      expectSourceOnFace(drain, stocks.Reservoir, 'drain');
      const savedDrain = viewElement(snap8, 'drain');
      const sinkUid = savedDrain.points![savedDrain.points!.length - 1].attachedToUid;
      const savedCloud = snap8.models[0].views[0].elements.find((e) => e.uid === sinkUid);
      expect(savedCloud?.type, 'the drawn flow ends in a cloud').toBe('cloud');
      expect(near(savedCloud as XY, target, 1e-6), 'at the pointer').toBe(true);
      expect(stockLists(snap8, 'Reservoir').outflows, 'the saved stock lists the drawn flow').toEqual(['drain']);
    }
    const geometry8After = await geometry(page);

    // ---- 9. Undo then redo the drawn flow ----------------------------------------
    {
      const undo = cell.getByRole('button', { name: 'Undo' });
      const redo = cell.getByRole('button', { name: 'Redo' });
      // Drawing the flow and naming it are two edits: undo takes the name back,
      // then the flow.
      await expect(undo).toBeEnabled();
      await undo.click();
      await expect
        .poll(async () => (await probeFlow(page, 'New Flow')) !== null, {
          message: 'the first undo takes the name back',
        })
        .toBe(true);
      await expect(undo).toBeEnabled();
      await undo.click();
      await expect
        .poll(async () => geometry(page), { message: 'the second undo restores the geometry before the flow' })
        .toEqual(geometry8Before);
      await waitForSnapshot(
        page,
        'the undo is saved',
        (p) => !p.models[0].flows.some((f) => canon(f.name) === 'drain' || canon(f.name) === 'new_flow'),
      );
      await shot(page, '09-undo-after');
      await expect(redo).toBeEnabled();
      await redo.click();
      await expect(redo).toBeEnabled();
      await redo.click();
      await expect
        .poll(async () => geometry(page), { message: 'redo reapplies the drawn and named flow' })
        .toEqual(geometry8After);
      const snap9 = await waitForSnapshot(page, 'the redo is saved', (p) =>
        p.models[0].flows.some((f) => canon(f.name) === 'drain'),
      );
      await shot(page, '09-redo-after');
      note('9-undo-redo', { before: geometry8Before, after: geometry8After });
      expect(stockLists(snap9, 'Reservoir').outflows).toEqual(['drain']);
    }

    const final = await snapshotState(page);
    expect(final.kernelJson, 'the kernel accepted the last snapshot').toBe(final.json);
    expect(errors).toEqual([]);
  });

  test('Escape during a stock drag cancels it: pre-drag geometry, nothing committed', async ({ page }) => {
    const errors: string[] = [];
    await openJourney(page, errors);
    const cell = page.locator('#cell1');
    const before = await geometry(page);
    const warehouse = stockCenter(before.stocks.Warehouse);
    const press = await toClient(page, warehouse);
    const moved = await toClient(page, { x: warehouse.x + 40, y: warehouse.y + 70 });
    expect((await hitAt(page, press)).group).toContain('simlin-stock');
    await shot(page, '07-escape-before');
    await page.mouse.move(press.x, press.y);
    await page.mouse.down();
    await page.mouse.move(moved.x, moved.y, { steps: 12 });
    await expect.poll(async () => (await probeStocks(page)).Warehouse.y).toBeCloseTo(before.stocks.Warehouse.y + 70, 6);
    const preview = await geometry(page);
    await shot(page, '07-escape-preview');
    await page.keyboard.press('Escape');
    await settle(page);
    const afterEscape = await geometry(page);
    await shot(page, '07-escape-after-escape');
    // Keep moving and release: a cancelled gesture must not come back to life.
    await page.mouse.move(moved.x + 10, moved.y + 10, { steps: 4 });
    await page.mouse.up();
    await settle(page);
    const afterRelease = await geometry(page);
    const snapshots = (await snapshotState(page)).count;
    await shot(page, '07-escape-after-release');
    note('7-escape', {
      before: before.stocks.Warehouse,
      preview: preview.stocks.Warehouse,
      afterEscape: afterEscape.stocks.Warehouse,
      afterRelease: afterRelease.stocks.Warehouse,
      snapshots,
      errors,
    });
    await expect(cell.locator('[data-simlin-editor-root]'), 'the editor is still rendered').toHaveCount(1);
    await expect(cell.getByText('Something went wrong'), 'no error boundary').toHaveCount(0);
    expect(errors).toEqual([]);
    expect(afterEscape, 'Escape restores the pre-drag geometry').toEqual(before);
    expect(afterRelease, 'the release after Escape commits nothing').toEqual(before);
    expect(snapshots, 'nothing is saved').toBe(0);
  });
});
