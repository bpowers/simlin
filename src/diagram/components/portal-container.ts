// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Where the Editor's overlay surfaces (drawer, dialogs, menus, the
 * autocomplete listbox) render, and what they position against.
 *
 * Two modes, decided by one value:
 *
 * - **Viewport mode** (`null` / `document.body`, the default): surfaces portal
 *   to `document.body` and are `position: fixed` against the browser viewport.
 *   Right for hosts that own the page (the app, simlin-serve).
 * - **Contained mode** (any other element): surfaces portal INTO that element
 *   and are `position: absolute` against it -- the drawer slides in from its
 *   left edge, dialogs centre in it, menus and listboxes are placed relative
 *   to it. For a host that gives the Editor one box on a page it does not own
 *   (a notebook cell): the host's tokens, `data-theme` and shortcut-scoping
 *   attributes on that box reach the surfaces (a portal to `body` leaves every
 *   `var(--*)` undefined), and a `transform` on some page ancestor
 *   (JupyterLab's windowed notebook translates its viewport) cannot displace
 *   fixed-position boxes, because nothing is fixed. The container MUST be a
 *   positioned element (the containing block for the absolute surfaces).
 *
 * The context is provided by the Editor from its `portalContainer` prop; the
 * surfaces read it through {@link usePortalContainer}. Surfaces rendered
 * outside any Editor (the app's own menus) see the default and stay
 * viewport-level.
 */

import * as React from 'react';

/** The element overlays render into; `null` means the default, `document.body`. */
export const PortalContainerContext = React.createContext<HTMLElement | null>(null);

export interface PortalTarget {
  /** The DOM node to portal into. */
  container: HTMLElement;
  /** True when `container` is a host box rather than `document.body`. */
  contained: boolean;
}

/**
 * Resolve the portal target for the current tree. Client-only (reads
 * `document.body`), which every portaling component already is.
 */
export function usePortalContainer(): PortalTarget {
  const provided = React.useContext(PortalContainerContext);
  // Memoized so consumers can hold it in hook dependency lists without
  // re-running effects every render.
  return React.useMemo(() => resolvePortalTarget(provided), [provided]);
}

/** The pure decision behind {@link usePortalContainer}: body (or nothing) is viewport mode. */
export function resolvePortalTarget(provided: HTMLElement | null): PortalTarget {
  if (provided === null || provided === document.body) {
    return { container: document.body, contained: false };
  }
  return { container: provided, contained: true };
}

/**
 * The box an anchored overlay's offsets are measured from, in viewport
 * coordinates, plus the scroll offsets an absolutely positioned child of a
 * scrolled container has to add. Viewport mode: the viewport itself (a fixed
 * box needs no scroll correction). Contained mode: the container's padding
 * box -- the containing block of its absolutely positioned children.
 */
export interface OverlayBox {
  top: number;
  left: number;
  width: number;
  height: number;
  scrollTop: number;
  scrollLeft: number;
}

export function overlayBoxFor(target: PortalTarget): OverlayBox {
  if (!target.contained) {
    return { top: 0, left: 0, width: window.innerWidth, height: window.innerHeight, scrollTop: 0, scrollLeft: 0 };
  }
  const el = target.container;
  const rect = el.getBoundingClientRect();
  return {
    // getBoundingClientRect is the border box; absolute children position
    // against the padding box, which starts clientTop/clientLeft further in.
    top: rect.top + el.clientTop,
    left: rect.left + el.clientLeft,
    width: el.clientWidth,
    height: el.clientHeight,
    scrollTop: el.scrollTop,
    scrollLeft: el.scrollLeft,
  };
}

/** The four edges of an anchor element, in viewport coordinates (a `DOMRect` subset). */
export interface AnchorRect {
  top: number;
  bottom: number;
  left: number;
  right: number;
}

/**
 * Offsets that pin an overlay to an anchor inside `box`, for use as CSS
 * `top`/`bottom`/`left`/`right` (fixed in viewport mode, absolute in
 * contained mode -- the same numbers serve both, which is the point of
 * measuring against `box`): `top` puts the overlay's top edge at the anchor's
 * bottom edge, `bottom` its bottom edge at the anchor's top edge, `left` its
 * left edge at the anchor's left edge, `right` its right edge at the anchor's
 * right edge. A caller uses one of each pair.
 */
export interface AnchoredOffsets {
  top: number;
  bottom: number;
  left: number;
  right: number;
}

export function anchoredOffsets(anchor: AnchorRect, box: OverlayBox): AnchoredOffsets {
  // An absolute child at `top: t` inside a container scrolled by s appears at
  // box.top + t - s on screen; solve for t (and likewise for the others).
  return {
    top: anchor.bottom - box.top + box.scrollTop,
    left: anchor.left - box.left + box.scrollLeft,
    bottom: box.top + box.height - anchor.top - box.scrollTop,
    right: box.left + box.width - anchor.right - box.scrollLeft,
  };
}

/** The CSS `position` an overlay uses in each mode. */
export function overlayPosition(target: PortalTarget): 'fixed' | 'absolute' {
  return target.contained ? 'absolute' : 'fixed';
}
