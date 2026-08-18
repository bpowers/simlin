// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Pure viewport math for the canvas (the functional core extracted from
 * `Canvas.tsx`). Every function here is a pure transform over plain numbers:
 * given an already-resolved canvas-space point and the current viewport, it
 * returns the next offset/zoom. The DOM-bound parts -- mapping a screen
 * (clientX/Y) point into canvas space via `getBoundingClientRect` +
 * `screenToCanvasPoint`, the rAF loop, the debounce timer, and React state --
 * stay in the Canvas shell, which resolves screen->canvas and then calls these.
 *
 * Keeping the arithmetic here makes the pan/zoom/pinch/momentum behavior unit
 * testable without jsdom and keeps the shell focused on wiring and lifecycle.
 */

import type { Point, Rect } from './common';
import type { Rect as ViewRect } from '@simlin/core/datamodel';

// --- physics / interaction constants -------------------------------------

// Momentum scrolling physics for macOS-native feel. macOS apps (Finder,
// Safari, Maps) have snappier deceleration than iOS. A friction coefficient of
// 0.05 means velocity retains 5% after 1 second, giving a ~0.5-0.8s coast.
export const FRICTION_COEFFICIENT = 0.05;
export const FRICTION_LOG = Math.log(FRICTION_COEFFICIENT); // ~= -3.0

// Stop momentum when velocity drops below this threshold. At 60fps, 15 px/s =
// 0.25 px/frame -- imperceptible motion. Lower values make the stop feel more
// gradual and natural.
export const VELOCITY_THRESHOLD = 15;

// Pinch/wheel zoom uses exponential scaling for a natural feel. A divisor of
// 100 means a cumulative deltaY of ~100 results in a 2x zoom change, matching
// native macOS apps like Maps and Preview.
export const PINCH_ZOOM_DIVISOR = 100;

// The zoom range gestures may produce. The Canvas also uses these bounds (via
// isRenderableZoom) to decide whether a STORED view zoom is usable at all:
// keeping the gesture clamps and the render-time validity check identical
// avoids a mismatch between stored view state and what is actually drawn.
export const MIN_ZOOM = 0.2;
export const MAX_ZOOM = 5.0;

// A wheel-zoom step below this delta is treated as a no-op so floating-point
// noise at the zoom clamps doesn't churn the viewport.
const ZOOM_EPSILON = 0.0001;

/** A timestamped pointer sample used for momentum velocity estimation. */
export interface VelocitySample {
  x: number;
  y: number;
  timestamp: number;
}

/** Clamp a zoom value into the supported [MIN_ZOOM, MAX_ZOOM] range. */
export function clampZoom(zoom: number): number {
  return Math.max(MIN_ZOOM, Math.min(MAX_ZOOM, zoom));
}

/**
 * Whether a stored view zoom is one the Canvas can draw as-is: finite and
 * within [MIN_ZOOM, MAX_ZOOM]. Gestures can never produce anything else, so a
 * value outside this range comes from data -- an unset/zero zoom, or a file
 * whose zoom was recorded in the wrong unit (e.g. an XMILE percentage such as
 * 200 stored where a factor of 2 belongs). The Canvas treats such a value as
 * "no usable zoom" and falls back to 1.0 rather than clamping: a 200x (or 0x)
 * request carries no information about the intended scale, and rendering at
 * the clamp edge would still hand the user an unreadable canvas.
 */
export function isRenderableZoom(zoom: number): boolean {
  return isFinite(zoom) && zoom >= MIN_ZOOM && zoom <= MAX_ZOOM;
}

// --- wheel pan -----------------------------------------------------------

/**
 * The new canvas offset after a wheel/trackpad pan. `delta.mode` is the native
 * `WheelEvent.deltaMode` (0 = pixels, 1 = lines, 2 = pages); line and page
 * deltas are resolved to pixels (pages use the live viewport size, which the
 * shell measures from the DOM since the stored size may be stale mid-resize).
 * The delta is divided by `zoom` because a higher zoom means a smaller visible
 * model area, so a given screen delta covers fewer model units. Dragging the
 * surface down/right moves the content the same way, hence the offset moves
 * opposite the wheel delta.
 */
export function wheelPanOffset(
  base: Point,
  delta: { x: number; y: number; mode: number },
  zoom: number,
  viewportPx: { width: number; height: number },
): Point {
  let deltaX = delta.x;
  let deltaY = delta.y;

  if (delta.mode === 1) {
    // Lines -- multiply by an approximate line height.
    deltaX *= 16;
    deltaY *= 16;
  } else if (delta.mode === 2) {
    // Pages -- one notch scrolls a full viewport.
    deltaX *= viewportPx.width;
    deltaY *= viewportPx.height;
  }

  deltaX /= zoom;
  deltaY /= zoom;

  return {
    x: base.x - deltaX,
    y: base.y - deltaY,
  };
}

// --- wheel / pinch zoom --------------------------------------------------

/**
 * Exponential wheel zoom: a `deltaY` of `PINCH_ZOOM_DIVISOR` halves/doubles the
 * zoom, so zooming in then out by equal deltas returns to the original level.
 * Negative `deltaY` (pinch out) zooms in. The result is clamped; `changed` is
 * false when the clamped delta is within `ZOOM_EPSILON` so the caller can skip a
 * no-op update at the zoom limits.
 */
