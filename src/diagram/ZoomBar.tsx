// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import clsx from 'clsx';
import { styled } from '@material-ui/core/styles';
import IconButton from '@material-ui/core/IconButton';
import Paper from '@material-ui/core/Paper';
import AddIcon from '@material-ui/icons/Add';
import RemoveIcon from '@material-ui/icons/Remove';

interface ZoomBarProps {
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

export const ZoomBar = styled(
  class InnerVariableDetails extends React.PureComponent<ZoomBarProps & { className?: string }> {
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
      const { className } = this.props;

      const zoom = snapToZoom(this.props.zoom);

      const zoomInEnabled = zoom < zooms[zooms.length - 1];
      const zoomOutEnabled = zoom > zooms[0];

      return (
        <Paper className={clsx(className, 'simlin-zoombar-card')} elevation={2}>
          <IconButton
            disabled={!zoomOutEnabled}
            style={{ display: 'inline-block' }}
            aria-label="Zoom Out"
            onClick={this.handleZoomOut}
          >
            <RemoveIcon />
          </IconButton>
          <div className="simlin-zoombar-divider1" />
          <p className="simlin-zoombar-zoomtext">{(zoom * 100).toFixed(0)}%</p>
          <div className="simlin-zoombar-divider2" />
          <IconButton
            disabled={!zoomInEnabled}
            style={{ display: 'inline-block' }}
            aria-label="Zoom In"
            onClick={this.handleZoomIn}
          >
            <AddIcon />
          </IconButton>
        </Paper>
      );
    }
  },
)(({ theme }) => ({
  '&.simlin-zoombar-card': {
    height: 40,
    marginRight: theme.spacing(1),
  },
  '.simlin-zoombar-divider1': {
    display: 'inline-block',
    top: 0,
    left: 0,
    marginRight: 6,
    marginTop: 12,
    height: 16,
    borderLeftWidth: 1,
    borderLeftStyle: 'solid',
    borderColor: '#ddd',
  },
  '.simlin-zoombar-divider2': {
    display: 'inline-block',
    top: 0,
    left: 0,
    marginLeft: 8,
    marginTop: 12,
    height: 16,
    borderLeftWidth: 1,
    borderLeftStyle: 'solid',
    borderColor: '#ddd',
  },
  '.simlin-zoombar-zoomtext': {
    width: 21,
    fontSize: '.6rem',
    color: '#888',
    textAlign: 'center',
    display: 'inline-block',
    verticalAlign: 4,
    margin: 0,
  },
}));
