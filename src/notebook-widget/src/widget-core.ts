// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Functional core of the notebook widget: every decision the imperative shell
 * (index.tsx, engine-bootstrap.ts) makes is a pure function here, tested
 * against hand-built inputs that mirror what the shell actually passes.
 *
 * Nothing in this file touches the DOM, React, or the anywidget model.
 */

/** Trait names shared with the Python `ModelWidget` (see the design plan). */
export const TRAITS = {
  projectJson: 'project_json',
  revision: 'revision',
  selection: 'selection',
  height: 'height',
  theme: 'theme',
  readOnly: 'read_only',
} as const;

export const DEFAULT_HEIGHT_PX = 600;

export type Theme = 'auto' | 'light' | 'dark';

/** The kernel-owned traits the shell reads, coerced to the types it needs. */
export interface WidgetTraits {
  projectJson: string;
  revision: number;
  height: number;
  theme: Theme;
  readOnly: boolean;
}

/**
 * Coerce raw trait values into {@link WidgetTraits}. Traits arrive untyped
 * (`unknown`) from the model, and a kernel bug or a stale notebook state
 * should degrade to sane defaults rather than a broken widget: a missing or
 * non-positive height becomes {@link DEFAULT_HEIGHT_PX}, an unknown theme
 * becomes `auto`, a non-string project becomes the empty string (which the
 * Editor reports as an error rather than crashing on).
 */
export function readTraits(get: (key: string) => unknown): WidgetTraits {
  const heightRaw = get(TRAITS.height);
  const height =
    typeof heightRaw === 'number' && Number.isFinite(heightRaw) && heightRaw > 0
      ? Math.round(heightRaw)
      : DEFAULT_HEIGHT_PX;
  const themeRaw = get(TRAITS.theme);
  const theme: Theme = themeRaw === 'light' || themeRaw === 'dark' ? themeRaw : 'auto';
  const revisionRaw = get(TRAITS.revision);
  const revision = typeof revisionRaw === 'number' && Number.isInteger(revisionRaw) ? revisionRaw : 0;
  const projectRaw = get(TRAITS.projectJson);
  return {
    projectJson: typeof projectRaw === 'string' ? projectRaw : '',
    revision,
    height,
    theme,
    readOnly: get(TRAITS.readOnly) === true,
  };
}

/**
 * Resolve `theme: 'auto'` against the host. JupyterLab announces its theme
 * on `document.body[data-jp-theme-light]`; other hosts (VS Code, Colab)
 * only expose the OS preference through `prefers-color-scheme`. Explicit
 * `light`/`dark` win over both.
 */
export function resolveTheme(theme: Theme, host: { jpThemeLight?: string; prefersDark: boolean }): 'light' | 'dark' {
  if (theme === 'light' || theme === 'dark') {
    return theme;
  }
  if (host.jpThemeLight === 'false') {
    return 'dark';
  }
  if (host.jpThemeLight === 'true') {
    return 'light';
  }
  return host.prefersDark ? 'dark' : 'light';
}

/** Inline style for the wrapper the Editor's chrome anchors to. */
export function wrapperStyle(heightPx: number): { position: 'relative'; height: string; width: string } {
  return { position: 'relative', height: `${heightPx}px`, width: '100%' };
}

// ---- wasm reply parsing ------------------------------------------------------

export type WasmReply = { kind: 'bytes'; bytes: ArrayBuffer } | { kind: 'error'; message: string } | { kind: 'ignore' };

/** The custom message the widget sends to ask the kernel for the engine. */
export const WASM_REQUEST = { type: 'wasm' } as const;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

/**
 * Copy an ArrayBuffer view (DataView / typed array) or ArrayBuffer into a
 * standalone ArrayBuffer holding exactly the wasm bytes. anywidget hands the
 * buffers of a custom message over as `DataView`s in JupyterLab; the copy also
 * detaches us from whatever larger buffer the transport sliced them from.
 */
function toStandaloneArrayBuffer(buffer: unknown): ArrayBuffer | null {
  if (buffer instanceof ArrayBuffer) {
    return buffer;
  }
  if (ArrayBuffer.isView(buffer)) {
    const view = new Uint8Array(buffer.buffer, buffer.byteOffset, buffer.byteLength);
    const copy = new Uint8Array(view.byteLength);
    copy.set(view);
    return copy.buffer;
  }
  return null;
}

/**
 * Interpret a `msg:custom` delivery. Only `{type: 'wasm'}` messages are ours;
 * anything else is `ignore`d so unrelated custom messages (present or future)
 * pass through untouched. A `{type: 'wasm', error}` reply is how the kernel
 * reports that it could not supply the artifact (e.g. a wheel missing the
 * asset), and a wasm reply with no binary buffer is treated the same way
 * rather than as a silent hang.
 */
export function parseWasmReply(msg: unknown, buffers: ReadonlyArray<unknown> | undefined): WasmReply {
  if (!isRecord(msg) || msg.type !== 'wasm') {
    return { kind: 'ignore' };
  }
  if (typeof msg.error === 'string' && msg.error !== '') {
    return { kind: 'error', message: msg.error };
  }
  const bytes = buffers && buffers.length > 0 ? toStandaloneArrayBuffer(buffers[0]) : null;
  if (bytes === null || bytes.byteLength === 0) {
    return { kind: 'error', message: 'kernel replied to the wasm request without a binary buffer' };
  }
  return { kind: 'bytes', bytes };
}

