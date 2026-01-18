// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { Link } from 'wouter';
import Button from '@mui/material/Button';
import IconButton from '@mui/material/IconButton';
import SwipeableDrawer from '@mui/material/SwipeableDrawer';
import TextField from '@mui/material/TextField';
import ArrowBackIcon from '@mui/icons-material/ArrowBack';
import ClearIcon from '@mui/icons-material/Clear';
import CloudDownloadIcon from '@mui/icons-material/CloudDownload';

import { ModelIcon } from './ModelIcon';

import styles from './ModelPropertiesDrawer.module.css';

const iOS = typeof navigator !== undefined && /iPad|iPhone|iPod/.test(navigator.userAgent);

interface ModelPropertiesDrawerProps {
  modelName: string;
  open: boolean;
  onDrawerToggle: (isOpen: boolean) => void;
  startTime: number;
  stopTime: number;
  dt: number;
  timeUnits: string;
  onStartTimeChange: (event: React.ChangeEvent<HTMLInputElement>) => void;
  onStopTimeChange: (event: React.ChangeEvent<HTMLInputElement>) => void;
  onDtChange: (event: React.ChangeEvent<HTMLInputElement>) => void;
  onTimeUnitsChange: (event: React.ChangeEvent<HTMLInputElement>) => void;
  onDownloadXmile: () => void;
}

export class ModelPropertiesDrawer extends React.PureComponent<ModelPropertiesDrawerProps> {
  handleOpen = () => {
    this.props.onDrawerToggle(true);
  };

  handleClose = () => {
    this.props.onDrawerToggle(false);
  };

  render() {
    const { modelName, open } = this.props;
    return (
      <SwipeableDrawer
        disableBackdropTransition={false}
        disableDiscovery={iOS}
        open={open}
        onOpen={this.handleOpen}
        onClose={this.handleClose}
      >
        <div className={styles.content}>
          <div>
            <div className={styles.modelApp}>
              <div className={styles.imageWrap}>
                <ModelIcon className={styles.modelIcon} />
              </div>
              <div className={styles.modelName}>Simlin</div>
            </div>
            <Link to="/" className={styles.exitLink}>
              <IconButton className={styles.menuButton} color="inherit" aria-label="Exit">
                <ArrowBackIcon />
              </IconButton>
            </Link>
            <IconButton className={styles.closeButton} color="inherit" aria-label="Close" onClick={this.handleClose}>
              <ClearIcon />
            </IconButton>
          </div>

          <div className={styles.propsForm}>
            <h2>{modelName}</h2>
            <TextField
              label="Start Time"
              value={this.props.startTime}
              onChange={this.props.onStartTimeChange}
              type="number"
              margin="normal"
              fullWidth
            />
            <TextField
              label="Stop Time"
              value={this.props.stopTime}
              onChange={this.props.onStopTimeChange}
              type="number"
              margin="normal"
              fullWidth
            />
            <TextField
              label="dt"
              value={this.props.dt}
              onChange={this.props.onDtChange}
              type="number"
              margin="normal"
              fullWidth
            />
            <TextField
              label="Time Units"
              value={this.props.timeUnits}
              onChange={this.props.onTimeUnitsChange}
              margin="normal"
              fullWidth
            />
            <br />
            <br />
            <Button
              className={styles.downloadButton}
              variant="contained"
              color="primary"
              size="large"
              startIcon={<CloudDownloadIcon />}
              onClick={this.props.onDownloadXmile}
            >
              Download model
            </Button>
          </div>
        </div>
      </SwipeableDrawer>
    );
  }
}
