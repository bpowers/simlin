// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * The React shell between the anywidget model and `@simlin/diagram`'s Editor.
 * It owns no project state: the kernel seeds `project_json`/`revision`, the
 * Editor edits locally and autosaves whole-project snapshots back through
 * `onSave`, and every decision (remount vs. keep, theme, wrapper size) is a
 * pure function in widget-core.ts applied here.
 */

import * as React from 'react';

import { Editor } from '@simlin/diagram/Editor';
import type { JsonProjectData } from '@simlin/diagram/Editor';

import type { AnyModel } from './anywidget-model';
import styles from './widget.module.css';
import { WIDGET_ROOT_CLASS } from './widget-root-class';
import {
  classifyPush,
  inFlightFor,
  parseNoticeMessage,
  parseSaveReply,
  readTraits,
  resolveTheme,
  seedAfterSaved,
  snapshotMessage,
  TRAITS,
  versionAfterReply,
  wrapperStyle,
  type EditorSeedPair,
  type InFlightSnapshot,
  type Notice,
  type WidgetTraits,
} from './widget-core';

export { WIDGET_ROOT_CLASS };

/** How long a kernel notice stays visible before it fades on its own. */
export const NOTICE_TIMEOUT_MS = 5000;

/**
 * Coalesce a burst of selection changes into one trait sync (same value as
 * simlin-serve's EditorHost): a box-select fires per drag frame.
 */
export const SELECTION_DEBOUNCE_MS = 150;

interface EditorSeed extends EditorSeedPair {
  // Bumped on every kernel-originated remount so a remount whose revision
  // number is unchanged (a reject re-seed at the same revision, a disk reload
  // that restored an older revision number) still gets a fresh Editor --
  // `revision` alone would not change the key.
  generation: number;
}

interface WidgetRefs {
  // Set by the effect cleanup: a save that arrives after unmount (a queued
  // controller flush racing the dispose) is refused, not sent.
  disposed: boolean;
  // The pair the live Editor was seeded from; the reference for classifyPush
  // and for making remounts idempotent.
  seed: EditorSeedPair;
  // At most one snapshot in flight (see handleSave); null between saves.
  inFlight: (InFlightSnapshot & { resolve: (version: number | undefined) => void }) | null;
  selectionTimer: ReturnType<typeof setTimeout> | null;
  noticeTimer: ReturnType<typeof setTimeout> | null;
}

/**
 * The host chrome-theme signals `resolveTheme` needs. Read at mount and
 * re-read whenever JupyterLab flips `body[data-jp-theme-light]` or the OS
 * color scheme changes; kept out of widget-core so that file stays DOM-free.
 */
interface HostThemeSignals {
  jpThemeLight?: string;
  prefersDark: boolean;
}

const DARK_SCHEME_QUERY = '(prefers-color-scheme: dark)';

function matchDarkScheme(): MediaQueryList | null {
  return typeof window !== 'undefined' && typeof window.matchMedia === 'function'
    ? window.matchMedia(DARK_SCHEME_QUERY)
    : null;
}

function readHostThemeSignals(): HostThemeSignals {
  const body = typeof document !== 'undefined' ? document.body : null;
  return { jpThemeLight: body?.dataset.jpThemeLight, prefersDark: matchDarkScheme()?.matches ?? false };
}

/**
 * Subscribe to both theme signals; returns the unsubscribe. `matchMedia` may
 * be absent (some embedded webviews), in which case only the JupyterLab
 * attribute is watched.
 */
function watchHostThemeSignals(onChange: () => void): () => void {
  const observer = new MutationObserver(onChange);
  observer.observe(document.body, { attributes: true, attributeFilter: ['data-jp-theme-light'] });
  const mql = matchDarkScheme();
  mql?.addEventListener('change', onChange);
  return () => {
    observer.disconnect();
    mql?.removeEventListener('change', onChange);
  };
}

