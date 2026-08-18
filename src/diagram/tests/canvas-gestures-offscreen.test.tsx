// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Shell wiring for the mount-time offscreen re-center (issue #52). A saved
// viewBox/zoom can strand the whole diagram outside the visible canvas; once
// the Canvas knows both the element bounds and the real svg size, it should
// center the diagram exactly once. These tests drive a real <Canvas> through
// the gesture harness and assert on the onViewBoxChange callback, never on
// Canvas internals. The pure predicate + centering math are covered in
// viewport.test.ts.

import { describe, it, expect } from '@rstest/core';
import type { Mock } from '@rstest/core';

import { renderCanvas, makeAux, makeStock } from './canvas-gesture-harness';

// The harness mounts with a jsdom canvas whose clientWidth/Height is 0, so the
// first real size arrives via a synthesized ResizeObserver delivery (resize()).
// A resize from a known old size also runs the pre-existing quarter-delta
// resize recenter (handleSvgResize), so onViewBoxChange can fire once for that
// BEFORE our offscreen effect fires; our commit is always the LAST call.
function lastViewBoxCall(onViewBoxChange: Mock): {
  viewBox: { x: number; y: number; width: number; height: number };
  zoom: number;
} {
  const calls = onViewBoxChange.mock.calls;
  const [viewBox, zoom] = calls[calls.length - 1];
  return { viewBox, zoom };
}

