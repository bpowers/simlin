// Copyright 2020 Bobby Powers. All rights reserved.
// Use of this source code is governed by the MIT License
// that can be found in the LICENSE file.

import type { Engine, Error, ErrorKind } from './core/engine_main';

import { ErrorCode } from './core/engine_main';

export { Engine, Error, ErrorCode, ErrorKind };

let cachedWasmModule: typeof import('./core/engine_main') | undefined;
function getWasmModule(): Promise<typeof import('./core/engine_main')> {
  return new Promise((resolve, reject) => {
    if (cachedWasmModule) {
      resolve(cachedWasmModule);
      return;
    }

    import('./core/engine_main')
      .then((module) => {
        cachedWasmModule = module;
        resolve(module);
      })
      .catch(reject);
  });
}

export async function open(projectPb: Uint8Array): Promise<Engine | undefined> {
  const wasm = await getWasmModule();
  return wasm.open(projectPb);
}