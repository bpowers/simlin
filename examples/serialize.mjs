import { readFileSync, createWriteStream } from 'fs';
import { fileURLToPath } from 'url';
import { dirname, resolve } from 'path';

import { convertMdlToXmile } from '@system-dynamics/xmutil';
import { Project as Engine2Project } from '@system-dynamics/engine2';

// Compute the WASM path relative to the engine2 package
const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const wasmPath = resolve(__dirname, '../src/engine2/core/libsimlin.wasm');

const args = process.argv.slice(2);
const inputFile = args[0];
let contents = readFileSync(args[0], 'utf-8');

if (inputFile.endsWith('.mdl')) {
  contents = await convertMdlToXmile(contents, false);
}

const project = await Engine2Project.open(contents, { wasm: wasmPath });
const pb = project.serializeProtobuf();

const outputFile = createWriteStream(args[1]);

outputFile.write(pb);
outputFile.end();