describe('mount offscreen re-center (issue #52)', () => {
  it('centers a diagram whose saved viewBox leaves it (mostly) offscreen', () => {
    // Elements clustered near the model origin.
    const elements = [makeAux(1, 'a', 0, 0), makeStock(2, 'b', 120, 60)];
    const h = renderCanvas({ elements });

    // Model loads with a viewBox panned far away: the diagram is entirely
    // outside the visible canvas.
    h.setViewport({ x: -5000, y: -5000 });
    h.clearMountCalls();

    // The real size arrives -> the offscreen effect gets its one measured,
    // idle, non-embedded shot and re-centers.
    h.resize(1000, 1000);

    expect(h.callbacks.onViewBoxChange).toHaveBeenCalled();
    const { viewBox, zoom } = lastViewBoxCall(h.callbacks.onViewBoxChange);

    // Zoom is left unchanged; the viewBox adopts the measured size.
    expect(zoom).toBe(1);
    expect(viewBox.width).toBe(1000);
    expect(viewBox.height).toBe(1000);

    // Every element now maps into the visible [0,1000] x [0,1000] region.
    for (const el of elements) {
      const screenX = (el.x + viewBox.x) * zoom;
      const screenY = (el.y + viewBox.y) * zoom;
      expect(screenX).toBeGreaterThanOrEqual(0);
      expect(screenX).toBeLessThanOrEqual(1000);
      expect(screenY).toBeGreaterThanOrEqual(0);
      expect(screenY).toBeLessThanOrEqual(1000);
    }

    // The diagram's bounding-box center lands near the viewport center. The
    // exact center depends on element radii/label extent (auxBounds/stockBounds
    // pad past the raw x/y), so allow a modest tolerance around the element-
    // position midpoint rather than pinning the padded-bbox center exactly.
    const cx = (0 + 120) / 2;
    const cy = (0 + 60) / 2;
    expect(Math.abs((cx + viewBox.x) * zoom - 500)).toBeLessThan(40);
    expect(Math.abs((cy + viewBox.y) * zoom - 500)).toBeLessThan(40);
  });

  it('does not re-center a diagram that mounts (mostly) visible', () => {
    const elements = [makeAux(1, 'a', 400, 400), makeStock(2, 'b', 520, 460)];
    const h = renderCanvas({ elements });

    // Default viewBox offset (0,0): the elements near (400..520) are well inside
    // a 1000x1000 canvas, so the offscreen check must NOT fire.
    h.clearMountCalls();
    h.resize(1000, 1000);

    // The ONLY onViewBoxChange here is the pre-existing quarter-delta resize
    // recenter (handleSvgResize): from offset (0,0) with a +1000/+1000 size delta
    // it shifts by a quarter -> (250,250). Our offscreen effect adds nothing, so
    // the single commit is exactly that resize value, not a centering update.
    expect(h.callbacks.onViewBoxChange).toHaveBeenCalledTimes(1);
    const { viewBox, zoom } = lastViewBoxCall(h.callbacks.onViewBoxChange);
    expect(viewBox).toEqual({ x: 250, y: 250, width: 1000, height: 1000 });
    expect(zoom).toBe(1);
    // Cross-check: a centering commit would have placed the bbox center at
    // screen 500; the resize-only offset leaves it far from center.
    const cx = (400 + 520) / 2;
    expect(Math.abs(cx + viewBox.x - 500)).toBeGreaterThan(50);
  });

  it('is skipped when the host carried the viewport in (recenterOffscreenOnMount=false), and runs otherwise', () => {
    // A remounting host (the notebook widget on a kernel push) hands the new
    // Editor the user's live framing; a diagram the user panned offscreen must
    // stay where they put it. Same offscreen fixture as the first test, with
    // the opt-out: no centering commit, only the resize handler's quarter shift
    // from the pushed offset.
    const elements = [makeAux(1, 'a', 0, 0), makeStock(2, 'b', 120, 60)];
    const carried = renderCanvas({ elements, recenterOffscreenOnMount: false });
    carried.setViewport({ x: -5000, y: -5000 });
    carried.clearMountCalls();
    carried.resize(1000, 1000);
    for (const [viewBox] of carried.callbacks.onViewBoxChange.mock.calls) {
      // Every commit leaves the diagram far offscreen: never a re-center.
      expect(Math.abs(60 + viewBox.x - 500)).toBeGreaterThan(1000);
    }
    carried.unmount();

    // Control: the identical mount with the flag explicitly true re-centers,
    // which is what proves the skip above came from the flag.
    const data = renderCanvas({ elements, recenterOffscreenOnMount: true });
    data.setViewport({ x: -5000, y: -5000 });
    data.clearMountCalls();
    data.resize(1000, 1000);
    const { viewBox } = lastViewBoxCall(data.callbacks.onViewBoxChange);
    expect(Math.abs(60 + viewBox.x - 500)).toBeLessThan(40);
    data.unmount();
  });

  it('runs at most once per mount: a later external offscreen pan is not auto-centered', () => {
    const elements = [makeAux(1, 'a', 400, 400)];
    const h = renderCanvas({ elements });

    // First measured shot happens while visible -> latch consumed, no center.
    h.clearMountCalls();
    h.resize(1000, 1000);
    h.callbacks.onViewBoxChange.mockClear();

    // A later external viewport change strands the diagram offscreen. The
    // mount-only check has already run, so nothing auto-centers it.
    h.setViewport({ x: -5000, y: -5000 });
    // Nudge svgSize again to give any (incorrectly re-armed) effect a chance.
    h.resize(1000, 1000);

    // Any onViewBoxChange here is at most the resize handler's quarter shift,
    // never a re-center that brings the element to screen 500.
    for (const [viewBox] of h.callbacks.onViewBoxChange.mock.calls) {
      const screenX = 400 + viewBox.x;
      expect(Math.abs(screenX - 500)).toBeGreaterThan(50);
    }
  });

  it('ignores an empty model (no centering commit)', () => {
    const h = renderCanvas({ elements: [] });
    h.clearMountCalls();
    h.resize(1000, 1000);
    // No elements -> calcViewBox is undefined -> the offscreen effect returns
    // without committing. The ONLY onViewBoxChange is the pre-existing quarter-
    // delta resize handler (from offset (0,0) -> (250,250)); the offscreen effect
    // adds no second call. If it had (wrongly) committed a center, this would be
    // two calls. That single resize-only commit is the direct proof no centering
    // occurred.
    expect(h.callbacks.onViewBoxChange).toHaveBeenCalledTimes(1);
    const { viewBox } = lastViewBoxCall(h.callbacks.onViewBoxChange);
    expect(viewBox).toEqual({ x: 250, y: 250, width: 1000, height: 1000 });
  });

  it('does nothing in embedded mode (viewport-inert)', () => {
    const elements = [makeAux(1, 'a', 0, 0)];
    const h = renderCanvas({ elements, embedded: true });
    h.clearMountCalls();
    h.resize(1000, 1000);
    // Embedded canvases draw to tight element bounds and ignore viewBox/zoom;
    // the offscreen effect early-returns, so it never notifies the host.
    expect(h.callbacks.onViewBoxChange).not.toHaveBeenCalled();
  });
});
