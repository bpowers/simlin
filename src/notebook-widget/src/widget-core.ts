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
  maxSnapshotBytes: 'max_snapshot_bytes',
} as const;

export const DEFAULT_HEIGHT_PX = 600;

/**
 * Default cap on the snapshot `onSave` will send to the kernel, measured AS
 * IT RIDES IN THE MESSAGE: the UTF-8 bytes of the snapshot text
 * JSON-string-escaped (`snapshotWireSize`), which is how it sits in the comm
 * envelope. The kernel's `max_snapshot_bytes` trait carries the value in
 * force and this is what a missing/invalid trait falls back to. MUST equal
 * `MAX_SNAPSHOT_BYTES` in pysimlin's `simlin/_widget_core.py`, which holds
 * the full rationale: the notebook server drops browser->kernel websocket
 * messages above tornado's `websocket_max_message_size` (10 MiB by default)
 * by closing the connection, so a snapshot that large would never arrive and
 * its save would wait forever. Measuring the raw text would under-count by
 * 7-39% depending on content (every quote and backslash in the project costs
 * an extra byte); on the escaped size, 8 MiB leaves ~2 MiB of headroom under
 * the default regardless of content, the envelope's other fields and the
 * message header being a few hundred bytes. Above the cap `onSave` sends
 * `{type:'oversize', bytes}` instead and the edit is reported as not saved.
 */
export const MAX_SNAPSHOT_BYTES = 8 * 1024 * 1024;

export type Theme = 'auto' | 'light' | 'dark';

/**
 * The model the Editor mounts first (`ProjectController.modelName`), and
 * therefore the one whose first view must exist for anything to render.
 */
export const EDITOR_ROOT_MODEL = 'main';

/**
 * Give the root model of an engine-native project JSON text an empty
 * stock-flow view when it has none, returning the EXACT input text otherwise.
 *
 * The Editor renders `model.views[0]` and is a dead, blank canvas without it.
 * The kernel lays a viewless model out before seeding (pysimlin
 * `Project._ensure_view`); this is the defence in depth for a seed that
 * arrives viewless anyway (the layout failed, an older kernel). Text that is
 * not a repairable project (not JSON, no models list, no root model) is
 * returned as it is -- the Editor reports its own error for that. Returning
 * the identical string when nothing is missing matters: `classifyPush`
 * compares trait text with the bytes the Editor sent, and an Editor snapshot
 * always carries the view once one exists.
 */
export function withEditorView(projectJson: string): string {
  let doc: unknown;
  try {
    doc = JSON.parse(projectJson);
  } catch {
    return projectJson;
  }
  if (!isRecord(doc) || !Array.isArray(doc.models)) {
    return projectJson;
  }
  const index = doc.models.findIndex((m: unknown) => isRecord(m) && m.name === EDITOR_ROOT_MODEL);
  if (index < 0) {
    return projectJson;
  }
  const root = doc.models[index] as Record<string, unknown>;
  if (Array.isArray(root.views) && root.views.length > 0) {
    return projectJson;
  }
  const models = [...doc.models];
  models[index] = {
    ...root,
    views: [{ kind: 'stock_flow', elements: [], viewBox: { x: 0, y: 0, width: 0, height: 0 }, zoom: 1 }],
  };
  return JSON.stringify({ ...doc, models });
}

/** The kernel-owned traits the shell reads, coerced to the types it needs. */
export interface WidgetTraits {
  projectJson: string;
  revision: number;
  height: number;
  theme: Theme;
  readOnly: boolean;
  maxSnapshotBytes: number;
}

/**
 * Coerce raw trait values into {@link WidgetTraits}. Traits arrive untyped
 * (`unknown`) from the model, and a kernel bug or a stale notebook state
 * should degrade to sane defaults rather than a broken widget: a missing or
 * non-positive height becomes {@link DEFAULT_HEIGHT_PX}, an unknown theme
 * becomes `auto`, a non-string project becomes the empty string (which the
 * Editor reports as an error rather than crashing on) and a project whose
 * root model has no view gets an empty one ({@link withEditorView}; applied
 * here so the seed and every later push are normalised the same way and the
 * pair comparisons in `classifyPush` stay consistent), a missing or
 * non-positive snapshot cap becomes {@link MAX_SNAPSHOT_BYTES}.
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
  const capRaw = get(TRAITS.maxSnapshotBytes);
  const maxSnapshotBytes =
    typeof capRaw === 'number' && Number.isInteger(capRaw) && capRaw > 0 ? capRaw : MAX_SNAPSHOT_BYTES;
  return {
    projectJson: typeof projectRaw === 'string' ? withEditorView(projectRaw) : '',
    revision,
    height,
    theme,
    readOnly: get(TRAITS.readOnly) === true,
    maxSnapshotBytes,
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

/**
 * Inline style for the wrapper the Editor's chrome anchors to. It paints the
 * editor's page-background token itself (the same ground `src/app` gives the
 * Editor): the Editor root and canvas are transparent, so without it the
 * canvas shows whatever the notebook cell is -- white under a forced
 * `theme="dark"` in a light JupyterLab, with dark chrome and light-on-dark
 * primitives floating on it. The token is defined on this very element (the
 * scoped theme.css puts `:root` on the widget root class) and flips with its
 * `data-theme`, so the ground always matches the theme the wrapper resolved.
 */
