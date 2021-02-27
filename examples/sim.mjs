import { readFileSync } from 'fs';

import { open } from '@system-dynamics/engine';

const args = process.argv.slice(2);
const inputFile = args[0];
const pb = readFileSync(inputFile);

const engine = await open(pb);

const simError = engine.getSimError();
if (simError) {
  console.log(`simulation error: ${simError.getDetails()} (code: ${simError.code})`);
  process.exit(1);
}

engine.simRunToEnd();

let varNames = engine.simVarNames();
varNames.sort();
varNames = varNames.filter(n => n !== 'time');
varNames.unshift('time');


const time = engine.simSeries('time');
const data = new Map(varNames.map((ident) => [ident, { name: ident, time, values: engine.simSeries(ident) }]));

engine.simClose();

// output a tsv to stdout
console.log(varNames.join('\t'));
for (let i = 0; i < time.length; i++) {
  const row = [];
  for (const name of varNames) {
    row.push(data.get(name).values[i]);
  }
  console.log(row.join('\t'));
}