export function WidgetApp({ model, name }: { model: AnyModel; name: string }): React.ReactElement {
  const readModelTraits = React.useCallback((): WidgetTraits => readTraits((key) => model.get(key)), [model]);

  const [traits, setTraits] = React.useState<WidgetTraits>(readModelTraits);
  const [seed, setSeed] = React.useState<EditorSeed>(() => ({
    projectJson: traits.projectJson,
    revision: traits.revision,
    generation: 0,
  }));
  const [notice, setNotice] = React.useState<Notice | null>(null);
  const [hostTheme, setHostTheme] = React.useState<HostThemeSignals>(readHostThemeSignals);

  const refs = React.useRef<WidgetRefs>({
    disposed: false,
    seed: { revision: traits.revision, projectJson: traits.projectJson },
    inFlight: null,
    selectionTimer: null,
    noticeTimer: null,
  });

  // Remount the Editor on the kernel-authoritative pair. Idempotent on the
  // pair: a push and the `rejected` that follows it (or the two change events
  // of one hold_sync) remount once.
  const remountFrom = React.useCallback((pair: EditorSeedPair): void => {
    const r = refs.current;
    if (pair.revision === r.seed.revision && pair.projectJson === r.seed.projectJson) {
      return;
    }
    r.seed = { revision: pair.revision, projectJson: pair.projectJson };
    setSeed((prev) => ({ revision: pair.revision, projectJson: pair.projectJson, generation: prev.generation + 1 }));
  }, []);

  // Kernel pushes. Only the kernel writes `project_json` and `revision`, so
  // every change event on them is a kernel push. The two travel in ONE
  // hold_sync but surface as up to two change events (Backbone fires per
  // changed key), so both handlers read the FINAL pair and classify it
  // idempotently.
  React.useEffect(() => {
    const r = refs.current;
    const onKernelState = (): void => {
      const next = readModelTraits();
      setTraits(next);
      const incoming = { revision: next.revision, projectJson: next.projectJson };
      const action = classifyPush(r.seed, r.inFlight, incoming);
      if (action === 'own-ack') {
        // Our snapshot's state; the `saved` reply resolves the save. Adopt
        // it as the seed so a later identical push is `none`, no remount.
        r.seed = incoming;
        return;
      }
      if (action === 'remount') {
        remountFrom(incoming);
      }
    };
    const onOther = (): void => {
      setTraits(readModelTraits());
    };
    const onCustom = (...args: unknown[]): void => {
      const reply = parseSaveReply(args[0]);
      if (reply !== null) {
        const flight = r.inFlight;
        if (flight === null) {
          // A reply for a snapshot this view no longer tracks (another view of
          // the same model, or a reply after unmount/abort resolved it): the
          // traits already carry the authoritative state; nothing to do.
          return;
        }
        r.inFlight = null;
        if (reply.kind === 'saved') {
          // Adopt the accepted state as the seed NOW, before resolving and
          // independently of the kernel's trait push: if the push arrived
          // first it classified `own-ack` and adopted the same pair; if it
          // arrives later (a host that dispatches the custom message before
          // applying the state update, or a kernel that sends `saved` early)
          // it now classifies `none` instead of remounting and discarding
          // local edits.
          r.seed = seedAfterSaved(flight, reply.revision);
        } else {
          // Rejected -- or a malformed reply, which is treated as a reject
          // rather than ignored (every snapshot gets exactly one reply; an
          // ignored one would hang the controller's save queue). The kernel's
          // traits are authoritative: re-seed the Editor from them so its
          // local state and version match the kernel's again. When the
          // kernel's state moved, its push already remounted us and this is a
          // no-op (remountFrom is idempotent on the pair); when it did not,
          // the pair equals the seed and this is a no-op too -- the reject
          // costs the local edits only in the moved case.
          const t = readModelTraits();
          remountFrom({ revision: t.revision, projectJson: t.projectJson });
        }
        flight.resolve(versionAfterReply(reply));
        return;
      }
      const parsed = parseNoticeMessage(args[0]);
      if (parsed === null) {
        return;
      }
      if (r.noticeTimer !== null) {
        clearTimeout(r.noticeTimer);
      }
      setNotice(parsed);
      r.noticeTimer = setTimeout(() => {
        r.noticeTimer = null;
        setNotice(null);
      }, NOTICE_TIMEOUT_MS);
    };
    const onHostTheme = (): void => {
      setHostTheme(readHostThemeSignals());
    };
    model.on(`change:${TRAITS.revision}`, onKernelState);
    model.on(`change:${TRAITS.projectJson}`, onKernelState);
    model.on(`change:${TRAITS.height}`, onOther);
    model.on(`change:${TRAITS.theme}`, onOther);
    model.on(`change:${TRAITS.readOnly}`, onOther);
    model.on('msg:custom', onCustom);
    const unwatchTheme = watchHostThemeSignals(onHostTheme);
    return () => {
      model.off(`change:${TRAITS.revision}`, onKernelState);
      model.off(`change:${TRAITS.projectJson}`, onKernelState);
      model.off(`change:${TRAITS.height}`, onOther);
      model.off(`change:${TRAITS.theme}`, onOther);
      model.off(`change:${TRAITS.readOnly}`, onOther);
      model.off('msg:custom', onCustom);
      unwatchTheme();
      r.disposed = true;
      // Nothing may dangle past unmount: an in-flight save resolves
      // undefined (the controller is being disposed anyway).
      if (r.inFlight !== null) {
        const flight = r.inFlight;
        r.inFlight = null;
        flight.resolve(undefined);
      }
      if (r.selectionTimer !== null) {
        clearTimeout(r.selectionTimer);
        r.selectionTimer = null;
      }
      if (r.noticeTimer !== null) {
        clearTimeout(r.noticeTimer);
        r.noticeTimer = null;
      }
    };
  }, [model, readModelTraits, remountFrom]);

  // Editor autosave -> kernel. The whole snapshot rides in a `snapshot` custom
  // message with the revision it was edited from (`base`); the promise
  // resolves ONLY when the kernel answers `saved` (the new revision) or
  // `rejected` (undefined). ProjectController serialises saves -- one in
  // flight, one queued flush that re-reads the acknowledged version -- so at
  // most one snapshot is ever in flight from this view, and a busy kernel
  // (long-running cell) means the Editor keeps working locally and one flush
  // of the LATEST state goes out when the answer arrives. Deliberately no
  // timeout: a long cell legitimately delays the reply and a timeout would
  // misfire into a spurious failure; unmount resolves it instead.
  const handleSave = React.useCallback(
    (project: JsonProjectData, currVersion: number): Promise<number | undefined> => {
      if (project.format !== 'json') {
        return Promise.resolve(undefined);
      }
      const r = refs.current;
      if (r.disposed) {
        // A queued controller flush racing the unmount: nothing may be sent
        // after cleanup (no listener would ever resolve it).
        return Promise.resolve(undefined);
      }
      if (r.inFlight !== null) {
        // The controller never does this; if it ever did, two snapshots in
        // flight would make the replies ambiguous. Refuse rather than guess.
        return Promise.resolve(undefined);
      }
      return new Promise<number | undefined>((resolve) => {
        r.inFlight = { ...inFlightFor(currVersion, project.data), resolve };
        model.send(snapshotMessage(currVersion, project.data));
      });
    },
    [model],
  );

  const handleSelectionChanged = React.useCallback(
    (idents: ReadonlyArray<string>): void => {
      const r = refs.current;
      if (r.selectionTimer !== null) {
        clearTimeout(r.selectionTimer);
      }
      r.selectionTimer = setTimeout(() => {
        r.selectionTimer = null;
        model.set(TRAITS.selection, [...idents]);
        model.save_changes();
      }, SELECTION_DEBOUNCE_MS);
    },
    [model],
  );

  const theme = resolveTheme(traits.theme, hostTheme);

  return (
    <div
      className={`${WIDGET_ROOT_CLASS} ${styles.host}`}
      style={wrapperStyle(traits.height)}
      data-theme={theme}
      data-lm-suppress-shortcuts=""
    >
      {notice !== null ? (
        <div
          className={notice.level === 'warn' ? `${styles.notice} ${styles.noticeWarn}` : styles.notice}
          role="status"
          aria-live="polite"
        >
          {notice.text}
        </div>
      ) : null}
      <Editor
        key={`${seed.revision}#${seed.generation}`}
        inputFormat="json"
        initialProjectJson={seed.projectJson}
        initialProjectVersion={seed.revision}
        name={name}
        readOnlyMode={traits.readOnly}
        onSave={handleSave}
        onSelectionChanged={handleSelectionChanged}
      />
    </div>
  );
}
