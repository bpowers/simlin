// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { Document, Model, model, Schema, Types } from 'mongoose';

import { renderToPNG } from '../render';
import { File } from './file';
import { ProjectDocument } from './project';

const ObjectId = Schema.Types.ObjectId;

interface PreviewModel {
  project: Types.ObjectId;
  png: Types.Buffer;
  created: Date;
}

const PreviewSchema: Schema = new Schema({
  project: {
    type: ObjectId,
    ref: 'Project',
    required: true,
  },
  png: {
    type: Schema.Types.Buffer,
    required: true,
  },
  created: {
    type: Date,
    required: true,
  },
});
PreviewSchema.index({ project: 1 }, { unique: true });

export interface PreviewDocument extends PreviewModel, Document {}

export const Preview: Model<PreviewDocument> = model<PreviewDocument>('Preview', PreviewSchema);

export async function updatePreview(project: ProjectDocument): Promise<PreviewDocument> {
  const fileDoc = await File.findById(project.fileId).exec();
  if (!fileDoc) {
    throw new Error(`no File document found for project ${project.name}`);
  }

  let png: Buffer;
  try {
    png = await renderToPNG(fileDoc);
  } catch (err) {
    throw new Error(`renderToPNG: ${err.message}`);
  }

  const preview = await Preview.create({
    project: project.id,
    png,
    created: new Date(Date.now()),
  });

  return preview;
}
