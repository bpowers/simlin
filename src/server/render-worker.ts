// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { parentPort, workerData } from 'worker_threads';

import { Box } from '@simlin/diagram/drawing/common';

import { renderToPNG } from './render-inner';

setImmediate(async () => {
  const result = await renderToPNG(workerData.svgString as string, workerData.viewbox as Box);
  parentPort?.postMessage(result, [result.buffer as ArrayBuffer]);
});
