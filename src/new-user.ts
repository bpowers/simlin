// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { parse as parseToml } from '@iarna/toml';
import * as fs from 'fs-extra';
import * as logger from 'winston';

import { fileFromXmile } from './models/file';
import { newProject } from './models/project';
import { UserDocument } from './models/user';

async function populateExample(user: UserDocument, exampleModelPath: string): Promise<void> {
  const metadataPath = `${exampleModelPath}/project.toml`;
  const metadataContents = await fs.readFile(metadataPath, 'utf8');
  const metadata = parseToml(metadataContents);

  const modelPath = `${exampleModelPath}/model.xmile`;
  const modelContents = await fs.readFile(modelPath, 'utf8');

  if (!metadata.project) {
    throw new Error(`expected [project] section in ${metadataPath}`);
  }
  const projectMeta: { name: string; description: string } = metadata.project as any;
  const projectName = projectMeta.name;
  const projectDescription = projectMeta.description;
  const userId = user._id;

  const project = await newProject(user, projectName, projectDescription);
  const file = await fileFromXmile(project._id, userId, modelContents);

  project.fileId = file._id;
  await project.save();

  return Promise.resolve(undefined);
}

export async function populateExamples(user: UserDocument, examplesDirName: string): Promise<void> {
  let files = await fs.readdir(examplesDirName);
  files = files.filter(async (file: string) => {
    const path = `${examplesDirName}/${file}`;
    try {
      const stats = await fs.stat(path);
      return stats.isDirectory();
    } catch (err) {
      logger.warn(`fs.stat(${path}): ${err}`);
      return false;
    }
  });
  for (const dir of files) {
    const path = `${examplesDirName}/${dir}`;

    // if an individual example fails, continue trying with any
    // other examples we have left
    try {
      await populateExample(user, path);
    } catch (err) {
      logger.error(`populateExample(${user.email}, ${path}): ${err}`);
      continue;
    }
  }

  return Promise.resolve(undefined);
}
