// Copyright 2025 The Simlin Authors. All rights reserved.
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
  componentDidMount() {
    document.addEventListener('keydown', this.handleKeyDown);
  }

  componentWillUnmount() {
    document.removeEventListener('keydown', this.handleKeyDown);
  }

  handleKeyDown = (event: KeyboardEvent) => {
    if (event.key === 'Escape' && this.props.open) {
      this.props.onClose();
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
        />
        <div className={clsx(styles.panel, !open && styles.panelHidden)}>
          {children}
        </div>
      </>
    );

    return ReactDOM.createPortal(content, document.body);
  }
}
