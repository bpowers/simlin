// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// The overlay-surface contract (components/portal-container.ts): every surface
// the Editor portals -- the Drawer, the Dialog, the Menu, the Autocomplete
// listbox -- renders into document.body and positions fixed by default, and
// renders INTO the provided container and positions absolute when a
// PortalContainerContext is set (the Editor sets it from its `portalContainer`
// prop). The four surfaces are the enumeration; each arm is driven for each of
// them, plus the pure geometry the anchored surfaces (Menu, Autocomplete)
// share. The stylesheet side of the modes (which class flips `position`) is
// read from the compiled CSS in editor-portal-container.test.tsx.

import { describe, it, expect, afterEach } from '@rstest/core';

import * as React from 'react';
import { render, fireEvent, screen } from '@testing-library/react';

import Autocomplete from '../components/Autocomplete';
import { Dialog } from '../components/Dialog';
import Drawer from '../components/Drawer';
import { Menu, MenuItem } from '../components/Menu';
import {
  PortalContainerContext,
  anchoredOffsets,
  overlayBoxFor,
  resolvePortalTarget,
  type OverlayBox,
} from '../components/portal-container';

// Nodes this file appends to <body> itself (host boxes, menu anchors); RTL's
// own cleanup unmounts what render() mounted.
const appended: HTMLElement[] = [];

function appendToBody<T extends HTMLElement>(el: T): T {
  document.body.appendChild(el);
  appended.push(el);
  return el;
}

function makeBox(): HTMLElement {
  const box = document.createElement('div');
  box.setAttribute('data-testid', 'host-box');
  return appendToBody(box);
}

afterEach(() => {
  for (const el of appended.splice(0)) {
    el.remove();
  }
});

describe('resolvePortalTarget', () => {
  it('null and document.body are viewport mode; any other element is contained', () => {
    expect(resolvePortalTarget(null)).toEqual({ container: document.body, contained: false });
    expect(resolvePortalTarget(document.body)).toEqual({ container: document.body, contained: false });
    const box = makeBox();
    expect(resolvePortalTarget(box)).toEqual({ container: box, contained: true });
  });
});

describe('overlayBoxFor', () => {
  it('viewport mode is the window with no scroll correction', () => {
    expect(overlayBoxFor({ container: document.body, contained: false })).toEqual({
      top: 0,
      left: 0,
      width: window.innerWidth,
      height: window.innerHeight,
      scrollTop: 0,
      scrollLeft: 0,
    });
  });

  it("contained mode is the container's padding box (border box inset by clientTop/Left) plus its scroll offsets", () => {
    const box = makeBox();
    box.getBoundingClientRect = () =>
      ({
        top: 100,
        left: 40,
        width: 500,
        height: 300,
        bottom: 400,
        right: 540,
        x: 40,
        y: 100,
        toJSON: () => ({}),
      }) as DOMRect;
    // jsdom has no layout; the client metrics are what a 1px-bordered,
    // scrolled box would report.
    Object.defineProperties(box, {
      clientTop: { value: 1 },
      clientLeft: { value: 1 },
      clientWidth: { value: 498 },
      clientHeight: { value: 298 },
      scrollTop: { value: 20, writable: true },
      scrollLeft: { value: 5, writable: true },
    });
    expect(overlayBoxFor({ container: box, contained: true })).toEqual({
      top: 101,
      left: 41,
      width: 498,
      height: 298,
      scrollTop: 20,
      scrollLeft: 5,
    });
  });
});

describe('anchoredOffsets', () => {
  const anchor = { top: 150, bottom: 170, left: 60, right: 100 };

  it('viewport mode reproduces fixed-position anchoring (top/left at the anchor, bottom/right from the far edges)', () => {
    const box: OverlayBox = { top: 0, left: 0, width: 1280, height: 720, scrollTop: 0, scrollLeft: 0 };
    expect(anchoredOffsets(anchor, box)).toEqual({ top: 170, left: 60, bottom: 720 - 150, right: 1280 - 100 });
  });

  it('contained mode measures from the box edges and adds the scroll offsets, so the overlay lands on the anchor on screen', () => {
    const box: OverlayBox = { top: 101, left: 41, width: 498, height: 298, scrollTop: 20, scrollLeft: 5 };
    const o = anchoredOffsets(anchor, box);
    // Screen position of an absolute child at `top: t` is box.top + t - scrollTop.
    expect(box.top + o.top - box.scrollTop).toBe(anchor.bottom);
    expect(box.left + o.left - box.scrollLeft).toBe(anchor.left);
    // and of one at `bottom: b` its bottom edge is box.top + box.height - b - scrollTop.
    expect(box.top + box.height - o.bottom - box.scrollTop).toBe(anchor.top);
    expect(box.left + box.width - o.right - box.scrollLeft).toBe(anchor.right);
  });
});

// The four portaled surfaces, each rendered open, with a locator for the DOM
// node that carries the mode (the one whose `position` flips).
const surfaces: Array<{
  name: string;
  render: () => React.ReactElement;
  node: () => HTMLElement;
  // The mode-carrying class or inline style the surface applies in contained mode.
  containedMarker: (node: HTMLElement) => boolean;
}> = [
  {
    name: 'Drawer',
    render: () => (
      <Drawer open onClose={() => {}}>
        <div>drawer content</div>
      </Drawer>
    ),
    node: () => document.querySelector('[role="dialog"]') as HTMLElement,
    containedMarker: (node) => node.classList.contains('contained'),
  },
  {
    name: 'Dialog',
    render: () => (
      <Dialog open>
        <div>dialog content</div>
      </Dialog>
    ),
    node: () => document.querySelector('[role="dialog"]') as HTMLElement,
    containedMarker: (node) => node.classList.contains('contained'),
  },
  {
    name: 'Menu',
    render: () => {
      const anchor = appendToBody(document.createElement('button'));
      return (
        <Menu anchorEl={anchor} open onClose={() => {}}>
          <MenuItem>one</MenuItem>
        </Menu>
      );
    },
    node: () => document.querySelector('[role="menu"]') as HTMLElement,
    containedMarker: (node) => node.style.position === 'absolute',
  },
  {
    name: 'Autocomplete listbox',
    render: () => (
      <Autocomplete
        value={null}
        options={['apple', 'apricot']}
        onChange={() => {}}
        renderInput={(params) => (
          <div ref={params.InputProps.ref}>
            <input {...params.inputProps} data-testid="ac-input" />
          </div>
        )}
      />
    ),
    node: () => {
      // Typing opens the listbox.
      fireEvent.change(screen.getByTestId('ac-input'), { target: { value: 'ap' } });
      return document.querySelector('ul[role="listbox"]') as HTMLElement;
    },
    containedMarker: (node) => node.style.position === 'absolute',
  },
];

describe.each(surfaces)('$name', ({ render: renderSurface, node, containedMarker }) => {
  it('portals to document.body and positions fixed with no container in the tree', () => {
    const box = makeBox();
    render(renderSurface());
    const el = node();
    expect(el).not.toBeNull();
    expect(box.contains(el)).toBe(false);
    expect(document.body.contains(el)).toBe(true);
    expect(containedMarker(el)).toBe(false);
  });

  it('portals into the provided container and positions absolute when a PortalContainerContext is set', () => {
    const box = makeBox();
    render(<PortalContainerContext.Provider value={box}>{renderSurface()}</PortalContainerContext.Provider>);
    const el = node();
    expect(el).not.toBeNull();
    expect(box.contains(el)).toBe(true);
    expect(containedMarker(el)).toBe(true);
  });
});
