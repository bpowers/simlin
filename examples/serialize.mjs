import { readFileSync, createWriteStream } from 'fs';
import { fileURLToPath } from 'url';
import { dirname, resolve } from 'path';

import { Project as Engine2Project } from '@simlin/engine';

// Compute the WASM path relative to the engine package
const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const wasmPath = resolve(__dirname, '../src/engine/core/libsimlin.wasm');

const args = process.argv.slice(2);
const inputFile = args[0];
let contents = readFileSync(args[0], 'utf-8');

const project = inputFile.endsWith('.mdl')
  ? await Engine2Project.openVensim(contents, { wasm: wasmPath })
  : await Engine2Project.open(contents, { wasm: wasmPath });
const pb = project.serializeProtobuf();

const outputFile = createWriteStream(args[1]);

outputFile.write(pb);
outputFile.end();
