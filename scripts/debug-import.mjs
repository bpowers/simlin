#!/usr/bin/env node
import { readFileSync } from 'fs';
import { fileURLToPath } from 'url';
import { dirname, resolve } from 'path';

import base64 from 'js-base64';
import { Project } from '@simlin/engine';
import { createFile, createProject } from '@simlin/server/lib/project-creation.js';
import { createDatabase } from '@simlin/server/lib/models/db.js';

// Compute the WASM path relative to the engine package
const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const wasmPath = resolve(__dirname, '../src/engine/core/libsimlin.wasm');

const args = process.argv.slice(2);
const inputFile = args[0];
const projectName = inputFile.split('.')[0];

let pb = readFileSync(inputFile);

// Validate the protobuf by opening it with engine
// If it fails (throws), try decoding from base64 and retry
let project;
try {
  project = await Project.openProtobuf(pb, { wasm: wasmPath });
} catch {
  // Try decoding from base64
  pb = base64.toUint8Array(pb.toString('utf-8'));
  project = await Project.openProtobuf(pb, { wasm: wasmPath });
}

const userName = process.env.USER;

process.env['FIRESTORE_EMULATOR_HOST'] = '127.0.0.1:8092';

const db = await createDatabase({ backend: 'firestore' });
const user = await db.user.findOne(userName);

const dbProject = createProject(user, projectName, `imported from ${inputFile}`, false);

const filePb = createFile(dbProject.getId(), user.getId(), undefined, pb);

await db.file.create(filePb.getId(), filePb);

dbProject.setFileId(filePb.getId());
await db.project.create(dbProject.getId(), dbProject);

console.log(`imported ${inputFile}`);
