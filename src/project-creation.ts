// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { createHash } from 'crypto';
import { Timestamp } from 'google-protobuf/google/protobuf/timestamp_pb';
import { List } from 'immutable';

import {
  File as XmileFile,
  Header as XmileHeader,
  Model as XmileModel,
  Product as XmileProduct,
  SimSpec as XmileSimSpec,
  View as XmileView,
  ViewDefaults,
} from './engine/xmile';

import { File as FilePb } from './schemas/file_pb';
import { Project as ProjectPb } from './schemas/project_pb';
import { User as UserPb } from './schemas/user_pb';

export function emptyProject(name: string, userName: string): XmileFile {
  return new XmileFile({
    header: new XmileHeader({
      vendor: 'systemdynamics.net',
      product: new XmileProduct({ name: 'Model v1.0' }),
      name,
      author: userName,
    }),
    simSpec: new XmileSimSpec({
      start: 0,
      stop: 100,
    }),
    models: List([
      new XmileModel({
        views: List([new XmileView(ViewDefaults)]),
      }),
    ]),
  });
}

const whitespace = /\s/gi;

export function createProject(
  user: UserPb,
  projectName: string,
  projectDescription: string,
  isPublic: boolean,
): ProjectPb {
  if (!user.getCanCreateProjects()) {
    throw new Error(`user ${user.getId()} can't create projects`);
  }
  const projectSlug = projectName.replace(whitespace, '-').toLowerCase();
  const userId = user.getId();
  const id = `${userId}/${projectSlug}`;

  const projectPb = new ProjectPb();
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
  jsonContents: string,
  pbContents: Uint8Array | undefined,
): FilePb {
  const created = new Timestamp();
  created.fromDate(new Date());

  const filePb = new FilePb();
  filePb.setProjectId(projectId);
  filePb.setUserId(userId);
  filePb.setCreated(created);
  filePb.setJsonContents(jsonContents);
  if (pbContents) {
    filePb.setProjectContents(pbContents);
  }

  const hash = createHash('sha256');
  hash.update(filePb.serializeBinary());
  filePb.setId(hash.digest('hex'));

  return filePb;
}
