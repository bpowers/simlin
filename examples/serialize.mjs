import { readFileSync, createWriteStream } from 'fs';

import { convertMdlToXmile } from '@system-dynamics/xmutil';
import { fromXmile } from '@system-dynamics/importer';

const args = process.argv.slice(2);
const inputFile = args[0];
let contents = readFileSync(args[0], 'utf-8');

if (inputFile.endsWith('.mdl')) {
  contents = await convertMdlToXmile(contents, false);
}

let pb = await fromXmile(contents);

const outputFile = createWriteStream(args[1]);

outputFile.write(pb);
outputFile.end();
