import * as React from 'react';
import * as RadixDialog from '@radix-ui/react-dialog';
import clsx from 'clsx';

import styles from './Dialog.module.css';

export interface DialogProps {
  open: boolean;
  onClose?: () => void;
  disableEscapeKeyDown?: boolean;
  'aria-labelledby'?: string;
  className?: string;
  style?: React.CSSProperties;
  children?: React.ReactNode;
}

export function Dialog(props: DialogProps): React.ReactElement {
  const { open, onClose, disableEscapeKeyDown, className, style, children } = props;
  const ariaLabelledBy = props['aria-labelledby'];

  return (
    <RadixDialog.Root
      open={open}
      onOpenChange={(isOpen) => {
        if (!isOpen && onClose) {
          onClose();
        }
      }}
    >
      <RadixDialog.Portal>
        <RadixDialog.Overlay className={styles.overlay} />
        <RadixDialog.Content
          className={clsx(styles.content, className)}
          style={style}
          aria-labelledby={ariaLabelledBy}
          onEscapeKeyDown={(event) => {
            if (disableEscapeKeyDown) {
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
