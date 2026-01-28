// Copyright 2021 The Simlin Authors. All rights reserved.
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

export class UndoRedoBar extends React.PureComponent<UndoRedoBarProps> {
  handleUndo = () => {
    this.props.onUndoRedo('undo');
  };

  handleRedo = () => {
    this.props.onUndoRedo('redo');
  };

  render() {
    const { undoEnabled, redoEnabled } = this.props;

    return (
      <div className={styles.card}>
        <IconButton disabled={!undoEnabled} aria-label="Undo" onClick={this.handleUndo}>
          <UndoIcon />
        </IconButton>
        <div className={styles.divider} />
        <IconButton disabled={!redoEnabled} aria-label="Redo" onClick={this.handleRedo}>
          <RedoIcon />
        </IconButton>
      </div>
    );
  }
}
