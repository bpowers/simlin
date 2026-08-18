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
import {
  initialSyncState,
  optimisticVersionAfterSave,
  readTraits,
  reconcileRevision,
  recordSentSnapshot,
  resolveTheme,
  TRAITS,
  wrapperStyle,
  type SyncState,
  type WidgetTraits,
} from './widget-core';

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
  // revision number (defensive) still gets a fresh Editor.
  generation: number;
}

interface WidgetRefs {
  sync: SyncState;
  selectionTimer: ReturnType<typeof setTimeout> | null;
  noticeTimer: ReturnType<typeof setTimeout> | null;
}

/**
 * The host chrome-theme signals, read at render time. Only what
 * `resolveTheme` needs; kept out of widget-core so that file stays DOM-free.
 */
function hostThemeSignals(): { jpThemeLight?: string; prefersDark: boolean } {
  const body = typeof document !== 'undefined' ? document.body : null;
  const jpThemeLight = body?.dataset.jpThemeLight;
  const prefersDark =
    typeof window !== 'undefined' && typeof window.matchMedia === 'function'
      ? window.matchMedia('(prefers-color-scheme: dark)').matches
      : false;
  return { jpThemeLight, prefersDark };
}

export function WidgetApp({ model, name }: { model: AnyModel; name: string }): React.ReactElement {
  const readModelTraits = React.useCallback((): WidgetTraits => readTraits((key) => model.get(key)), [model]);

  const [traits, setTraits] = React.useState<WidgetTraits>(readModelTraits);
  const [seed, setSeed] = React.useState<EditorSeed>(() => ({
    json: traits.projectJson,
    revision: traits.revision,
    generation: 0,
  }));
  const [noticeVisible, setNoticeVisible] = React.useState<boolean>(traits.notice !== '');

  const refs = React.useRef<WidgetRefs>({
    sync: initialSyncState(traits.revision),
    selectionTimer: null,
    noticeTimer: null,
  });

  // Kernel pushes. Only the kernel sets `revision`, so its change event is
  // never an echo of one of our own model.set calls, which makes it the one
  // signal that decides remount-vs-keep (see reconcileRevision). The other
  // traits just re-render.
  React.useEffect(() => {
    const r = refs.current;
    const onRevision = (): void => {
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
    const onNotice = (): void => {
      const next = readModelTraits();
      setTraits(next);
      if (r.noticeTimer !== null) {
        clearTimeout(r.noticeTimer);
        r.noticeTimer = null;
      }
      if (next.notice === '') {
        setNoticeVisible(false);
        return;
      }
      setNoticeVisible(true);
      r.noticeTimer = setTimeout(() => {
        r.noticeTimer = null;
        setNoticeVisible(false);
      }, NOTICE_TIMEOUT_MS);
    };
    model.on(`change:${TRAITS.revision}`, onRevision);
    model.on(`change:${TRAITS.height}`, onOther);
    model.on(`change:${TRAITS.theme}`, onOther);
    model.on(`change:${TRAITS.readOnly}`, onOther);
    model.on(`change:${TRAITS.notice}`, onNotice);
    return () => {
      model.off(`change:${TRAITS.revision}`, onRevision);
      model.off(`change:${TRAITS.height}`, onOther);
      model.off(`change:${TRAITS.theme}`, onOther);
      model.off(`change:${TRAITS.readOnly}`, onOther);
      model.off(`change:${TRAITS.notice}`, onNotice);
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
  // if that base is still current (see the design plan, Section 3).
  const handleSave = React.useCallback(
    async (project: JsonProjectData, currVersion: number): Promise<number | undefined> => {
      if (project.format !== 'json') {
        return undefined;
      }
      refs.current.sync = recordSentSnapshot(refs.current.sync, project.data);
      model.set(TRAITS.projectJson, project.data);
      model.set(TRAITS.pendingBase, currVersion);
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

  const theme = resolveTheme(traits.theme, hostThemeSignals());

  return (
    <div className={styles.host} style={wrapperStyle(traits.height)} data-theme={theme} data-lm-suppress-shortcuts="">
      {noticeVisible && traits.notice !== '' ? (
        <div className={styles.notice} role="status" aria-live="polite">
          {traits.notice}
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
