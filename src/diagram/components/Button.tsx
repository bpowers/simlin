// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import clsx from 'clsx';

import styles from './Button.module.css';

interface ButtonProps {
  variant?: 'text' | 'contained';
  color?: 'primary' | 'secondary';
  size?: 'small' | 'medium' | 'large';
  disabled?: boolean;
  onClick?: (event: React.MouseEvent<HTMLButtonElement>) => void;
  className?: string;
  startIcon?: React.ReactNode;
  children?: React.ReactNode;
}

export default class Button extends React.PureComponent<ButtonProps> {
  render() {
    const {
      variant = 'text',
      color = 'primary',
      size = 'medium',
      disabled,
      onClick,
      className,
      startIcon,
      children,
    } = this.props;

    const sizeClass = size === 'small' ? styles.sizeSmall : size === 'large' ? styles.sizeLarge : styles.sizeMedium;

    let variantColorClass: string;
    let disabledClass: string | undefined;
    if (variant === 'contained') {
      variantColorClass = color === 'secondary' ? styles.containedSecondary : styles.containedPrimary;
      disabledClass = disabled ? styles.disabledContained : undefined;
    } else {
      variantColorClass = color === 'secondary' ? styles.textSecondary : styles.textPrimary;
      disabledClass = disabled ? styles.disabledText : undefined;
    }

    return (
      <button
        className={clsx(styles.button, sizeClass, variantColorClass, disabledClass, className)}
        disabled={disabled}
        onClick={onClick}
        type="button"
      >
        {startIcon && <span className={styles.startIcon}>{startIcon}</span>}
        {children}
      </button>
    );
  }
}
