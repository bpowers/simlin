// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { Link } from 'react-router-dom';
import clsx from 'clsx';
import { styled } from '@mui/material/styles';
import Button from '@mui/material/Button';
import IconButton from '@mui/material/IconButton';
import SwipeableDrawer from '@mui/material/SwipeableDrawer';
import TextField from '@mui/material/TextField';
import ArrowBackIcon from '@mui/icons-material/ArrowBack';
import ClearIcon from '@mui/icons-material/Clear';
import CloudDownloadIcon from '@mui/icons-material/CloudDownload';

import { ModelIcon } from './ModelIcon';

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

export const ModelPropertiesDrawer = styled(
  class InnerModelPropertiesDrawer extends React.PureComponent<ModelPropertiesDrawerProps & { className?: string }> {
    handleOpen = () => {
      this.props.onDrawerToggle(true);
    };

    handleClose = () => {
      this.props.onDrawerToggle(false);
    };

    render() {
      const { className } = this.props;
      const { modelName, open } = this.props;
      debugger;
      return (
        <SwipeableDrawer
          disableBackdropTransition={false}
          disableDiscovery={iOS}
          open={open}
          onOpen={this.handleOpen}
          onClose={this.handleClose}
        >
          <div className={clsx(className, 'simlin-modelpropertiesdrawer-content')}>
            <div>
              <div className="simlin-modelpropertiesdrawer-modelapp">
                <div className="simlin-modelpropertiesdrawer-imagewrap">
                  <ModelIcon className="simlin-modelpropertiesdrawer-modelicon" />
                </div>
                <div className="simlin-modelpropertiesdrawer-modelname">Simlin</div>
              </div>
              <Link to="/" className="simlin-modelpropertiesdrawer-exitlink">
                <IconButton className="simlin-modelpropertiesdrawer-menubutton" color="inherit" aria-label="Exit">
                  <ArrowBackIcon />
                </IconButton>
              </Link>
              <IconButton
                className="simlin-modelpropertiesdrawer-closebutton"
                color="inherit"
                aria-label="Close"
                onClick={this.handleClose}
              >
                <ClearIcon />
              </IconButton>
            </div>

            <div className="simlin-modelpropertiesdrawer-propsform">
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
                className="simlin-modelpropertiesdrawer-downloadbutton"
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
  },
)(() => ({
  '&.simlin-modelpropertiesdrawer-content': {
    width: 359 + 16,
  },
  '.simlin-modelpropertiesdrawer-imagewrap': {
    display: 'inline-block',
    verticalAlign: 'top',
    height: 48,
  },
  '.simlin-modelpropertiesdrawer-modelapp': {
    textAlign: 'center',
    position: 'relative',
    top: 0,
    left: 0,
    paddingLeft: 64,
    paddingTop: 12,
    paddingBottom: 12,
    paddingRight: 70,
    height: '100%',
    width: '100%',
  },
  '.simlin-modelpropertiesdrawer-modelname': {
    paddingLeft: 6,
    paddingTop: 2,
    display: 'inline-block',
    height: 48,
    fontSize: 32,
  },
  '.simlin-modelpropertiesdrawer-menubutton': {
    position: 'absolute',
    left: 8,
    top: 8,
    marginLeft: 4,
  },
  '.simlin-modelpropertiesdrawer-closebutton': {
    position: 'absolute',
    top: 8,
    right: 8,
    marginRight: 4,
  },
  '.simlin-modelpropertiesdrawer-modelicon': {
    width: 48,
    height: 48,
  },
  '.simlin-modelpropertiesdrawer-propsform': {
    padding: 32,
  },
  '.simlin-modelpropertiesdrawer-exitlink': {
    color: 'inherit',
  },
  '.simlin-modelpropertiesdrawer-downloadbutton': {
    justifyContent: 'center',
    width: '100%',
  },
}));
