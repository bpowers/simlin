// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { fromUint8Array, toUint8Array } from '../base64';

describe('base64', () => {
  it('encodes bytes to standard base64', () => {
    expect(fromUint8Array(new TextEncoder().encode('hello'))).toBe('aGVsbG8=');
    expect(fromUint8Array(new Uint8Array([]))).toBe('');
    expect(fromUint8Array(new Uint8Array([0xfb, 0xff]))).toBe('+/8=');
  });

  it('decodes standard base64 to bytes', () => {
    expect(new TextDecoder().decode(toUint8Array('aGVsbG8='))).toBe('hello');
    expect(toUint8Array('')).toEqual(new Uint8Array([]));
    expect(toUint8Array('+/8=')).toEqual(new Uint8Array([0xfb, 0xff]));
  });

  it('round-trips every byte value', () => {
    const all = new Uint8Array(256);
    for (let i = 0; i < 256; i++) {
      all[i] = i;
    }
    expect(toUint8Array(fromUint8Array(all))).toEqual(all);
  });

  it('round-trips data large enough to require chunked encoding', () => {
    const big = new Uint8Array(1_000_000);
    for (let i = 0; i < big.length; i++) {
      big[i] = (i * 31) & 0xff;
    }
    expect(toUint8Array(fromUint8Array(big))).toEqual(big);
  });

  it('accepts URL-safe and unpadded input like js-base64 did', () => {
    expect(toUint8Array('-_8')).toEqual(new Uint8Array([0xfb, 0xff]));
    expect(new TextDecoder().decode(toUint8Array('aGVsbG8'))).toBe('hello');
  });
});
