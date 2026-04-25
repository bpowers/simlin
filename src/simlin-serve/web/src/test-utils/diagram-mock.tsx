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
}>;

export class Editor extends React.Component<EditorMockProps> {
  static lastProps: EditorMockProps | null = null;

  render(): React.ReactNode {
    Editor.lastProps = this.props;
    return (
      <div
        data-testid="editor-mock"
        data-name={this.props.name}
        data-embedded={String(this.props.embedded ?? false)}
        data-read-only={String(this.props.readOnlyMode ?? false)}
        data-input-format={this.props.inputFormat}
      />
    );
  }
}
