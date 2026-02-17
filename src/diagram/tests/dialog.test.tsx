// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import { render, screen } from '@testing-library/react';
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

  test('applies dialogContent class', () => {
    const { container } = render(<DialogContent>Content</DialogContent>);
    const div = container.firstChild as HTMLElement;
    expect(div.className).toContain('dialogContent');
  });
});

describe('DialogContentText', () => {
  test('renders as a paragraph', () => {
    render(<DialogContentText>Some text</DialogContentText>);
    const p = screen.getByText('Some text');
    expect(p.tagName).toBe('P');
  });

  test('applies contentText class', () => {
    render(<DialogContentText>Text</DialogContentText>);
    const p = screen.getByText('Text');
    expect(p.className).toContain('contentText');
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

  test('applies actions class', () => {
    const { container } = render(
      <DialogActions>
        <button>OK</button>
      </DialogActions>,
    );
    const div = container.firstChild as HTMLElement;
    expect(div.className).toContain('actions');
  });
});
