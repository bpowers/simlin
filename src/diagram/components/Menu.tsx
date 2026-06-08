// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import * as DropdownMenu from '@radix-ui/react-dropdown-menu';
import clsx from 'clsx';

import styles from './Menu.module.css';

export interface MenuProps {
  anchorEl: HTMLElement | null;
  open: boolean;
  onClose: () => void;
  anchorOrigin?: { vertical: 'top' | 'bottom'; horizontal: 'left' | 'right' };
  transformOrigin?: { vertical: 'top' | 'bottom'; horizontal: 'left' | 'right' };
  id?: string;
  className?: string;
  style?: React.CSSProperties;
  children?: React.ReactNode;
}

/**
 * Track an anchor element's viewport rect live while a menu is open.
 *
 * The rect must follow the anchor if it moves while the menu is open (window
 * resize, scroll of a non-fixed scroll container, layout reflow). Memoizing on
 * `anchorEl` identity alone captures a stale snapshot that never re-measures, so
 * the menu detaches from its trigger (issue #710). Re-measure on scroll
 * (capture phase, so nested scroll containers count, not just window) and
 * resize, and observe the anchor's own size changes. `useLayoutEffect`
 * re-measures synchronously before paint, so the first open positions correctly
 * without a visible jump.
 */
function useAnchorRect(anchorEl: HTMLElement | null, open: boolean): DOMRect | undefined {
  const [rect, setRect] = React.useState<DOMRect | undefined>(undefined);

  React.useLayoutEffect(() => {
    if (!open || !anchorEl) {
      return;
    }
    // Re-measure, but bail out (return the previous reference) when nothing
    // changed so a stream of scroll/resize ticks over a stationary anchor does
    // not force a re-render on every event -- this fires hottest exactly in the
    // scrollable-region case the issue is about.
    const update = (): void => {
      const next = anchorEl.getBoundingClientRect();
      setRect((prev) =>
        prev &&
        prev.top === next.top &&
        prev.left === next.left &&
        prev.width === next.width &&
        prev.height === next.height
          ? prev
          : next,
      );
    };
    update();

    window.addEventListener('scroll', update, true);
    window.addEventListener('resize', update);
    const observer = typeof ResizeObserver !== 'undefined' ? new ResizeObserver(update) : undefined;
    observer?.observe(anchorEl);

    return () => {
      window.removeEventListener('scroll', update, true);
      window.removeEventListener('resize', update);
      observer?.disconnect();
    };
  }, [anchorEl, open]);

  return rect;
}

export function Menu(props: MenuProps): React.ReactElement {
  const { anchorEl, open, onClose, anchorOrigin, id, className, style, children } = props;

  const side = anchorOrigin?.vertical === 'top' ? 'top' : 'bottom';
  const align = anchorOrigin?.horizontal === 'right' ? 'end' : 'start';

  const anchorRect = useAnchorRect(anchorEl, open);

  return (
    <DropdownMenu.Root open={open} onOpenChange={(isOpen) => !isOpen && onClose()}>
      <DropdownMenu.Trigger asChild>
        <span
          style={{
            position: 'fixed',
            top: anchorRect?.bottom ?? 0,
            left: anchorRect?.left ?? 0,
            width: anchorRect?.width ?? 0,
            height: 0,
            pointerEvents: 'none',
          }}
        />
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          id={id}
          className={clsx(styles.menuContent, className)}
          style={style}
          side={side}
          align={align}
          sideOffset={0}
        >
          {children}
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}

export interface MenuItemProps {
  onClick?: (event: React.MouseEvent) => void;
  disabled?: boolean;
  className?: string;
  style?: React.CSSProperties;
  children?: React.ReactNode;
}

export function MenuItem(props: MenuItemProps): React.ReactElement {
  const { onClick, disabled, className, style, children } = props;

  return (
    <DropdownMenu.Item
      className={clsx(styles.menuItem, className)}
      style={style}
      disabled={disabled}
      onSelect={(event) => {
        if (onClick) {
          onClick(event as unknown as React.MouseEvent);
        }
      }}
    >
      {children}
    </DropdownMenu.Item>
  );
}
