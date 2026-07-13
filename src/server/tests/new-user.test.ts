// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// populateExamples shares the create-path write ordering with POST
// /api/projects: the example's File is persisted before its project row.
// If the row create fails, the File must be cleaned up -- without a
// project doc naming it, deleteProjectAndFiles would never reap it.

import { describe, it, expect } from '@rstest/core';
import path from 'path';

import type { Database } from '../models/db-interfaces';
import { populateExamples } from '../new-user';
import { File } from '../schemas/file_pb';
import { Project } from '../schemas/project_pb';
import { User } from '../schemas/user_pb';

const EXAMPLES_DIR = path.join(__dirname, 'fixtures', 'example-projects');

function makeCreator(id: string): User {
  const user = new User();
  user.setId(id);
  user.setCanCreateProjects(true);
  return user;
}

interface FakeDb {
  db: Database;
  files: Map<string, File>;
  projects: Map<string, Project>;
}

function makeFakeDb(projectCreate?: () => Promise<void>): FakeDb {
  const files = new Map<string, File>();
  const projects = new Map<string, Project>();
  const db = {
    file: {
      create: (id: string, pb: File): Promise<void> => {
        files.set(id, pb);
        return Promise.resolve();
      },
      deleteOne: (id: string): Promise<void> => {
        files.delete(id);
        return Promise.resolve();
      },
    },
    project: {
      create:
        projectCreate ??
        ((id: string, pb: Project): Promise<void> => {
          projects.set(id, pb);
          return Promise.resolve();
        }),
    },
  } as unknown as Database;
  return { db, files, projects };
}

describe('populateExamples', () => {
  it('creates a project and file per example', async () => {
    const fake = makeFakeDb();
    await populateExamples(fake.db, makeCreator('alice'), EXAMPLES_DIR);

    expect(fake.projects.size).toBe(1);
    const project = fake.projects.get('alice/population');
    expect(project).toBeDefined();
    expect(fake.files.has(project?.getFileId() ?? '')).toBe(true);
  });

  it('cleans up the persisted File when the project row cannot be created', async () => {
    const fake = makeFakeDb(() => Promise.reject(new Error('firestore unavailable')));
    // populateExamples swallows per-example failures by design (best-effort
    // seeding); the observable contract is that no orphan File remains.
    await populateExamples(fake.db, makeCreator('alice'), EXAMPLES_DIR);

    expect(fake.projects.size).toBe(0);
    expect(fake.files.size).toBe(0);
  });
});
