// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import ReactDOM from 'react-dom';

import clsx from 'clsx';

import styles from './Snackbar.module.css';

interface SnackbarProps {
  anchorOrigin?: { vertical: string; horizontal: string };
  open: boolean;
  autoHideDuration?: number;
  onClose?: () => void;
  children?: React.ReactNode;
}

export default class Snackbar extends React.PureComponent<SnackbarProps> {
  timerHandle: ReturnType<typeof setTimeout> | undefined;

  componentDidMount() {
    // Start timer if mounted with open={true}
    if (this.props.open) {
      this.startTimer();
    }
  }

  componentDidUpdate(prevProps: SnackbarProps) {
    // Only restart timer when open or autoHideDuration changes
    const openChanged = this.props.open !== prevProps.open;
    const durationChanged = this.props.autoHideDuration !== prevProps.autoHideDuration;

    if (openChanged || durationChanged) {
      this.clearTimer();
      if (this.props.open) {
        this.startTimer();
      }
    }
  }

  componentWillUnmount() {
    this.clearTimer();
  }

  startTimer() {
    if (this.props.autoHideDuration && this.props.onClose) {
      this.timerHandle = setTimeout(this.props.onClose, this.props.autoHideDuration);
    }
  }

  clearTimer() {
    if (this.timerHandle !== undefined) {
      window.clearTimeout(this.timerHandle);
      this.timerHandle = undefined;
    }
  }

  render() {
    const { open, children } = this.props;

    const content = <div className={clsx(styles.snackbar, !open && styles.snackbarHidden)}>{children}</div>;

    return ReactDOM.createPortal(content, document.body);
  }
}

interface SnackbarContentProps {
  className?: string;
  message?: React.ReactNode;
  action?: React.ReactNode;
  'aria-describedby'?: string;
  [key: string]: unknown;
}

export class SnackbarContent extends React.PureComponent<SnackbarContentProps> {
  render() {
    const { className, message, action, 'aria-describedby': ariaDescribedby, ...rest } = this.props;

    // filter out non-DOM props that may be spread from parent destructuring
    const domRest: Record<string, unknown> = {};
    for (const [key, val] of Object.entries(rest)) {
      if (key === 'onClose' || key === 'variant') {
        continue;
      }
      domRest[key] = val;
    }

    return (
      <div className={clsx(styles.snackbarContent, className)} aria-describedby={ariaDescribedby} {...domRest}>
        <div className={styles.snackbarContentMessage}>{message}</div>
        {action && <div className={styles.snackbarContentAction}>{action}</div>}
      </div>
    );
  }
}
