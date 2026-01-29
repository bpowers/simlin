// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import clsx from 'clsx';

import styles from './SpeedDial.module.css';

export type CloseReason = 'toggle' | 'blur' | 'mouseLeave' | 'escapeKeyDown' | 'actionClick';

interface SpeedDialProps {
  ariaLabel: string;
  className?: string;
  hidden?: boolean;
  icon: React.ReactNode;
  onClick?: (event: React.MouseEvent<HTMLButtonElement>) => void;
  onClose?: (event: React.SyntheticEvent, reason: CloseReason) => void;
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

  handleActionClick = (event: React.MouseEvent<HTMLButtonElement>) => {
    this.props.onClose?.(event, 'actionClick');
  };

  render() {
    const { ariaLabel, className, hidden, icon, onClick, open, children } = this.props;

    const enrichedIcon = React.isValidElement<SpeedDialIconProps>(icon)
      ? React.cloneElement(icon, { open })
      : icon;

    // Inject onActionClick into children so they can close the dial
    const enrichedChildren = React.Children.map(children, (child) => {
      if (React.isValidElement<SpeedDialActionProps>(child)) {
        return React.cloneElement(child, {
          _onActionClick: this.handleActionClick,
        });
      }
      return child;
    });

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
          aria-expanded={open}
          onClick={onClick}
          onBlur={this.handleBlur}
          type="button"
        >
          {enrichedIcon}
        </button>
        {open && (
          <div className={styles.actions} role="menu">
            {enrichedChildren}
          </div>
        )}
      </div>
    );
  }
}

interface SpeedDialActionProps {
  icon: React.ReactNode;
  title: string;
  onClick?: (event: React.MouseEvent<HTMLButtonElement>) => void;
  className?: string;
  /** @internal Injected by SpeedDial parent via cloneElement */
  _onActionClick?: (event: React.MouseEvent<HTMLButtonElement>) => void;
}

export class SpeedDialAction extends React.PureComponent<SpeedDialActionProps> {
  handleClick = (event: React.MouseEvent<HTMLButtonElement>) => {
    const { onClick, _onActionClick } = this.props;

    onClick?.(event);
    _onActionClick?.(event);
  };

  render() {
    const { icon, title, className } = this.props;
    return (
      <div className={styles.action} role="menuitem">
        <button
          className={clsx(styles.actionButton, styles.actionButtonOpen, className)}
          onClick={this.handleClick}
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
      return <span className={styles.iconWrapper}>{open ? openIcon : icon}</span>;
    }

    return <span className={clsx(styles.iconWrapper, open && styles.iconWrapperOpen)}>{icon}</span>;
  }
}
