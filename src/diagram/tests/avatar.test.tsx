// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import { render, screen } from '@testing-library/react';
import Avatar from '../components/Avatar';

describe('Avatar', () => {
  test('renders an image when src is provided', () => {
    render(<Avatar src="https://example.com/photo.jpg" alt="User" />);
    const img = screen.getByRole('img');
    expect(img.getAttribute('src')).toBe('https://example.com/photo.jpg');
    expect(img.getAttribute('alt')).toBe('User');
  });

  test('renders children when no src is provided', () => {
    render(<Avatar>AB</Avatar>);
    expect(screen.getByText('AB')).not.toBeNull();
  });

  test('prefers image over children when src is provided', () => {
    render(
      <Avatar src="https://example.com/photo.jpg" alt="User">
        AB
      </Avatar>,
    );
    expect(screen.getByRole('img')).not.toBeNull();
    expect(screen.queryByText('AB')).toBeNull();
  });

  test('applies custom className', () => {
    const { container } = render(<Avatar className="custom-avatar">AB</Avatar>);
    const div = container.firstChild as HTMLElement;
    expect(div.className).toContain('custom-avatar');
  });

  test('applies custom style', () => {
    const { container } = render(<Avatar style={{ width: 64, height: 64 }}>AB</Avatar>);
    const div = container.firstChild as HTMLElement;
    expect(div.style.width).toBe('64px');
  });

  test('uses empty alt when alt is not provided', () => {
    const { container } = render(<Avatar src="https://example.com/photo.jpg" />);
    // Empty alt="" gives the image a presentation role, so query the DOM directly
    const img = container.querySelector('img')!;
    expect(img.getAttribute('alt')).toBe('');
  });
});
