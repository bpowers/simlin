// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * The in-page DOM probe the diagram-editing journeys read the canvas through:
 * flow paths, valves, arrowheads, clouds, stock rects and links of the Editor
 * mounted in `#cell1`, model-to-client coordinate mapping, hit testing, and a
 * settle point. Everything reads rendered SVG, never Editor internals.
 */

import * as path from 'node:path';

import type { Page } from '@playwright/test';

import type { HarnessWindow } from './support';

const here = import.meta.dirname;

// Screenshots, step measurements and perf numbers land here (gitignored by
// default); point SIMLIN_JOURNEY_OUTPUT elsewhere to collect them for a PR.
export const outDir = process.env.SIMLIN_JOURNEY_OUTPUT ?? path.join(here, '.output', 'diagram-editing');

export interface XY {
  x: number;
  y: number;
}

export interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface FlowProbe {
  d: string;
  /** The rendered path's points (the sink end is retracted for the arrowhead). */
  points: XY[];
  valve: XY;
  /** The arrowhead tip: the stored sink endpoint when the sink is a stock. */
  tip: XY;
}

export interface CloudProbe {
  transform: string;
  center: XY;
}

/**
 * Installed with `page.addInitScript(installProbe, CloudWidth)` before the
 * harness loads. Self-contained (serialized into the page), so it may not close
 * over anything.
 */
export function installProbe(cloudWidth: number): void {
  const canvas = (): SVGSVGElement => {
    const svg = document.querySelector('#cell1 svg.simlin-canvas');
    if (!(svg instanceof SVGSVGElement)) {
      throw new Error('no canvas in #cell1');
    }
    return svg;
  };
  // The transformed group every element is drawn in: its CTM maps model
  // coordinates to client pixels (zoom and pan included).
  const layer = (): SVGGraphicsElement => {
    const g = canvas().querySelector(':scope > g');
    if (!(g instanceof SVGGraphicsElement)) {
      throw new Error('no canvas layer');
    }
    return g;
  };
  const labelOf = (g: Element): string =>
    Array.from(g.querySelectorAll('text'))
      .map((t) => t.textContent ?? '')
      .join('');
  const num = (el: Element, attr: string): number => Number(el.getAttribute(attr));
  const parsePath = (d: string): Array<{ x: number; y: number }> =>
    Array.from(d.matchAll(/[ML](-?[\d.e+-]+),(-?[\d.e+-]+)/g)).map((m) => ({ x: Number(m[1]), y: Number(m[2]) }));
  const flowProbe = (g: Element) => {
    const inner = g.querySelector('path.simlin-inner');
    const valve = g.querySelector(':scope > g > circle');
    const arrow = g.querySelector('path.simlin-arrowhead-flow');
    if (!inner || !valve || !arrow) {
      throw new Error('flow group without pipe, valve or arrowhead');
    }
    const d = inner.getAttribute('d') ?? '';
    return {
      d,
      points: parsePath(d),
      valve: { x: num(valve, 'cx'), y: num(valve, 'cy') },
      tip: parsePath(arrow.getAttribute('d') ?? '')[0],
    };
  };
  const probe = {
    toClient(p: { x: number; y: number }) {
      const m = layer().getScreenCTM();
      if (m === null) {
        throw new Error('canvas layer has no CTM');
      }
      return { x: m.a * p.x + m.c * p.y + m.e, y: m.b * p.x + m.d * p.y + m.f };
    },
    flows() {
      const out: Record<string, ReturnType<typeof flowProbe>> = {};
      for (const g of Array.from(canvas().querySelectorAll('g.simlin-flow'))) {
        out[labelOf(g)] = flowProbe(g);
      }
      return out;
    },
    flow(name: string) {
      const g = Array.from(canvas().querySelectorAll('g.simlin-flow')).find((el) => labelOf(el) === name);
      return g === undefined ? null : flowProbe(g);
    },
    stocks() {
      const out: Record<string, { x: number; y: number; width: number; height: number }> = {};
      for (const g of Array.from(canvas().querySelectorAll('g.simlin-stock'))) {
        const rect = g.querySelector('rect')!;
        out[labelOf(g)] = {
          x: num(rect, 'x'),
          y: num(rect, 'y'),
          width: num(rect, 'width'),
          height: num(rect, 'height'),
        };
      }
      return out;
    },
    clouds() {
      return Array.from(canvas().querySelectorAll('path.simlin-cloud')).map((el) => {
        const transform = el.getAttribute('transform') ?? '';
        const m = /matrix\(([^,]+), 0, 0, ([^,]+), ([^,]+), ([^)]+)\)/.exec(transform);
        if (m === null) {
          throw new Error(`unexpected cloud transform ${transform}`);
        }
        const radius = (Number(m[1]) * cloudWidth) / 2;
        return { transform, center: { x: Number(m[3]) + radius, y: Number(m[4]) + radius } };
      });
    },
    links() {
      return Array.from(canvas().querySelectorAll('path.simlin-connector')).map((el) => el.getAttribute('d') ?? '');
    },
    linkArrowheadCenter() {
      const heads = canvas().querySelectorAll('path.simlin-arrowhead-link');
      if (heads.length !== 1) {
        throw new Error(`expected one link arrowhead, found ${heads.length}`);
      }
      const b = heads[0].getBoundingClientRect();
      return { x: b.left + b.width / 2, y: b.top + b.height / 2 };
    },
    /** What a press at a client point lands on: the hit element's class and its element group's class. */
    hit(x: number, y: number) {
      const el = document.elementFromPoint(x, y);
      return {
        tag: el?.tagName ?? '',
        cls: el?.getAttribute('class') ?? '',
        group: el?.closest('g[class*="simlin-"]')?.getAttribute('class') ?? '',
      };
    },
    settle(): Promise<void> {
      return new Promise((resolve) =>
        requestAnimationFrame(() => requestAnimationFrame(() => setTimeout(() => resolve(), 0))),
      );
    },
  };
  (window as unknown as { __journey: typeof probe }).__journey = probe;
}

