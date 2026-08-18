// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { describe, it, expect } from '@rstest/core';

import {
  DEFAULT_HEIGHT_PX,
  initialSyncState,
  MAX_PENDING_SNAPSHOTS,
  optimisticVersionAfterSave,
  parseWasmReply,
  readTraits,
  reconcileRevision,
  recordSentSnapshot,
  resolveTheme,
  TRAITS,
  wrapperStyle,
} from './widget-core';

function getterFor(state: Record<string, unknown>): (key: string) => unknown {
  return (key) => state[key];
}

describe('readTraits', () => {
  it('reads well-typed traits through', () => {
    const t = readTraits(
      getterFor({
        [TRAITS.projectJson]: '{"a":1}',
        [TRAITS.revision]: 7,
        [TRAITS.height]: 480,
        [TRAITS.theme]: 'dark',
        [TRAITS.notice]: 'hi',
        [TRAITS.readOnly]: true,
      }),
    );
    expect(t).toEqual({
      projectJson: '{"a":1}',
      revision: 7,
      height: 480,
      theme: 'dark',
      notice: 'hi',
      readOnly: true,
    });
  });

  it('coerces missing or malformed traits to safe defaults', () => {
    const t = readTraits(getterFor({}));
    expect(t).toEqual({
      projectJson: '',
      revision: 0,
      height: DEFAULT_HEIGHT_PX,
      theme: 'auto',
      notice: '',
      readOnly: false,
    });
  });

  // Enumerate every coercion arm rather than one representative each.
  it.each([
    ['height 0', { [TRAITS.height]: 0 }, DEFAULT_HEIGHT_PX],
    ['height negative', { [TRAITS.height]: -10 }, DEFAULT_HEIGHT_PX],
    ['height NaN', { [TRAITS.height]: NaN }, DEFAULT_HEIGHT_PX],
    ['height Infinity', { [TRAITS.height]: Infinity }, DEFAULT_HEIGHT_PX],
    ['height string', { [TRAITS.height]: '300' }, DEFAULT_HEIGHT_PX],
    ['height fractional rounds', { [TRAITS.height]: 300.6 }, 301],
  ])('height: %s', (_label, state, expected) => {
    expect(readTraits(getterFor(state)).height).toBe(expected);
  });

  it.each([
    ['unknown theme', 'sepia', 'auto'],
    ['non-string theme', 3, 'auto'],
    ['light', 'light', 'light'],
    ['dark', 'dark', 'dark'],
    ['auto', 'auto', 'auto'],
  ])('theme: %s', (_label, raw, expected) => {
    expect(readTraits(getterFor({ [TRAITS.theme]: raw })).theme).toBe(expected);
  });

  it.each([
    ['fractional revision', 1.5, 0],
    ['string revision', '4', 0],
    ['integer revision', 4, 4],
  ])('revision: %s', (_label, raw, expected) => {
    expect(readTraits(getterFor({ [TRAITS.revision]: raw })).revision).toBe(expected);
  });

  it('read_only is only true for the boolean true', () => {
    expect(readTraits(getterFor({ [TRAITS.readOnly]: 'true' })).readOnly).toBe(false);
    expect(readTraits(getterFor({ [TRAITS.readOnly]: 1 })).readOnly).toBe(false);
    expect(readTraits(getterFor({ [TRAITS.readOnly]: true })).readOnly).toBe(true);
  });
});

describe('resolveTheme', () => {
  it.each([
    ['explicit light beats a dark host', 'light', { jpThemeLight: 'false', prefersDark: true }, 'light'],
    ['explicit dark beats a light host', 'dark', { jpThemeLight: 'true', prefersDark: false }, 'dark'],
    ['auto follows JupyterLab dark', 'auto', { jpThemeLight: 'false', prefersDark: false }, 'dark'],
    ['auto follows JupyterLab light over OS dark', 'auto', { jpThemeLight: 'true', prefersDark: true }, 'light'],
    ['auto without JupyterLab follows OS dark', 'auto', { prefersDark: true }, 'dark'],
    ['auto without any signal is light', 'auto', { prefersDark: false }, 'light'],
  ] as const)('%s', (_label, theme, host, expected) => {
    expect(resolveTheme(theme, host)).toBe(expected);
  });
});

describe('wrapperStyle', () => {
  it('anchors the Editor chrome to an explicit height and full width', () => {
    expect(wrapperStyle(520)).toEqual({ position: 'relative', height: '520px', width: '100%' });
  });
});

