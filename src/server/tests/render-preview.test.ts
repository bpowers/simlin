// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { previewDimensions, parseSvgDimensions } from '../render';

describe('previewDimensions', () => {
  const MAX = 800;

  it('constrains by width for landscape diagrams', () => {
    const dims = previewDimensions(1000, 500, MAX);
    expect(dims.width).toBe(800);
    expect(dims.height).toBe(400);
  });

  it('constrains by height for portrait diagrams', () => {
    const dims = previewDimensions(200, 1000, MAX);
    expect(dims.width).toBe(160);
    expect(dims.height).toBe(800);
  });

  it('constrains by width for square diagrams', () => {
    const dims = previewDimensions(600, 600, MAX);
    expect(dims.width).toBe(800);
    expect(dims.height).toBe(800);
  });

  it('preserves aspect ratio for landscape', () => {
    const dims = previewDimensions(1600, 900, MAX);
    const inputRatio = 1600 / 900;
    const outputRatio = dims.width / dims.height;
    expect(Math.abs(inputRatio - outputRatio)).toBeLessThan(0.02);
  });

  it('preserves aspect ratio for portrait', () => {
    const dims = previewDimensions(300, 1200, MAX);
    const inputRatio = 300 / 1200;
    const outputRatio = dims.width / dims.height;
    expect(Math.abs(inputRatio - outputRatio)).toBeLessThan(0.02);
  });

  it('neither dimension exceeds maxSize for landscape', () => {
    const dims = previewDimensions(2000, 500, MAX);
    expect(dims.width).toBeLessThanOrEqual(MAX);
    expect(dims.height).toBeLessThanOrEqual(MAX);
  });

  it('neither dimension exceeds maxSize for portrait', () => {
    const dims = previewDimensions(100, 2000, MAX);
    expect(dims.width).toBeLessThanOrEqual(MAX);
    expect(dims.height).toBeLessThanOrEqual(MAX);
  });

  it('returns zeros for zero-width input', () => {
    expect(previewDimensions(0, 500, MAX)).toEqual({ width: 0, height: 0 });
  });

  it('returns zeros for zero-height input', () => {
    expect(previewDimensions(500, 0, MAX)).toEqual({ width: 0, height: 0 });
  });

  it('returns zeros for zero maxSize', () => {
    expect(previewDimensions(500, 500, 0)).toEqual({ width: 0, height: 0 });
  });

  it('returns zeros for negative dimensions', () => {
    expect(previewDimensions(-100, 500, MAX)).toEqual({ width: 0, height: 0 });
  });

  it('handles extreme aspect ratios without overflow', () => {
    const tall = previewDimensions(10, 10000, MAX);
    expect(tall.height).toBe(800);
    expect(tall.width).toBeLessThanOrEqual(800);

    const wide = previewDimensions(10000, 10, MAX);
    expect(wide.width).toBe(800);
    expect(wide.height).toBeLessThanOrEqual(800);
  });
});

describe('parseSvgDimensions', () => {
  it('parses standard viewBox', () => {
    const svg = '<svg viewBox="0 0 500 300" xmlns="http://www.w3.org/2000/svg"></svg>';
    expect(parseSvgDimensions(svg)).toEqual({ width: 500, height: 300 });
  });

  it('parses viewBox with negative offsets', () => {
    const svg = '<svg viewBox="-10 -20 400 600" xmlns="http://www.w3.org/2000/svg"></svg>';
    expect(parseSvgDimensions(svg)).toEqual({ width: 400, height: 600 });
  });

  it('returns zeros when viewBox is missing', () => {
    const svg = '<svg xmlns="http://www.w3.org/2000/svg"></svg>';
    expect(parseSvgDimensions(svg)).toEqual({ width: 0, height: 0 });
  });

  it('returns zeros for malformed viewBox', () => {
    const svg = '<svg viewBox="bad data" xmlns="http://www.w3.org/2000/svg"></svg>';
    expect(parseSvgDimensions(svg)).toEqual({ width: 0, height: 0 });
  });

  it('handles extra whitespace in viewBox', () => {
    const svg = '<svg viewBox="  0   0   800   600  " xmlns="http://www.w3.org/2000/svg"></svg>';
    expect(parseSvgDimensions(svg)).toEqual({ width: 800, height: 600 });
  });

  it('handles viewBox with style attribute present', () => {
    const svg =
      '<svg style="width: 500; height: 300;" viewBox="0 0 500 300" xmlns="http://www.w3.org/2000/svg"></svg>';
    expect(parseSvgDimensions(svg)).toEqual({ width: 500, height: 300 });
  });
});
