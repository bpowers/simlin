// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import { render, fireEvent, screen } from '@testing-library/react';
import Button from '../components/Button';

describe('Button', () => {
  test('renders children text', () => {
    render(<Button>Click me</Button>);
    expect(screen.getByText('Click me')).not.toBeNull();
  });

  test('renders as a button element by default', () => {
    render(<Button>Test</Button>);
    const button = screen.getByRole('button');
    expect(button.tagName).toBe('BUTTON');
  });

  test('renders as a label when component="label"', () => {
    const { container } = render(<Button component="label">Label Button</Button>);
    const label = container.querySelector('label');
    expect(label).not.toBeNull();
    expect(label!.textContent).toBe('Label Button');
  });

  test('calls onClick when clicked', () => {
    const onClick = jest.fn();
    render(<Button onClick={onClick}>Click</Button>);
    fireEvent.click(screen.getByRole('button'));
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  test('does not call onClick when disabled', () => {
    const onClick = jest.fn();
    render(
      <Button onClick={onClick} disabled>
        Click
      </Button>,
    );
    fireEvent.click(screen.getByRole('button'));
    expect(onClick).not.toHaveBeenCalled();
  });

  test('applies contained primary classes', () => {
    render(
      <Button variant="contained" color="primary">
        Primary
      </Button>,
    );
    const button = screen.getByRole('button');
    expect(button.className).toContain('containedPrimary');
  });

  test('applies contained secondary classes', () => {
    render(
      <Button variant="contained" color="secondary">
        Secondary
      </Button>,
    );
    const button = screen.getByRole('button');
    expect(button.className).toContain('containedSecondary');
  });

  test('applies text variant by default', () => {
    render(<Button>Text</Button>);
    const button = screen.getByRole('button');
    expect(button.className).toContain('textPrimary');
  });

  test('applies outlined primary classes', () => {
    render(
      <Button variant="outlined" color="primary">
        Outlined
      </Button>,
    );
    const button = screen.getByRole('button');
    expect(button.className).toContain('outlinedPrimary');
  });

  test('applies outlined inherit classes', () => {
    render(
      <Button variant="outlined" color="inherit">
        Inherit
      </Button>,
    );
    const button = screen.getByRole('button');
    expect(button.className).toContain('outlinedInherit');
  });

  test('applies size classes', () => {
    const { rerender } = render(<Button size="small">Small</Button>);
    expect(screen.getByRole('button').className).toContain('sizeSmall');

    rerender(<Button size="large">Large</Button>);
    expect(screen.getByRole('button').className).toContain('sizeLarge');
  });

  test('applies medium size by default', () => {
    render(<Button>Medium</Button>);
    expect(screen.getByRole('button').className).toContain('sizeMedium');
  });

  test('renders startIcon', () => {
    render(<Button startIcon={<span data-testid="icon">★</span>}>With Icon</Button>);
    expect(screen.getByTestId('icon')).not.toBeNull();
    const iconWrapper = screen.getByTestId('icon').parentElement;
    expect(iconWrapper!.className).toContain('startIcon');
  });

  test('applies disabled class for text variant', () => {
    render(<Button disabled>Disabled</Button>);
    expect(screen.getByRole('button').className).toContain('disabledText');
  });

  test('applies disabled class for contained variant', () => {
    render(
      <Button variant="contained" disabled>
        Disabled
      </Button>,
    );
    expect(screen.getByRole('button').className).toContain('disabledContained');
  });

  test('applies disabled class for outlined variant', () => {
    render(
      <Button variant="outlined" disabled>
        Disabled
      </Button>,
    );
    expect(screen.getByRole('button').className).toContain('disabledOutlined');
  });

  test('applies custom className', () => {
    render(<Button className="custom">Custom</Button>);
    expect(screen.getByRole('button').className).toContain('custom');
  });

  test('passes through aria attributes', () => {
    render(
      <Button aria-label="test label" aria-haspopup="true">
        Aria
      </Button>,
    );
    const button = screen.getByRole('button');
    expect(button.getAttribute('aria-label')).toBe('test label');
    expect(button.getAttribute('aria-haspopup')).toBe('true');
  });

  test('sets button type', () => {
    render(<Button type="submit">Submit</Button>);
    expect(screen.getByRole('button').getAttribute('type')).toBe('submit');
  });

  test('defaults to type="button"', () => {
    render(<Button>Default</Button>);
    expect(screen.getByRole('button').getAttribute('type')).toBe('button');
  });
});
