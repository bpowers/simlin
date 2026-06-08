// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { Link } from 'wouter';
import Button from './components/Button';
import IconButton from './components/IconButton';
import Drawer from './components/Drawer';
import TextField from './components/TextField';
import { ArrowBackIcon, ClearIcon, CloudDownloadIcon } from './components/icons';

import { DeleteProjectButton } from './DeleteProjectButton';
import { ModelIcon } from './ModelIcon';

import styles from './ModelPropertiesDrawer.module.css';

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
  // When provided, a destructive "Delete project" action is shown. Hosts that
  // can't (or shouldn't) delete -- read-only viewers, embeds, the local
  // file-backed viewer -- simply leave this undefined.
  onDelete?: () => Promise<void>;
}

export function ModelPropertiesDrawer(props: ModelPropertiesDrawerProps): React.ReactElement {
  const {
    modelName,
    open,
    onDrawerToggle,
    startTime,
    stopTime,
    dt,
    timeUnits,
    onStartTimeChange,
    onStopTimeChange,
    onDtChange,
    onTimeUnitsChange,
    onDownloadXmile,
    onDelete,
  } = props;

  const handleOpen = (): void => {
    onDrawerToggle(true);
  };

  const handleClose = (): void => {
    onDrawerToggle(false);
  };

  return (
    <Drawer open={open} onOpen={handleOpen} onClose={handleClose}>
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
          <IconButton className={styles.closeButton} color="inherit" aria-label="Close" onClick={handleClose}>
            <ClearIcon />
          </IconButton>
        </div>

        <div className={styles.propsForm}>
          <h2>{modelName}</h2>
          <TextField
            label="Start Time"
            value={startTime}
            onChange={onStartTimeChange}
            type="number"
            margin="normal"
            fullWidth
          />
          <TextField
            label="Stop Time"
            value={stopTime}
            onChange={onStopTimeChange}
            type="number"
            margin="normal"
            fullWidth
          />
          <TextField label="dt" value={dt} onChange={onDtChange} type="number" margin="normal" fullWidth />
          <TextField label="Time Units" value={timeUnits} onChange={onTimeUnitsChange} margin="normal" fullWidth />
          <br />
          <br />
          <Button
            className={styles.downloadButton}
            variant="contained"
            color="primary"
            size="large"
            startIcon={<CloudDownloadIcon />}
            onClick={onDownloadXmile}
          >
            Download model
          </Button>
          {onDelete ? <DeleteProjectButton projectName={modelName} onDelete={onDelete} /> : null}
        </div>
      </div>
    </Drawer>
  );
}
