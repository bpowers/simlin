// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Stand-in for @simlin/diagram/Editor in the shell tests: records the props of
// every mount and reproduces the ONE piece of controller behaviour the save
// protocol leans on -- ProjectController.save()'s serialisation: a save while
// one is in flight queues exactly one flush; each flush serialises the LATEST
// local state and sends the live acknowledged version; a resolved version
// advances it, undefined leaves it. The buttons drive edits and selection so
// the tests exercise the widget's onSave / onSelectionChanged wiring without
// the WASM engine. The real Editor runs in the Playwright journey (e2e/).

import * as React from 'react';

export type JsonProjectData = { format: 'json'; data: string };

export interface EditorMockProps {
  inputFormat: 'json';
  initialProjectJson: string;
  initialProjectVersion: number;
  name: string;
  readOnlyMode?: boolean;
  onSave: (project: JsonProjectData, currVersion: number) => Promise<number | undefined>;
  onSelectionChanged?: (idents: string[]) => void;
}

interface MountRecord {
  props: EditorMockProps;
  // The controller-side acknowledged version, exactly as ProjectController
  // tracks serverVersion.
  serverVersion: number;
  // Every value onSave resolved with, in order.
  saveResults: Array<number | undefined>;
  // Local edit counter: the "content" of this mount's project.
  edits: number;
  inSave: boolean;
  saveQueued: boolean;
}

export const mounts: MountRecord[] = [];
export function resetEditorMock(): void {
  mounts.length = 0;
}

/** The snapshot a mount produces for its current local state. */
export function localJson(initial: string, edits: number): string {
  return JSON.stringify({ edited: edits, from: initial });
}

export function Editor(props: EditorMockProps): React.ReactElement {
  const record = React.useMemo<MountRecord>(() => {
    const r: MountRecord = {
      props,
      serverVersion: props.initialProjectVersion,
      saveResults: [],
      edits: 0,
      inSave: false,
      saveQueued: false,
    };
    mounts.push(r);
    return r;
  }, []);
  record.props = props;
  const [, rerender] = React.useState(0);

  // ProjectController.save() semantics.
  const save = async (): Promise<void> => {
    if (record.inSave) {
      record.saveQueued = true;
      return;
    }
    record.inSave = true;
    try {
      const json = localJson(record.props.initialProjectJson, record.edits);
      const next = await record.props.onSave({ format: 'json', data: json }, record.serverVersion);
      record.saveResults.push(next);
      if (next) {
        record.serverVersion = next;
      }
    } finally {
      record.inSave = false;
      rerender((n) => n + 1);
      if (record.saveQueued) {
        record.saveQueued = false;
        await save();
      }
    }
  };

  // A discrete edit: bump local content and autosave (scheduleSave).
  const edit = (): void => {
    record.edits += 1;
    void save();
  };

  return (
    <div
      data-testid="editor-mock"
      data-initial-json={props.initialProjectJson}
      data-initial-version={props.initialProjectVersion}
      data-read-only={String(props.readOnlyMode ?? false)}
      data-server-version={record.serverVersion}
    >
      <button type="button" onClick={edit}>
        edit
      </button>
      <button type="button" onClick={() => props.onSelectionChanged?.(['a', 'b'])}>
        select
      </button>
    </div>
  );
}
