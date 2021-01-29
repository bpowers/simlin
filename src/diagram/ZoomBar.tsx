// Copyright 2021 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import IconButton from '@material-ui/core/IconButton';
import Paper from '@material-ui/core/Paper';
import { createStyles, withStyles, WithStyles, Theme } from '@material-ui/core/styles';
import AddIcon from '@material-ui/icons/Add';
import RemoveIcon from '@material-ui/icons/Remove';

const styles = ({ spacing }: Theme) =>
  createStyles({
    card: {
      height: 36,
      marginRight: spacing(1),
    },
    divider1: {
      display: 'inline-block',
      top: 0,
      left: 0,
      marginRight: 6,
      marginTop: 10,
      height: 16,
      borderLeftWidth: 1,
      borderLeftStyle: 'solid',
      borderColor: '#ddd',
    },
    divider2: {
      display: 'inline-block',
      top: 0,
      left: 0,
      marginLeft: 8,
      marginTop: 10,
      height: 16,
      borderLeftWidth: 1,
      borderLeftStyle: 'solid',
      borderColor: '#ddd',
    },
    zoomOutButton: {
      paddingTop: 6,
      paddingRight: 9,
    },
    zoomInButton: {
      paddingTop: 6,
      paddingLeft: 9,
    },
    zoomText: {
      width: 21,
      fontSize: '.6rem',
      color: '#888',
      textAlign: 'center',
      display: 'inline-block',
      verticalAlign: 4,
      margin: 0,
    },
  });

interface ZoomBarPropsFull extends WithStyles<typeof styles> {
  zoom: number;
  onChangeZoom: (zoom: number) => void;
}

const zooms: Readonly<Array<number>> = [0.2, 0.5, 0.75, 0.9, 1, 1.1, 1.25, 1.5, 2, 2.5, 3];
function snapToZoom(zoom: number): number {
  return zooms.reduce((a, b) => {
    return Math.abs(a - zoom) < Math.abs(b - zoom) ? a : b;
  });
}

const ε = 0.001;
function eq(a: number, b: number): boolean {
  return Math.abs(a - b) < ε;
}

function findNext(zoom: number, dir: 'out' | 'in'): number | undefined {
  // take care of the special cases first
  if (dir === 'out' && (zoom < zooms[0] || eq(zoom, zooms[0]))) {
    return undefined;
  }

  if (dir === 'in' && (zoom > zooms[zooms.length - 1] || eq(zoom, zooms[zooms.length - 1]))) {
    return undefined;
  }

  const snappedZoom = snapToZoom(zoom);
  const snappedIndex = zooms.indexOf(snappedZoom);
  if (
    snappedIndex < 0 ||
    (dir === 'out' && snappedIndex === 0) ||
    (dir === 'in' && snappedIndex === zooms.length - 1)
  ) {
    console.log('problem with zoom logic');
    return undefined;
  }

  return zooms[snappedIndex + (dir === 'in' ? 1 : -1)];
}

// export type UndoRedoProps = Pick<ZoomBarPropsFull, 'undoEnabled' | 'redoEnabled' | 'onUndoRedo'>;

export const ZoomBar = withStyles(styles)(
  class InnerVariableDetails extends React.PureComponent<ZoomBarPropsFull> {
    handleZoomOut = () => {
      const next = findNext(this.props.zoom, 'out');
      if (next) {
        this.props.onChangeZoom(next);
      }
    };

    handleZoomIn = () => {
      const next = findNext(this.props.zoom, 'in');
      if (next) {
        this.props.onChangeZoom(next);
      }
    };

    render() {
      const { classes } = this.props;

      const zoom = snapToZoom(this.props.zoom);

      const zoomInEnabled = zoom < zooms[zooms.length - 1];
      const zoomOutEnabled = zoom > zooms[0];

      return (
        <Paper className={classes.card} elevation={2}>
          <IconButton
            disabled={!zoomOutEnabled}
            className={classes.zoomOutButton}
            aria-label="Zoom Out"
            onClick={this.handleZoomOut}
          >
            <RemoveIcon />
          </IconButton>
          <div className={classes.divider1} />
          <p className={classes.zoomText}>{(zoom * 100).toFixed(0)}%</p>
          <div className={classes.divider2} />
          <IconButton
            disabled={!zoomInEnabled}
            className={classes.zoomInButton}
            aria-label="Zoom In"
            onClick={this.handleZoomIn}
          >
            <AddIcon />
          </IconButton>
        </Paper>
      );
    }
  },
);
