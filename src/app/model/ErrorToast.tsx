// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { createStyles, withStyles, WithStyles } from '@material-ui/core/styles';

import { amber, green } from '@material-ui/core/colors';
import IconButton from '@material-ui/core/IconButton';
import SnackbarContent from '@material-ui/core/SnackbarContent';
import { Theme } from '@material-ui/core/styles';
import CheckCircleIcon from '@material-ui/icons/CheckCircle';
import CloseIcon from '@material-ui/icons/Close';
import ErrorIcon from '@material-ui/icons/Error';
import InfoIcon from '@material-ui/icons/Info';
import WarningIcon from '@material-ui/icons/Warning';

const variantIcon = {
  success: CheckCircleIcon,
  warning: WarningIcon,
  error: ErrorIcon,
  info: InfoIcon,
};

const styles = ({ spacing, palette }: Theme) =>
  createStyles({
    success: {
      backgroundColor: green[600],
    },
    error: {
      backgroundColor: palette.error.dark,
    },
    info: {
      backgroundColor: palette.primary.main,
    },
    warning: {
      backgroundColor: amber[700],
    },
    icon: {
      fontSize: 20,
    },
    iconVariant: {
      opacity: 0.9,
      marginRight: spacing(1),
    },
    message: {
      display: 'flex',
      alignItems: 'center',
    },
  });

interface ToastPropsFull extends WithStyles<typeof styles> {
  className?: string;
  message: string;
  onClose: (msg: string) => void;
  variant: keyof typeof variantIcon;
}

export type ToastProps = Pick<ToastPropsFull, 'className' | 'message' | 'onClose' | 'variant'>;

export const Toast = withStyles(styles)(
  class InnerToast extends React.PureComponent<ToastPropsFull> {
    handleClose = () => {
      this.props.onClose(this.props.message);
    };

    render() {
      const { classes, className, message, variant, ...other } = this.props;
      const Icon = variantIcon[variant];

      return (
        <SnackbarContent
          className={classes[variant] + ' ' + className}
          aria-describedby="client-snackbar"
          message={
            <span id="client-snackbar" className={classes.message}>
              <Icon className={classes.icon + ' ' + classes.iconVariant} />
              {message}
            </span>
          }
          action={[
            <IconButton key="close" aria-label="close" color="inherit" onClick={this.handleClose}>
              <CloseIcon className={classes.icon} />
            </IconButton>,
          ]}
          {...other}
        />
      );
    }
  },
);
