// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Base64 <-> Uint8Array helpers on top of the atob/btoa globals (available
// in every supported browser and in Node since 16). These replace the
// js-base64 package for the two functions we used; toUint8Array keeps its
// tolerance for URL-safe alphabets and stripped padding.

// The tsconfig deliberately includes neither DOM nor Node type libs (this
// package targets both); declare the two host globals we rely on.
declare function atob(data: string): string;
declare function btoa(data: string): string;

/** Encode bytes as standard (RFC 4648) base64. */
export function fromUint8Array(data: Uint8Array): string {
  // String.fromCharCode.apply over the whole array would blow the
  // argument-count limit on large projects, so build the intermediate
  // binary string in bounded chunks.
  const chunkSize = 0x8000;
  let binary = '';
  for (let i = 0; i < data.length; i += chunkSize) {
    binary += String.fromCharCode(...data.subarray(i, i + chunkSize));
  }
  return btoa(binary);
}

/** Decode standard or URL-safe base64 (with or without padding) to bytes. */
export function toUint8Array(b64: string): Uint8Array {
  const normalized = b64.replace(/-/g, '+').replace(/_/g, '/');
  const padded = normalized + '='.repeat((4 - (normalized.length % 4)) % 4);
  const binary = atob(padded);
  const out = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) {
    out[i] = binary.charCodeAt(i);
  }
  return out;
}
