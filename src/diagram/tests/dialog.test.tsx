// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { describe, test, expect, rs } from '@rstest/core';

import * as React from 'react';
import { render, screen, fireEvent } from '@testing-library/react';
import { Dialog, DialogTitle, DialogContent, DialogContentText, DialogActions } from '../components/Dialog';

describe('Dialog', () => {
  test('renders children when open', () => {
    render(
      <Dialog open={true}>
        <div data-testid="dialog-child">Hello</div>
      </Dialog>,
    );
    expect(screen.getByTestId('dialog-child')).not.toBeNull();
  });

  test('does not render children when closed', () => {
    render(
      <Dialog open={false}>
        <div data-testid="dialog-child">Hello</div>
      </Dialog>,
    );
    expect(screen.queryByTestId('dialog-child')).toBeNull();
  });

  test('applies custom className to content', () => {
    render(
      <Dialog open={true} className="custom-dialog">
        <div>Content</div>
      </Dialog>,
    );
    const content = document.querySelector('.custom-dialog');
    expect(content).not.toBeNull();
  });

  // Radix's DismissableLayer registers its document-level pointerdown
  // listener on a deferred timer (to ignore the pointerdown that opened the
  // layer), so outside clicks must be fired a tick after render.
  const nextTick = () => new Promise((resolve) => setTimeout(resolve, 0));

  // A primary-button press outside the layer does not dismiss on pointerdown:
  // Radix waits for the matching `click` so a press that drags back inside (a
  // text selection started on the backdrop) does not close the dialog. Fire the
  // full sequence a real pointer produces, not just the pointer half of it.
  const clickOutside = () => {
    fireEvent.pointerDown(document.body);
    fireEvent.pointerUp(document.body);
    fireEvent.click(document.body);
  };

  test('a pointer-down outside dismisses the dialog by default', async () => {
    const onClose = rs.fn();
    render(
      <Dialog open={true} onClose={onClose}>
        <div>Content</div>
      </Dialog>,
    );
    await nextTick();

    clickOutside();

    expect(onClose).toHaveBeenCalled();
  });

  test('disableBackdropClick blocks outside-click dismissal', async () => {
    // A dialog like NewUser's mandatory onboarding must be genuinely modal:
    // blocking Escape but letting a backdrop click through routes onClose
    // anyway (and in NewUser's case triggered an implicit submit).
    const onClose = rs.fn();
    render(
      <Dialog open={true} onClose={onClose} disableEscapeKeyDown disableBackdropClick>
        <div>Content</div>
      </Dialog>,
    );
    await nextTick();

    clickOutside();

    expect(onClose).not.toHaveBeenCalled();
  });
});

describe('DialogTitle', () => {
  // DialogTitle uses RadixDialog.Title which requires a Dialog context
  test('renders children within Dialog', () => {
    render(
      <Dialog open={true}>
        <DialogTitle>My Title</DialogTitle>
      </Dialog>,
    );
    expect(screen.getByText('My Title')).not.toBeNull();
  });

  test('applies id attribute within Dialog', () => {
    render(
      <Dialog open={true}>
        <DialogTitle id="test-title">Title</DialogTitle>
      </Dialog>,
    );
    const title = screen.getByText('Title');
    expect(title.id).toBe('test-title');
  });

  test('applies custom className within Dialog', () => {
    render(
      <Dialog open={true}>
        <DialogTitle className="custom">Title</DialogTitle>
      </Dialog>,
    );
    const title = screen.getByText('Title');
    expect(title.className).toContain('custom');
  });
});

describe('DialogContent', () => {
  test('renders children', () => {
    render(<DialogContent>Content area</DialogContent>);
    expect(screen.getByText('Content area')).not.toBeNull();
  });
});

describe('DialogContentText', () => {
  test('renders as a paragraph', () => {
    render(<DialogContentText>Some text</DialogContentText>);
    const p = screen.getByText('Some text');
    expect(p.tagName).toBe('P');
  });
});

describe('DialogActions', () => {
  test('renders children', () => {
    render(
      <DialogActions>
        <button>OK</button>
      </DialogActions>,
    );
    expect(screen.getByText('OK')).not.toBeNull();
  });
});
