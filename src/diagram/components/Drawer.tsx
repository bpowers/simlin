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

export default class Drawer extends React.PureComponent<DrawerProps> {
  private panelRef = React.createRef<HTMLDivElement>();
  private previousActiveElement: Element | null = null;

  componentDidMount() {
    document.addEventListener('keydown', this.handleKeyDown);
  }

  componentDidUpdate(prevProps: DrawerProps) {
    if (this.props.open && !prevProps.open) {
      // Drawer just opened - save current focus and focus the panel
      this.previousActiveElement = document.activeElement;
      this.panelRef.current?.focus();
    } else if (!this.props.open && prevProps.open) {
      // Drawer just closed - restore focus
      if (this.previousActiveElement instanceof HTMLElement) {
        this.previousActiveElement.focus();
      }
      this.previousActiveElement = null;
    }
  }

  componentWillUnmount() {
    document.removeEventListener('keydown', this.handleKeyDown);
  }

  handleKeyDown = (event: KeyboardEvent) => {
    if (event.key === 'Escape' && this.props.open) {
      this.props.onClose();
    }

    // Focus trap: when Tab is pressed and drawer is open, keep focus within the panel
    if (event.key === 'Tab' && this.props.open && this.panelRef.current) {
      const panel = this.panelRef.current;
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

  handleBackdropClick = () => {
    this.props.onClose();
  };

  render() {
    const { open, children } = this.props;

    const content = (
      <>
        <div
          className={clsx(styles.backdrop, !open && styles.backdropHidden)}
          onClick={this.handleBackdropClick}
          aria-hidden="true"
        />
        <div
          ref={this.panelRef}
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
}
