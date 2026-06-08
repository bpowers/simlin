// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import IconButton from './components/IconButton';
import { RedoIcon, UndoIcon } from './components/icons';

import styles from './UndoRedoBar.module.css';

interface UndoRedoBarProps {
  undoEnabled: boolean;
  redoEnabled: boolean;
  onUndoRedo: (kind: 'undo' | 'redo') => void;
}

// Memoized: the Editor re-renders on every pan/momentum frame (optimistic view
// updates replace the controller snapshot wholesale), but UndoRedoBar's props
// (enabled flags + the stable bound `onUndoRedo` handler) don't change during a
// pan. React.memo restores the old PureComponent shallow-prop bailout so it
// does not re-render every frame.
export const UndoRedoBar = React.memo(function UndoRedoBar({
  undoEnabled,
  redoEnabled,
  onUndoRedo,
}: UndoRedoBarProps): React.ReactElement {
  const handleUndo = (): void => {
    onUndoRedo('undo');
  };

  const handleRedo = (): void => {
    onUndoRedo('redo');
  };

  return (
    <div className={styles.card}>
      <IconButton disabled={!undoEnabled} aria-label="Undo" onClick={handleUndo}>
        <UndoIcon />
      </IconButton>
      <div className={styles.divider} />
      <IconButton disabled={!redoEnabled} aria-label="Redo" onClick={handleRedo}>
        <RedoIcon />
      </IconButton>
    </div>
  );
});
