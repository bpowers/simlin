// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import clsx from 'clsx';
import { styled } from '@mui/material/styles';
import IconButton from '@mui/material/IconButton';
import Paper from '@mui/material/Paper';
import RedoIcon from '@mui/icons-material/Redo';
import UndoIcon from '@mui/icons-material/Undo';

interface UndoRedoBarProps {
  undoEnabled: boolean;
  redoEnabled: boolean;
  onUndoRedo: (kind: 'undo' | 'redo') => void;
}

export const UndoRedoBar = styled(
  class InnerVariableDetails extends React.PureComponent<UndoRedoBarProps & { className?: string }> {
    handleUndo = () => {
      this.props.onUndoRedo('undo');
    };

    handleRedo = () => {
      this.props.onUndoRedo('redo');
    };

    render() {
      const { undoEnabled, redoEnabled, className } = this.props;

      return (
        <Paper className={clsx(className, 'simlin-undoredobar-card')} elevation={2}>
          <IconButton disabled={!undoEnabled} aria-label="Undo" onClick={this.handleUndo}>
            <UndoIcon />
          </IconButton>
          <div className="simlin-undoredobar-divider" />
          <IconButton disabled={!redoEnabled} aria-label="Redo" onClick={this.handleRedo}>
            <RedoIcon />
          </IconButton>
        </Paper>
      );
    }
  },
)(({ theme }) => ({
  '&.simlin-undoredobar-card': {
    height: 40,
    marginRight: theme.spacing(1),
  },
  '.simlin-undoredobar-divider': {
    display: 'inline-block',
    position: 'absolute',
    top: 0,
    left: 0,
    marginLeft: 40,
    marginTop: 12,
    height: 16,
    borderLeftWidth: 1,
    borderLeftStyle: 'solid',
    borderColor: '#ddd',
  },
}));
