// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import clsx from 'clsx';
import IconButton from '@mui/material/IconButton';
import SnackbarContent from '@mui/material/SnackbarContent';
import CheckCircleIcon from '@mui/icons-material/CheckCircle';
import CloseIcon from '@mui/icons-material/Close';
import ErrorIcon from '@mui/icons-material/Error';
import InfoIcon from '@mui/icons-material/Info';
import WarningIcon from '@mui/icons-material/Warning';

import styles from './ErrorToast.module.css';

const variantIcon = {
  success: CheckCircleIcon,
  warning: WarningIcon,
  error: ErrorIcon,
  info: InfoIcon,
};

const variantClass: Record<keyof typeof variantIcon, string> = {
  success: styles.success,
  warning: styles.warning,
  error: styles.error,
  info: styles.info,
};

export interface ToastProps {
  message: string;
  onClose: (msg: string) => void;
  variant: keyof typeof variantIcon;
}

export class Toast extends React.PureComponent<ToastProps> {
  handleClose = () => {
    this.props.onClose(this.props.message);
  };

  render() {
    const { message, variant, ...other } = this.props;
    const Icon = variantIcon[variant];

    return (
      <SnackbarContent
        className={variantClass[variant]}
        aria-describedby="client-snackbar"
        message={
          <span id="client-snackbar" className={styles.message}>
            <Icon className={clsx(styles.icon, styles.iconVariant)} />
            {message}
          </span>
        }
        action={[
          <IconButton key="close" aria-label="close" color="inherit" onClick={this.handleClose}>
            <CloseIcon className={styles.icon} />
          </IconButton>,
        ]}
        {...other}
      />
    );
  }
}
