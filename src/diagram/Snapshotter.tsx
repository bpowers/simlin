// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import clsx from 'clsx';
import { styled } from '@mui/material/styles';
import IconButton from '@mui/material/IconButton';
import Paper from '@mui/material/Paper';
import PhotoCamera from '@mui/icons-material/PhotoCamera';

interface SnapshotterProps {
  onSnapshot: (kind: 'show' | 'close') => void;
}

export const Snapshotter = styled(
  class InnerSnapshotter extends React.PureComponent<SnapshotterProps & { className?: string }> {
    handleSnapshot = () => {
      this.props.onSnapshot('show');
    };

    render() {
      const { className } = this.props;

      return (
        <Paper className={clsx(className, 'simlin-snapshotter-card')} elevation={2}>
          <IconButton className="simlin-snapshotter-button" aria-label="Snapshot" onClick={this.handleSnapshot}>
            <PhotoCamera />
          </IconButton>
        </Paper>
      );
    }
  },
)(() => ({
  '&.simlin-snapshotter-card': {
    height: 40,
    marginRight: 8,
  },
  '.simlin-snapshotter-button': {
    paddingTop: 8,
  },
}));
