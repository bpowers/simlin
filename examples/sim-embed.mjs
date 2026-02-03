import { readFileSync } from 'fs';
import { fileURLToPath } from 'url';
import { dirname, resolve } from 'path';

import { Project } from '@simlin/engine';

// Compute the WASM path relative to the engine package
const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const wasmPath = resolve(__dirname, '../src/engine/core/libsimlin.wasm');

const args = process.argv.slice(2);
const inputFile = args[0];
const pb = readFileSync(inputFile);

const project = await Project.openProtobuf(pb, { wasm: wasmPath });
const model = project.mainModel;
const issues = model.check();
if (issues.length > 0) {
  for (const issue of issues) {
    console.log(`${issue.severity}: ${issue.message}${issue.variable ? ` (${issue.variable})` : ''}`);
  }
  process.exit(1);
}

const run = model.run();

let varNames = [...run.varNames];
varNames.sort();
varNames = varNames.filter((n) => n !== 'time');
varNames.unshift('time');

const time = run.getSeries('time');

console.log('Map<string, Series>([');
for (const ident of varNames) {
  if (ident === 'dt' || ident === 'final_time' || ident === 'initial_time') {
    continue;
  }
  const values = run.getSeries(ident);
  console.log(
    `    ["${ident}", { name: "${ident}", time: (new Float64Array([${time}])) as Readonly<Float64Array>, values: (new Float64Array([${values}])) as Readonly<Float64Array> } as const ] as const,`,
  );
}
console.log(`] as Array<[string, Series]>);`);
