import { readFileSync, createWriteStream } from 'fs';
import { fileURLToPath } from 'url';
import { dirname, resolve } from 'path';

import { convertMdlToXmile } from '@system-dynamics/xmutil';
import { Project as Engine2Project, init } from '@system-dynamics/engine2';
import { Project } from '@system-dynamics/core/datamodel';
import { renderSvgToString } from '@system-dynamics/diagram/render-common';

// Compute the WASM path relative to the engine2 package
const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const wasmPath = resolve(__dirname, '../src/engine2/core/libsimlin.wasm');

// Initialize WASM explicitly for Node.js
await init(wasmPath);

const args = process.argv.slice(2);
const inputFile = args[0];
let contents = readFileSync(args[0], 'utf-8');

if (inputFile.endsWith('.mdl')) {
  contents = await convertMdlToXmile(contents, false);
}

const engine2Project = await Engine2Project.open(contents);
const pb = engine2Project.serializeProtobuf();
engine2Project.dispose();
const project = Project.deserializeBinary(pb);


const [ svgString ] = renderSvgToString(project, 'main');


const outputFile = createWriteStream('/dev/stdout');
outputFile.write(svgString);
outputFile.end();
