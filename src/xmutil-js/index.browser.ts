// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Type definition for the xmutil WASM module exports
interface XmutilWasmExports {
  memory: WebAssembly.Memory;
  malloc(size: number): number;
  free(ptr: number): void;
  xmutil_clear_log(): void;
  xmutil_get_log(): number;
  xmutil_convert_mdl_to_xmile(
    mdlSourcePtr: number,
    mdlSourceLen: number,
    fileNamePtr: number,
    isCompact: boolean,
    isLongName: boolean,
    isAsSectors: boolean,
  ): number;
}

export function defined<T>(object: T | undefined): T {
  if (object === undefined) {
    throw new Error('expected non-undefined object');
  }
  return object;
}

let cachedWasmModule: XmutilWasmExports | undefined;
function getWasmModule(): Promise<XmutilWasmExports> {
  return new Promise((resolve, reject) => {
    if (cachedWasmModule) {
      resolve(cachedWasmModule);
      return;
    }

    // Dynamic import of WASM module - handled by bundler
    import('./xmutil.wasm')
      .then((module) => {
        cachedWasmModule = module as unknown as XmutilWasmExports;
        resolve(cachedWasmModule);
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
    return '';
  }
  const mem = getUint8Memory0();
  let off = 0;
  while (mem[ptr + off] !== 0) {
    off++;
  }
  return cachedTextDecoder.decode(getUint8Memory0().subarray(ptr, ptr + off));
}

export async function convertMdlToXmile(
  mdlSource: string | Readonly<Uint8Array>,
  pretty = true,
): Promise<[string, string?]> {
  if (typeof mdlSource === 'string') {
    mdlSource = cachedTextEncoder.encode(mdlSource);
  }

  const wasm = await getWasmModule();

  wasm.xmutil_clear_log();

  const mdlSourcePtr = wasm.malloc(mdlSource.length);
  getUint8Memory0()
    .subarray(mdlSourcePtr, mdlSourcePtr + mdlSource.length)
    .set(mdlSource);

  const resultPtr = wasm.xmutil_convert_mdl_to_xmile(mdlSourcePtr, mdlSource.length, 0, !pretty, false, true);
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
