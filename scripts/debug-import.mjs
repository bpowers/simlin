#!/usr/bin/env node
import { readFileSync } from 'fs';

import base64 from 'js-base64';
import { open } from '@system-dynamics/engine';
import { createFile, createProject } from '@system-dynamics/server/lib/project-creation.js';
import { createDatabase } from '@system-dynamics/server/lib/models/db.js';
import userPb from '@system-dynamics/server/lib/schemas/user_pb.js';

const args = process.argv.slice(2);
const inputFile = args[0];
const projectName = inputFile.split('.')[0];

let pb = readFileSync(inputFile);
let engine = await open(pb);
if (!engine) {
  pb = base64.toUint8Array(pb.toString('utf-8'))
  engine = await open(pb);
}

engine.simRunToEnd();

const userName = process.env.USER;

process.env['FIRESTORE_EMULATOR_HOST'] = '127.0.0.1:8092';

const db = await createDatabase({ backend: 'firestore' });
const user = await db.user.findOne(userName);

const project = createProject(user, projectName, `imported from ${inputFile}`, false);

const filePb = createFile(project.getId(), user.getId(), undefined, pb);

await db.file.create(filePb.getId(), filePb);

project.setFileId(filePb.getId());
await db.project.create(project.getId(), project);

console.log(`imported ${inputFile}`);