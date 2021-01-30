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

  return new Promise<Uint8Array>((ok) => {
    const worker = new Worker(__dirname + '/render-worker.js', {
      workerData: {
        svgString,
        viewbox,
      },
    });

    worker.on('message', (result: Uint8Array) => {
      ok(result);
    });
  });
}
