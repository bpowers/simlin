// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Canvas labels theme with the other canvas primitives in the INTERACTIVE
// canvas, and stay literal in the EXPORT canvas (drawing/canvas-render-context.ts).
//
// The two arms of that decision are pinned side by side because they pull in
// opposite directions: the export path's markup is byte-compared with the Rust
// renderer (tests/svg-rendering.test.ts) and rasterized by resvg, which
// resolves no CSS custom property -- so its label fill and halo colour must be
// literal; the interactive path must NOT pin the fill inline, since an inline
// value beats every stylesheet rule and the fill there is what the stylesheets
// theme (`.canvas text { fill: var(--color-black) }`) and recolour for a
// selected element (`.aux.selected text { fill: var(--color-selected) }`).
// This test compiles those stylesheets into jsdom and reads computed values
// (declared, unresolved var() -- jsdom resolves the cascade, not tokens); the
// dark-mode pixel claim is the notebook widget's Playwright journey.

import { describe, it, expect, beforeAll, afterEach } from '@rstest/core';

import * as fs from 'fs';
import * as path from 'path';

import { renderCanvas, makeAux } from './canvas-gesture-harness';
import { EXPORT_LABEL_FILTER_ID } from '../drawing/canvas-render-context';

const drawingDir = path.join(__dirname, '..', 'drawing');

describe('Canvas labels: theming by render mode', () => {
  beforeAll(() => {
    // The two stylesheets whose rules the interactive fill depends on. Class
    // names equal their local names in tests (rstest.config.mts, localIdentName
    // '[local]'), so the raw text applies to the rendered markup.
    const style = document.createElement('style');
    style.setAttribute('data-test', 'canvas-label-theme');
    style.textContent = ['Canvas.module.css', 'Auxiliary.module.css']
      .map((name) => fs.readFileSync(path.join(drawingDir, name), 'utf-8'))
      .join('\n');
    document.head.appendChild(style);
  });

  afterEach(() => {
    document.body.innerHTML = '';
  });

  it('interactive: the label declares no inline fill, so the stylesheets theme it and recolour it when selected', () => {
    const h = renderCanvas({ elements: [makeAux(1, 'a', 100, 100)] });
    const text = h.query('g.aux text') as SVGTextElement;
    expect(text).not.toBeNull();
    // No inline fill: the token-based stylesheet rule is what applies...
    expect(text.style.fill).toBe('');
    expect(window.getComputedStyle(text).fill).toBe('var(--color-black)');
    // ...and the element's selected rule can override it (an inline fill would
    // have beaten it, which is how selected labels once stayed black).
    h.setProps({ selection: new Set([1]) });
    expect(window.getComputedStyle(h.query('g.aux.selected text')!).fill).toBe('var(--color-selected)');
    // The font pins the export path needs stay inline in both modes.
    expect(text.style.fontSize).toBe('12px');
    expect(text.style.fontWeight).toBe('300');
    h.unmount();
  });

  it('interactive: the halo is a token-coloured flood behind the label, in a filter unique to this canvas', () => {
    const h = renderCanvas({ elements: [makeAux(1, 'a', 100, 100)] });
    const filters = h.queryAll('defs > filter');
    expect(filters).toHaveLength(1);
    const filter = filters[0];
    const id = filter.getAttribute('id');
    expect(id).not.toBeNull();
    expect(id).not.toBe(EXPORT_LABEL_FILTER_ID);
    // A plain identifier, usable unquoted inside url(#...), drawn per mount
    // (not React.useId, which repeats across roots -- one per notebook cell).
    expect(id).toMatch(/^label-halo-[a-z0-9]+$/);
    // The plate is a flood whose colour is the canvas-primitive fill token, at
    // the same 0.85 opacity the export matrix applies; no literal colour.
    const flood = filter.querySelector('feFlood') as SVGElement | null;
    expect(flood).not.toBeNull();
    expect(flood!.style.floodColor).toBe('var(--color-white)');
    expect(flood!.getAttribute('flood-opacity')).toBe('0.85');
    expect(filter.querySelector('feColorMatrix')).toBeNull();
    // Dilate + blur feed the plate; the flood is clipped to it (`in`) and the
    // glyphs go over the result.
    expect(filter.querySelector('feMorphology')?.getAttribute('operator')).toBe('dilate');
    expect(filter.querySelector('feGaussianBlur')?.getAttribute('result')).toBe('plate');
    const composites = Array.from(filter.querySelectorAll('feComposite'));
    expect(composites.map((c) => c.getAttribute('operator'))).toEqual(['in', 'over']);
    expect(composites[0].getAttribute('in2')).toBe('plate');
    expect(composites[1].getAttribute('in')).toBe('SourceGraphic');
    // Every label references THIS canvas's filter.
    const text = h.query('g.aux text') as SVGTextElement;
    expect(text.style.filter).toMatch(new RegExp(`^url\\("?#${id}"?\\)$`));

    // A second interactive canvas on the same page defines its own filter: the
    // flood colour resolves in the filter's ancestor chain, so two canvases
    // under different themes must never share one.
    const other = renderCanvas({ elements: [makeAux(1, 'a', 100, 100)] });
    const otherId = other.query('defs > filter')!.getAttribute('id');
    expect(otherId).toMatch(/^label-halo-[a-z0-9]+$/);
    expect(otherId).not.toBe(id);
    expect((other.query('g.aux text') as SVGTextElement).style.filter).toMatch(
      new RegExp(`^url\\("?#${otherId}"?\\)$`),
    );
    other.unmount();
    h.unmount();
  });

  it('export (embedded): literal black fill, the fixed labelBackground id, and the white colour-matrix plate', () => {
    const h = renderCanvas({ elements: [makeAux(1, 'a', 100, 100)], embedded: true });
    const filters = h.queryAll('defs > filter');
    expect(filters).toHaveLength(1);
    expect(filters[0].getAttribute('id')).toBe(EXPORT_LABEL_FILTER_ID);
    expect(filters[0].querySelector('feColorMatrix')).not.toBeNull();
    expect(filters[0].querySelector('feFlood')).toBeNull();
    const text = h.query('g.aux text') as SVGTextElement;
    // jsdom's CSSOM normalizes the hex to rgb() on read.
    expect(text.style.fill).toBe('rgb(0, 0, 0)');
    expect(text.style.filter).toMatch(new RegExp(`^url\\("?#${EXPORT_LABEL_FILTER_ID}"?\\)$`));
    h.unmount();
  });
});
