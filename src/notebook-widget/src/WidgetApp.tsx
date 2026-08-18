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
  initialSyncState,
  optimisticVersionAfterSave,
  parseNoticeMessage,
  readTraits,
  reconcileRevision,
  recordSentSnapshot,
  resolveTheme,
  TRAITS,
  wrapperStyle,
  type Notice,
  type SyncState,
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

interface EditorSeed {
  json: string;
  revision: number;
  // Bumped on every kernel-originated remount so a remount at an unchanged
  // revision number (a rejected snapshot re-seeding us) still gets a fresh
  // Editor -- `revision` alone would not change the key.
  generation: number;
}

interface WidgetRefs {
  sync: SyncState;
  // True while handleSave is inside its own synchronous model.set calls, so
  // the change listeners can tell the widget's own writes from kernel pushes
  // (Backbone fires change events synchronously from set()).
  selfSet: boolean;
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
    json: traits.projectJson,
    revision: traits.revision,
    generation: 0,
  }));
  const [notice, setNotice] = React.useState<Notice | null>(null);
  const [hostTheme, setHostTheme] = React.useState<HostThemeSignals>(readHostThemeSignals);

  const refs = React.useRef<WidgetRefs>({
    sync: initialSyncState(traits.revision, traits.projectJson),
    selfSet: false,
    selectionTimer: null,
    noticeTimer: null,
  });

  // Kernel pushes. `revision` and `project_json` travel in ONE kernel message
  // but surface as up to two change events (Backbone fires per changed key,
  // and skips a key whose value did not change -- an accepted snapshot echoes
  // the exact bytes we already hold, so only `change:revision` fires then).
  // Both handlers therefore read the current pair and run the idempotent
  // reconcile; the widget's own synchronous sets are skipped via `selfSet`.
  React.useEffect(() => {
    const r = refs.current;
    const onKernelState = (): void => {
      if (r.selfSet) {
        return;
      }
      const next = readModelTraits();
      const { state, action } = reconcileRevision(r.sync, {
        revision: next.revision,
        projectJson: next.projectJson,
      });
      r.sync = state;
      setTraits(next);
      if (action === 'remount') {
        setSeed((prev) => ({ json: next.projectJson, revision: next.revision, generation: prev.generation + 1 }));
      }
    };
    const onOther = (): void => {
      setTraits(readModelTraits());
    };
    const onCustom = (...args: unknown[]): void => {
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
      if (r.selectionTimer !== null) {
        clearTimeout(r.selectionTimer);
        r.selectionTimer = null;
      }
      if (r.noticeTimer !== null) {
        clearTimeout(r.noticeTimer);
        r.noticeTimer = null;
      }
    };
  }, [model, readModelTraits]);

  // Editor autosave -> kernel. One coalesced sync message carries the whole
  // snapshot and the revision it was edited from; the kernel accepts it only
  // if that base is still current. A snapshot equal to the trait's current
  // value would produce NO comm message (ipywidgets sends only changed keys),
  // so no echo would ever come back: return undefined so the controller does
  // not advance its acknowledged version for a save that never happened.
  const handleSave = React.useCallback(
    async (project: JsonProjectData, currVersion: number): Promise<number | undefined> => {
      if (project.format !== 'json') {
        return undefined;
      }
      if (project.data === model.get(TRAITS.projectJson)) {
        return undefined;
      }
      const r = refs.current;
      r.sync = recordSentSnapshot(r.sync, project.data);
      r.selfSet = true;
      try {
        model.set(TRAITS.projectJson, project.data);
        model.set(TRAITS.pendingBase, currVersion);
      } finally {
        r.selfSet = false;
      }
      model.save_changes();
      return optimisticVersionAfterSave(currVersion);
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
        initialProjectJson={seed.json}
        initialProjectVersion={seed.revision}
        name={name}
        readOnlyMode={traits.readOnly}
        onSave={handleSave}
        onSelectionChanged={handleSelectionChanged}
      />
    </div>
  );
}
