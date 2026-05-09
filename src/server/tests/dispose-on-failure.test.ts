// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { Project } from '@simlin/engine';

import { emptyProject } from '../project-creation';

// Why these tests exist:
//
// emptyProject (and fileFromXmile in new-user.ts) opens an engine
// project, serializes it, then disposes it. The engine project owns
// a WASM handle that must be released explicitly -- if
// serializeProtobuf throws, a naive sequential implementation leaks
// the handle. These tests pin the dispose-on-failure invariant so
// the cleanup pattern matches render.ts.

interface FakeEngineProject {
  serializeProtobuf: jest.Mock<Promise<Uint8Array>, []>;
  dispose: jest.Mock<Promise<void>, []>;
}

function fakeProject(serializeImpl: () => Promise<Uint8Array>): FakeEngineProject {
  return {
    serializeProtobuf: jest.fn().mockImplementation(serializeImpl),
    dispose: jest.fn().mockResolvedValue(undefined),
  };
}

describe('emptyProject', () => {
  it('disposes the engine handle when serializeProtobuf rejects', async () => {
    const fp = fakeProject(() => Promise.reject(new Error('boom')));
    const openJsonSpy = jest.spyOn(Project, 'openJson').mockResolvedValue(fp as unknown as Project);

    try {
      await expect(emptyProject('test', 'bobby')).rejects.toThrow('boom');
      expect(fp.dispose).toHaveBeenCalledTimes(1);
      expect(fp.serializeProtobuf).toHaveBeenCalledTimes(1);
    } finally {
      openJsonSpy.mockRestore();
    }
  });

  it('still calls dispose on the happy path', async () => {
    const fp = fakeProject(() => Promise.resolve(new Uint8Array([1, 2, 3])));
    const openJsonSpy = jest.spyOn(Project, 'openJson').mockResolvedValue(fp as unknown as Project);

    try {
      const result = await emptyProject('test', 'bobby');
      expect(result).toEqual(new Uint8Array([1, 2, 3]));
      expect(fp.dispose).toHaveBeenCalledTimes(1);
    } finally {
      openJsonSpy.mockRestore();
    }
  });
});
