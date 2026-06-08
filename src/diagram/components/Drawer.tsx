// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import ReactDOM from 'react-dom';

import clsx from 'clsx';

import styles from './Drawer.module.css';

interface DrawerProps {
  open: boolean;
  onOpen?: () => void;
  onClose: () => void;
  children?: React.ReactNode;
}

export default function Drawer(props: DrawerProps): React.ReactElement {
  const { open, onClose, children } = props;

  const panelRef = React.useRef<HTMLDivElement>(null);
  // Remembers the element focused before the drawer opened so focus can be
  // restored on close. Held in a ref (not state) because it is read/written
  // only from effects and never affects rendering.
  const previousActiveElement = React.useRef<Element | null>(null);

  // Manage focus on open/close transitions: on open, save the prior focus and
  // move focus into the panel; on close, restore the previously-focused element.
  // Keyed on `open` so it runs once per transition (the class did this in
  // componentDidUpdate).
  React.useEffect(() => {
    if (open) {
      // Guard against React StrictMode double-invoking this mount effect: the
      // first run saves the real prior focus and focuses the panel, so on the
      // second run document.activeElement IS the panel. Skip the save in that
      // case, otherwise we'd overwrite previousActiveElement with the panel
      // itself and a later close would "restore" focus to the hidden drawer.
      if (document.activeElement !== panelRef.current) {
        previousActiveElement.current = document.activeElement;
      }
      panelRef.current?.focus();
    } else {
      if (previousActiveElement.current instanceof HTMLElement) {
        previousActiveElement.current.focus();
      }
      previousActiveElement.current = null;
    }
  }, [open]);

  React.useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent): void => {
      if (event.key === 'Escape' && open) {
        onClose();
      }

      // Focus trap: when Tab is pressed and drawer is open, keep focus within the panel
      if (event.key === 'Tab' && open && panelRef.current) {
        const panel = panelRef.current;
        const focusableElements = panel.querySelectorAll<HTMLElement>(
          'a, button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"]), [contenteditable]',
        );

        if (focusableElements.length === 0) {
          event.preventDefault();
          return;
        }

        const firstElement = focusableElements[0];
        const lastElement = focusableElements[focusableElements.length - 1];

        if (event.shiftKey && document.activeElement === firstElement) {
          event.preventDefault();
          lastElement.focus();
        } else if (!event.shiftKey && document.activeElement === lastElement) {
          event.preventDefault();
          firstElement.focus();
        }
      }
    };

    document.addEventListener('keydown', handleKeyDown);
    return () => document.removeEventListener('keydown', handleKeyDown);
  }, [open, onClose]);

  const handleBackdropClick = (): void => {
    onClose();
  };

  const content = (
    <>
      <div
        className={clsx(styles.backdrop, !open && styles.backdropHidden)}
        onClick={handleBackdropClick}
        aria-hidden="true"
      />
      <div
        ref={panelRef}
        className={clsx(styles.panel, !open && styles.panelHidden)}
        role="dialog"
        aria-modal="true"
        tabIndex={-1}
      >
        {children}
      </div>
    </>
  );

  return ReactDOM.createPortal(content, document.body);
}
