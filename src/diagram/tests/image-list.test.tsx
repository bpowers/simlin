// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import { render, screen } from '@testing-library/react';
import ImageList, { ImageListItem } from '../components/ImageList';

describe('ImageList', () => {
  test('renders as a list element', () => {
    render(
      <ImageList>
        <ImageListItem>Item 1</ImageListItem>
      </ImageList>,
    );
    const list = screen.getByRole('list');
    expect(list.tagName).toBe('UL');
  });

  test('renders children', () => {
    render(
      <ImageList>
        <ImageListItem>Item 1</ImageListItem>
        <ImageListItem>Item 2</ImageListItem>
      </ImageList>,
    );
    expect(screen.getByText('Item 1')).not.toBeNull();
    expect(screen.getByText('Item 2')).not.toBeNull();
  });

  test('sets grid-template-columns based on cols prop', () => {
    const { container } = render(
      <ImageList cols={3}>
        <ImageListItem>Item</ImageListItem>
      </ImageList>,
    );
    const list = container.querySelector('ul')!;
    expect(list.style.gridTemplateColumns).toBe('repeat(3, 1fr)');
  });

  test('defaults to 2 columns', () => {
    const { container } = render(
      <ImageList>
        <ImageListItem>Item</ImageListItem>
      </ImageList>,
    );
    const list = container.querySelector('ul')!;
    expect(list.style.gridTemplateColumns).toBe('repeat(2, 1fr)');
  });

  test('sets gap based on gap prop', () => {
    const { container } = render(
      <ImageList gap={16}>
        <ImageListItem>Item</ImageListItem>
      </ImageList>,
    );
    const list = container.querySelector('ul')!;
    expect(list.style.gap).toBe('16px');
  });

  test('applies custom className', () => {
    const { container } = render(
      <ImageList className="custom-grid">
        <ImageListItem>Item</ImageListItem>
      </ImageList>,
    );
    const list = container.querySelector('ul')!;
    expect(list.className).toContain('custom-grid');
  });
});

describe('ImageListItem', () => {
  test('renders as a list item', () => {
    render(
      <ImageList>
        <ImageListItem>Content</ImageListItem>
      </ImageList>,
    );
    const item = screen.getByText('Content');
    expect(item.tagName).toBe('LI');
  });

  test('applies imageListItem class', () => {
    render(
      <ImageList>
        <ImageListItem>Content</ImageListItem>
      </ImageList>,
    );
    const item = screen.getByText('Content');
    expect(item.className).toContain('imageListItem');
  });
});
