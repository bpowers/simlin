// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { Document, Model, model, Schema, Types } from 'mongoose';
import { DOMParser } from 'xmldom';

import { stdProject } from '../engine/project';

const ObjectId = Schema.Types.ObjectId;

interface FileModel {
  project: Types.ObjectId;
  prevId?: Types.ObjectId;
  user: Types.ObjectId;
  created: Date;
  contents: string;
}

const FileSchema: Schema = new Schema({
  project: {
    type: ObjectId,
    ref: 'Project',
    required: true,
  },
  prevId: ObjectId,
  user: {
    type: ObjectId,
    ref: 'User',
    required: true,
  },
  created: {
    type: Date,
    required: true,
  },
  contents: {
    type: String,
    required: true,
  },
});

export async function fileFromXmile(
  projectId: Types.ObjectId,
  userId: Types.ObjectId,
  xmile: string,
): Promise<FileDocument> {
  const xml = new DOMParser().parseFromString(xmile, 'application/xml');
  const [project, err] = stdProject.addXmileFile(xml);
  if (err) {
    throw err;
  }
  if (!project) {
    throw new Error('project not defined');
  }

  const sdFile = project.toFile();
  const sdJson = JSON.stringify(sdFile);

  const file = await File.create({
    project: projectId,
    user: userId,
    created: new Date(Date.now()),
    contents: sdJson,
  });

  return file;
}

export interface FileDocument extends FileModel, Document {}

export const File: Model<FileDocument> = model<FileDocument>('File', FileSchema);
