// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { Project as EngineProject } from '@simlin/engine';
import { File } from './schemas/file_pb';

const MAX_PREVIEW_PX = 400;
const RETINA_SCALE = 2;
const MAX_PREVIEW_SIZE = MAX_PREVIEW_PX * RETINA_SCALE; // 800

/**
 * Compute the width and height to pass to renderPng so that the
 * larger dimension is clamped to `maxSize` and aspect ratio is
 * preserved.
 *
 * When the diagram is wider than tall, width is constrained.
 * When taller than wide, height is constrained.
 */
export function previewDimensions(
  svgWidth: number,
  svgHeight: number,
  maxSize: number,
): { width: number; height: number } {
  if (svgWidth <= 0 || svgHeight <= 0 || maxSize <= 0) {
    return { width: 0, height: 0 };
  }
  if (svgWidth >= svgHeight) {
    // Landscape or square: constrain width, derive height
    const scale = maxSize / svgWidth;
    return { width: maxSize, height: Math.ceil(svgHeight * scale) };
  }
  // Portrait: constrain height, derive width
  const scale = maxSize / svgHeight;
  return { width: Math.ceil(svgWidth * scale), height: maxSize };
}

/**
 * Parse the viewBox dimensions from an SVG string.
 *
 * Returns `{width, height}` from the third and fourth viewBox values.
 * Falls back to `{0, 0}` when the viewBox is absent or unparseable.
 */
export function parseSvgDimensions(svg: string): { width: number; height: number } {
  const match = svg.match(/viewBox="([^"]*)"/);
  if (!match) {
    return { width: 0, height: 0 };
  }
  const parts = match[1].trim().split(/\s+/).map(Number);
  if (parts.length < 4 || parts.some(isNaN)) {
    return { width: 0, height: 0 };
  }
  return { width: parts[2], height: parts[3] };
}

export async function renderToPNG(fileDoc: File): Promise<Uint8Array> {
  const engineProject = await EngineProject.openProtobuf(fileDoc.getProjectContents_asU8());
  try {
    const svg = await engineProject.renderSvgString('main');
    const intrinsic = parseSvgDimensions(svg);
    const dims = previewDimensions(intrinsic.width, intrinsic.height, MAX_PREVIEW_SIZE);
    return await engineProject.renderPng('main', dims.width, dims.height);
  } finally {
    await engineProject.dispose();
  }
}
