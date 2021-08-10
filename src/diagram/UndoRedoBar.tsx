// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import IconButton from '@material-ui/core/IconButton';
import Paper from '@material-ui/core/Paper';
import { createStyles, withStyles, WithStyles, Theme } from '@material-ui/core/styles';
import RedoIcon from '@material-ui/icons/Redo';
import UndoIcon from '@material-ui/icons/Undo';

const styles = ({ spacing }: Theme) =>
  createStyles({
    card: {
      height: 36,
      marginRight: spacing(1),
    },
    divider: {
      display: 'inline-block',
      position: 'absolute',
      top: 0,
      left: 0,
      marginLeft: 44,
      marginTop: 10,
      height: 16,
      borderLeftWidth: 1,
      borderLeftStyle: 'solid',
      borderColor: '#ddd',
    },
    undoButton: {
      paddingTop: 6,
      paddingRight: 9,
    },
    redoButton: {
      paddingTop: 6,
      paddingLeft: 9,
    },
  });

interface UndoRedoBarPropsFull extends WithStyles<typeof styles> {
  undoEnabled: boolean;
  redoEnabled: boolean;
  onUndoRedo: (kind: 'undo' | 'redo') => void;
}

// export type UndoRedoProps = Pick<UndoRedoBarPropsFull, 'undoEnabled' | 'redoEnabled' | 'onUndoRedo'>;

export const UndoRedoBar = withStyles(styles)(
  class InnerVariableDetails extends React.PureComponent<UndoRedoBarPropsFull> {
    handleUndo = () => {
      this.props.onUndoRedo('undo');
    };

    handleRedo = () => {
      this.props.onUndoRedo('redo');
    };

    render() {
      const { undoEnabled, redoEnabled, classes } = this.props;

      return (
        <Paper className={classes.card} elevation={2}>
          <IconButton
            disabled={!undoEnabled}
            className={classes.undoButton}
            aria-label="Undo"
            onClick={this.handleUndo}
          >
            <UndoIcon />
          </IconButton>
          <div className={classes.divider} />
          <IconButton
            disabled={!redoEnabled}
            className={classes.redoButton}
            aria-label="Redo"
            onClick={this.handleRedo}
          >
            <RedoIcon />
          </IconButton>
        </Paper>
      );
    }
  },
);
