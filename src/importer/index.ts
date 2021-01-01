// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

let cachedWasmModule: typeof import('./pkg') | undefined;
function getWasmModule(): Promise<typeof import('./pkg')> {
  return new Promise((resolve, reject) => {
    if (cachedWasmModule) {
      resolve(cachedWasmModule);
      return;
    }

    import('./pkg')
      .then((module) => {
        cachedWasmModule = module;
        resolve(module);
      })
      .catch(reject);
  });
}

export async function fromXmile(xmileXml: string): Promise<Uint8Array> {
  const wasm = await getWasmModule();
  return wasm.from_xmile(xmileXml);
}
