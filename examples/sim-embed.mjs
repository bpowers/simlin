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

console.log('Map<string, Series>([');
for (const ident of varNames) {
  if (ident === 'dt' || ident === 'final_time' || ident === 'initial_time') {
    continue;
  }
  const values = engine.simSeries(ident);
  console.log(`    ["${ident}", { name: "${ident}", time: (new Float64Array([${time}])) as Readonly<Float64Array>, values: (new Float64Array([${values}])) as Readonly<Float64Array> } as const ] as const,`);
}
console.log(`] as Array<[string, Series]>);`);
// const data = new Map(varNames.map((ident) => [ident, { name: ident, time, values:  }]));

engine.simClose();

//console.log(data);
