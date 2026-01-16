// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { parse as parseToml } from '@iarna/toml';
import { promises as fs } from 'fs';
import * as logger from 'winston';

import { Database } from './models/db-interfaces.js';
import { Table } from './models/table.js';
import { createFile, createProject } from './project-creation.js';
import { File } from './schemas/file_pb.js';
import { User } from './schemas/user_pb.js';
import { Project as Engine2Project } from '@system-dynamics/engine2';

async function fileFromXmile(files: Table<File>, projectId: string, userId: string, xmile: string): Promise<File> {
  const project = await Engine2Project.open(xmile);
  const sdPB = project.serializeProtobuf();

  const file = createFile(projectId, userId, undefined, sdPB);
  await files.create(file.getId(), file);

  return file;
}

async function populateExample(db: Database, user: User, exampleModelPath: string): Promise<void> {
  const metadataPath = `${exampleModelPath}/project.toml`;
  const metadataContents = await fs.readFile(metadataPath, 'utf8');
  const metadata = parseToml(metadataContents);

  const modelPath = `${exampleModelPath}/model.xmile`;
  const modelContents = await fs.readFile(modelPath, 'utf8');

  if (!metadata.project) {
    throw new Error(`expected [project] section in ${metadataPath}`);
  }
  const projectMeta = metadata.project as unknown as { name: string; description: string };
  const projectName = projectMeta.name;
  const projectDescription = projectMeta.description;
  const userId = user.getId();

  const project = createProject(user, projectName, projectDescription, false);
  const file = await fileFromXmile(db.file, project.getId(), userId, modelContents);

  project.setFileId(file.getId());
  await db.project.create(project.getId(), project);

  return Promise.resolve(undefined);
}

export async function populateExamples(db: Database, user: User, examplesDirName: string): Promise<void> {
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
      await populateExample(db, user, path);
    } catch (err) {
      logger.error(`populateExample(${user.getId()}, ${path}): ${err}`);
    }
  }

  return Promise.resolve(undefined);
}
