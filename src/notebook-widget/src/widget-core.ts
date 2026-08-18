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

// ---- revision reconciliation -------------------------------------------------

/**
 * What the shell remembers between kernel pushes: the last (revision,
 * project_json) it adopted from the kernel and the snapshots it has sent that
 * the kernel has not echoed yet, oldest first.
 */
export interface SyncState {
  revision: number;
  // The project_json the current Editor mount was seeded from (or last acked
  // with). Together with `revision` it identifies "nothing new" pushes.
  knownJson: string;
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

export function initialSyncState(revision: number, knownJson: string): SyncState {
  return { revision, knownJson, pendingSnapshots: [] };
}

/** Remember a snapshot the widget just sent to the kernel. */
export function recordSentSnapshot(state: SyncState, json: string): SyncState {
  const pending = [...state.pendingSnapshots, json];
  return {
    ...state,
    pendingSnapshots:
      pending.length > MAX_PENDING_SNAPSHOTS ? pending.slice(pending.length - MAX_PENDING_SNAPSHOTS) : pending,
  };
}

export type ReconcileAction =
  // The push echoes a snapshot this widget sent: adopt the revision, keep the
  // live Editor (and its undo history) as is.
  | 'ack'
  // The kernel produced this state (Python edit(), disk change, a rejected
  // stale snapshot re-seeding us, a revision bump we did not cause): remount
  // the Editor on the pushed snapshot and revision.
  | 'remount'
  // Nothing new (same revision and content as already known).
  | 'none';

/**
 * Decide how to treat the kernel-owned (revision, project_json) pair the
 * shell reads after a `change:revision` or `change:project_json` event that
 * it did not cause itself. The two traits arrive in ONE kernel message but
 * fire two change events (or one, when only one value differs), so this is
 * idempotent: re-running it on the same pair is `none`.
 *
 * Kernel obligations this relies on: an accepted widget snapshot is pushed
 * back as the EXACT bytes the widget sent (with revision+1); a rejected
 * snapshot is answered by re-pushing the kernel's authoritative project_json
 * (revision unchanged). Hence:
 * - content equal to a pending snapshot => the kernel accepted one of ours.
 *   The OLDEST pending entry is dropped (the kernel accepts in order; the
 *   entries can be equal strings -- an edit/undo/redo burst is [A, B, A] --
 *   so matching by position, not by value, is what keeps every later echo
 *   an ack instead of a spurious remount);
 * - the known pair again while snapshots are pending: the kernel re-sent
 *   what we were seeded from without advancing -- that is a REJECT of the
 *   pending snapshot(s), re-seeding us (the frontend trait had held our own
 *   bytes, so the kernel's authoritative value fired a change event). The
 *   Editor has moved past that content locally and must be remounted on it,
 *   which also resets the version it believes is acknowledged. With nothing
 *   pending the same pair is simply the second change event of a push already
 *   handled;
 * - otherwise any change in content or revision came from the kernel and
 *   the Editor must be remounted on it. Every pending snapshot is dead at
 *   that point (the kernel has moved past them and will reject them, if it
 *   has not already).
 */
export function reconcileRevision(
  state: SyncState,
  incoming: { revision: number; projectJson: string },
): { state: SyncState; action: ReconcileAction } {
  if (incoming.revision === state.revision && incoming.projectJson === state.knownJson) {
    if (state.pendingSnapshots.length === 0) {
      return { state, action: 'none' };
    }
    return { state: { ...state, pendingSnapshots: [] }, action: 'remount' };
  }
  if (state.pendingSnapshots.includes(incoming.projectJson)) {
    return {
      state: {
        revision: incoming.revision,
        knownJson: incoming.projectJson,
        pendingSnapshots: state.pendingSnapshots.slice(1),
      },
      action: 'ack',
    };
  }
  return {
    state: { revision: incoming.revision, knownJson: incoming.projectJson, pendingSnapshots: [] },
    action: 'remount',
  };
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
