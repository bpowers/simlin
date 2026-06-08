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

import type { Point } from './common';
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

// MIN_ZOOM matches the 0.2 floor used in the render transform (which clamps
// zoom < 0.2 to 1.0); keeping the state floor and the render floor identical
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
