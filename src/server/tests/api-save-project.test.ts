// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Wire-level coverage for POST /api/projects/:username/:projectName, the
// optimistic-concurrency save path: a conflicted save must not leave an
// orphaned File document behind (neither on the fast-fail version check nor
// when the conditional project update loses a race), version 0 is a
// saveable legacy state rather than a missing field, and auth failures use
// the same {error} envelope as the authz middleware.

import { describe, it, expect, rs } from '@rstest/core';
import type http from 'http';

import type { File } from '../schemas/file_pb';
import { createHarness, login, request, withServer, Harness } from './wire-harness';

const PB_CONTENTS = Buffer.from('serialized project bytes').toString('base64');

function save(
  server: http.Server,
  cookie: string,
  slug: string,
  body: Record<string, unknown>,
): ReturnType<typeof request> {
  return request(
    server,
    'POST',
    `/api/projects/${slug}`,
    { cookie, 'content-type': 'application/json' },
    JSON.stringify(body),
  );
}

function fileIds(harness: Harness): string[] {
  return [...harness.files.keys()].sort();
}

// Preview invalidation is scheduled with setTimeout after the response is
// written, so successful-save tests have to wait for the effect.
async function waitFor(cond: () => boolean, what: string): Promise<void> {
  const deadline = Date.now() + 2000;
  while (!cond()) {
    if (Date.now() > deadline) {
      throw new Error(`timed out waiting for ${what}`);
    }
    await new Promise((resolve) => setTimeout(resolve, 5));
  }
}

