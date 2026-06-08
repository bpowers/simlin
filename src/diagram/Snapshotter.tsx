// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import IconButton from './components/IconButton';
import { PhotoCameraIcon } from './components/icons';

import styles from './Snapshotter.module.css';

interface SnapshotterProps {
  onSnapshot: (kind: 'show' | 'close') => void;
}

export function Snapshotter({ onSnapshot }: SnapshotterProps): React.ReactElement {
  const handleSnapshot = (): void => {
    onSnapshot('show');
  };

  return (
    <div className={styles.card}>
      <IconButton className={styles.button} aria-label="Snapshot" onClick={handleSnapshot}>
        <PhotoCameraIcon />
      </IconButton>
    </div>
  );
}
