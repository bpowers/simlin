// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import type { Engine, Error, EquationError, UnitError } from './iengine';
export type { Engine, Error, EquationError, UnitError };

export { ErrorCode, errorCodeDescription } from './error_codes';
export { ErrorKind } from './iengine';

let cachedWasmModule: typeof import('./core/engine') | undefined;
function getWasmModule(): Promise<typeof import('./core/engine')> {
  return new Promise((resolve, reject) => {
    if (cachedWasmModule) {
      resolve(cachedWasmModule);
      return;
    }

    import('./core/engine')
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