// ---- notice messages ---------------------------------------------------------

export type NoticeLevel = 'info' | 'warn';

export interface Notice {
  text: string;
  level: NoticeLevel;
}

/**
 * Interpret a `msg:custom` delivery as a kernel notice
 * (`{type:'notice', text, level?}`): transient text the kernel wants shown
 * over the diagram -- "Updated on disk", "Your change conflicted...". Not a
 * trait, because a trait is state (a second identical notice would be
 * silent) and a notice is an event. Anything else, including a notice with
 * no usable text, is `null`.
 */
export function parseNoticeMessage(msg: unknown): Notice | null {
  if (!isRecord(msg) || msg.type !== 'notice' || typeof msg.text !== 'string' || msg.text === '') {
    return null;
  }
  return { text: msg.text, level: msg.level === 'warn' ? 'warn' : 'info' };
}

// ---- snapshot protocol -------------------------------------------------------

/**
 * The widget -> kernel save message. The whole snapshot rides in a custom
 * message, never in a trait: ipywidgets allows ONE in-flight trait sync per
 * model and assign-merges the `patch` messages it buffers behind it, so
 * consecutive trait writes can reach the kernel collapsed into one (a burst
 * {S1,base n},{S2,base n+1},{S3,base n+2} arrives as {S1,base n},{S3,base
 * n+2} -- S2 lost, S3 rejected). Custom messages are queued in order and
 * never merged.
 */
export function snapshotMessage(base: number, json: string): { type: 'snapshot'; base: number; json: string } {
  return { type: 'snapshot', base, json };
}

/** The kernel's answer to a snapshot: accepted at `revision`, or rejected. */
export type SaveReply = { kind: 'saved'; revision: number } | { kind: 'rejected'; revision: number };

/**
 * Interpret a `msg:custom` delivery as a save reply (`{type:'saved',
 * revision}` / `{type:'rejected', revision}`). Anything else -- wasm,
 * notices, unknown types, a reply with a non-integer revision -- is `null`.
 */
export function parseSaveReply(msg: unknown): SaveReply | null {
  if (!isRecord(msg) || (msg.type !== 'saved' && msg.type !== 'rejected')) {
    return null;
  }
  if (typeof msg.revision !== 'number' || !Number.isInteger(msg.revision)) {
    return null;
  }
  return { kind: msg.type, revision: msg.revision };
}

/**
 * The one snapshot the widget may have in flight: what it sent, and the
 * revision the kernel will hold if it accepts it (`base + 1`).
 */
export interface InFlightSnapshot {
  json: string;
  base: number;
  expectedRevision: number;
}

export function inFlightFor(base: number, json: string): InFlightSnapshot {
  return { json, base, expectedRevision: base + 1 };
}

/** The (revision, project_json) pair the live Editor was seeded from. */
export interface EditorSeedPair {
  revision: number;
  projectJson: string;
}

export type PushAction =
  // The pair is the state of our own accepted in-flight snapshot: keep the
  // live Editor (and its undo history); the matching `saved` reply resolves
  // the save.
  | 'own-ack'
  // The pair is already what the Editor was seeded from (the second change
  // event of a push already handled, or an idempotent re-push): nothing.
  | 'none'
  // The kernel produced this state (Python edit(), disk reload, a re-seed
  // after a reject when its state moved): remount the Editor on it.
  | 'remount';

/**
 * Decide how to treat the kernel-owned (revision, project_json) pair after a
 * `change:revision` or `change:project_json` event. Only the kernel writes
 * those two traits (the widget never sets project_json), so every change
 * event is a kernel push. The two traits travel in one hold_sync but surface
 * as up to two change events, so callers run this on the FINAL pair after
 * each event and it is idempotent: the same pair twice is `none`.
 *
 * With a snapshot in flight, a pair equal to (in-flight json, base + 1) is
 * the kernel having accepted it (obligation: an accept pushes the exact bytes
 * received and revision + 1). Everything else that differs from the seed is
 * a kernel-side change and remounts -- including while a snapshot is in
 * flight, whose `rejected` reply then arrives; the reject path re-checks the
 * pair so a remount already done for the push is not done twice.
 */
export function classifyPush(
  seed: EditorSeedPair,
  inFlight: InFlightSnapshot | null,
  incoming: EditorSeedPair,
): PushAction {
  if (inFlight !== null && incoming.projectJson === inFlight.json && incoming.revision === inFlight.expectedRevision) {
    return 'own-ack';
  }
  if (incoming.revision === seed.revision && incoming.projectJson === seed.projectJson) {
    return 'none';
  }
  return 'remount';
}

/**
 * The version the Editor's controller treats as acknowledged after a save.
 * Resolved ONLY from the kernel's `saved` reply -- never optimistically -- so
 * `ProjectController` (which serialises saves: one in flight, one queued
 * flush that re-reads the acknowledged version) never has two snapshots in
 * flight and never sends a base it has not been told about. A `rejected`
 * reply resolves `undefined`: the controller keeps its version, and the
 * shell remounts the Editor from the kernel-authoritative traits.
 */
export function versionAfterReply(reply: SaveReply): number | undefined {
  return reply.kind === 'saved' ? reply.revision : undefined;
}
