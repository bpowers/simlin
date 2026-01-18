// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import IconButton from '@mui/material/IconButton';
import Paper from '@mui/material/Paper';
import RedoIcon from '@mui/icons-material/Redo';
import UndoIcon from '@mui/icons-material/Undo';

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
      <Paper className={styles.card} elevation={2}>
        <IconButton disabled={!undoEnabled} aria-label="Undo" onClick={this.handleUndo}>
          <UndoIcon />
        </IconButton>
        <div className={styles.divider} />
        <IconButton disabled={!redoEnabled} aria-label="Redo" onClick={this.handleRedo}>
          <RedoIcon />
        </IconButton>
      </Paper>
    );
  }
}
