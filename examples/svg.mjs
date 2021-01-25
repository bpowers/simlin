import { readFileSync, createWriteStream } from 'fs';

import { convertMdlToXmile } from '@system-dynamics/xmutil';
import { fromXmile } from '@system-dynamics/importer';
import { Project } from '@system-dynamics/core/datamodel';
import { renderSvgToString } from '@system-dynamics/diagram/render-common';

const args = process.argv.slice(2);
const inputFile = args[0];
let contents = readFileSync(args[0], 'utf-8');

if (inputFile.endsWith('.mdl')) {
  contents = await convertMdlToXmile(contents, false);
}

let pb = await fromXmile(contents);
let project = Project.deserializeBinary(pb);


const [ svgString ] = renderSvgToString(project, 'main');


const outputFile = createWriteStream('/dev/stdout');
outputFile.write(svgString);
outputFile.end();
