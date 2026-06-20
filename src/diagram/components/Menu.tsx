// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import ReactDOM from 'react-dom';
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

// Radix closed the menu automatically when an item was chosen; MenuItem reads
// this to reproduce that close-on-select without each caller wiring onClose.
const MenuCloseContext = React.createContext<(() => void) | undefined>(undefined);

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
  const contentRef = React.useRef<HTMLDivElement>(null);

  // Dismiss on Escape or a pointer press outside the menu, only while open.
  // The listeners are registered after the render that mounts the menu, so the
  // click that opened it (already past its mousedown) never self-closes it.
  React.useEffect(() => {
    if (!open) {
      return;
    }
    const onKeyDown = (event: KeyboardEvent): void => {
      if (event.key === 'Escape') {
        onClose();
        // Keyboard dismissal returns focus to the trigger (Radix did this);
        // outside-click dismissal does not, so the clicked element keeps focus.
        anchorEl?.focus();
      }
    };
    const onMouseDown = (event: MouseEvent): void => {
      const target = event.target as Node;
      const content = contentRef.current;
      // The anchor is logically part of the trigger, so a press on it is not
      // "outside": treating it as outside would close the menu on the same
      // press that a toggling trigger handler then reopens (a redundant
      // close->reopen flicker), and would defeat a trigger meant to toggle.
      if (content && content.contains(target)) {
        return;
      }
      if (anchorEl && anchorEl.contains(target)) {
        return;
      }
      onClose();
    };
    document.addEventListener('keydown', onKeyDown);
    document.addEventListener('mousedown', onMouseDown);
    return () => {
      document.removeEventListener('keydown', onKeyDown);
      document.removeEventListener('mousedown', onMouseDown);
    };
  }, [open, onClose, anchorEl]);

  const enabledItems = React.useCallback(
    (): HTMLElement[] =>
      Array.from(
        contentRef.current?.querySelectorAll<HTMLElement>('[role="menuitem"]:not([aria-disabled="true"])') ?? [],
      ),
    [],
  );

  // Move focus into the menu when it opens so keyboard users land on it; the
  // portal is appended at the end of <body>, so without this they would have to
  // tab through the whole page to reach it.
  React.useEffect(() => {
    if (!open) {
      return;
    }
    enabledItems()[0]?.focus();
  }, [open, enabledItems]);

  const onContentKeyDown = (event: React.KeyboardEvent<HTMLDivElement>): void => {
    const items = enabledItems();
    if (items.length === 0) {
      return;
    }
    const current = items.indexOf(document.activeElement as HTMLElement);
    switch (event.key) {
      case 'ArrowDown':
        event.preventDefault();
        items[(current + 1) % items.length]?.focus();
        break;
      case 'ArrowUp':
        event.preventDefault();
        items[(current - 1 + items.length) % items.length]?.focus();
        break;
      case 'Home':
        event.preventDefault();
        items[0]?.focus();
        break;
      case 'End':
        event.preventDefault();
        items[items.length - 1]?.focus();
        break;
      default:
        break;
    }
  };

  // Tabbing (or any focus move) out of the menu dismisses it. A move BETWEEN
  // items, or back to the anchor (the Escape path focuses it), is not "out".
  const onContentBlur = (event: React.FocusEvent<HTMLDivElement>): void => {
    const next = event.relatedTarget as Node | null;
    if (!next) {
      return;
    }
    if (contentRef.current?.contains(next) || anchorEl?.contains(next)) {
      return;
    }
    onClose();
  };

  // Position the content as a fixed overlay derived directly from the live
  // anchor rect -- top/bottom from the vertical origin, left/right from the
  // horizontal one -- so we never need to measure the menu's own size.
  const contentStyle: React.CSSProperties = { position: 'fixed', zIndex: 1300, ...style };
  if (anchorRect) {
    if (side === 'bottom') {
      contentStyle.top = anchorRect.bottom;
    } else {
      contentStyle.bottom = window.innerHeight - anchorRect.top;
    }
    if (align === 'start') {
      contentStyle.left = anchorRect.left;
    } else {
      contentStyle.right = window.innerWidth - anchorRect.right;
    }
  }

  return ReactDOM.createPortal(
    <>
      {/* The proxy carries aria-haspopup and mirrors the anchor rect; it gives
          assistive tech a stable popup owner and pins the anchor geometry the
          positioning test asserts against (issue #710). */}
      <span
        aria-haspopup="menu"
        style={{
          position: 'fixed',
          top: anchorRect?.bottom ?? 0,
          left: anchorRect?.left ?? 0,
          width: anchorRect?.width ?? 0,
          height: 0,
          pointerEvents: 'none',
        }}
      />
      {open && (
        <div
          ref={contentRef}
          id={id}
          role="menu"
          className={clsx(styles.menuContent, className)}
          style={contentStyle}
          onKeyDown={onContentKeyDown}
          onBlur={onContentBlur}
        >
          <MenuCloseContext.Provider value={onClose}>{children}</MenuCloseContext.Provider>
        </div>
      )}
    </>,
    document.body,
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
  const close = React.useContext(MenuCloseContext);

  const activate = (event: React.MouseEvent | React.KeyboardEvent): void => {
    if (disabled) {
      return;
    }
    if (onClick) {
      onClick(event as React.MouseEvent);
    }
    close?.();
  };

  return (
    <div
      role="menuitem"
      tabIndex={disabled ? -1 : 0}
      aria-disabled={disabled || undefined}
      data-disabled={disabled ? '' : undefined}
      className={clsx(styles.menuItem, className)}
      style={style}
      onClick={activate}
      onKeyDown={(event) => {
        if (event.key === 'Enter' || event.key === ' ') {
          event.preventDefault();
          activate(event);
        }
      }}
    >
      {children}
    </div>
  );
}
