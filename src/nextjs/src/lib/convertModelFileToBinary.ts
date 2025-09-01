import { Project } from '@system-dynamics/core/datamodel';
import { fromXmile, test } from '@system-dynamics/importer';
import { convertMdlToXmile } from '@system-dynamics/xmutil-js';

export default async function convertModelFileToBinary(modelFile: File) {
  await test();
  console.log('JS');

  let fileContents = await modelFile.text();
  if (modelFile.name.endsWith('.mdl')) {
    const [xmileContent, errors] = await convertMdlToXmile(fileContents, true);
    if (xmileContent.length === 0) throw new Error(errors ?? 'Unkown error converting MDL file to XMILE');
    else fileContents = xmileContent;
  }

  const projectBinary = await fromXmile(fileContents);
  const project = Project.deserializeBinary(projectBinary);
  const views = project.models.get('main')?.views;

  if (!views || views.isEmpty()) {
    throw new Error('We do not support models with no views');
  }

  return projectBinary;
}
