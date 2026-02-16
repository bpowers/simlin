// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import IconButton from './components/IconButton';
import { AddIcon, RemoveIcon } from './components/icons';

import styles from './ZoomBar.module.css';

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

const E = 0.001;
function eq(a: number, b: number): boolean {
  return Math.abs(a - b) < E;
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

export class ZoomBar extends React.PureComponent<ZoomBarProps> {
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
    const zoom = snapToZoom(this.props.zoom);

    const zoomInEnabled = zoom < zooms[zooms.length - 1];
    const zoomOutEnabled = zoom > zooms[0];

    return (
      <div className={styles.card}>
        <IconButton
          disabled={!zoomOutEnabled}
          style={{ display: 'inline-block' }}
          aria-label="Zoom Out"
          onClick={this.handleZoomOut}
        >
          <RemoveIcon />
        </IconButton>
        <div className={styles.divider1} />
        <p className={styles.zoomText}>{(zoom * 100).toFixed(0)}%</p>
        <div className={styles.divider2} />
        <IconButton
          disabled={!zoomInEnabled}
          style={{ display: 'inline-block' }}
          aria-label="Zoom In"
          onClick={this.handleZoomIn}
        >
          <AddIcon />
        </IconButton>
      </div>
    );
  }
}
