// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import clsx from 'clsx';

import styles from './SpeedDial.module.css';

export type CloseReason = 'toggle' | 'blur' | 'mouseLeave' | 'escapeKeyDown';

interface SpeedDialProps {
  ariaLabel: string;
  className?: string;
  hidden?: boolean;
  icon: React.ReactNode;
  onClick?: (event: React.MouseEvent<HTMLDivElement, MouseEvent>) => void;
  onClose?: (event: React.SyntheticEvent<{}>, reason: CloseReason) => void;
  open: boolean;
  children?: React.ReactNode;
}

export default class SpeedDial extends React.PureComponent<SpeedDialProps> {
  handleMouseLeave = (event: React.MouseEvent<HTMLDivElement>) => {
    this.props.onClose?.(event, 'mouseLeave');
  };

  handleBlur = (event: React.FocusEvent<HTMLButtonElement>) => {
    this.props.onClose?.(event, 'blur');
  };

  handleKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if (event.key === 'Escape') {
      this.props.onClose?.(event, 'escapeKeyDown');
    }
  };

  render() {
    const { ariaLabel, className, hidden, icon, onClick, open, children } = this.props;

    const enrichedIcon = React.isValidElement(icon)
      ? React.cloneElement(icon as React.ReactElement<any>, { open })
      : icon;

    return (
      <div
        className={clsx(styles.speedDial, hidden && styles.speedDialHidden, className)}
        onMouseLeave={this.handleMouseLeave}
        onKeyDown={this.handleKeyDown}
        role="presentation"
      >
        <button
          className={styles.fab}
          aria-label={ariaLabel}
          onClick={onClick as any}
          onBlur={this.handleBlur}
          type="button"
        >
          {enrichedIcon}
        </button>
        {open && (
          <div className={styles.actions}>
            {React.Children.map(children, (child) => child)}
          </div>
        )}
      </div>
    );
  }
}

interface SpeedDialActionProps {
  icon: React.ReactNode;
  title: string;
  onClick?: (event: React.MouseEvent<HTMLDivElement>) => void;
  className?: string;
}

export class SpeedDialAction extends React.PureComponent<SpeedDialActionProps> {
  render() {
    const { icon, title, onClick, className } = this.props;
    return (
      <div className={styles.action}>
        <button
          className={clsx(styles.actionButton, styles.actionButtonOpen, className)}
          onClick={onClick as any}
          aria-label={title}
          type="button"
        >
          {icon}
        </button>
        <span className={clsx(styles.actionLabel, styles.actionLabelOpen)}>{title}</span>
      </div>
    );
  }
}

interface SpeedDialIconProps {
  icon: React.ReactNode;
  openIcon?: React.ReactNode;
  open?: boolean;
}

export class SpeedDialIcon extends React.PureComponent<SpeedDialIconProps> {
  render() {
    const { icon, openIcon, open } = this.props;

    if (openIcon) {
      return (
        <span className={styles.iconWrapper}>
          {open ? openIcon : icon}
        </span>
      );
    }

    return (
      <span className={clsx(styles.iconWrapper, open && styles.iconWrapperOpen)}>
        {icon}
      </span>
    );
  }
}
