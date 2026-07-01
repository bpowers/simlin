// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Worker-thread entry point for server-side preview rendering.
//
// The whole pipeline (protobuf parse -> SVG -> PNG rasterize) is CPU-bound
// WASM work; running it here keeps the Express event loop responsive and
// contains a WASM OOM or panic to a disposable thread that render.ts can
// terminate on timeout (issue #694). Worker threads have their own module
// registry, so this thread instantiates its own engine WASM instance instead
// of sharing the server's process-wide DirectBackend; the engine resolves
// core/libsimlin.wasm relative to its own compiled lib/, which is identical
// on the main thread and in a worker.

import { isMainThread, parentPort, workerData } from 'worker_threads';

import { Project as EngineProject } from '@simlin/engine';

import { MAX_PREVIEW_SIZE, parseSvgDimensions, previewDimensions } from './preview-geometry';

/** Payload render.ts passes to the worker via workerData. */
export interface RenderWorkerData {
  projectContents: Uint8Array;
}

/** The single message this worker posts back: a PNG or an error string. */
export type RenderWorkerResult = { ok: true; png: Uint8Array } | { ok: false; error: string };

/**
 * Render a serialized project's `main` model to a preview-sized PNG.
 *
 * This is the functional core of the worker, exported so tests can exercise
 * the real pipeline in-process (via ts-jest) without spawning a thread.
 */
export async function renderProjectToPng(projectContents: Uint8Array): Promise<Uint8Array> {
  const engineProject = await EngineProject.openProtobuf(projectContents);
  try {
    const svg = await engineProject.renderSvgString('main');
    const intrinsic = parseSvgDimensions(svg);
    const dims = previewDimensions(intrinsic.width, intrinsic.height, MAX_PREVIEW_SIZE);
    return await engineProject.renderPng('main', dims.width, dims.height);
  } finally {
    await engineProject.dispose();
  }
}

// Imperative shell: runs only when this module is loaded as a Worker entry.
// Importing it on the main thread (tests) leaves isMainThread true, so the
// import stays side-effect free there.
if (!isMainThread && parentPort) {
  const port = parentPort;
  const { projectContents } = workerData as RenderWorkerData;
  renderProjectToPng(projectContents)
    .then((png) => {
      const result: RenderWorkerResult = { ok: true, png };
      port.postMessage(result);
    })
    .catch((err: unknown) => {
      const result: RenderWorkerResult = {
        ok: false,
        error: err instanceof Error ? err.message : String(err),
      };
      port.postMessage(result);
    });
}
