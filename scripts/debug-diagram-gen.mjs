#!/usr/bin/env node
// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { readFileSync, writeFileSync, mkdtempSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { execSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';

import { Project as Engine2Project } from '@simlin/engine';
import { Project as ProjectDM } from '@simlin/core/datamodel';
import { renderSvgToString } from '@simlin/diagram/render-common';

// Compute the WASM path relative to the engine package
const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const wasmPath = resolve(__dirname, '../src/engine/core/libsimlin.wasm');

/**
 * Generate an SVG from a project and save it to a file
 * @param {ProjectDM} project - The project to render
 * @param {string} outputPath - Path where the SVG should be saved
 * @param {string} label - Label for console output
 * @returns {void}
 */
function generateAndSaveSvg(project, outputPath, label) {
  // Check if the model has views
  const mainModel = project.models.get('main');
  if (!mainModel) {
    throw new Error('No main model found in the project');
  }

  const views = mainModel.views;
  if (!views || views.isEmpty()) {
    throw new Error(`Cannot generate ${label} diagram: model has no views`);
  }

  // Generate the SVG string
  let [svgString, viewbox] = renderSvgToString(project, 'main');

  // Add Google Fonts import for Roboto (with escaped ampersand for XML/SVG)
  const fontImport = `<style>
@import url('https://fonts.googleapis.com/css2?family=Roboto:wght@300;400;500;700&amp;display=swap');
</style>`;

  // Insert the font import after the opening <svg tag
  const svgTagEnd = svgString.indexOf('>');
  if (svgTagEnd > -1) {
    svgString = svgString.slice(0, svgTagEnd + 1) + '\n' + fontImport + svgString.slice(svgTagEnd + 1);
  }

  // Write the SVG to the file
  writeFileSync(outputPath, svgString, 'utf-8');

  console.log(`${label} SVG generated: ${outputPath}`);
  console.log(`Viewbox dimensions: ${viewbox.width}x${viewbox.height}`);
}

/**
 * Open a file in the default browser
 * @param {string} filePath - Path to the file to open
 * @param {string} label - Label for console output
 */
function openInBrowser(filePath, label) {
  console.log(`Opening ${label} in browser...`);
  execSync(`open "${filePath}"`);
}

async function main() {
  const args = process.argv.slice(2);

  if (args.length !== 1) {
    console.error('Usage: debug-diagram-gen.mjs <path-to-xmile-file>');
    process.exit(1);
  }

  const inputFile = args[0];
  const tempDir = process.env.TMPDIR || tmpdir();

  try {
    // Read the input file
    const contents = readFileSync(inputFile, 'utf-8');

    // Import the content using the engine
    const engine2Project = inputFile.endsWith('.mdl')
      ? await Engine2Project.openVensim(contents, { wasm: wasmPath })
      : await Engine2Project.open(contents, { wasm: wasmPath });
    const projectPB = engine2Project.serializeProtobuf();
    const project = ProjectDM.deserializeBinary(projectPB);

    // Generate and open the original SVG
    const originalTempPath = mkdtempSync(join(tempDir, 'simlin-diagram-'));
    const originalSvgPath = join(originalTempPath, 'diagram.svg');
    generateAndSaveSvg(project, originalSvgPath, 'Original');
    openInBrowser(originalSvgPath, 'original SVG');

    // Create a copy of the XMILE file without views
    console.log('\nCreating XMILE copy without views...');

    // Get XMILE from the engine project (handles both XMILE and MDL inputs)
    const xmileContent = engine2Project.toXmileString();

    // Remove the <views>...</views> section using regex
    // This regex matches <views> tags with any attributes and all content until the closing </views>
    const viewsRegex = /<views[^>]*>[\s\S]*?<\/views>/gi;
    const xmileWithoutViews = xmileContent.replace(viewsRegex, '');

    // Get just the filename without path and extension
    const inputFilename = inputFile
      .split('/')
      .pop()
      .replace(/\.(xmile|stmx|itmx|mdl)$/i, '');

    // Create a unique temp directory for this file
    const noViewsTempPath = mkdtempSync(join(tempDir, `${inputFilename}-no-views-`));
    const outputFile = join(noViewsTempPath, `${inputFilename}.xmile`);

    // Write the modified XMILE file
    writeFileSync(outputFile, xmileWithoutViews, 'utf-8');
    console.log(`Created XMILE file without views: ${outputFile}`);

    // Load the no-views XMILE into the engine
    console.log('\nLoading model into engine...');
    const noViewsEngine2Project = await Engine2Project.open(xmileWithoutViews, { wasm: wasmPath });

    // Note: generateViews is not yet implemented in the engine
    console.log('Note: View generation is not yet implemented');

    // Serialize back to protobuf and then to XMILE
    const regeneratedPB = noViewsEngine2Project.serializeProtobuf();
    const regeneratedXmile = noViewsEngine2Project.toXmileString();

    if (!regeneratedXmile) {
      throw new Error('Failed to convert regenerated model to XMILE');
    }

    // Save the regenerated XMILE to a temp file for debugging
    const regeneratedXmilePath = mkdtempSync(join(tempDir, `${inputFilename}-regenerated-xmile-`));
    const regeneratedXmileFile = join(regeneratedXmilePath, `${inputFilename}-regenerated.xmile`);
    writeFileSync(regeneratedXmileFile, regeneratedXmile, 'utf-8');
    console.log(`Created regenerated XMILE file: ${regeneratedXmileFile}`);

    // Parse the regenerated XMILE back into a project to generate SVG
    console.log('\nGenerating SVG from regenerated model...');
    const regeneratedProject = ProjectDM.deserializeBinary(regeneratedPB);

    // Check if the regenerated model has views before trying to render
    const regeneratedMainModel = regeneratedProject.models.get('main');
    if (regeneratedMainModel && regeneratedMainModel.views && !regeneratedMainModel.views.isEmpty()) {
      // Generate and open the regenerated SVG
      const regeneratedSvgPath = mkdtempSync(join(tempDir, `${inputFilename}-regenerated-svg-`));
      const regeneratedSvgFile = join(regeneratedSvgPath, 'regenerated-diagram.svg');
      generateAndSaveSvg(regeneratedProject, regeneratedSvgFile, 'Regenerated');
      openInBrowser(regeneratedSvgFile, 'regenerated SVG');
    } else {
      console.log('Note: Regenerated model has no views (generateViews not yet implemented)');
    }
  } catch (error) {
    console.error('Error generating diagram:', error);
    process.exit(1);
  }
}

// Run the main function
main().catch((error) => {
  console.error('Unhandled error:', error);
  process.exit(1);
});