export function wrapperStyle(heightPx: number): {
  position: 'relative';
  height: string;
  width: string;
  background: string;
} {
  return { position: 'relative', height: `${heightPx}px`, width: '100%', background: 'var(--color-background)' };
}

// ---- viewport carried across a remount --------------------------------------

/**
 * A view's viewport as the Editor reports (`onViewportChange`) and accepts
 * (`initialViewport`) it: the pan offset + pixel size of the canvas and the
 * zoom factor. Structurally the Editor's `Viewport`.
 */
export interface Viewport {
  readonly viewBox: { readonly x: number; readonly y: number; readonly width: number; readonly height: number };
  readonly zoom: number;
}

/** The Editor's last committed viewport, with the model it belongs to. */
export interface LiveViewport extends Viewport {
  readonly modelName: string;
}

/**
 * The viewport STORED for the root model's first view in an engine-native
 * project JSON text, with the reader's defaults (`stockFlowViewFromJson`):
 * a missing viewBox is 0/0/0/0 (the writer omits a zero-size box), a missing
 * zoom is 1. `undefined` when the text has no parsable root view at all.
 */
export function storedViewport(projectJson: string): Viewport | undefined {
  let doc: unknown;
  try {
    doc = JSON.parse(projectJson);
  } catch {
    return undefined;
  }
  if (!isRecord(doc) || !Array.isArray(doc.models)) {
    return undefined;
  }
  const root = doc.models.find((m: unknown) => isRecord(m) && m.name === EDITOR_ROOT_MODEL);
  if (!isRecord(root) || !Array.isArray(root.views) || !isRecord(root.views[0])) {
    return undefined;
  }
  const view = root.views[0];
  const box = isRecord(view.viewBox) ? view.viewBox : {};
  const num = (v: unknown, fallback: number): number => (typeof v === 'number' && Number.isFinite(v) ? v : fallback);
  return {
    viewBox: { x: num(box.x, 0), y: num(box.y, 0), width: num(box.width, 0), height: num(box.height, 0) },
    zoom: num(view.zoom, 1),
  };
}

function sameViewport(a: Viewport, b: Viewport): boolean {
  return (
    a.zoom === b.zoom &&
    a.viewBox.x === b.viewBox.x &&
    a.viewBox.y === b.viewBox.y &&
    a.viewBox.width === b.viewBox.width &&
    a.viewBox.height === b.viewBox.height
  );
}

/**
 * The viewport the NEXT Editor mount should open with when a kernel push
 * remounts it: the outgoing Editor's live viewport, so a pan or zoom the user
 * made -- which a pan alone never persists (only the next edit's save carries
 * it) -- survives a Python `edit()` or a disk reload instead of silently
 * resetting to the stored one; and so a project whose stored viewBox is still
 * the unset 0/0/0/0 (a converted model that has never been edited in the
 * browser) keeps the framing the canvas fitted on first display instead of
 * re-centring on the now-larger content every time the kernel adds a variable.
 *
 * Carried only when the kernel change did not itself move the viewport: the
 * incoming project's stored viewport equals the outgoing project's stored one
 * (`outgoingJson`, the seed the live Editor was mounted from), or the incoming
 * one is unset. When the kernel moved it (a Python edit that set the view's
 * viewBox/zoom), the kernel's wins: `undefined`. Nothing is carried when the
 * live viewport belongs to a model other than the root (the user had drilled
 * into a module; the remount opens the root, whose framing that is not).
 */
