// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { Document, Model, model, Schema, Types } from 'mongoose';

import { File } from './file';
import { User, UserDocument } from './user';

const ObjectId = Schema.Types.ObjectId;

const projectKeysAllowlist = new Set(['name', 'description', 'tags', 'version', 'isPublic']);

interface ProjectModel {
  name: string;
  owner: Types.ObjectId; // username
  isPublic: boolean;
  description: string;
  tags: string[];
  collaborators: Types.ObjectId[];
  version: number;
  fileId?: Types.ObjectId;
}

const ProjectSchema: Schema = new Schema({
  name: {
    type: String,
    required: true,
    validate: (name: string) => {
      // we don't allow '/' or spaces in project names
      return !(name.includes('/') || /\s/.test(name));
    },
  },
  owner: {
    type: ObjectId,
    ref: 'User',
    required: true,
  },
  description: String,
  isPublic: {
    type: Boolean,
    required: true,
  },
  tags: {
    type: [String],
    required: true,
  },
  collaborators: {
    type: [
      {
        type: ObjectId,
        ref: 'User',
      },
    ],
    required: true,
  },
  version: {
    type: Number,
    required: true,
  },
  fileId: {
    type: ObjectId,
    ref: 'File',
  },
});
ProjectSchema.index({ owner: 1, name: 1 }, { unique: true });
ProjectSchema.set('toJSON', {
  transform: async (doc: any, ret: any, options: any): Promise<any> => {
    const allKeys = Object.keys(ret);
    const toRemove = allKeys.filter((key: string) => !projectKeysAllowlist.has(key));
    for (const key of toRemove) {
      delete ret[key];
    }
    let owner: UserDocument | null = options.user;
    if (!owner) {
      owner = await User.findById(doc.owner).exec();
    }
    if (!owner) {
      throw new Error(`Failed finding owner of Project(${doc._id})`);
    }
    ret.path = `${owner.username}/${doc.name}`;
    return ret;
  },
});

export interface ProjectDocument extends ProjectModel, Document {}

export const Project: Model<ProjectDocument> = model<ProjectDocument>('Project', ProjectSchema);

const whitespace = /\s/gi;

export async function newProject(
  user: UserDocument,
  projectName: string,
  projectDescription: string,
): Promise<ProjectDocument> {
  projectName = projectName.replace(whitespace, '-');
  const project = await Project.create({
    name: projectName,
    owner: user._id,
    slug: `${user.username}/${projectName}`,
    description: projectDescription,
    isPublic: false,
    version: 1,
    tags: [],
    collaborators: [],
  });

  return project;
}