describe('POST /api/projects/:username/:projectName', () => {
  it('persists the new file and increments the version on a successful save', async () => {
    const harness = createHarness();
    await withServer(harness.app, async (server) => {
      const cookie = await login(server, 'alice');

      const res = await save(server, cookie, 'alice/secret', { currVersion: 1, projectPB: PB_CONTENTS });
      expect(res.status).toBe(200);
      expect(res.body).toEqual({ version: 2 });

      const project = harness.projects.get('alice/secret');
      expect(project).toBeDefined();
      expect(project?.getVersion()).toBe(2);

      const newFileId = project?.getFileId() ?? '';
      expect(newFileId).not.toBe('file-2');
      const newFile = harness.files.get(newFileId);
      expect(newFile).toBeDefined();
      expect(newFile?.getProjectContents_asB64()).toBe(PB_CONTENTS);
      expect(fileIds(harness)).toEqual(['file-1', 'file-2', newFileId].sort());

      // a successful save invalidates the cached preview (async, best-effort)
      await waitFor(() => !harness.previews.has('alice/secret'), 'preview invalidation');
    });
  });

  it('409s a conflicted save without persisting an orphaned File', async () => {
    const harness = createHarness();
    // Count writes rather than only inspecting the final table state: the
    // pre-check must mean the stale save never touches the file table --
    // write-then-clean-up would leave the same end state but a different
    // (crash-exposed) intermediate one.
    let fileWrites = 0;
    const originalCreate = harness.db.file.create.bind(harness.db.file);
    harness.db.file.create = async (id, filePb): Promise<void> => {
      fileWrites++;
      return originalCreate(id, filePb);
    };

    await withServer(harness.app, async (server) => {
      const cookie = await login(server, 'alice');

      const res = await save(server, cookie, 'alice/secret', { currVersion: 7, projectPB: PB_CONTENTS });
      expect(res.status).toBe(409);
      expect((res.body as { error?: string }).error).toMatch(/old version/);

      // nothing changed: no orphaned File, no file write at all, project
      // row untouched
      expect(fileWrites).toBe(0);
      expect(fileIds(harness)).toEqual(['file-1', 'file-2']);
      const project = harness.projects.get('alice/secret');
      expect(project?.getVersion()).toBe(1);
      expect(project?.getFileId()).toBe('file-2');

      // the still-valid preview must survive a failed save
      await new Promise((resolve) => setTimeout(resolve, 10));
      expect(harness.previews.has('alice/secret')).toBe(true);
    });
  });

  it('409s a stale save after a concurrent one won, leaving exactly the winner file', async () => {
    const harness = createHarness();
    await withServer(harness.app, async (server) => {
      const cookie = await login(server, 'alice');

      const winner = await save(server, cookie, 'alice/secret', { currVersion: 1, projectPB: PB_CONTENTS });
      expect(winner.status).toBe(200);
      const winnerFileId = harness.projects.get('alice/secret')?.getFileId() ?? '';

      const stale = await save(server, cookie, 'alice/secret', {
        currVersion: 1,
        projectPB: Buffer.from('older edits').toString('base64'),
      });
      expect(stale.status).toBe(409);

      expect(fileIds(harness)).toEqual(['file-1', 'file-2', winnerFileId].sort());
      const project = harness.projects.get('alice/secret');
      expect(project?.getVersion()).toBe(2);
      expect(project?.getFileId()).toBe(winnerFileId);
    });
  });

  it('deletes the just-created File when the conditional update loses the post-pre-check race', async () => {
    const harness = createHarness();
    // Interleave a concurrent winner in the window between the handler's
    // version pre-check and its conditional update: once the loser's File
    // is persisted, bump the stored version as a competing save would.
    const originalCreate = harness.db.file.create.bind(harness.db.file);
    harness.db.file.create = async (id: string, file: File): Promise<void> => {
      await originalCreate(id, file);
      harness.projects.get('alice/secret')?.setVersion(2);
    };

    await withServer(harness.app, async (server) => {
      const cookie = await login(server, 'alice');

      const res = await save(server, cookie, 'alice/secret', { currVersion: 1, projectPB: PB_CONTENTS });
      expect(res.status).toBe(409);

      // the File persisted mid-request must have been cleaned back up
      expect(fileIds(harness)).toEqual(['file-1', 'file-2']);
      expect(harness.projects.get('alice/secret')?.getFileId()).toBe('file-2');

      // this 409 came from the lost conditional update, not the pre-check;
      // the still-valid preview must survive on this path too
      await new Promise((resolve) => setTimeout(resolve, 10));
      expect(harness.previews.has('alice/secret')).toBe(true);
    });
  });

  it('skips cleanup when the concurrent winner carries the same file id (row re-read guard)', async () => {
    const harness = createHarness();
    await withServer(harness.app, async (server) => {
      const cookie = await login(server, 'alice');

      // Freeze only the Date so the interleaved identical save hashes to
      // the same file id as this request's.
      rs.useFakeTimers({ toFake: ['Date'], now: new Date() });
      try {
        // After the loser persists its File, run a full identical save to
        // completion: it reuses that File (AlreadyExists on the same id)
        // and wins the version race, leaving the row pointing at the very
        // id the loser then considers deleting.
        let interleaved = false;
        const originalCreate = harness.db.file.create.bind(harness.db.file);
        harness.db.file.create = async (id, filePb): Promise<void> => {
          await originalCreate(id, filePb);
          if (!interleaved) {
            interleaved = true;
            const winner = await save(server, cookie, 'alice/secret', { currVersion: 1, projectPB: PB_CONTENTS });
            if (winner.status !== 200) {
              throw new Error(`expected interleaved winner to succeed, got ${winner.status}`);
            }
          }
        };

        const res = await save(server, cookie, 'alice/secret', { currVersion: 1, projectPB: PB_CONTENTS });
        expect(res.status).toBe(409);

        // the shared File must survive: it is what the winner's row references
        const sharedFileId = harness.projects.get('alice/secret')?.getFileId() ?? '';
        expect(sharedFileId).not.toBe('file-2');
        expect(harness.files.has(sharedFileId)).toBe(true);
        expect(fileIds(harness)).toEqual(['file-1', 'file-2', sharedFileId].sort());
        expect(harness.projects.get('alice/secret')?.getVersion()).toBe(2);
      } finally {
        rs.useRealTimers();
      }
    });
  });

  it('skips cleanup of a reused File that belongs to an earlier save (createdFile guard)', async () => {
    const harness = createHarness();
    await withServer(harness.app, async (server) => {
      const cookie = await login(server, 'alice');

      rs.useFakeTimers({ toFake: ['Date'], now: new Date() });
      try {
        // History under the frozen clock: v1->v2 persists File X, v2->v3
        // supersedes it. X is now version history owned by the first save.
        const first = await save(server, cookie, 'alice/secret', { currVersion: 1, projectPB: PB_CONTENTS });
        expect(first.status).toBe(200);
        const historicFileId = harness.projects.get('alice/secret')?.getFileId() ?? '';

        const second = await save(server, cookie, 'alice/secret', {
          currVersion: 2,
          projectPB: Buffer.from('second edit').toString('base64'),
        });
        expect(second.status).toBe(200);

        // The loser re-saves X's exact contents (same frozen millisecond,
        // so it REUSES X rather than writing), and a different-content
        // winner takes the version race before the loser's update. The
        // row then references neither X nor anything the loser wrote --
        // only the createdFile guard stops it from deleting X, which
        // would destroy the first save's history File.
        let interleaved = false;
        const originalUpdate = harness.db.project.update.bind(harness.db.project);
        harness.db.project.update = async (id, cond, pb) => {
          if (!interleaved) {
            interleaved = true;
            const winner = await save(server, cookie, 'alice/secret', {
              currVersion: 3,
              projectPB: Buffer.from('third edit').toString('base64'),
            });
            if (winner.status !== 200) {
              throw new Error(`expected interleaved winner to succeed, got ${winner.status}`);
            }
          }
          return originalUpdate(id, cond, pb);
        };

        const res = await save(server, cookie, 'alice/secret', { currVersion: 3, projectPB: PB_CONTENTS });
        expect(res.status).toBe(409);

        expect(harness.files.has(historicFileId)).toBe(true);
        expect(harness.projects.get('alice/secret')?.getVersion()).toBe(4);
      } finally {
        rs.useRealTimers();
      }
    });
  });

  it('surfaces a failing conditional update as a 500, not a bogus conflict, and still cleans up', async () => {
    const harness = createHarness();
    await withServer(harness.app, async (server) => {
      const cookie = await login(server, 'alice');
      harness.db.project.update = () => Promise.reject(new Error('firestore unavailable'));

      const res = await save(server, cookie, 'alice/secret', { currVersion: 1, projectPB: PB_CONTENTS });
      expect(res.status).toBe(500);

      // the File written before the update must not leak, the row must be
      // untouched, and the still-valid preview must survive
      expect(fileIds(harness)).toEqual(['file-1', 'file-2']);
      const project = harness.projects.get('alice/secret');
      expect(project?.getVersion()).toBe(1);
      expect(project?.getFileId()).toBe('file-2');
      await new Promise((resolve) => setTimeout(resolve, 10));
      expect(harness.previews.has('alice/secret')).toBe(true);
    });
  });

  it('accepts currVersion 0: rows created before versions were seeded carry the proto3 default', async () => {
    const harness = createHarness();
    harness.projects.get('alice/secret')?.setVersion(0);

    await withServer(harness.app, async (server) => {
      const cookie = await login(server, 'alice');

      const res = await save(server, cookie, 'alice/secret', { currVersion: 0, projectPB: PB_CONTENTS });
      expect(res.status).toBe(200);
      expect(res.body).toEqual({ version: 1 });
      expect(harness.projects.get('alice/secret')?.getVersion()).toBe(1);
    });
  });

  it('reuses the byte-identical File when identical saves land in the same millisecond', async () => {
    const harness = createHarness();
    await withServer(harness.app, async (server) => {
      const cookie = await login(server, 'alice');

      // File ids hash content plus creation-millisecond; freezing only the
      // Date (timers stay real for http and the preview setTimeout) makes
      // two identical saves produce the same id deterministically.
      rs.useFakeTimers({ toFake: ['Date'], now: new Date() });
      try {
        const first = await save(server, cookie, 'alice/secret', { currVersion: 1, projectPB: PB_CONTENTS });
        expect(first.status).toBe(200);
        const fileId = harness.projects.get('alice/secret')?.getFileId() ?? '';
        expect(fileId).not.toBe('');

        const second = await save(server, cookie, 'alice/secret', { currVersion: 2, projectPB: PB_CONTENTS });
        expect(second.status).toBe(200);
        expect(second.body).toEqual({ version: 3 });

        // one File doc, referenced by the row, version advanced twice
        expect(harness.projects.get('alice/secret')?.getFileId()).toBe(fileId);
        expect(fileIds(harness)).toEqual(['file-1', 'file-2', fileId].sort());
        expect(harness.projects.get('alice/secret')?.getVersion()).toBe(3);
      } finally {
        rs.useRealTimers();
      }
    });
  });

  it('400s a save with a missing or non-integer currVersion', async () => {
    const harness = createHarness();
    await withServer(harness.app, async (server) => {
      const cookie = await login(server, 'alice');

      const missing = await save(server, cookie, 'alice/secret', { projectPB: PB_CONTENTS });
      expect(missing.status).toBe(400);
      expect(missing.body).toEqual({ error: 'currVersion is required' });

      const fractional = await save(server, cookie, 'alice/secret', { currVersion: 1.5, projectPB: PB_CONTENTS });
      expect(fractional.status).toBe(400);
      expect(fractional.body).toEqual({ error: 'currVersion must be an integer' });

      expect(fileIds(harness)).toEqual(['file-1', 'file-2']);
    });
  });
});

describe('project-route 401 bodies', () => {
  it("saving another user's project answers with the {error} envelope", async () => {
    const harness = createHarness();
    await withServer(harness.app, async (server) => {
      const cookie = await login(server, 'bob');

      const res = await save(server, cookie, 'alice/secret', { currVersion: 1, projectPB: PB_CONTENTS });
      expect(res.status).toBe(401);
      expect(res.body).toEqual({ error: 'unauthorized' });
    });
  });

  it("fetching another user's private project answers with the {error} envelope", async () => {
    const harness = createHarness();
    await withServer(harness.app, async (server) => {
      const cookie = await login(server, 'bob');

      const res = await request(server, 'GET', '/api/projects/alice/secret', { cookie });
      expect(res.status).toBe(401);
      expect(res.body).toEqual({ error: 'unauthorized' });
    });
  });

  it("fetching another user's private preview answers with the {error} envelope", async () => {
    const harness = createHarness();
    await withServer(harness.app, async (server) => {
      const cookie = await login(server, 'bob');

      const res = await request(server, 'GET', '/api/preview/alice/secret', { cookie });
      expect(res.status).toBe(401);
      expect(res.body).toEqual({ error: 'unauthorized' });
    });
  });
});
