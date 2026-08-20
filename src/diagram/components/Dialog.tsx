// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import * as RadixDialog from '@radix-ui/react-dialog';
import clsx from 'clsx';

import styles from './Dialog.module.css';
import { usePortalContainer } from './portal-container';

export interface DialogProps {
  open: boolean;
  onClose?: () => void;
  disableEscapeKeyDown?: boolean;
  // Block dismissal via clicks/interaction outside the dialog. A truly modal
  // dialog (e.g. mandatory onboarding) needs this in addition to
  // disableEscapeKeyDown -- Radix otherwise treats a backdrop click as a
  // close request and routes it to onClose.
  disableBackdropClick?: boolean;
  'aria-labelledby'?: string;
  className?: string;
  style?: React.CSSProperties;
  children?: React.ReactNode;
}

export function Dialog(props: DialogProps): React.ReactElement {
  const { open, onClose, disableEscapeKeyDown, disableBackdropClick, className, style, children } = props;
  const ariaLabelledBy = props['aria-labelledby'];
  // Viewport mode (document.body): overlay and content are fixed against the
  // viewport. Contained mode (a host box): absolute inside it, so the overlay
  // covers the box and the dialog centres in it -- see portal-container.ts.
  const { container, contained } = usePortalContainer();

  return (
    <RadixDialog.Root
      open={open}
      onOpenChange={(isOpen) => {
        if (!isOpen && onClose) {
          onClose();
        }
      }}
    >
      <RadixDialog.Portal container={container}>
        <RadixDialog.Overlay className={clsx(styles.overlay, contained && styles.contained)} />
        <RadixDialog.Content
          className={clsx(styles.content, contained && styles.contained, className)}
          style={style}
          aria-labelledby={ariaLabelledBy}
          onEscapeKeyDown={(event) => {
            if (disableEscapeKeyDown) {
              event.preventDefault();
            }
          }}
          onPointerDownOutside={(event) => {
            if (disableBackdropClick) {
              event.preventDefault();
            }
          }}
          onInteractOutside={(event) => {
            if (disableBackdropClick) {
              event.preventDefault();
            }
          }}
        >
          {children}
        </RadixDialog.Content>
      </RadixDialog.Portal>
    </RadixDialog.Root>
  );
}

export interface DialogTitleProps {
  id?: string;
  className?: string;
  style?: React.CSSProperties;
  children?: React.ReactNode;
}

export function DialogTitle(props: DialogTitleProps): React.ReactElement {
  const { id, className, style, children } = props;

  return (
    <RadixDialog.Title id={id} className={clsx(styles.title, className)} style={style}>
      {children}
    </RadixDialog.Title>
  );
}

export interface DialogContentProps {
  className?: string;
  style?: React.CSSProperties;
  children?: React.ReactNode;
}

export function DialogContent(props: DialogContentProps): React.ReactElement {
  const { className, style, children } = props;

  return (
    <div className={clsx(styles.dialogContent, className)} style={style}>
      {children}
    </div>
  );
}

export interface DialogContentTextProps {
  className?: string;
  style?: React.CSSProperties;
  children?: React.ReactNode;
}

export function DialogContentText(props: DialogContentTextProps): React.ReactElement {
  const { className, style, children } = props;

  return (
    <p className={clsx(styles.contentText, className)} style={style}>
      {children}
    </p>
  );
}

export interface DialogActionsProps {
  className?: string;
  style?: React.CSSProperties;
  children?: React.ReactNode;
}

export function DialogActions(props: DialogActionsProps): React.ReactElement {
  const { className, style, children } = props;

  return (
    <div className={clsx(styles.actions, className)} style={style}>
      {children}
    </div>
  );
}
