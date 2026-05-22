// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Unit tests for the pure functions in internal/wasmgen: parseWasmLayout
 * (decode the little-endian WasmLayout wire format) and readStridedSeries
 * (strided f64 read of one variable's series out of a linear-memory buffer).
 *
 * These are functional-core tests: hand-built byte buffers, no WASM instance
 * and no libsimlin. The imperative-shell FFI wrapper (simlin_model_compile_to_wasm)
 * needs a live instance and is covered by the DirectBackend integration tests.
 */

import { parseWasmLayout, readStridedSeries, WasmLayout } from '../src/internal/wasmgen';

const textEncoder = new TextEncoder();

/**
 * Build a serialized WasmLayout buffer in the documented little-endian wire
 * format: u64 nSlots, u64 nChunks, u64 resultsOffset, u32 count, then `count`
 * entries of { u32 nameLen, utf8 name, u64 offset }.
 */
function buildLayoutBytes(options: {
  readonly nSlots: number;
  readonly nChunks: number;
  readonly resultsOffset: number;
  readonly entries: ReadonlyArray<readonly [string, number]>;
}): Uint8Array {
  const { nSlots, nChunks, resultsOffset, entries } = options;

  const encodedNames = entries.map(([name]) => textEncoder.encode(name));
  let total = 8 + 8 + 8 + 4;
  for (const name of encodedNames) {
    total += 4 + name.length + 8;
  }

  const bytes = new Uint8Array(total);
  const view = new DataView(bytes.buffer);
  let p = 0;
  view.setBigUint64(p, BigInt(nSlots), true);
  p += 8;
  view.setBigUint64(p, BigInt(nChunks), true);
  p += 8;
  view.setBigUint64(p, BigInt(resultsOffset), true);
  p += 8;
  view.setUint32(p, entries.length, true);
  p += 4;
  for (let i = 0; i < entries.length; i++) {
    const name = encodedNames[i];
    view.setUint32(p, name.length, true);
    p += 4;
    bytes.set(name, p);
    p += name.length;
    view.setBigUint64(p, BigInt(entries[i][1]), true);
    p += 8;
  }
  return bytes;
}

describe('parseWasmLayout', () => {
  it('decodes geometry and the name->offset map from the wire format', () => {
    const bytes = buildLayoutBytes({
      nSlots: 4,
      nChunks: 11,
      resultsOffset: 64,
      entries: [
        ['time', 0],
        ['population', 2],
      ],
    });

    const layout = parseWasmLayout(bytes);

    expect(layout.nSlots).toBe(4);
    expect(layout.nChunks).toBe(11);
    expect(layout.resultsOffset).toBe(64);
    expect(layout.varOffsets).toBeInstanceOf(Map);
    expect(layout.varOffsets.size).toBe(2);
    expect(layout.varOffsets.get('time')).toBe(0);
    expect(layout.varOffsets.get('population')).toBe(2);
  });

  it('handles an empty variable map (count == 0)', () => {
    const bytes = buildLayoutBytes({ nSlots: 0, nChunks: 0, resultsOffset: 0, entries: [] });

    const layout = parseWasmLayout(bytes);

    expect(layout.nSlots).toBe(0);
    expect(layout.nChunks).toBe(0);
    expect(layout.resultsOffset).toBe(0);
    expect(layout.varOffsets.size).toBe(0);
  });

  it('decodes multi-byte UTF-8 names by byte length, not code-unit length', () => {
    // The middle dot (U+00B7) encodes to two UTF-8 bytes; the wire format's
    // nameLen is a byte count, so an off-by-byte parser would corrupt the map.
    const bytes = buildLayoutBytes({
      nSlots: 2,
      nChunks: 3,
      resultsOffset: 8,
      entries: [['model\u{00B7}variable', 1]],
    });

    const layout = parseWasmLayout(bytes);

    expect(layout.varOffsets.get('model\u{00B7}variable')).toBe(1);
  });

  it('parses offsets that exceed 32 bits via the u64 fields', () => {
    const big = 0x1_0000_0001; // 4294967297, beyond u32 range
    const bytes = buildLayoutBytes({
      nSlots: 1,
      nChunks: 1,
      resultsOffset: big,
      entries: [['x', big]],
    });

    const layout = parseWasmLayout(bytes);

    expect(layout.resultsOffset).toBe(big);
    expect(layout.varOffsets.get('x')).toBe(big);
  });

  it('round-trips the documented wire format against a hand-built buffer', () => {
    const entries: ReadonlyArray<readonly [string, number]> = [
      ['time', 0],
      ['dt', 1],
      ['births', 2],
      ['deaths', 3],
    ];
    const bytes = buildLayoutBytes({ nSlots: 4, nChunks: 7, resultsOffset: 32, entries });

    const layout = parseWasmLayout(bytes);

    expect(layout.nSlots).toBe(4);
    expect(layout.nChunks).toBe(7);
    expect(layout.resultsOffset).toBe(32);
    expect([...layout.varOffsets.entries()]).toEqual(entries.map(([n, o]) => [n, o]));
  });
});

describe('readStridedSeries', () => {
  /**
   * Build a step-major (nChunks x nSlots) f64 results region at a known
   * resultsOffset inside a larger ArrayBuffer, filling cell (chunk, slot)
   * with a deterministic value so a wrong stride is detectable.
   */
  function buildResultsBuffer(options: {
    readonly nSlots: number;
    readonly nChunks: number;
    readonly resultsOffset: number;
    readonly cell: (chunk: number, slot: number) => number;
  }): ArrayBuffer {
    const { nSlots, nChunks, resultsOffset, cell } = options;
    const totalBytes = resultsOffset + nChunks * nSlots * 8;
    const buffer = new ArrayBuffer(totalBytes);
    const view = new DataView(buffer);
    for (let c = 0; c < nChunks; c++) {
      for (let s = 0; s < nSlots; s++) {
        view.setFloat64(resultsOffset + (c * nSlots + s) * 8, cell(c, s), true);
      }
    }
    return buffer;
  }

  function makeLayout(nSlots: number, nChunks: number, resultsOffset: number): WasmLayout {
    return { nSlots, nChunks, resultsOffset, varOffsets: new Map() };
  }

  it('extracts one variable column exactly, striding by nSlots', () => {
    const nSlots = 3;
    const nChunks = 5;
    const resultsOffset = 16;
    // cell value encodes both chunk and slot so a mis-stride is visible.
    const buffer = buildResultsBuffer({
      nSlots,
      nChunks,
      resultsOffset,
      cell: (c, s) => c * 10 + s,
    });
    const layout = makeLayout(nSlots, nChunks, resultsOffset);

    const slot1 = readStridedSeries(buffer, layout, 1);

    expect(Array.from(slot1)).toEqual([1, 11, 21, 31, 41]);
  });

  it('returns a Float64Array of length nChunks', () => {
    const nSlots = 4;
    const nChunks = 9;
    const resultsOffset = 24;
    const buffer = buildResultsBuffer({
      nSlots,
      nChunks,
      resultsOffset,
      cell: (c, s) => c + s,
    });
    const layout = makeLayout(nSlots, nChunks, resultsOffset);

    const series = readStridedSeries(buffer, layout, 0);

    expect(series).toBeInstanceOf(Float64Array);
    expect(series.length).toBe(nChunks);
  });

  it('reads the first and last slots correctly (column boundaries)', () => {
    const nSlots = 3;
    const nChunks = 4;
    const resultsOffset = 0;
    const buffer = buildResultsBuffer({
      nSlots,
      nChunks,
      resultsOffset,
      cell: (c, s) => c * 100 + s,
    });
    const layout = makeLayout(nSlots, nChunks, resultsOffset);

    expect(Array.from(readStridedSeries(buffer, layout, 0))).toEqual([0, 100, 200, 300]);
    expect(Array.from(readStridedSeries(buffer, layout, 2))).toEqual([2, 102, 202, 302]);
  });

  it('honors a nonzero resultsOffset (does not assume base 0)', () => {
    const nSlots = 2;
    const nChunks = 3;
    const resultsOffset = 40;
    const buffer = buildResultsBuffer({
      nSlots,
      nChunks,
      resultsOffset,
      cell: (c, s) => c + s * 1000,
    });
    const layout = makeLayout(nSlots, nChunks, resultsOffset);

    expect(Array.from(readStridedSeries(buffer, layout, 1))).toEqual([1000, 1001, 1002]);
  });

  it('allocates exactly one Float64Array of length nChunks and nothing else', () => {
    const nSlots = 2;
    const nChunks = 6;
    const resultsOffset = 8;
    const buffer = buildResultsBuffer({
      nSlots,
      nChunks,
      resultsOffset,
      cell: (c) => c,
    });
    const layout = makeLayout(nSlots, nChunks, resultsOffset);

    // Spy on the Float64Array constructor to assert a single typed-array
    // allocation of exactly nChunks elements (no intermediate arrays).
    const RealFloat64Array = Float64Array;
    const allocations: Array<number> = [];
    const spy = jest.spyOn(global, 'Float64Array').mockImplementation(function (this: unknown, arg: number) {
      allocations.push(arg);
      return new RealFloat64Array(arg);
    } as unknown as typeof Float64Array);

    try {
      const series = readStridedSeries(buffer, layout, 0);
      expect(allocations).toEqual([nChunks]);
      expect(Array.from(series)).toEqual([0, 1, 2, 3, 4, 5]);
    } finally {
      spy.mockRestore();
    }
  });
});