export function wheelZoom(currentZoom: number, deltaY: number): { zoom: number; changed: boolean } {
  const scale = Math.pow(2, -deltaY / PINCH_ZOOM_DIVISOR);
  const zoom = clampZoom(currentZoom * scale);
  return { zoom, changed: Math.abs(zoom - currentZoom) >= ZOOM_EPSILON };
}

/**
 * The offset that keeps a fixed model point under the cursor across a zoom
 * change. `cursorCanvasOld`/`cursorCanvasNew` are the same screen position
 * mapped into canvas space at the old and new zoom respectively (the shell does
 * those DOM-bound conversions). The model point under the cursor is
 * `cursorCanvasOld - oldOffset`; after zooming we re-anchor that same model
 * point under the (re-measured) cursor.
 */
export function zoomAroundPoint(oldOffset: Point, cursorCanvasOld: Point, cursorCanvasNew: Point): Point {
  const modelX = cursorCanvasOld.x - oldOffset.x;
  const modelY = cursorCanvasOld.y - oldOffset.y;
  return {
    x: cursorCanvasNew.x - modelX,
    y: cursorCanvasNew.y - modelY,
  };
}

/** Pinch zoom: scale the starting zoom by the finger-distance ratio, clamped. */
export function pinchZoom(initialZoom: number, scale: number): number {
  return clampZoom(initialZoom * scale);
}

/**
 * The offset that keeps `modelPoint` (the model point under the fingers when the
 * pinch began) under the current pinch center. `centerCanvasNew` is the pinch
 * center mapped into canvas space at the new zoom (resolved by the shell).
 */
export function pinchOffset(centerCanvasNew: Point, modelPoint: Point): Point {
  return {
    x: centerCanvasNew.x - modelPoint.x,
    y: centerCanvasNew.y - modelPoint.y,
  };
}

// --- momentum ------------------------------------------------------------

/**
 * Flutter-style friction simulation: displacement at time `t` for an initial
 * velocity `v0`. `x(t) - x0 = v0 * (friction^t - 1) / ln(friction)`.
 */
export function frictionPosition(velocity: number, time: number): number {
  return (velocity * (Math.pow(FRICTION_COEFFICIENT, time) - 1)) / FRICTION_LOG;
}

/** Velocity at time `t`: `v(t) = v0 * friction^t`. */
export function frictionVelocity(velocity: number, time: number): number {
  return velocity * Math.pow(FRICTION_COEFFICIENT, time);
}

/** The momentum-decayed offset at `elapsedSec` after release. */
export function momentumOffsetAt(startOffset: Point, v0: Point, elapsedSec: number): Point {
  return {
    x: startOffset.x + frictionPosition(v0.x, elapsedSec),
    y: startOffset.y + frictionPosition(v0.y, elapsedSec),
  };
}

/** True once the decayed momentum speed has dropped below `VELOCITY_THRESHOLD`. */
export function isMomentumDone(v0: Point, elapsedSec: number): boolean {
  const vx = frictionVelocity(v0.x, elapsedSec);
  const vy = frictionVelocity(v0.y, elapsedSec);
  return Math.hypot(vx, vy) < VELOCITY_THRESHOLD;
}

/**
 * Estimate release velocity (px/s) from recent pointer samples. Returns zero --
 * an intentional stop, no momentum -- when there are too few samples or the
 * pointer was stationary for >40ms before release (~2.5 frames at 60fps, enough
 * to distinguish a deliberate stop from a quick flick-and-release). Otherwise
 * averages over the last 100ms of samples, falling back to the final two.
 */
export function calculateVelocity(positions: readonly VelocitySample[], now: number): Point {
  if (positions.length < 2) {
    return { x: 0, y: 0 };
  }

  const lastPosition = positions[positions.length - 1];
  if (now - lastPosition.timestamp > 40) {
    return { x: 0, y: 0 };
  }

  const recentPositions = positions.filter((p) => now - p.timestamp < 100);

  if (recentPositions.length < 2) {
    const lastP = positions[positions.length - 1];
    const prev = positions[positions.length - 2];
    const dt = (lastP.timestamp - prev.timestamp) / 1000;
    if (dt <= 0) {
      return { x: 0, y: 0 };
    }
    return {
      x: (lastP.x - prev.x) / dt,
      y: (lastP.y - prev.y) / dt,
    };
  }

  const firstP = recentPositions[0];
  const lastP = recentPositions[recentPositions.length - 1];
  const dt = (lastP.timestamp - firstP.timestamp) / 1000;
  if (dt <= 0) {
    return { x: 0, y: 0 };
  }
  return {
    x: (lastP.x - firstP.x) / dt,
    y: (lastP.y - firstP.y) / dt,
  };
}

// --- resize --------------------------------------------------------------

/**
 * The viewBox after the canvas element resizes by (`dWidth`, `dHeight`) to the
 * new (`width`, `height`). The offset shifts by a quarter of the delta so the
 * content stays roughly centered as the surface grows/shrinks.
 */
