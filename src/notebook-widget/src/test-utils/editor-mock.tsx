// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Stand-in for @simlin/diagram/Editor in the shell tests: records the props of
// every mount and exposes buttons that drive the two host callbacks, so the
// tests exercise the widget's onSave / onSelectionChanged wiring without the
// WASM engine. The real Editor runs in the Playwright journey (e2e/).

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
  // The controller-side "server version" this mount currently believes in,
  // advanced by onSave's return value exactly like ProjectController does.
  serverVersion: number;
}

export const mounts: MountRecord[] = [];
export function resetEditorMock(): void {
  mounts.length = 0;
}

export function Editor(props: EditorMockProps): React.ReactElement {
  const record = React.useMemo<MountRecord>(() => {
    const r = { props, serverVersion: props.initialProjectVersion };
    mounts.push(r);
    return r;
  }, []);
  record.props = props;
  const [saves, setSaves] = React.useState(0);
  const save = async (): Promise<void> => {
    // Mirror ProjectController.save: send the acknowledged version, adopt the
    // returned one.
    const json = JSON.stringify({ edited: saves + 1, from: props.initialProjectJson });
    const next = await props.onSave({ format: 'json', data: json }, record.serverVersion);
    if (next) {
      record.serverVersion = next;
    }
    setSaves(saves + 1);
  };
  return (
    <div
      data-testid="editor-mock"
      data-initial-json={props.initialProjectJson}
      data-initial-version={props.initialProjectVersion}
      data-read-only={String(props.readOnlyMode ?? false)}
      data-server-version={record.serverVersion}
    >
      <button type="button" onClick={() => void save()}>
        save
      </button>
      <button type="button" onClick={() => props.onSelectionChanged?.(['a', 'b'])}>
        select
      </button>
    </div>
  );
}
