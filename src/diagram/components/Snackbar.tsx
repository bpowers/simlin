// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import * as Toast from '@radix-ui/react-toast';

import clsx from 'clsx';

import styles from './Snackbar.module.css';

interface SnackbarProps {
  anchorOrigin?: { vertical: string; horizontal: string };
  open: boolean;
  autoHideDuration?: number;
  onClose?: () => void;
  children?: React.ReactNode;
}

export const SnackbarDurationContext = React.createContext<number | undefined>(undefined);

export default class Snackbar extends React.PureComponent<SnackbarProps> {
  render() {
    const { open, autoHideDuration, children } = this.props;
    // We manage timing per-toast; keep Radix's provider duration effectively disabled.
    const providerDuration = 2147483647;

    return (
      <SnackbarDurationContext.Provider value={autoHideDuration}>
        <Toast.Provider duration={providerDuration}>
          {open ? children : null}
          <Toast.Viewport className={clsx(styles.toastViewport)} />
        </Toast.Provider>
      </SnackbarDurationContext.Provider>
    );
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
