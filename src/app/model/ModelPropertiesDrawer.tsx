// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { createStyles, withStyles, WithStyles } from '@material-ui/core/styles';
import SwipeableDrawer from '@material-ui/core/SwipeableDrawer';
import TextField from '@material-ui/core/TextField';

import IconButton from '@material-ui/core/IconButton';

import ArrowBackIcon from '@material-ui/icons/ArrowBack';
import ClearIcon from '@material-ui/icons/Clear';

import Logo from '../model-icon-small.svg';

const iOS = typeof navigator !== undefined && /iPad|iPhone|iPod/.test(navigator.userAgent);

const styles = createStyles({
  content: {
    width: 359 + 16,
  },
  imageWrap: {
    display: 'inline-block',
    verticalAlign: 'top',
    height: 48,
  },
  modelApp: {
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
  modelName: {
    paddingLeft: 6,
    paddingTop: 2,
    display: 'inline-block',
    height: 48,
    fontSize: 32,
  },
  menuButton: {
    position: 'absolute',
    left: 8,
    top: 8,
    marginLeft: 4,
  },
  closeButton: {
    position: 'absolute',
    top: 8,
    right: 8,
    marginRight: 4,
  },
  propsForm: {
    padding: 32,
  },
});

interface ModelPropertiesDrawerPropsFull extends WithStyles<typeof styles> {
  modelName: string;
  open: boolean;
  onExit: () => void;
  onDrawerToggle: (isOpen: boolean) => void;
  startTime: number;
  stopTime: number;
  dt: number;
  timeUnits: string;
  onStartTimeChange: (event: React.ChangeEvent<HTMLInputElement>) => void;
  onStopTimeChange: (event: React.ChangeEvent<HTMLInputElement>) => void;
  onDtChange: (event: React.ChangeEvent<HTMLInputElement>) => void;
  onTimeUnitsChange: (event: React.ChangeEvent<HTMLInputElement>) => void;
}

export const ModelPropertiesDrawer = withStyles(styles)(
  class InnerModelPropertiesDrawer extends React.PureComponent<ModelPropertiesDrawerPropsFull> {
    constructor(props: ModelPropertiesDrawerPropsFull) {
      super(props);
    }

    handleOpen = () => {
      this.props.onDrawerToggle(true);
    };

    handleClose = () => {
      this.props.onDrawerToggle(false);
    };

    handleExit = () => {
      this.props.onExit();
    };

    render() {
      const { classes } = this.props;
      const { modelName, open } = this.props;
      return (
        <SwipeableDrawer
          disableBackdropTransition={false}
          disableDiscovery={iOS}
          open={open}
          onOpen={this.handleOpen}
          onClose={this.handleClose}
        >
          <div className={classes.content}>
            <div>
              <div className={classes.modelApp}>
                <div className={classes.imageWrap}>
                  <Logo />
                </div>
                <div className={classes.modelName}>Model</div>
              </div>
              <IconButton className={classes.menuButton} color="inherit" aria-label="Exit" onClick={this.handleExit}>
                <ArrowBackIcon />
              </IconButton>
              <IconButton className={classes.closeButton} color="inherit" aria-label="Close" onClick={this.handleClose}>
                <ClearIcon />
              </IconButton>
            </div>

            <div className={classes.propsForm}>
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
            </div>
          </div>
        </SwipeableDrawer>
      );
    }
  },
);
