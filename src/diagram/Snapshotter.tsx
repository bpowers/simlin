// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import IconButton from '@material-ui/core/IconButton';
import Paper from '@material-ui/core/Paper';
import { createStyles, withStyles, WithStyles } from '@material-ui/core/styles';
import PhotoCamera from '@material-ui/icons/PhotoCamera';

const styles = createStyles({
  card: {
    height: 36,
    marginRight: 8,
  },
  snapshotButton: {
    paddingTop: 6,
  },
});

interface SnapshotterPropsFull extends WithStyles<typeof styles> {
  onSnapshot: (kind: 'show' | 'close') => void;
}

// export type UndoRedoProps = Pick<UndoRedoBarPropsFull, 'undoEnabled' | 'redoEnabled' | 'onUndoRedo'>;

export const Snapshotter = withStyles(styles)(
  class InnerVariableDetails extends React.PureComponent<SnapshotterPropsFull> {
    handleSnapshot = () => {
      this.props.onSnapshot('show');
    };

    render() {
      const { classes } = this.props;

      return (
        <Paper className={classes.card} elevation={2}>
          <IconButton className={classes.snapshotButton} aria-label="Snapshot" onClick={this.handleSnapshot}>
            <PhotoCamera />
          </IconButton>
        </Paper>
      );
    }
  },
);
