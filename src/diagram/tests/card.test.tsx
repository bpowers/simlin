// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import { render, screen } from '@testing-library/react';
import Card, { CardContent, CardActions } from '../components/Card';

describe('Card', () => {
  test('renders children', () => {
    render(<Card>Card content</Card>);
    expect(screen.getByText('Card content')).not.toBeNull();
  });

  test('applies elevation variant by default', () => {
    const { container } = render(<Card>Content</Card>);
    const card = container.firstChild as HTMLElement;
    expect(card.className).toContain('elevation');
  });

  test('applies outlined variant', () => {
    const { container } = render(<Card variant="outlined">Content</Card>);
    const card = container.firstChild as HTMLElement;
    expect(card.className).toContain('outlined');
    expect(card.className).not.toContain('elevation');
  });

  test('applies custom className', () => {
    const { container } = render(<Card className="custom">Content</Card>);
    const card = container.firstChild as HTMLElement;
    expect(card.className).toContain('custom');
  });

  test('applies custom style', () => {
    const { container } = render(<Card style={{ maxWidth: 300 }}>Content</Card>);
    const card = container.firstChild as HTMLElement;
    expect(card.style.maxWidth).toBe('300px');
  });
});

describe('CardContent', () => {
  test('renders children', () => {
    render(<CardContent>Inner content</CardContent>);
    expect(screen.getByText('Inner content')).not.toBeNull();
  });

  test('applies cardContent class', () => {
    const { container } = render(<CardContent>Content</CardContent>);
    const div = container.firstChild as HTMLElement;
    expect(div.className).toContain('cardContent');
  });

  test('applies custom className', () => {
    const { container } = render(<CardContent className="custom">Content</CardContent>);
    const div = container.firstChild as HTMLElement;
    expect(div.className).toContain('custom');
  });
});

describe('CardActions', () => {
  test('renders children', () => {
    render(
      <CardActions>
        <button>Action</button>
      </CardActions>,
    );
    expect(screen.getByText('Action')).not.toBeNull();
  });

  test('applies cardActions class', () => {
    const { container } = render(
      <CardActions>
        <button>Action</button>
      </CardActions>,
    );
    const div = container.firstChild as HTMLElement;
    expect(div.className).toContain('cardActions');
  });

  test('applies custom className', () => {
    const { container } = render(
      <CardActions className="custom">
        <button>Action</button>
      </CardActions>,
    );
    const div = container.firstChild as HTMLElement;
    expect(div.className).toContain('custom');
  });
});
