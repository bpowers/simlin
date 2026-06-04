// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Pure geometry helpers used by Flow.render():
//
//  - retractFinalPointIntoCloud pulls a cloud-terminated flow's endpoint
//    back by CloudRadius so the arrowhead lands on the cloud's edge. The
//    old inline version applied independent x and y shifts, so a diagonal
//    final segment was double-shifted by sqrt(2)*CloudRadius.
//  - finalSegmentAngle yields the direction of the last non-degenerate
//    segment; atan2(0, 0) === 0 used to force a zero-length final segment's
//    arrowhead to point right regardless of the flow's real orientation.

import { finalSegmentAngle, retractFinalPointIntoCloud } from '../drawing/Flow';
import { CloudRadius } from '../drawing/default';

type Pt = { x: number; y: number; attachedToUid: number | undefined };
const pt = (x: number, y: number): Pt => ({ x, y, attachedToUid: undefined });

describe('retractFinalPointIntoCloud', () => {
  it('retracts a horizontal final segment by exactly CloudRadius in x', () => {
    const pts = retractFinalPointIntoCloud([pt(0, 0), pt(100, 0)], CloudRadius);
    expect(pts[1].x).toBeCloseTo(100 - CloudRadius);
    expect(pts[1].y).toBeCloseTo(0);
  });

  it('retracts a vertical final segment by exactly CloudRadius in y', () => {
    const pts = retractFinalPointIntoCloud([pt(0, 0), pt(0, -100)], CloudRadius);
    expect(pts[1].x).toBeCloseTo(0);
    expect(pts[1].y).toBeCloseTo(-100 + CloudRadius);
  });

  it('retracts a diagonal final segment by CloudRadius along the segment, not per axis', () => {
    const pts = retractFinalPointIntoCloud([pt(0, 0), pt(100, 100)], CloudRadius);
    const dx = 100 - pts[1].x;
    const dy = 100 - pts[1].y;
    // The pullback distance must be CloudRadius, not sqrt(2)*CloudRadius.
    expect(Math.sqrt(dx * dx + dy * dy)).toBeCloseTo(CloudRadius);
    // ...and along the segment direction (45 degrees).
    expect(dx).toBeCloseTo(dy);
  });

  it('leaves a zero-length final segment unchanged', () => {
    const pts = retractFinalPointIntoCloud([pt(50, 50), pt(50, 50)], CloudRadius);
    expect(pts[1]).toEqual(pt(50, 50));
  });

  it('only moves the final point', () => {
    const original = [pt(0, 0), pt(50, 0), pt(100, 0)];
    const pts = retractFinalPointIntoCloud(original, CloudRadius);
    expect(pts[0]).toEqual(original[0]);
    expect(pts[1]).toEqual(original[1]);
  });
});

describe('finalSegmentAngle', () => {
  it('returns the angle of the final segment in degrees [0, 360)', () => {
    expect(finalSegmentAngle([pt(0, 0), pt(100, 0)])).toBeCloseTo(0);
    expect(finalSegmentAngle([pt(0, 0), pt(0, 100)])).toBeCloseTo(90);
    expect(finalSegmentAngle([pt(0, 0), pt(-100, 0)])).toBeCloseTo(180);
    expect(finalSegmentAngle([pt(0, 0), pt(0, -100)])).toBeCloseTo(270);
  });

  it('walks back past coincident trailing points to find the real direction', () => {
    // The final two points coincide; the direction comes from the last
    // non-degenerate segment (pointing up: 270 in SVG coordinates).
    expect(finalSegmentAngle([pt(0, 100), pt(0, 0), pt(0, 0)])).toBeCloseTo(270);
  });

  it('returns undefined when every point coincides', () => {
    expect(finalSegmentAngle([pt(5, 5), pt(5, 5), pt(5, 5)])).toBeUndefined();
  });
});
