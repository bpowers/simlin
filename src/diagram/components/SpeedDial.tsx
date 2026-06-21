// Copyright 2026 The Simlin Authors. All rights reserved.
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

export default function SpeedDial(props: SpeedDialProps): React.ReactElement {
  const { ariaLabel, className, hidden, icon, onClick, onClose, open, children } = props;

  const handleMouseLeave = (event: React.MouseEvent<HTMLDivElement>): void => {
    onClose?.(event, 'mouseLeave');
  };

  const handleBlur = (event: React.FocusEvent<HTMLButtonElement>): void => {
    onClose?.(event, 'blur');
  };

  const handleKeyDown = (event: React.KeyboardEvent<HTMLDivElement>): void => {
    if (event.key === 'Escape') {
      onClose?.(event, 'escapeKeyDown');
    }
  };

  const enrichedIcon = React.isValidElement<SpeedDialIconProps>(icon) ? React.cloneElement(icon, { open }) : icon;

  return (
    <div
      className={clsx(styles.speedDial, hidden && styles.speedDialHidden, className)}
      onMouseLeave={handleMouseLeave}
      onKeyDown={handleKeyDown}
      role="presentation"
    >
      <button
        className={styles.fab}
        aria-label={ariaLabel}
        aria-expanded={open}
        onClick={onClick}
        onBlur={handleBlur}
        type="button"
      >
        {enrichedIcon}
      </button>
      {open && (
        <div className={styles.actions} role="menu">
          {children}
        </div>
      )}
    </div>
  );
}

interface SpeedDialActionProps {
  icon: React.ReactNode;
  title: string;
  onClick?: (event: React.MouseEvent<HTMLButtonElement>) => void;
  className?: string;
  selected?: boolean;
}

export function SpeedDialAction(props: SpeedDialActionProps): React.ReactElement {
  const { icon, title, className, onClick, selected } = props;
  return (
    <div className={styles.action} role="menuitem">
      <button
        className={clsx(
          styles.actionButton,
          styles.actionButtonOpen,
          selected && styles.actionButtonSelected,
          className,
        )}
        onClick={onClick}
        aria-label={title}
        type="button"
      >
        {icon}
      </button>
      {/* CSS-only tooltip: shown on hover/focus of the action via the sibling
          selectors in the stylesheet. aria-hidden because the button's
          aria-label already conveys the name to assistive tech. */}
      <span className={styles.tooltip} role="tooltip" aria-hidden="true">
        {title}
      </span>
    </div>
  );
}

interface SpeedDialIconProps {
  icon: React.ReactNode;
  openIcon?: React.ReactNode;
  open?: boolean;
}

export function SpeedDialIcon(props: SpeedDialIconProps): React.ReactElement {
  const { icon, openIcon, open } = props;

  if (openIcon) {
    return <span className={styles.iconWrapper}>{open ? openIcon : icon}</span>;
  }

  return <span className={clsx(styles.iconWrapper, open && styles.iconWrapperOpen)}>{icon}</span>;
}