describe('parseWasmReply', () => {
  const bytes = new Uint8Array([0, 97, 115, 109, 1, 0, 0, 0]);

  it('ignores messages that are not wasm replies', () => {
    expect(parseWasmReply({ type: 'other' }, [new DataView(bytes.buffer)])).toEqual({ kind: 'ignore' });
    expect(parseWasmReply('wasm', [])).toEqual({ kind: 'ignore' });
    expect(parseWasmReply(null, undefined)).toEqual({ kind: 'ignore' });
  });

  it('accepts a DataView buffer (what ipywidgets delivers) as standalone bytes', () => {
    const backing = new Uint8Array(bytes.length + 8);
    backing.set(bytes, 4);
    const view = new DataView(backing.buffer, 4, bytes.length);
    const reply = parseWasmReply({ type: 'wasm' }, [view]);
    expect(reply.kind).toBe('bytes');
    if (reply.kind === 'bytes') {
      expect(new Uint8Array(reply.bytes)).toEqual(bytes);
      expect(reply.bytes.byteLength).toBe(bytes.length);
      expect(reply.bytes).not.toBe(backing.buffer);
    }
  });

  it('accepts a bare ArrayBuffer and a typed array', () => {
    const fromBuffer = parseWasmReply({ type: 'wasm' }, [bytes.buffer]);
    expect(fromBuffer.kind).toBe('bytes');
    const fromTyped = parseWasmReply({ type: 'wasm' }, [bytes]);
    expect(fromTyped.kind).toBe('bytes');
    if (fromTyped.kind === 'bytes') {
      expect(new Uint8Array(fromTyped.bytes)).toEqual(bytes);
    }
  });

  it('surfaces a kernel-side error', () => {
    expect(parseWasmReply({ type: 'wasm', error: 'asset missing' }, [])).toEqual({
      kind: 'error',
      message: 'asset missing',
    });
  });

  it('treats a wasm reply without a usable buffer as an error, not a hang', () => {
    expect(parseWasmReply({ type: 'wasm' }, [])).toMatchObject({ kind: 'error' });
    expect(parseWasmReply({ type: 'wasm' }, undefined)).toMatchObject({ kind: 'error' });
    expect(parseWasmReply({ type: 'wasm' }, ['not a buffer'])).toMatchObject({ kind: 'error' });
    expect(parseWasmReply({ type: 'wasm' }, [new ArrayBuffer(0)])).toMatchObject({ kind: 'error' });
  });
});

describe('revision reconciliation', () => {
  it('same revision is a no-op', () => {
    const s = initialSyncState(3);
    expect(reconcileRevision(s, { revision: 3, projectJson: 'x' })).toEqual({ state: s, action: 'none' });
  });

  it('an echo of our own snapshot is an ack that keeps the Editor', () => {
    let s = initialSyncState(3);
    s = recordSentSnapshot(s, 'A');
    const r = reconcileRevision(s, { revision: 4, projectJson: 'A' });
    expect(r.action).toBe('ack');
    expect(r.state).toEqual({ revision: 4, pendingSnapshots: [] });
  });

  it('an echo of an older pending snapshot drops it and everything before, keeps newer ones', () => {
    let s = initialSyncState(0);
    for (const j of ['A', 'B', 'C']) {
      s = recordSentSnapshot(s, j);
    }
    const r = reconcileRevision(s, { revision: 1, projectJson: 'B' });
    expect(r.action).toBe('ack');
    expect(r.state.pendingSnapshots).toEqual(['C']);
    const r2 = reconcileRevision(r.state, { revision: 2, projectJson: 'C' });
    expect(r2.action).toBe('ack');
    expect(r2.state.pendingSnapshots).toEqual([]);
  });

  it('a foreign snapshot remounts and discards every pending snapshot', () => {
    let s = initialSyncState(0);
    s = recordSentSnapshot(s, 'A');
    const r = reconcileRevision(s, { revision: 1, projectJson: 'Z' });
    expect(r).toEqual({ state: { revision: 1, pendingSnapshots: [] }, action: 'remount' });
  });

  it('a revision that goes backwards (kernel restart / reseed) still remounts', () => {
    const r = reconcileRevision(initialSyncState(5), { revision: 0, projectJson: 'fresh' });
    expect(r.action).toBe('remount');
    expect(r.state.revision).toBe(0);
  });

  it('bounds the pending snapshot memory', () => {
    let s = initialSyncState(0);
    for (let i = 0; i < MAX_PENDING_SNAPSHOTS + 5; i++) {
      s = recordSentSnapshot(s, `S${i}`);
    }
    expect(s.pendingSnapshots).toHaveLength(MAX_PENDING_SNAPSHOTS);
    expect(s.pendingSnapshots[0]).toBe('S5');
    expect(s.pendingSnapshots[MAX_PENDING_SNAPSHOTS - 1]).toBe(`S${MAX_PENDING_SNAPSHOTS + 4}`);
  });

  it('optimistic version chains by exactly one', () => {
    expect(optimisticVersionAfterSave(0)).toBe(1);
    expect(optimisticVersionAfterSave(41)).toBe(42);
  });
});
