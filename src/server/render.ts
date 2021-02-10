// Copyright 2021 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { Worker } from 'worker_threads';

import { Project as DmProject } from '@system-dynamics/core/datamodel';
import { renderSvgToString } from '@system-dynamics/diagram/render-common';
import { File } from './schemas/file_pb';

export function renderToPNG(fileDoc: File): Promise<Uint8Array> {
  const project = DmProject.deserializeBinary(fileDoc.getProjectContents_asU8());
  const [svgString, viewbox] = renderSvgToString(project, 'main');

  return new Promise<Uint8Array>((ok, error) => {
    // the Worker thing below only works when we deploy, not under
    // ts-node-dev for development
    if (process.env.NODE_ENV !== 'production') {
      import("@system-dynamics/server/render-inner").then((renderer) => {
        renderer.renderToPNG(svgString, viewbox).then(ok).catch(error);
      }).catch(error);
      return;
    }

    try {
      const worker = new Worker(__dirname + '/render-worker.js', {
        workerData: {
          svgString,
          viewbox,
        },
      });

      worker.on('message', (result: Uint8Array) => {
        ok(result);
      });
    } catch (err) {
      error(err);
    }
  });
}
