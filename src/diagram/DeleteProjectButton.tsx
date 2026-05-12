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
export class DeleteProjectButton extends React.PureComponent<DeleteProjectButtonProps, DeleteProjectButtonState> {
  state: DeleteProjectButtonState = { confirmOpen: false, deleting: false };

  private openConfirm = (): void => {
    this.setState({ confirmOpen: true, error: undefined });
  };

  private closeConfirm = (): void => {
    // Don't let an outside-click / Escape dismiss the dialog mid-delete.
    if (this.state.deleting) {
      return;
    }
    this.setState({ confirmOpen: false, error: undefined });
  };

  private confirmDelete = async (): Promise<void> => {
    if (this.state.deleting) {
      return;
    }
    this.setState({ deleting: true, error: undefined });
    try {
      await this.props.onDelete();
      // Success: the caller navigates away and this component unmounts. Leave
      // `deleting` set so the buttons stay disabled during that brief window.
    } catch (err) {
      this.setState({ deleting: false, error: errorMessage(err) });
    }
  };

  render(): React.ReactNode {
    const { projectName } = this.props;
    const { confirmOpen, deleting, error } = this.state;

    return (
      <>
        <Button
          className={styles.trigger}
          variant="outlined"
          color="error"
          size="large"
          startIcon={<DeleteIcon />}
          onClick={this.openConfirm}
        >
          Delete project
        </Button>
        <Dialog open={confirmOpen} onClose={this.closeConfirm} aria-labelledby="delete-project-title">
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
            <Button onClick={this.closeConfirm} disabled={deleting}>
              Cancel
            </Button>
            <Button variant="contained" color="error" onClick={this.confirmDelete} disabled={deleting}>
              {deleting ? 'Deleting…' : 'Delete'}
            </Button>
          </DialogActions>
        </Dialog>
      </>
    );
  }
}
