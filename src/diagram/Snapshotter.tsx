// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import IconButton from './components/IconButton';
import { PhotoCameraIcon } from './components/icons';

import styles from './Snapshotter.module.css';

interface SnapshotterProps {
  onSnapshot: (kind: 'show' | 'close') => void;
}

export class Snapshotter extends React.PureComponent<SnapshotterProps> {
  handleSnapshot = () => {
    this.props.onSnapshot('show');
  };

  render() {
    return (
      <div className={styles.card}>
        <IconButton className={styles.button} aria-label="Snapshot" onClick={this.handleSnapshot}>
          <PhotoCameraIcon />
        </IconButton>
      </div>
    );
  }
}
