// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { NewProjectButton } from './NewProjectButton';

type EmptyStateProps = Readonly<{
  // Invoked with the new file's relative path when the user creates a
  // model from this empty-directory affordance. App selects the new
  // path so the editor opens immediately.
  onCreated: (path: string) => void;
}>;

export class EmptyState extends React.Component<EmptyStateProps> {
  render(): React.ReactNode {
    return (
      <div className="serve-empty-state">
        <h2>No models found</h2>
        <p>
          simlin-serve scans the current directory tree for <code>.stmx</code>, <code>.xmile</code>, and{' '}
          <code>.mdl</code> files. Drop one in or create a new model below.
        </p>
        <NewProjectButton onCreated={this.props.onCreated} />
      </div>
    );
  }
}
