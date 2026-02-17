// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import { render } from '@testing-library/react';
import SvgIcon from '../components/SvgIcon';

describe('SvgIcon', () => {
  test('renders an svg element', () => {
    const { container } = render(
      <SvgIcon>
        <path d="M10 20v-6h4v6h5v-8h3L12 3 2 12h3v8z" />
      </SvgIcon>,
    );
    const svg = container.querySelector('svg');
    expect(svg).not.toBeNull();
  });

  test('applies default viewBox', () => {
    const { container } = render(
      <SvgIcon>
        <path d="M10 20v-6h4v6h5v-8h3L12 3 2 12h3v8z" />
      </SvgIcon>,
    );
    const svg = container.querySelector('svg')!;
    expect(svg.getAttribute('viewBox')).toBe('0 0 24 24');
  });

  test('applies custom viewBox', () => {
    const { container } = render(
      <SvgIcon viewBox="0 0 48 48">
        <path d="M10 20" />
      </SvgIcon>,
    );
    const svg = container.querySelector('svg')!;
    expect(svg.getAttribute('viewBox')).toBe('0 0 48 48');
  });

  test('sets aria-hidden', () => {
    const { container } = render(
      <SvgIcon>
        <path d="M10 20" />
      </SvgIcon>,
    );
    const svg = container.querySelector('svg')!;
    expect(svg.getAttribute('aria-hidden')).toBe('true');
  });

  test('sets focusable to false', () => {
    const { container } = render(
      <SvgIcon>
        <path d="M10 20" />
      </SvgIcon>,
    );
    const svg = container.querySelector('svg')!;
    expect(svg.getAttribute('focusable')).toBe('false');
  });

  test('applies svgIcon class', () => {
    const { container } = render(
      <SvgIcon>
        <path d="M10 20" />
      </SvgIcon>,
    );
    const svg = container.querySelector('svg')!;
    expect(svg.className.baseVal).toContain('svgIcon');
  });

  test('applies custom className', () => {
    const { container } = render(
      <SvgIcon className="custom-icon">
        <path d="M10 20" />
      </SvgIcon>,
    );
    const svg = container.querySelector('svg')!;
    expect(svg.className.baseVal).toContain('custom-icon');
  });
});
