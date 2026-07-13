// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Wire-level coverage for POST /api/projects, the project-creation path.
// A taken name is the COMMON create failure, and it used to be detected
// only after the new File document was persisted -- leaving an orphan that
// no deletion path would ever reap (deleteProjectAndFiles is keyed by a
// project doc that was never created). It also surfaced as a 500: the
// duplicate check matched a placeholder error code ('wut') that Firestore
// never throws.

import { describe, it, expect, rs } from '@rstest/core';
import type http from 'http';

import type { File } from '../schemas/file_pb';
import { createHarness, login, makeProject, request, withServer, Harness } from './wire-harness';

const PB_CONTENTS = Buffer.from('serialized project bytes').toString('base64');

function createProjectReq(
  server: http.Server,
  cookie: string,
  body: Record<string, unknown>,
): ReturnType<typeof request> {
  return request(server, 'POST', '/api/projects', { cookie, 'content-type': 'application/json' }, JSON.stringify(body));
}

function fileIds(harness: Harness): string[] {
  return [...harness.files.keys()].sort();
}

describe('POST /api/projects', () => {
  it('persists the project and its file on success', async () => {
    const harness = createHarness();
    await withServer(harness.app, async (server) => {
      const cookie = await login(server, 'alice');

      const res = await createProjectReq(server, cookie, { projectName: 'Rockets', projectPB: PB_CONTENTS });
      expect(res.status).toBe(200);
      expect((res.body as { id?: string }).id).toBe('alice/rockets');

      const project = harness.projects.get('alice/rockets');
      expect(project).toBeDefined();
      const newFileId = project?.getFileId() ?? '';
      expect(harness.files.get(newFileId)?.getProjectContents_asB64()).toBe(PB_CONTENTS);
    });
  });

  it('400s a taken name without persisting an orphaned File', async () => {
    const harness = createHarness();
    await withServer(harness.app, async (server) => {
      const cookie = await login(server, 'alice');

      // 'secret' collides with the alice/secret fixture
      const res = await createProjectReq(server, cookie, { projectName: 'secret', projectPB: PB_CONTENTS });
      expect(res.status).toBe(400);
      expect(res.body).toEqual({ error: 'project name already taken' });

      expect(fileIds(harness)).toEqual(['file-1', 'file-2']);
      expect(harness.projects.get('alice/secret')?.getFileId()).toBe('file-2');
    });
  });

  it('cleans up its File when a concurrent create wins the name after the pre-check', async () => {
    const harness = createHarness();
    // Interleave the competing create in the window between this request's
    // duplicate pre-check and its project.create: once the File is
    // persisted, materialize the competitor's project row.
    const originalCreate = harness.db.file.create.bind(harness.db.file);
    harness.db.file.create = async (id: string, file: File): Promise<void> => {
      await originalCreate(id, file);
      harness.projects.set('alice/rockets', makeProject('alice/rockets', 'alice', false, 'competitor-file'));
    };

    await withServer(harness.app, async (server) => {
      const cookie = await login(server, 'alice');

      const res = await createProjectReq(server, cookie, { projectName: 'Rockets', projectPB: PB_CONTENTS });
      expect(res.status).toBe(400);
      expect(res.body).toEqual({ error: 'project name already taken' });

      // the loser's File must not linger, and the winner's row must survive
      expect(fileIds(harness)).toEqual(['file-1', 'file-2']);
      expect(harness.projects.get('alice/rockets')?.getFileId()).toBe('competitor-file');
    });
  });

  it("skips cleanup when the concurrent winner reused this request's File (row re-read guard)", async () => {
    const harness = createHarness();
    await withServer(harness.app, async (server) => {
      const cookie = await login(server, 'alice');

      // Freeze only the Date so the interleaved identical create hashes to
      // the same file id as this request's.
      rs.useFakeTimers({ toFake: ['Date'], now: new Date() });
      try {
        // After the loser persists its File, run a full identical create
        // to completion: it reuses that File (AlreadyExists on the same
        // id) and wins the project row, which then references the very id
        // the loser considers deleting.
        let interleaved = false;
        const originalCreate = harness.db.file.create.bind(harness.db.file);
        harness.db.file.create = async (id, filePb): Promise<void> => {
          await originalCreate(id, filePb);
          if (!interleaved) {
            interleaved = true;
            const winner = await createProjectReq(server, cookie, { projectName: 'Rockets', projectPB: PB_CONTENTS });
            if (winner.status !== 200) {
              throw new Error(`expected interleaved winner to succeed, got ${winner.status}`);
            }
          }
        };

        const res = await createProjectReq(server, cookie, { projectName: 'Rockets', projectPB: PB_CONTENTS });
        expect(res.status).toBe(400);
        expect(res.body).toEqual({ error: 'project name already taken' });

        // the shared File must survive: it is what the winner's row references
        const sharedFileId = harness.projects.get('alice/rockets')?.getFileId() ?? '';
        expect(sharedFileId).not.toBe('');
        expect(harness.files.has(sharedFileId)).toBe(true);
        expect(fileIds(harness)).toEqual(['file-1', 'file-2', sharedFileId].sort());
      } finally {
        rs.useRealTimers();
      }
    });
  });
});
