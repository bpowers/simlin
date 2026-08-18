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
  pendingBase: 'pending_base',
  selection: 'selection',
  height: 'height',
  theme: 'theme',
  notice: 'notice',
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
  notice: string;
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
  const noticeRaw = get(TRAITS.notice);
  return {
    projectJson: typeof projectRaw === 'string' ? projectRaw : '',
    revision,
    height,
    theme,
    notice: typeof noticeRaw === 'string' ? noticeRaw : '',
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

// ---- revision reconciliation -------------------------------------------------

/**
 * What the shell remembers between kernel pushes: the last revision it saw and
 * the snapshots it has sent that the kernel has not echoed yet, oldest first.
 */
export interface SyncState {
  revision: number;
  pendingSnapshots: ReadonlyArray<string>;
}

/**
 * Upper bound on remembered unacknowledged snapshots. Each is a whole-project
 * JSON string; the Editor autosaves after every discrete edit, so a burst of
 * quick edits against a busy kernel can leave several in flight. Past this
 * many the oldest are forgotten, which at worst turns an ancient echo into an
 * unnecessary remount rather than a memory leak.
 */
export const MAX_PENDING_SNAPSHOTS = 32;

export function initialSyncState(revision: number): SyncState {
  return { revision, pendingSnapshots: [] };
}

/** Remember a snapshot the widget just sent to the kernel. */
export function recordSentSnapshot(state: SyncState, json: string): SyncState {
  const pending = [...state.pendingSnapshots, json];
  return {
    revision: state.revision,
    pendingSnapshots:
      pending.length > MAX_PENDING_SNAPSHOTS ? pending.slice(pending.length - MAX_PENDING_SNAPSHOTS) : pending,
  };
}

export type ReconcileAction =
  // The push echoes a snapshot this widget sent: adopt the revision, keep the
  // live Editor (and its undo history) as is.
  | 'ack'
  // Someone else (Python edit(), disk change, a rejected stale snapshot)
  // produced this state: remount the Editor on the new snapshot.
  | 'remount'
  // Nothing new (same revision as already known).
  | 'none';

/**
 * Decide how to treat a kernel push of (revision, project_json). Only the
 * kernel ever sets `revision`, so a change of it is never our own local
 * `model.set`. If the JSON matches one of the snapshots we sent, it is the
 * kernel accepting our edit -- that snapshot and every OLDER pending one are
 * dropped (the kernel applies snapshots in order, so an older one either was
 * already echoed or was rejected and superseded). Otherwise the state came
 * from elsewhere and every pending snapshot is dead: the kernel has moved on,
 * and it will reject them anyway.
 */
export function reconcileRevision(
  state: SyncState,
  incoming: { revision: number; projectJson: string },
): { state: SyncState; action: ReconcileAction } {
  if (incoming.revision === state.revision) {
    return { state, action: 'none' };
  }
  const idx = state.pendingSnapshots.lastIndexOf(incoming.projectJson);
  if (idx >= 0) {
    return {
      state: { revision: incoming.revision, pendingSnapshots: state.pendingSnapshots.slice(idx + 1) },
      action: 'ack',
    };
  }
  return { state: { revision: incoming.revision, pendingSnapshots: [] }, action: 'remount' };
}

/**
 * The version the Editor's controller should treat as acknowledged after a
 * save is handed to the kernel. The kernel bumps `revision` by exactly one per
 * accepted widget snapshot, so chaining optimistically lets a burst of edits
 * flow without waiting for each round trip; if the guess is ever wrong the
 * kernel rejects the next snapshot as stale and re-seeds the widget (a
 * remount), never a wrong write -- the kernel is authoritative.
 */
export function optimisticVersionAfterSave(base: number): number {
  return base + 1;
}
