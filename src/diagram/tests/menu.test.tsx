// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import { render, act } from '@testing-library/react';

import { Menu, MenuItem } from '../components/Menu';

// Install a controllable getBoundingClientRect on an element so we can simulate
// the anchor moving while the menu is open (issue #710). Returns a setter.
function mockRect(el: HTMLElement, initial: { top: number; left: number; width: number; height: number }) {
  let current = initial;
  el.getBoundingClientRect = () =>
    ({
      top: current.top,
      left: current.left,
      width: current.width,
      height: current.height,
      bottom: current.top + current.height,
      right: current.left + current.width,
      x: current.left,
      y: current.top,
      toJSON() {
        return current;
      },
    }) as DOMRect;
  return (next: { top: number; left: number; width: number; height: number }) => {
    current = next;
  };
}

// The proxy span Radix anchors to is the (asChild) trigger; it carries
// aria-haspopup="menu" and is the only position:fixed element we render.
function getProxySpan(): HTMLElement {
  const el = document.querySelector<HTMLElement>('[aria-haspopup="menu"]');
  if (!el) {
    throw new Error('proxy trigger span not found');
  }
  return el;
}

describe('Menu anchor positioning (issue #710)', () => {
  it('positions the proxy at the anchor rect on open', () => {
    const anchor = document.createElement('button');
    document.body.appendChild(anchor);
    mockRect(anchor, { top: 10, left: 20, width: 40, height: 8 });

    render(
      <Menu anchorEl={anchor} open onClose={() => {}}>
        <MenuItem>One</MenuItem>
      </Menu>,
    );

    const span = getProxySpan();
    // top is anchored to the rect's bottom (top + height), left to its left.
    expect(span.style.top).toBe('18px');
    expect(span.style.left).toBe('20px');
    expect(span.style.width).toBe('40px');

    anchor.remove();
  });

  it('re-measures and follows the anchor when it moves on scroll while open', () => {
    const anchor = document.createElement('button');
    document.body.appendChild(anchor);
    const setRect = mockRect(anchor, { top: 10, left: 20, width: 40, height: 8 });

    render(
      <Menu anchorEl={anchor} open onClose={() => {}}>
        <MenuItem>One</MenuItem>
      </Menu>,
    );

    expect(getProxySpan().style.top).toBe('18px');

    // The anchor moves (e.g. a scroll container shifts it up) and a scroll event
    // fires. The stale-snapshot bug would leave the proxy at the old position.
    setRect({ top: 100, left: 200, width: 40, height: 8 });
    act(() => {
      window.dispatchEvent(new Event('scroll'));
    });

    const span = getProxySpan();
    expect(span.style.top).toBe('108px');
    expect(span.style.left).toBe('200px');

    anchor.remove();
  });

  it('re-measures on window resize while open', () => {
    const anchor = document.createElement('button');
    document.body.appendChild(anchor);
    const setRect = mockRect(anchor, { top: 10, left: 20, width: 40, height: 8 });

    render(
      <Menu anchorEl={anchor} open onClose={() => {}}>
        <MenuItem>One</MenuItem>
      </Menu>,
    );

    setRect({ top: 10, left: 300, width: 40, height: 8 });
    act(() => {
      window.dispatchEvent(new Event('resize'));
    });

    expect(getProxySpan().style.left).toBe('300px');

    anchor.remove();
  });

  it('detaches its scroll/resize listeners when closed', () => {
    const anchor = document.createElement('button');
    document.body.appendChild(anchor);
    const setRect = mockRect(anchor, { top: 10, left: 20, width: 40, height: 8 });

    const { rerender } = render(
      <Menu anchorEl={anchor} open onClose={() => {}}>
        <MenuItem>One</MenuItem>
      </Menu>,
    );

    const spanWhileOpen = getProxySpan();
    expect(spanWhileOpen.style.top).toBe('18px');

    // Close the menu, then move the anchor and fire scroll. With listeners
    // detached, no stale work runs; reopening re-measures from scratch.
    rerender(
      <Menu anchorEl={anchor} open={false} onClose={() => {}}>
        <MenuItem>One</MenuItem>
      </Menu>,
    );

    setRect({ top: 500, left: 600, width: 40, height: 8 });
    act(() => {
      window.dispatchEvent(new Event('scroll'));
    });

    // Reopen: the layout effect re-measures synchronously to the current rect.
    rerender(
      <Menu anchorEl={anchor} open onClose={() => {}}>
        <MenuItem>One</MenuItem>
      </Menu>,
    );

    expect(getProxySpan().style.top).toBe('508px');
    expect(getProxySpan().style.left).toBe('600px');

    anchor.remove();
  });
});
