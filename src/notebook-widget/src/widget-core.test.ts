// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { describe, it, expect } from '@rstest/core';

import {
  classifyPush,
  DEFAULT_HEIGHT_PX,
  inFlightFor,
  parseNoticeMessage,
  parseSaveReply,
  parseWasmReply,
  readTraits,
  resolveTheme,
  seedAfterSaved,
  snapshotMessage,
  TRAITS,
  versionAfterReply,
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
        [TRAITS.readOnly]: true,
      }),
    );
    expect(t).toEqual({
      projectJson: '{"a":1}',
      revision: 7,
      height: 480,
      theme: 'dark',
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

describe('parseNoticeMessage', () => {
  it('parses a notice with and without a level', () => {
    expect(parseNoticeMessage({ type: 'notice', text: 'Updated on disk' })).toEqual({
      text: 'Updated on disk',
      level: 'info',
    });
    expect(parseNoticeMessage({ type: 'notice', text: 'conflict', level: 'warn' })).toEqual({
      text: 'conflict',
      level: 'warn',
    });
    expect(parseNoticeMessage({ type: 'notice', text: 'x', level: 'loud' })).toEqual({ text: 'x', level: 'info' });
  });

  it('rejects everything else', () => {
    expect(parseNoticeMessage({ type: 'wasm' })).toBeNull();
    expect(parseNoticeMessage({ type: 'notice' })).toBeNull();
    expect(parseNoticeMessage({ type: 'notice', text: '' })).toBeNull();
    expect(parseNoticeMessage({ type: 'notice', text: 3 })).toBeNull();
    expect(parseNoticeMessage(null)).toBeNull();
    expect(parseNoticeMessage('notice')).toBeNull();
  });
});

describe('snapshot protocol', () => {
  it('snapshotMessage carries base and the whole json', () => {
    expect(snapshotMessage(4, '{"a":1}')).toEqual({ type: 'snapshot', base: 4, json: '{"a":1}' });
  });

  it('parseSaveReply accepts saved/rejected with an integer revision, nothing else', () => {
    expect(parseSaveReply({ type: 'saved', revision: 5 })).toEqual({ kind: 'saved', revision: 5 });
    expect(parseSaveReply({ type: 'rejected', revision: 4 })).toEqual({ kind: 'rejected', revision: 4 });
    // Reply-typed but unusable: malformed, not ignored (it still consumes the
    // one reply the in-flight snapshot is owed).
    expect(parseSaveReply({ type: 'saved' })).toEqual({ kind: 'malformed' });
    expect(parseSaveReply({ type: 'saved', revision: 1.5 })).toEqual({ kind: 'malformed' });
    expect(parseSaveReply({ type: 'saved', revision: '5' })).toEqual({ kind: 'malformed' });
    expect(parseSaveReply({ type: 'rejected', revision: null })).toEqual({ kind: 'malformed' });
    expect(parseSaveReply({ type: 'notice', text: 'x' })).toBeNull();
    expect(parseSaveReply({ type: 'wasm' })).toBeNull();
    expect(parseSaveReply(null)).toBeNull();
    expect(parseSaveReply('saved')).toBeNull();
  });

  it('inFlightFor expects base + 1', () => {
    expect(inFlightFor(3, 'J')).toEqual({ json: 'J', base: 3, expectedRevision: 4 });
  });

  it('versionAfterReply resolves the saved revision, undefined on reject or malformed', () => {
    expect(versionAfterReply({ kind: 'saved', revision: 7 })).toBe(7);
    expect(versionAfterReply({ kind: 'rejected', revision: 6 })).toBeUndefined();
    expect(versionAfterReply({ kind: 'malformed' })).toBeUndefined();
  });

  it('seedAfterSaved is the in-flight bytes at the reported revision', () => {
    expect(seedAfterSaved(inFlightFor(3, 'S1'), 4)).toEqual({ revision: 4, projectJson: 'S1' });
    // A kernel that reports a different revision than base+1 (it should not,
    // but the reply is authoritative): the seed follows the reply.
    expect(seedAfterSaved(inFlightFor(3, 'S1'), 6)).toEqual({ revision: 6, projectJson: 'S1' });
  });

  describe('classifyPush', () => {
    const seed = { revision: 3, projectJson: 'SEED' };

    it('the seed pair again is none (second change event, idempotent re-push)', () => {
      expect(classifyPush(seed, null, seed)).toBe('none');
      expect(classifyPush(seed, inFlightFor(3, 'S1'), seed)).toBe('none');
    });

    it('the in-flight snapshot at base+1 is our own ack', () => {
      expect(classifyPush(seed, inFlightFor(3, 'S1'), { revision: 4, projectJson: 'S1' })).toBe('own-ack');
    });

    it('the in-flight json at any other revision is a kernel change (remount)', () => {
      // A disk change carrying the same bytes at a different revision is not
      // an ack of ours; the kernel decides via saved/rejected.
      expect(classifyPush(seed, inFlightFor(3, 'S1'), { revision: 5, projectJson: 'S1' })).toBe('remount');
    });

    it('a different pair with nothing in flight remounts (Python edit, disk reload)', () => {
      expect(classifyPush(seed, null, { revision: 4, projectJson: 'X' })).toBe('remount');
      expect(classifyPush(seed, null, { revision: 4, projectJson: 'SEED' })).toBe('remount');
      expect(classifyPush(seed, null, { revision: 3, projectJson: 'X' })).toBe('remount');
      expect(classifyPush(seed, null, { revision: 0, projectJson: 'fresh' })).toBe('remount');
    });

    it('a different pair while a snapshot is in flight remounts (disk change raced our save)', () => {
      expect(classifyPush(seed, inFlightFor(3, 'S1'), { revision: 4, projectJson: 'DISK' })).toBe('remount');
    });
  });
});
