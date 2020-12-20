// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { createHash } from 'crypto';
import { Timestamp } from 'google-protobuf/google/protobuf/timestamp_pb';

import { File as FilePb } from './schemas/file_pb';
import { Project as ProjectPb } from './schemas/project_pb';
import { User as UserPb } from './schemas/user_pb';

import { project_io as pb } from './system-dynamics-engine/src/project_io_pb';

export function emptyProject(name: string, _userName: string): pb.Project {
  const model = new pb.Model();
  model.setName('main');
  model.setViewsList([new pb.View()]);

  const dt = new pb.Dt();
  dt.setValue(1);

  const simSpecs = new pb.SimSpecs();
  simSpecs.setStart(0);
  simSpecs.setStop(100);
  simSpecs.setDt(dt);

  const project = new pb.Project();
  project.setName(name);
  project.setModelsList([model]);
  project.setSimSpecs(simSpecs);

  return project;
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
  pbContents: Uint8Array | undefined,
): FilePb {
  const created = new Timestamp();
  created.fromDate(new Date());

  const filePb = new FilePb();
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
