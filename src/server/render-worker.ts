// Copyright 2021 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { parentPort, workerData } from 'worker_threads';

import { renderToPNG } from "@system-dynamics/server/render-inner";
import { Box } from "@system-dynamics/diagram/drawing/common";

// eslint-disable-next-line @typescript-eslint/no-misused-promises
setImmediate(async () => {
  const result = await renderToPNG(workerData.svgString as string, workerData.viewbox as Box);
  parentPort?.postMessage(result, [result.buffer]);
});
