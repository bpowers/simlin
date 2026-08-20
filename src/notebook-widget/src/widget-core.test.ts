// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { describe, it, expect } from '@rstest/core';

import {
  checkSnapshotSize,
  classifyPush,
  DEFAULT_HEIGHT_PX,
  EDITOR_ROOT_MODEL,
  formatSize,
  inFlightFor,
  MAX_SNAPSHOT_BYTES,
  oversizeMessage,
  oversizeNotice,
  parseNoticeMessage,
  parseSaveReply,
  parseWasmReply,
  readTraits,
  replyIsFor,
  requestId,
  resolveTheme,
  seedAfterSaved,
  snapshotMessage,
  TRAITS,
  snapshotWireSize,
  versionAfterReply,
  withEditorView,
  wrapperStyle,
  storedViewport,
  viewportToCarry,
} from './widget-core';

function getterFor(state: Record<string, unknown>): (key: string) => unknown {
  return (key) => state[key];
}

describe('withEditorView', () => {
  const EMPTY_VIEW = { kind: 'stock_flow', elements: [], viewBox: { x: 0, y: 0, width: 0, height: 0 }, zoom: 1 };

  it('gives the root model an empty stock-flow view when it has none', () => {
    const viewless = JSON.stringify({ name: 'p', models: [{ name: EDITOR_ROOT_MODEL, auxiliaries: [] }] });
    const doc = JSON.parse(withEditorView(viewless)) as { models: Array<{ name: string; views?: unknown[] }> };
    expect(doc.models[0].views).toEqual([EMPTY_VIEW]);
    expect(doc.models[0].name).toBe(EDITOR_ROOT_MODEL);
  });

  it('treats an empty views list like a missing one', () => {
    const viewless = JSON.stringify({ name: 'p', models: [{ name: EDITOR_ROOT_MODEL, views: [] }] });
    const doc = JSON.parse(withEditorView(viewless)) as { models: Array<{ views?: unknown[] }> };
    expect(doc.models[0].views).toEqual([EMPTY_VIEW]);
  });

  it('returns the exact input text when the root model already has a view (own-ack comparisons rely on this)', () => {
    const withView = '{"name":"p","models":[{"name":"main","views":[{"kind":"stock_flow","elements":[]}]}]}';
    expect(withEditorView(withView)).toBe(withView);
  });

  it('touches only the root model: other models keep their (missing) views', () => {
    const doc = { name: 'p', models: [{ name: 'sub' }, { name: EDITOR_ROOT_MODEL }] };
    const out = JSON.parse(withEditorView(JSON.stringify(doc))) as {
      models: Array<{ name: string; views?: unknown[] }>;
    };
    expect(out.models[0]).toEqual({ name: 'sub' });
    expect(out.models[1].views).toEqual([EMPTY_VIEW]);
  });

  it.each([
    ['not JSON', '{'],
    ['not an object', '[1]'],
    ['no models list', '{"name":"p"}'],
    ['models not a list', '{"models":{}}'],
    ['no root model', '{"models":[{"name":"other"}]}'],
    ['empty string', ''],
  ])('leaves text it cannot repair as it is (%s)', (_label, text) => {
    expect(withEditorView(text)).toBe(text);
  });
});

