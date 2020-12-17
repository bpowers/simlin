// Copyright 2020 Bobby Powers. All rights reserved.
// Use of this source code is governed by the MIT License
// that can be found in the LICENSE file.

import * as wasm from './xmutil.wasm';

const cachedTextEncoder = new TextEncoder();
const cachedTextDecoder = new TextDecoder();

let cachegetUint8Memory0: Uint8Array | null = null;
function getUint8Memory0() {
  if (cachegetUint8Memory0 === null || cachegetUint8Memory0.buffer !== wasm.memory.buffer) {
    cachegetUint8Memory0 = new Uint8Array(wasm.memory.buffer);
  }
  return cachegetUint8Memory0;
}

function getStringFromWasm(ptr: number) {
  const mem = getUint8Memory0();
  let off = 0;
  while (mem[ptr + off] !== 0) {
    off++;
  }
  return getUint8Memory0().subarray(ptr / 1, ptr / 1 + off);
}

export function convertMdlToXmile(mdlSource: string | Readonly<Uint8Array>, pretty = true): string {
  if (typeof mdlSource === 'string') {
    mdlSource = cachedTextEncoder.encode(mdlSource);
  }

  const mdlSourcePtr = wasm.malloc(mdlSource.length);
  getUint8Memory0()
    .subarray(mdlSourcePtr, mdlSourcePtr + mdlSource.length)
    .set(mdlSource);

  const resultPtr = wasm._convert_mdl_to_xmile(mdlSourcePtr, mdlSource.length, !pretty);
  const resultBuf = getStringFromWasm(resultPtr);
  wasm.free(resultPtr);

  return cachedTextDecoder.decode(resultBuf);
}