export type Probe = {
  toClient(p: XY): XY;
  flows(): Record<string, FlowProbe>;
  flow(name: string): FlowProbe | null;
  stocks(): Record<string, Rect>;
  clouds(): CloudProbe[];
  links(): string[];
  linkArrowheadCenter(): XY;
  hit(x: number, y: number): { tag: string; cls: string; group: string };
  settle(): Promise<void>;
};

export type ProbeWindow = { __journey: Probe } & HarnessWindow;

export const probeFlow = (page: Page, name: string): Promise<FlowProbe | null> =>
  page.evaluate((n) => (window as unknown as ProbeWindow).__journey.flow(n), name);
export const probeStocks = (page: Page): Promise<Record<string, Rect>> =>
  page.evaluate(() => (window as unknown as ProbeWindow).__journey.stocks());
export const probeClouds = (page: Page): Promise<CloudProbe[]> =>
  page.evaluate(() => (window as unknown as ProbeWindow).__journey.clouds());
export const toClient = (page: Page, p: XY): Promise<XY> =>
  page.evaluate((pt) => (window as unknown as ProbeWindow).__journey.toClient(pt), p);
export const hitAt = (page: Page, p: XY): Promise<{ tag: string; cls: string; group: string }> =>
  page.evaluate((pt) => (window as unknown as ProbeWindow).__journey.hit(pt.x, pt.y), p);

/**
 * Two frames and a macrotask, `rounds` times. The controller is timer-free (its
 * executor runs on promise continuations), so an edit a release enqueued has
 * rendered, landed and handed its snapshot to the kernel by then; negative
 * checks ("no snapshot") rest on this.
 */
export async function settle(page: Page, rounds = 3): Promise<void> {
  for (let i = 0; i < rounds; i++) {
    await page.evaluate(() => (window as unknown as ProbeWindow).__journey.settle());
  }
}

export async function shot(page: Page, name: string): Promise<string> {
  const file = path.join(outDir, `${name}.png`);
  await page.locator('#cell1').screenshot({ path: file });
  return file;
}