describe('readTraits', () => {
  it('seeds a viewless root model with an empty view (the Editor is blank without views[0])', () => {
    const viewless = JSON.stringify({ name: 'p', models: [{ name: EDITOR_ROOT_MODEL }] });
    const t = readTraits(getterFor({ [TRAITS.projectJson]: viewless }));
    expect(t.projectJson).toBe(withEditorView(viewless));
    expect(t.projectJson).not.toBe(viewless);
  });

  it('reads well-typed traits through', () => {
    const t = readTraits(
      getterFor({
        [TRAITS.projectJson]: '{"a":1}',
        [TRAITS.revision]: 7,
        [TRAITS.height]: 480,
        [TRAITS.theme]: 'dark',
        [TRAITS.readOnly]: true,
        [TRAITS.maxSnapshotBytes]: 4096,
      }),
    );
    expect(t).toEqual({
      projectJson: '{"a":1}',
      revision: 7,
      height: 480,
      theme: 'dark',
      readOnly: true,
      maxSnapshotBytes: 4096,
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
      maxSnapshotBytes: MAX_SNAPSHOT_BYTES,
    });
  });

  it.each([
    ['cap 0', 0, MAX_SNAPSHOT_BYTES],
    ['cap negative', -1, MAX_SNAPSHOT_BYTES],
    ['cap fractional', 1.5, MAX_SNAPSHOT_BYTES],
    ['cap NaN', NaN, MAX_SNAPSHOT_BYTES],
    ['cap string', '4096', MAX_SNAPSHOT_BYTES],
    ['cap positive integer', 4096, 4096],
  ])('maxSnapshotBytes: %s', (_label, raw, expected) => {
    expect(readTraits(getterFor({ [TRAITS.maxSnapshotBytes]: raw })).maxSnapshotBytes).toBe(expected);
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
  it('anchors the Editor chrome to an explicit height and full width, on the themed page background', () => {
    // The background is the theme.css page-background token, defined on the
    // wrapper itself and flipped by its data-theme: a forced dark theme in a
    // light notebook must not leave the transparent canvas white.
    expect(wrapperStyle(520)).toEqual({
      position: 'relative',
      height: '520px',
      width: '100%',
      background: 'var(--color-background)',
    });
  });
});

describe('storedViewport', () => {
  const project = (view: Record<string, unknown> | undefined, name = 'main'): string =>
    JSON.stringify({
      name: 'p',
      models: [
        { name: 'other', views: [{ viewBox: { x: 9, y: 9, width: 9, height: 9 } }] },
        { name, ...(view === undefined ? {} : { views: [view] }) },
      ],
    });

  it('reads the root view viewBox and zoom', () => {
    expect(storedViewport(project({ viewBox: { x: -1.5, y: 2, width: 800, height: 450 }, zoom: 1.25 }))).toEqual({
      viewBox: { x: -1.5, y: 2, width: 800, height: 450 },
      zoom: 1.25,
    });
  });

  it("applies the reader's defaults: an omitted viewBox is 0/0/0/0 (the writer omits a zero-size box), an omitted zoom is 1", () => {
    expect(storedViewport(project({ elements: [] }))).toEqual({
      viewBox: { x: 0, y: 0, width: 0, height: 0 },
      zoom: 1,
    });
    expect(storedViewport(project({ viewBox: { x: 3, y: 4, width: 5, height: 6 } }))).toEqual({
      viewBox: { x: 3, y: 4, width: 5, height: 6 },
      zoom: 1,
    });
    // Non-numeric / non-finite fields fall back field by field.
    expect(storedViewport(project({ viewBox: { x: 'a', y: null, width: 5 }, zoom: 'big' }))).toEqual({
      viewBox: { x: 0, y: 0, width: 5, height: 0 },
      zoom: 1,
    });
  });

  it.each([
    ['not JSON', 'nope'],
    ['no models list', '{"name":"p"}'],
    ['no root model', '{"name":"p","models":[{"name":"other","views":[{}]}]}'],
    ['root model without views', project(undefined)],
    ['root model with an empty views list', JSON.stringify({ models: [{ name: 'main', views: [] }] })],
    ['a non-object first view', JSON.stringify({ models: [{ name: 'main', views: [3] }] })],
  ])('is undefined when there is no parsable root view: %s', (_label, json) => {
    expect(storedViewport(json)).toBeUndefined();
  });
});

describe('viewportToCarry', () => {
  const stored = (
    viewBox: Record<string, number> | undefined,
    zoom?: number,
    extra: Record<string, unknown> = {},
  ): string =>
    JSON.stringify({
      name: 'p',
      models: [
        {
          name: 'main',
          views: [
            { elements: [], ...(viewBox === undefined ? {} : { viewBox }), ...(zoom === undefined ? {} : { zoom }) },
          ],
          ...extra,
        },
      ],
    });
  const box = { x: 10, y: 20, width: 800, height: 400 };
  const live = { modelName: 'main', viewBox: { x: -120, y: 35, width: 800, height: 400 }, zoom: 1.5 };
  const carried = { viewBox: live.viewBox, zoom: live.zoom };

  it('carries the live root viewport when the kernel change left the stored viewport unchanged', () => {
    // A Python edit that added a variable: same viewBox/zoom, different content.
    expect(viewportToCarry(live, stored(box, 1), stored(box, 1, { auxiliaries: [{ name: 'a' }] }))).toEqual(carried);
    // Both unset (a converted model never edited in the browser).
    expect(viewportToCarry(live, stored(undefined), stored(undefined, undefined, { auxiliaries: [] }))).toEqual(
      carried,
    );
    // Both unset, one spelled out as zeros.
    expect(viewportToCarry(live, stored(undefined), stored({ x: 0, y: 0, width: 0, height: 0 }, 1))).toEqual(carried);
  });

  it('carries the live viewport when the incoming stored viewport is unset, whatever the outgoing stored one was', () => {
    // A disk reload from a file with no viewBox while the browser had one.
    expect(viewportToCarry(live, stored(box, 2), stored(undefined))).toEqual(carried);
    expect(viewportToCarry(live, stored(box, 2), stored({ x: 5, y: 5, width: 0, height: 300 }))).toEqual(carried);
    // An incoming text with no parsable root view (the Editor will report it;
    // the carried viewport is harmless there and right if a view is repaired).
    expect(viewportToCarry(live, stored(box, 2), '{"name":"p"}')).toEqual(carried);
  });

  it("adopts the kernel's viewport (carries nothing) when the kernel change moved the stored one", () => {
    expect(viewportToCarry(live, stored(box, 1), stored({ ...box, x: 11 }, 1))).toBeUndefined();
    expect(viewportToCarry(live, stored(box, 1), stored(box, 2))).toBeUndefined();
    // Outgoing unset, incoming set: the kernel laid the view out and framed it.
    expect(viewportToCarry(live, stored(undefined), stored(box, 1))).toBeUndefined();
    // Outgoing unparsable, incoming set: nothing to compare against.
    expect(viewportToCarry(live, '{"name":"p"}', stored(box, 1))).toBeUndefined();
  });

  it('carries nothing before the Editor has reported a viewport, or when the live one belongs to a module', () => {
    expect(viewportToCarry(null, stored(box, 1), stored(box, 1))).toBeUndefined();
    expect(viewportToCarry({ ...live, modelName: 'child' }, stored(box, 1), stored(box, 1))).toBeUndefined();
  });

  it('returns a copy, not the live record', () => {
    const result = viewportToCarry(live, stored(box, 1), stored(box, 1));
    expect(result).toEqual(carried);
    expect(result).not.toBe(live);
    expect(result?.viewBox).not.toBe(live.viewBox);
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
  it('snapshotMessage carries the request id, base and the whole json', () => {
    expect(snapshotMessage('v:1', 4, '{"a":1}')).toEqual({ type: 'snapshot', id: 'v:1', base: 4, json: '{"a":1}' });
  });

  it('requestId is unique per view token and per sequence number', () => {
    expect(requestId('v', 1)).toBe('v:1');
    expect(requestId('v', 1)).not.toBe(requestId('v', 2));
    expect(requestId('v', 1)).not.toBe(requestId('w', 1));
  });

  it('parseSaveReply accepts saved/rejected with an integer revision, nothing else, and reads the echoed id', () => {
    expect(parseSaveReply({ type: 'saved', revision: 5, id: 'v:1' })).toEqual({
      kind: 'saved',
      revision: 5,
      id: 'v:1',
    });
    expect(parseSaveReply({ type: 'rejected', revision: 4, id: 'v:2' })).toEqual({
      kind: 'rejected',
      revision: 4,
      id: 'v:2',
    });
    // No id, or a non-string id: the reply is parsed but answers nothing.
    expect(parseSaveReply({ type: 'saved', revision: 5 })).toEqual({ kind: 'saved', revision: 5, id: undefined });
    expect(parseSaveReply({ type: 'saved', revision: 5, id: 7 })).toEqual({
      kind: 'saved',
      revision: 5,
      id: undefined,
    });
    // Reply-typed but unusable: malformed, not ignored (with the id it still
    // consumes the one reply the in-flight snapshot is owed).
    expect(parseSaveReply({ type: 'saved', id: 'v:1' })).toEqual({ kind: 'malformed', id: 'v:1' });
    expect(parseSaveReply({ type: 'saved', revision: 1.5 })).toEqual({ kind: 'malformed', id: undefined });
    expect(parseSaveReply({ type: 'saved', revision: '5' })).toEqual({ kind: 'malformed', id: undefined });
    expect(parseSaveReply({ type: 'rejected', revision: null })).toEqual({ kind: 'malformed', id: undefined });
    expect(parseSaveReply({ type: 'notice', text: 'x' })).toBeNull();
    expect(parseSaveReply({ type: 'wasm' })).toBeNull();
    expect(parseSaveReply(null)).toBeNull();
    expect(parseSaveReply('saved')).toBeNull();
  });

  it('inFlightFor carries the id and expects base + 1', () => {
    expect(inFlightFor('v:1', 3, 'J')).toEqual({ id: 'v:1', json: 'J', base: 3, expectedRevision: 4 });
  });

  it('replyIsFor matches on the echoed id only: another id, or none, answers nothing', () => {
    const flight = inFlightFor('v:1', 3, 'J');
    expect(replyIsFor(flight, { kind: 'saved', revision: 4, id: 'v:1' })).toBe(true);
    expect(replyIsFor(flight, { kind: 'rejected', revision: 3, id: 'v:1' })).toBe(true);
    expect(replyIsFor(flight, { kind: 'malformed', id: 'v:1' })).toBe(true);
    expect(replyIsFor(flight, { kind: 'saved', revision: 4, id: 'v:2' })).toBe(false);
    expect(replyIsFor(flight, { kind: 'saved', revision: 4, id: 'w:1' })).toBe(false);
    expect(replyIsFor(flight, { kind: 'saved', revision: 4, id: undefined })).toBe(false);
    expect(replyIsFor(flight, { kind: 'malformed', id: undefined })).toBe(false);
  });

  it('versionAfterReply resolves the saved revision, undefined on reject or malformed', () => {
    expect(versionAfterReply({ kind: 'saved', revision: 7, id: 'v:1' })).toBe(7);
    expect(versionAfterReply({ kind: 'rejected', revision: 6, id: 'v:1' })).toBeUndefined();
    expect(versionAfterReply({ kind: 'malformed', id: 'v:1' })).toBeUndefined();
  });

  it('seedAfterSaved is the in-flight bytes at the reported revision', () => {
    expect(seedAfterSaved(inFlightFor('v:1', 3, 'S1'), 4)).toEqual({ revision: 4, projectJson: 'S1' });
    // A kernel that reports a different revision than base+1 (it should not,
    // but the reply is authoritative): the seed follows the reply.
    expect(seedAfterSaved(inFlightFor('v:1', 3, 'S1'), 6)).toEqual({ revision: 6, projectJson: 'S1' });
  });

  describe('classifyPush', () => {
    const seed = { revision: 3, projectJson: 'SEED' };

    it('the seed pair again is none (second change event, idempotent re-push)', () => {
      expect(classifyPush(seed, null, seed)).toBe('none');
      expect(classifyPush(seed, inFlightFor('v:1', 3, 'S1'), seed)).toBe('none');
    });

    it('the in-flight snapshot at base+1 is our own ack', () => {
      expect(classifyPush(seed, inFlightFor('v:1', 3, 'S1'), { revision: 4, projectJson: 'S1' })).toBe('own-ack');
    });

    it('the in-flight json at any other revision is a kernel change (remount)', () => {
      // A disk change carrying the same bytes at a different revision is not
      // an ack of ours; the kernel decides via saved/rejected.
      expect(classifyPush(seed, inFlightFor('v:1', 3, 'S1'), { revision: 5, projectJson: 'S1' })).toBe('remount');
    });

    it('a different pair with nothing in flight remounts (Python edit, disk reload)', () => {
      expect(classifyPush(seed, null, { revision: 4, projectJson: 'X' })).toBe('remount');
      expect(classifyPush(seed, null, { revision: 4, projectJson: 'SEED' })).toBe('remount');
      expect(classifyPush(seed, null, { revision: 3, projectJson: 'X' })).toBe('remount');
      expect(classifyPush(seed, null, { revision: 0, projectJson: 'fresh' })).toBe('remount');
    });

    it('a different pair while a snapshot is in flight remounts (disk change raced our save)', () => {
      expect(classifyPush(seed, inFlightFor('v:1', 3, 'S1'), { revision: 4, projectJson: 'DISK' })).toBe('remount');
    });
  });
});

describe('snapshot size', () => {
  it('the default cap is 8 MiB, matching pysimlin', () => {
    expect(MAX_SNAPSHOT_BYTES).toBe(8 * 1024 * 1024);
  });

  // Same rows as tests/test_widget_core.py::test_snapshot_wire_size_is_the_escaped_utf8_length.
  it.each([
    ['empty: the two quotes', '', 2],
    ['plain', 'abc', 5],
    ['quotes escape', '{"a":1}', 11],
    ['backslash escapes', 'back\\slash', 13],
    ['UTF-8, not \\uXXXX', 'Ünï', 7],
    ['control character escapes', 'line\nbreak', 13],
  ])('snapshotWireSize: %s', (_label, json, expected) => {
    expect(snapshotWireSize(json)).toBe(expected);
  });

  it.each([
    ['at the limit is ok', 'ab', 4, { kind: 'ok', bytes: 4 }],
    ['below the limit is ok', 'a', 4, { kind: 'ok', bytes: 3 }],
    ['one byte over is oversize', 'abc', 4, { kind: 'oversize', bytes: 5, limit: 4 }],
    ['escaping counts: a quote costs two', '"', 3, { kind: 'oversize', bytes: 4, limit: 3 }],
    ['multi-byte characters count as bytes', 'Ü', 3, { kind: 'oversize', bytes: 4, limit: 3 }],
  ])('checkSnapshotSize: %s', (_label, json, limit, expected) => {
    expect(checkSnapshotSize(json, limit)).toEqual(expected);
  });

  // SIZE_FIXTURE: pinned identically in tests/test_widget_core.py
  // (`test_format_size`); both sides must print the same figure so the
  // kernel's notice and the browser's toast collapse into one message.
  const SIZE_FIXTURE: Array<[number, string]> = [
    [0, '0 KiB'],
    [1, '0 KiB'],
    [512, '0 KiB'], // 0.5: a tie, rounds to even
    [1536, '2 KiB'], // 1.5: a tie, rounds to even
    [1537, '2 KiB'],
    [1024, '1 KiB'],
    [16, '0 KiB'],
    [262144, '256 KiB'],
    [1024 * 1024 - 1, '1024 KiB'],
    [1024 * 1024, '1 MiB'],
    [1_357_590, '1.3 MiB'],
    [8 * 1024 * 1024, '8 MiB'],
    [8 * 1024 * 1024 + 256 * 1024, '8.2 MiB'], // 8.25: a tie, rounds to even
    [8 * 1024 * 1024 + 768 * 1024, '8.8 MiB'], // 8.75: a tie, rounds to even
    [9_000_000, '8.6 MiB'],
    [10 * 1024 * 1024, '10 MiB'],
    [Math.trunc(12.3 * 1024 * 1024), '12.3 MiB'],
    [104857600, '100 MiB'],
  ];
  it.each(SIZE_FIXTURE)('formatSize(%d) = %s', (bytes, expected) => {
    expect(formatSize(bytes)).toBe(expected);
  });

  it('the report and the toast carry both sizes', () => {
    expect(oversizeMessage(9_000_000)).toEqual({ type: 'oversize', bytes: 9_000_000 });
    expect(oversizeNotice(9_000_000, MAX_SNAPSHOT_BYTES)).toEqual({
      level: 'warn',
      text: 'Edit not saved: the model is too large for the notebook connection (8.6 MiB > 8 MiB limit); edit it from Python instead.',
    });
    // Small caps read in KiB, never "0.0 MiB > 0.0 MiB".
    expect(oversizeNotice(1024, 16).text).toContain('(1 KiB > 0 KiB limit)');
  });
});
