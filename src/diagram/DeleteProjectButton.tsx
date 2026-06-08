// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import Button from './components/Button';
import { Dialog, DialogActions, DialogContent, DialogContentText, DialogTitle } from './components/Dialog';
import { DeleteIcon } from './components/icons';

import styles from './DeleteProjectButton.module.css';

interface DeleteProjectButtonProps {
  /** Display name shown in the confirmation prompt. */
  projectName: string;
  /**
   * Performs the deletion. Resolving means the caller has navigated away (so
   * this component is about to unmount); rejecting surfaces the error in the
   * still-open confirmation dialog so the user can retry.
   */
  onDelete: () => Promise<void>;
}

interface DeleteProjectButtonState {
  confirmOpen: boolean;
  deleting: boolean;
  error?: string;
}

function errorMessage(err: unknown): string {
  if (err instanceof Error && err.message) {
    return err.message;
  }
  const s = String(err);
  return s && s !== '[object Object]' ? s : 'unable to delete project';
}

/**
 * "Delete project" action: a low-emphasis destructive button that opens a
 * modal confirmation before invoking `onDelete`. Kept separate from
 * `ModelPropertiesDrawer` so the confirmation state lives in one small place
 * and can be reused by other surfaces (e.g. a project list).
 */
export function DeleteProjectButton({ projectName, onDelete }: DeleteProjectButtonProps): React.ReactElement {
  const [state, setState] = React.useState<DeleteProjectButtonState>({ confirmOpen: false, deleting: false });
  const { confirmOpen, deleting, error } = state;

  const openConfirm = (): void => {
    setState({ confirmOpen: true, deleting: false, error: undefined });
  };

  const closeConfirm = (): void => {
    // Don't let an outside-click / Escape dismiss the dialog mid-delete.
    if (deleting) {
      return;
    }
    setState({ confirmOpen: false, deleting: false, error: undefined });
  };

  const confirmDelete = async (): Promise<void> => {
    if (deleting) {
      return;
    }
    setState((prev) => ({ ...prev, deleting: true, error: undefined }));
    try {
      await onDelete();
      // Success: the caller navigates away and this component unmounts. Leave
      // `deleting` set so the buttons stay disabled during that brief window.
      // No state update is queued here, so there is nothing to guard against a
      // post-unmount setState -- mirroring the class, which only set state in
      // the rejection path (where the component is still mounted with the
      // dialog open).
    } catch (err) {
      setState((prev) => ({ ...prev, deleting: false, error: errorMessage(err) }));
    }
  };

  return (
    <>
      <Button
        className={styles.trigger}
        variant="outlined"
        color="error"
        size="large"
        startIcon={<DeleteIcon />}
        onClick={openConfirm}
      >
        Delete project
      </Button>
      <Dialog open={confirmOpen} onClose={closeConfirm} aria-labelledby="delete-project-title">
        <DialogTitle id="delete-project-title">Delete this project?</DialogTitle>
        <DialogContent>
          <DialogContentText>
            This permanently deletes &ldquo;{projectName}&rdquo; and can&rsquo;t be undone.
          </DialogContentText>
          {error ? (
            <DialogContentText className={styles.errorText}>
              <b>{error}</b>
            </DialogContentText>
          ) : null}
        </DialogContent>
        <DialogActions>
          <Button onClick={closeConfirm} disabled={deleting}>
            Cancel
          </Button>
          <Button variant="contained" color="error" onClick={confirmDelete} disabled={deleting}>
            {deleting ? 'Deleting…' : 'Delete'}
          </Button>
        </DialogActions>
      </Dialog>
    </>
  );
}