export function resizeViewBox(offset: Point, dWidth: number, dHeight: number, width: number, height: number): ViewRect {
  return {
    x: offset.x + dWidth / 4,
    y: offset.y + dHeight / 4,
    width,
    height,
  };
}

// --- offscreen detection / recenter (issue #52) --------------------------

// A diagram whose visible fraction falls below this on mount is treated as
// "offscreen(ish)" and re-centered, so a saved viewBox/zoom that stranded the
// model outside the canvas doesn't greet the user with a blank surface.
export const OFFSCREEN_VISIBLE_FRACTION = 0.1;

/**
 * The fraction (0..1) of the diagram's bounding box that is currently visible
 * on the canvas, given the model-space `bounds`, the current `offset`/`zoom`,
 * and the measured `svgSize`. The render transform maps a model point (mx,my)
 * to screen ((mx+offset.x)*zoom, (my+offset.y)*zoom) and the visible region is
 * [0,width] x [0,height], so this intersects the box's screen rect with the
 * viewport. It is intentionally computed in screen space (not model space): the
 * viewport is a fixed pixel rectangle, so how much of the box is clipped depends
 * on zoom -- a model box occupies more of the screen when zoomed in and is more
 * likely to fall (partly) out of view.
 *
 * The intersection is normalized by the SMALLER of the box's screen area and
 * the viewport area, not by the box area alone. That distinction is the whole
 * point of the metric: the question is "does the user see enough diagram?", not
 * "what fraction of the diagram is on screen?". For a box smaller than the
 * viewport the two coincide (min = box area -> "fraction of the diagram
 * visible"). For a box LARGER than the viewport, box-area normalization would
 * report a large-but-well-framed model as mostly hidden -- a 3500x3000 model
 * perfectly centered in a 1200x800 viewport shows the entire viewport full of
 * diagram yet only ~9% of its own bbox, and would be needlessly yanked to
 * bbox-center on every open. Normalizing by the viewport area instead makes a
 * viewport full of diagram read 1.0 regardless of bbox size (min = viewport
 * area -> "fraction of the viewport filled by diagram").
 *
 * A degenerate box (zero or negative area -- e.g. an empty model) has no
 * meaningful "visible fraction", so this returns 1 (fully visible) to signal
 * "nothing to re-center"; `isDiagramOffscreen` relies on that to never trigger
 * on empty models.
 */
export function visibleDiagramFraction(
  bounds: Rect,
  offset: Point,
  zoom: number,
  svgSize: { width: number; height: number },
): number {
  const boxWidth = bounds.right - bounds.left;
  const boxHeight = bounds.bottom - bounds.top;
  if (boxWidth <= 0 || boxHeight <= 0) {
    return 1;
  }

  const screenLeft = (bounds.left + offset.x) * zoom;
  const screenTop = (bounds.top + offset.y) * zoom;
  const screenRight = (bounds.right + offset.x) * zoom;
  const screenBottom = (bounds.bottom + offset.y) * zoom;

  const interWidth = Math.min(screenRight, svgSize.width) - Math.max(screenLeft, 0);
  const interHeight = Math.min(screenBottom, svgSize.height) - Math.max(screenTop, 0);
  if (interWidth <= 0 || interHeight <= 0) {
    return 0;
  }

  const boxScreenArea = (screenRight - screenLeft) * (screenBottom - screenTop);
  const viewportArea = svgSize.width * svgSize.height;
  return (interWidth * interHeight) / Math.min(boxScreenArea, viewportArea);
}

/**
 * True when the diagram is (mostly) offscreen and should be re-centered:
 * either it does not intersect the viewport at all or its visible fraction is
 * below `threshold`. Empty models (zero-area bounds) and an unmeasured canvas
 * (zero-size viewport) never qualify -- there is nothing to center against.
 */
export function isDiagramOffscreen(
  bounds: Rect,
  offset: Point,
  zoom: number,
  svgSize: { width: number; height: number },
  threshold: number = OFFSCREEN_VISIBLE_FRACTION,
): boolean {
  const boxWidth = bounds.right - bounds.left;
  const boxHeight = bounds.bottom - bounds.top;
  if (boxWidth <= 0 || boxHeight <= 0) {
    return false;
  }
  if (svgSize.width <= 0 || svgSize.height <= 0) {
    return false;
  }
  return visibleDiagramFraction(bounds, offset, zoom, svgSize) < threshold;
}

/**
 * The offset that centers the diagram bounding box in the canvas at the CURRENT
 * zoom (zoom is intentionally left unchanged -- centering keeps the scope
 * tight). Solves (boxCenter + offset) * zoom = svgSize / 2 for `offset`, so the
 * box center lands at the middle of the viewport.
 */
export function centerOffsetForBounds(bounds: Rect, zoom: number, svgSize: { width: number; height: number }): Point {
  const centerX = (bounds.left + bounds.right) / 2;
  const centerY = (bounds.top + bounds.bottom) / 2;
  return {
    x: svgSize.width / (2 * zoom) - centerX,
    y: svgSize.height / (2 * zoom) - centerY,
  };
}
