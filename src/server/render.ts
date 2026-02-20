// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { Project as EngineProject } from '@simlin/engine';
import { File } from './schemas/file_pb';

const PREVIEW_WIDTH = 800; // 400px * 2x retina

export async function renderToPNG(fileDoc: File): Promise<Uint8Array> {
  const engineProject = await EngineProject.openProtobuf(fileDoc.getProjectContents_asU8());
  try {
    return await engineProject.renderPng('main', PREVIEW_WIDTH);
  } finally {
    await engineProject.dispose();
  }
}
