/**
 * @jest-environment node
 *
 * Copyright 2026 The Simlin Authors. All rights reserved.
 * Use of this source code is governed by the Apache License,
 * Version 2.0, that can be found in the LICENSE file.
 */

// Regression tests for the Editor.save() inSave gate. These tests exercise
// the real Editor.prototype.save method (bypassing React construction) to
// verify the critical invariant:
//
//   After any onSave outcome (success, undefined-return, or thrown error),
//   inSave is always reset to false so subsequent saves are never silently
//   dropped.
//
// We use Object.create(Editor.prototype) to bypass the constructor (which
// would spin up async WASM initialisation and React internals). Only the
// fields that save() actually touches are seeded. Any production refactor
// that breaks the try/finally gate will cause these tests to fail.

import { Editor } from '../Editor';

function makeEditor(onSave: (project: unknown, currVersion: number) => Promise<number | undefined>): InstanceType<typeof Editor> {
  const editor = Object.create(Editor.prototype) as InstanceType<typeof Editor>;

  editor.inSave = false;
  editor.saveQueued = false;

  editor.state = {
    modelErrors: [],
    projectVersion: 1,
  } as unknown as InstanceType<typeof Editor>['state'];

  editor.props = {
    inputFormat: 'json',
    onSave,
  } as unknown as InstanceType<typeof Editor>['props'];

  editor.engineProject = {
    serializeJson: async () => '{}',
  } as unknown as InstanceType<typeof Editor>['engineProject'];

  editor.setState = (updater: unknown) => {
    const next = typeof updater === 'function' ? updater(editor.state) : updater;
    Object.assign(editor.state, next);
  };

  return editor;
}

describe('Editor.save() inSave gate (real Editor.prototype.save)', () => {
  it('resets inSave after a successful save so subsequent saves proceed', async () => {
    let callCount = 0;
    const editor = makeEditor(async () => {
      callCount++;
      return 2;
    });

    await editor.save(1);
    expect(editor.inSave).toBe(false);
    expect(callCount).toBe(1);

    await editor.save(2);
    expect(callCount).toBe(2);
  });

  it('resets inSave after onSave throws so subsequent saves proceed', async () => {
    let callCount = 0;
    const editor = makeEditor(async () => {
      callCount++;
      if (callCount === 1) {
        throw new Error('network failure');
      }
      return 2;
    });

    await editor.save(0);

    expect(editor.inSave).toBe(false);
    expect(callCount).toBe(1);

    // A second save must invoke onSave (not be silently dropped).
    await editor.save(0);
    expect(callCount).toBe(2);
  });

  it('flushes a queued save after onSave throws', async () => {
    let callCount = 0;
    const editor = makeEditor(async () => {
      callCount++;
      if (callCount === 1) {
        await new Promise<void>((resolve) => setImmediate(resolve));
        throw new Error('transient error');
      }
      return 2;
    });

    const firstSave = editor.save(0);
    // At this point the first save is in-flight; queue a second.
    void editor.save(1);

    await firstSave;

    // The queued save must have been flushed by the finally block.
    expect(callCount).toBe(2);
  });

  it('queues a concurrent save when a save is already in flight', async () => {
    let callCount = 0;
    // serializeJson resolves after a microtask so save() is truly async from
    // the very first await, giving us a window to observe inSave = true before
    // the second save() call runs.
    const editor = makeEditor(async () => {
      callCount++;
      return 2;
    });

    // Manually enter the in-flight state before the second call.
    editor.inSave = true;
    void editor.save(0);
    // save() must have returned immediately (queued, not dispatched).
    expect(editor.saveQueued).toBe(true);
    expect(callCount).toBe(0);

    // Reset inSave so the queued save can flush; simulate what the finally
    // block would do. In production this happens inside the first save's
    // finally; here we drive it manually.
    editor.inSave = false;
    editor.saveQueued = false;
    // The real scenario: start a fresh save that flushes normally.
    await editor.save(0);
    expect(callCount).toBe(1);
  });
});
