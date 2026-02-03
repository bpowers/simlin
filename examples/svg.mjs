import { readFileSync, createWriteStream } from 'fs';
import { fileURLToPath } from 'url';
import { dirname, resolve } from 'path';

import { Project as EngineProject } from '@simlin/engine';
import { Project } from '@simlin/core/datamodel';
import { renderSvgToString } from '@simlin/diagram/render-common';

// Compute the WASM path relative to the engine package
const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const wasmPath = resolve(__dirname, '../src/engine/core/libsimlin.wasm');

const args = process.argv.slice(2);
const inputFile = args[0];
let contents = readFileSync(args[0], 'utf-8');

const engineProject = inputFile.endsWith('.mdl')
  ? await EngineProject.openVensim(contents, { wasm: wasmPath })
  : await EngineProject.open(contents, { wasm: wasmPath });
const pb = engineProject.serializeProtobuf();
const project = Project.deserializeBinary(pb);


const [ svgString ] = renderSvgToString(project, 'main');


const outputFile = createWriteStream('/dev/stdout');
outputFile.write(svgString);
outputFile.end();
