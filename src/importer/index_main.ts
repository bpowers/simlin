// Copyright 2020 Bobby Powers. All rights reserved.
// Use of this source code is governed by the MIT License
// that can be found in the LICENSE file.

let cachedWasmModule: typeof import('./core/importer_main') | undefined;
function getWasmModule(): Promise<typeof import('./core/importer_main')> {
  return new Promise((resolve, reject) => {
    if (cachedWasmModule) {
      resolve(cachedWasmModule);
      return;
    }

    import('./core/importer_main')
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
