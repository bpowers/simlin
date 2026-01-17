// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { Worker } from 'worker_threads';

import { Project as DmProject } from '@system-dynamics/core/datamodel';
import { Project as Engine2Project } from '@system-dynamics/engine2';
import type { JsonProject } from '@system-dynamics/engine2';
import { renderSvgToString } from '@system-dynamics/diagram/render-common';
import { File } from './schemas/file_pb';

export async function renderToPNG(fileDoc: File): Promise<Uint8Array> {
  const engineProject = await Engine2Project.openProtobuf(fileDoc.getProjectContents_asU8());
  const json = JSON.parse(engineProject.serializeJson()) as JsonProject;
  const project = DmProject.fromJson(json);
  engineProject.dispose();

  const [svgString, viewbox] = renderSvgToString(project, 'main');

  return new Promise<Uint8Array>((ok, error) => {
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
