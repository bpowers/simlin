// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { createHash } from 'crypto';
import { Timestamp } from 'google-protobuf/google/protobuf/timestamp_pb';

import { File as DbFilePb } from './schemas/file_pb';
import { Project as DbProjectPb } from './schemas/project_pb';
import { User as UserPb } from './schemas/user_pb';

import { Project as Engine2Project } from '@system-dynamics/engine2';
import type { JsonProject } from '@system-dynamics/engine2';

export async function emptyProject(name: string, _userName: string): Promise<Uint8Array> {
  const emptyJson: JsonProject = {
    name,
    simSpecs: {
      startTime: 0,
      endTime: 100,
      dt: '1',
    },
    models: [
      {
        name: 'main',
        stocks: [],
        flows: [],
        auxiliaries: [],
        views: [{ kind: 'stock_flow', elements: [] }],
      },
    ],
  };

  const engine2Project = await Engine2Project.openJson(JSON.stringify(emptyJson));
  const protobuf = engine2Project.serializeProtobuf();
  engine2Project.dispose();

  return protobuf;
}

const whitespace = /\s/gi;

export function createProject(
  user: UserPb,
  projectName: string,
  projectDescription: string,
  isPublic: boolean,
): DbProjectPb {
  if (!user.getCanCreateProjects()) {
    throw new Error(`user ${user.getId()} can't create projects`);
  }
  const projectSlug = projectName.replace(whitespace, '-').toLowerCase();
  const userId = user.getId();
  const id = `${userId}/${projectSlug}`;

  const projectPb = new DbProjectPb();
  projectPb.setId(id);
  projectPb.setDisplayName(projectName);
  projectPb.setOwnerId(userId);
  projectPb.setIsPublic(isPublic);
  projectPb.setDescription(projectDescription);
  projectPb.setVersion(1);

  return projectPb;
}

export function createFile(
  projectId: string,
  userId: string,
  prevId: string | undefined,
  pbContents: Uint8Array | undefined,
): DbFilePb {
  const created = new Timestamp();
  created.fromDate(new Date());

  const filePb = new DbFilePb();
  filePb.setProjectId(projectId);
  filePb.setUserId(userId);
  filePb.setCreated(created);
  if (pbContents) {
    filePb.setProjectContents(pbContents);
  }

  const hash = createHash('sha256');
  hash.update(filePb.serializeBinary());
  filePb.setId(hash.digest('hex'));

  return filePb;
}
