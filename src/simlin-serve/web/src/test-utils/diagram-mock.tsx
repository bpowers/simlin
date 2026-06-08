// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Lightweight mock for @simlin/diagram so that <EditorHost> tests don't try
// to spin up the WASM engine. The mock records the props it was last called
// with on a static field for assertions; the real component is exercised in
// the e2e/manual smoke path documented in the implementation plan.

import * as React from 'react';

type EditorMockProps = Readonly<{
  inputFormat: 'json' | 'protobuf';
  initialProjectJson?: string;
  initialProjectVersion: number;
  name: string;
  embedded?: boolean;
  readOnlyMode?: boolean;
  onSave: (...args: ReadonlyArray<unknown>) => Promise<number | undefined>;
  onSelectionChanged?: (idents: ReadonlyArray<string>) => void;
}>;

// Carries the `lastProps` recording slot tests read via `Editor.lastProps`.
interface EditorMock {
  (props: EditorMockProps): React.ReactElement;
  lastProps: EditorMockProps | null;
}

// Converted from a class to a function component to match the codebase's
// function-component convention. The recording slot stays a property on the
// component function (was a static class field), assigned during render just
// as the class's render() did, so it always reflects the most recently
// committed props.
export const Editor: EditorMock = (props: EditorMockProps): React.ReactElement => {
  Editor.lastProps = props;
  return (
    <div
      data-testid="editor-mock"
      data-name={props.name}
      data-embedded={String(props.embedded ?? false)}
      data-read-only={String(props.readOnlyMode ?? false)}
      data-input-format={props.inputFormat}
    />
  );
};
Editor.lastProps = null;