export function viewportToCarry(
  live: LiveViewport | null,
  outgoingJson: string,
  incomingJson: string,
): Viewport | undefined {
  if (live === null || live.modelName !== EDITOR_ROOT_MODEL) {
    return undefined;
  }
  const carried: Viewport = { viewBox: { ...live.viewBox }, zoom: live.zoom };
  const incoming = storedViewport(incomingJson);
  if (incoming === undefined || incoming.viewBox.width <= 0 || incoming.viewBox.height <= 0) {
    return carried;
  }
  const outgoing = storedViewport(outgoingJson);
  if (outgoing !== undefined && sameViewport(outgoing, incoming)) {
    return carried;
  }
  return undefined;
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

// ---- snapshot size --------------------------------------------------------------

/**
 * Byte length of a snapshot as it rides in the message: the text as a JSON
 * string value (`JSON.stringify` adds the quotes and escapes quotes,
 * backslashes and control characters), in UTF-8 (what the websocket frame
 * carries; the JS string length counts UTF-16 units and does not). Mirrors
 * pysimlin's `snapshot_wire_size` byte for byte: JSON.stringify leaves
 * non-ASCII as UTF-8 exactly as `json.dumps(ensure_ascii=False)` does.
 */
export function snapshotWireSize(json: string): number {
  return new TextEncoder().encode(JSON.stringify(json)).byteLength;
}

/**
 * Round-half-to-even on a rational `numerator / denominator` (both
 * non-negative integers): the rounding Python's `round()` applies, so the
 * figures printed here and by pysimlin's `format_size` are identical.
 * `Math.round` rounds ties up and would print 8.25 MiB as "8.3" against
 * Python's "8.2".
 */
function roundHalfEven(numerator: number, denominator: number): number {
  const quotient = Math.floor(numerator / denominator);
  const remainder2 = 2 * (numerator - quotient * denominator);
  if (remainder2 > denominator) {
    return quotient + 1;
  }
  if (remainder2 < denominator) {
    return quotient;
  }
  return quotient % 2 === 0 ? quotient : quotient + 1;
}

/**
 * `bytes` as a short human figure: KiB below 1 MiB, else MiB to one decimal
 * (whole numbers without it: `8 MiB`, `12.3 MiB`, `512 KiB`), rounding
 * half-to-even like pysimlin's `format_size` -- the toast the widget shows
 * and the notice the kernel sends back must read the same so they collapse
 * into one visible message. The shared fixture list is pinned in both test
 * suites.
 */
export function formatSize(bytes: number): string {
  const MIB = 1024 * 1024;
  if (bytes < MIB) {
    return `${roundHalfEven(bytes, 1024)} KiB`;
  }
  const tenths = roundHalfEven(bytes * 10, MIB);
  return tenths % 10 === 0 ? `${tenths / 10} MiB` : `${Math.floor(tenths / 10)}.${tenths % 10} MiB`;
}

export type SnapshotSizeCheck = { kind: 'ok'; bytes: number } | { kind: 'oversize'; bytes: number; limit: number };

/**
 * Whether a snapshot may be sent under `limit` (the kernel's
 * `max_snapshot_bytes`), measured on its wire size. Exactly at the limit is
 * fine; above it the save is refused up front -- a clear "not saved" instead
 * of a message the server drops and a promise that never resolves.
 */
export function checkSnapshotSize(json: string, limit: number): SnapshotSizeCheck {
  const bytes = snapshotWireSize(json);
  return bytes > limit ? { kind: 'oversize', bytes, limit } : { kind: 'ok', bytes };
}

/** The widget -> kernel report sent INSTEAD of an oversize snapshot; owed no `saved`/`rejected` reply. */
export function oversizeMessage(bytes: number): { type: 'oversize'; bytes: number } {
  return { type: 'oversize', bytes };
}

/** The toast for a refused oversize save; the kernel's own notice uses the same words. */
export function oversizeNotice(bytes: number, limit: number): Notice {
  return {
    level: 'warn',
    text: `Edit not saved: the model is too large for the notebook connection (${formatSize(bytes)} > ${formatSize(limit)} limit); edit it from Python instead.`,
  };
}

/**
 * The kernel's answer to a snapshot: accepted at `revision`, rejected, or a
 * reply-typed message whose payload is unusable (`malformed`: a `saved` /
 * `rejected` with a missing or non-integer revision).
 */
export type SaveReply =
  | { kind: 'saved'; revision: number }
  | { kind: 'rejected'; revision: number }
  | { kind: 'malformed' };

/**
 * Interpret a `msg:custom` delivery as a save reply. Anything that is not
 * reply-typed (wasm, notices, unknown types) is `null`. A reply-typed
 * message with a bad `revision` is `malformed`: the shell treats it as a
 * reject while a snapshot is in flight, because every snapshot gets exactly
 * one reply and ignoring a broken one would hang the Editor's save queue.
 */
export function parseSaveReply(msg: unknown): SaveReply | null {
  if (!isRecord(msg) || (msg.type !== 'saved' && msg.type !== 'rejected')) {
    return null;
  }
  if (typeof msg.revision !== 'number' || !Number.isInteger(msg.revision)) {
    return { kind: 'malformed' };
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

/**
 * The seed pair the Editor is at once the kernel has accepted the in-flight
 * snapshot: its bytes at the revision the kernel reported. Adopted on the
 * `saved` reply itself (not only on the state push) so the two are
 * order-independent: whichever arrives first, the other classifies `none`.
 */
export function seedAfterSaved(inFlight: InFlightSnapshot, revision: number): EditorSeedPair {
  return { revision, projectJson: inFlight.json };
}
