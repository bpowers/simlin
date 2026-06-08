// Copyright 2026 The Simlin Authors. All rights reserved.
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
  // Identifies *which* toast is closing. Callers that render several toasts
  // with identical message text must pass a stable per-toast id and key
  // their removal on it; otherwise closing one toast (or its auto-hide
  // timer firing) would ambiguously match every toast with the same text.
  // Defaults to `message` for callers that never have duplicates.
  id?: string | number;
  onClose: (id: string | number) => void;
  variant: keyof typeof variantIcon;
}

export function Toast(props: ToastProps): React.ReactElement {
  // `id` is intentionally pulled out (rest-sibling omission) so our numeric
  // toast-identity prop is not spread onto the DOM node as an HTML id.
  const { message, variant, id, onClose, ...other } = props;
  const Icon = variantIcon[variant];

  const duration = React.useContext(SnackbarDurationContext);

  const [open, setOpen] = React.useState(true);

  // The live timer handle and the values escaped callbacks must read at fire
  // time live in refs so the auto-hide timer is set up exactly once per
  // (open, duration) transition -- mirroring the class's instance fields and
  // the `lastDuration !== context` comparison in componentDidUpdate. Reading
  // current props/state through refs (rather than closing over them) keeps the
  // timer's effect dependent only on `open` and `duration`, so unrelated
  // re-renders (e.g. a message change) do not restart it.
  const timerHandle = React.useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const openRef = React.useRef(open);
  openRef.current = open;
  const onCloseRef = React.useRef(onClose);
  onCloseRef.current = onClose;
  const idRef = React.useRef(id);
  idRef.current = id;
  const messageRef = React.useRef(message);
  messageRef.current = message;

  const clearTimer = React.useCallback((): void => {
    if (timerHandle.current !== undefined) {
      window.clearTimeout(timerHandle.current);
      timerHandle.current = undefined;
    }
  }, []);

  const closeToast = React.useCallback((): void => {
    if (!openRef.current) {
      return;
    }
    clearTimer();
    setOpen(false);
    onCloseRef.current(idRef.current ?? messageRef.current);
  }, [clearTimer]);

  // (Re)start the auto-hide timer whenever the toast is open and the duration
  // changes. The class started the timer on mount and again in
  // componentDidUpdate when it (re)opened or the context duration changed; an
  // effect keyed on [open, duration] reproduces both transitions, and its
  // cleanup clears any pending timer on unmount or before the next run --
  // StrictMode-safe (mount/unmount/mount leaves no orphaned timer).
  React.useEffect(() => {
    if (!open) {
      return undefined;
    }
    if (duration !== undefined) {
      timerHandle.current = setTimeout(closeToast, duration);
    }
    return clearTimer;
  }, [open, duration, closeToast, clearTimer]);

  const handleOpenChange = (next: boolean): void => {
    if (!next) {
      closeToast();
    }
  };

  return (
    <RadixToast.Root open={open} onOpenChange={handleOpenChange}>
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
