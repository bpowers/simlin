// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import clsx from 'clsx';
import * as RadixToast from '@radix-ui/react-toast';
import IconButton from './components/IconButton';
import { SnackbarContent, SnackbarDurationContext } from './components/Snackbar';
import { CheckCircleIcon, CloseIcon, ErrorIcon, InfoIcon, WarningIcon } from './components/icons';

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

interface ToastState {
  open: boolean;
}

export class Toast extends React.PureComponent<ToastProps, ToastState> {
  static contextType = SnackbarDurationContext;
  declare context: React.ContextType<typeof SnackbarDurationContext>;

  timerHandle: ReturnType<typeof setTimeout> | undefined;
  lastDuration: number | undefined;

  state: ToastState = { open: true };

  componentDidMount() {
    this.lastDuration = this.context;
    this.startTimer();
  }

  componentDidUpdate(_prevProps: ToastProps, prevState: ToastState) {
    const becameOpen = !prevState.open && this.state.open;
    const durationChanged = prevState.open && this.state.open && this.lastDuration !== this.context;
    if (becameOpen || durationChanged) {
      this.lastDuration = this.context;
      this.startTimer();
    }
  }

  componentWillUnmount() {
    this.clearTimer();
  }

  clearTimer() {
    if (this.timerHandle !== undefined) {
      window.clearTimeout(this.timerHandle);
      this.timerHandle = undefined;
    }
  }

  startTimer() {
    this.clearTimer();
    const duration = this.context;
    if (duration !== undefined) {
      this.timerHandle = setTimeout(this.closeToast, duration);
    }
  }

  closeToast = () => {
    if (!this.state.open) {
      return;
    }
    this.clearTimer();
    this.setState({ open: false });
    this.props.onClose(this.props.message);
  };

  handleOpenChange = (open: boolean) => {
    if (!open) {
      this.closeToast();
    }
  };

  render() {
    const { message, variant, ...other } = this.props;
    const Icon = variantIcon[variant];

    return (
      <RadixToast.Root open={this.state.open} onOpenChange={this.handleOpenChange}>
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
            <RadixToast.Close asChild key="close">
              <IconButton aria-label="close" color="inherit">
                <CloseIcon className={styles.icon} />
              </IconButton>
            </RadixToast.Close>,
          ]}
          {...other}
        />
      </RadixToast.Root>
    );
  }
}
