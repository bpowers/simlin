// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { promises as fs } from 'fs';
import { join } from 'path';

export function defined<T>(object: T | undefined): T {
  if (object === undefined) {
    throw new Error('expected non-undefined object');
  }
  return object;
}

let cachedWasmModule: typeof import('./xmutil.wasm') | undefined;
function getWasmModule(): Promise<typeof import('./xmutil.wasm')> {
  return new Promise((resolve, reject) => {
    if (cachedWasmModule) {
      resolve(cachedWasmModule);
      return;
    }

    fs.readFile(join(__dirname, 'xmutil.wasm'))
      .then((contents) => {
        WebAssembly.instantiate(contents)
          .then((source) => {
            cachedWasmModule = source.instance.exports as unknown as typeof import('./xmutil.wasm');
            resolve(cachedWasmModule);
          })
          .catch(reject);
      })
      .catch(reject);
  });
}

const cachedTextEncoder = new TextEncoder();
const cachedTextDecoder = new TextDecoder();

let cachegetUint8Memory0: Uint8Array | null = null;
function getUint8Memory0() {
  const wasm = defined(cachedWasmModule);
  if (cachegetUint8Memory0 === null || cachegetUint8Memory0.buffer !== wasm.memory.buffer) {
    cachegetUint8Memory0 = new Uint8Array(wasm.memory.buffer);
  }
  return cachegetUint8Memory0;
}

function getStringFromWasm(ptr: number): string {
  if (ptr === 0) {
    return "";
  }
  const mem = getUint8Memory0();
  let off = 0;
  while (mem[ptr + off] !== 0) {
    off++;
  }
  return cachedTextDecoder.decode(getUint8Memory0().subarray(ptr, ptr + off));
}

export async function convertMdlToXmile(mdlSource: string | Readonly<Uint8Array>, pretty = true): Promise<[string, string?]> {
  if (typeof mdlSource === 'string') {
    mdlSource = cachedTextEncoder.encode(mdlSource);
  }

  const wasm = await getWasmModule();

  wasm.xmutil_clear_log();

  const mdlSourcePtr = wasm.malloc(mdlSource.length);
  getUint8Memory0()
    .subarray(mdlSourcePtr, mdlSourcePtr + mdlSource.length)
    .set(mdlSource);

  const resultPtr = wasm.xmutil_convert_mdl_to_xmile(
      mdlSourcePtr,
      mdlSource.length,
      0,
      !pretty,
      false,
      false,
  );
  const result = getStringFromWasm(resultPtr);
  wasm.free(resultPtr);

  const logPtr = wasm.xmutil_get_log();
  let log: string | undefined = getStringFromWasm(logPtr);
  if (!log) {
    log = undefined;
  }

  wasm.xmutil_clear_log();

  return [result, log];
}
