// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

// Phase 1 deliberately omits the "Create new model" affordance — that arrives
// in Phase 8. The empty state still ships so users have a hint about what
// `simlin-serve` is looking for.

export class EmptyState extends React.Component {
  render(): React.ReactNode {
    return (
      <div className="serve-empty-state">
        <h2>No models found</h2>
        <p>
          simlin-serve scans the current directory tree for <code>.stmx</code>, <code>.xmile</code>, and{' '}
          <code>.mdl</code> files. Drop one in or run the server in a different directory.
        </p>
      </div>
    );
  }
}
