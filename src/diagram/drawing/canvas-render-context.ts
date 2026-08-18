// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

/**
 * How the Canvas is being rendered, provided by `Canvas` to the element
 * primitives beneath it (read by `Label`).
 *
 * `embedded` is the export/static arm -- the Canvas mounted with
 * `embedded: true`: `renderSvgToString` and `StaticDiagram`, where the markup
 * is emitted as a standalone SVG string that must stay byte-identical to the
 * Rust renderer (`src/simlin-engine/src/diagram`, pinned by
 * `tests/svg-rendering.test.ts`) and be rasterizable by resvg, which resolves
 * no CSS custom properties -- so colours are literal there -- and also the
 * live, viewport-inert embed the app serves in an iframe (`HostedWebEditor`
 * with `embedded`, the `sd-model` element), which shares the flag and so
 * draws the same literal black-on-white labels; that embed is light-only. The
 * interactive path themes through the `theme.css` tokens instead, so a dark
 * host (`[data-theme="dark"]` on an ancestor) gets light label text and a dark
 * halo like every other canvas primitive.
 *
 * `labelFilterId` is the id of the label-halo `<filter>` this Canvas defined.
 * The interactive halo's flood colour is a token resolved in the FILTER
 * element's own ancestor chain, so two Editors on one page under different
 * themes must not share one filter (`url(#id)` resolves document-wide, to the
 * first match): each interactive Canvas defines its own, unique per instance,
 * and the export path keeps the fixed `labelBackground` the Rust renderer emits.
 */
export interface CanvasRenderContextValue {
  readonly embedded: boolean;
  readonly labelFilterId: string;
}

/** The label-halo filter id of the export path (mirrored by the Rust renderer). */
export const EXPORT_LABEL_FILTER_ID = 'labelBackground';

export const CanvasRenderContext = React.createContext<CanvasRenderContextValue>({
  embedded: false,
  labelFilterId: EXPORT_LABEL_FILTER_ID,
});
