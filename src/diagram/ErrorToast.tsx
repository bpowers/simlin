// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import clsx from 'clsx';
import { styled } from '@mui/material/styles';
import { amber, green } from '@mui/material/colors';
import IconButton from '@mui/material/IconButton';
import SnackbarContent from '@mui/material/SnackbarContent';
import CheckCircleIcon from '@mui/icons-material/CheckCircle';
import CloseIcon from '@mui/icons-material/Close';
import ErrorIcon from '@mui/icons-material/Error';
import InfoIcon from '@mui/icons-material/Info';
import WarningIcon from '@mui/icons-material/Warning';

const variantIcon = {
  success: CheckCircleIcon,
  warning: WarningIcon,
  error: ErrorIcon,
  info: InfoIcon,
};

export interface ToastProps {
  message: string;
  onClose: (msg: string) => void;
  variant: keyof typeof variantIcon;
}

export const Toast = styled(
  class InnerToast extends React.PureComponent<ToastProps & { className?: string }> {
    handleClose = () => {
      this.props.onClose(this.props.message);
    };

    render() {
      const { className, message, variant, ...other } = this.props;
      const Icon = variantIcon[variant];

      const variantClass = `simlin-errortoast-${variant}`;

      return (
        <SnackbarContent
          className={clsx(className, variantClass)}
          aria-describedby="client-snackbar"
          message={
            <span id="client-snackbar" className="simlin-errortoast-message">
              <Icon className={clsx('simlin-errortoast-icon', 'simlin-errortoast-iconvariant')} />
              {message}
            </span>
          }
          action={[
            <IconButton key="close" aria-label="close" color="inherit" onClick={this.handleClose}>
              <CloseIcon className="simlin-errortoast-icon" />
            </IconButton>,
          ]}
          {...other}
        />
      );
    }
  },
)(({ theme }) => ({
  '&.simlin-errortoast-success': {
    backgroundColor: green[600],
  },
  '&.simlin-errortoast-error': {
    backgroundColor: theme.palette.error.dark,
  },
  '&.simlin-errortoast-info': {
    backgroundColor: theme.palette.primary.main,
  },
  '&.simlin-errortoast-warning': {
    backgroundColor: amber[700],
  },
  '.simlin-errortoast-icon': {
    fontSize: 20,
  },
  '.simlin-errortoast-iconVariant': {
    opacity: 0.9,
    marginRight: theme.spacing(1),
  },
  '.simlin-errortoast-message': {
    display: 'flex',
    alignItems: 'center',
  },
}));
