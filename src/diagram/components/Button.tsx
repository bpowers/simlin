// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import clsx from 'clsx';

import styles from './Button.module.css';

interface ButtonProps {
  variant?: 'text' | 'contained' | 'outlined';
  color?: 'primary' | 'secondary' | 'inherit';
  size?: 'small' | 'medium' | 'large';
  disabled?: boolean;
  onClick?: (event: React.MouseEvent<HTMLButtonElement>) => void;
  className?: string;
  style?: React.CSSProperties;
  startIcon?: React.ReactNode;
  children?: React.ReactNode;
  type?: 'button' | 'submit' | 'reset';
  component?: 'button' | 'label';
  'aria-label'?: string;
  'aria-owns'?: string;
  'aria-haspopup'?: boolean | 'true' | 'false';
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
      style,
      startIcon,
      children,
      type = 'button',
      component = 'button',
      'aria-label': ariaLabel,
      'aria-owns': ariaOwns,
      'aria-haspopup': ariaHaspopup,
    } = this.props;

    const sizeClass = size === 'small' ? styles.sizeSmall : size === 'large' ? styles.sizeLarge : styles.sizeMedium;

    let variantColorClass: string;
    let disabledClass: string | undefined;
    if (variant === 'contained') {
      variantColorClass = color === 'secondary' ? styles.containedSecondary : styles.containedPrimary;
      disabledClass = disabled ? styles.disabledContained : undefined;
    } else if (variant === 'outlined') {
      variantColorClass =
        color === 'secondary'
          ? styles.outlinedSecondary
          : color === 'inherit'
            ? styles.outlinedInherit
            : styles.outlinedPrimary;
      disabledClass = disabled ? styles.disabledOutlined : undefined;
    } else {
      variantColorClass =
        color === 'secondary' ? styles.textSecondary : color === 'inherit' ? styles.textInherit : styles.textPrimary;
      disabledClass = disabled ? styles.disabledText : undefined;
    }

    const buttonClassName = clsx(styles.button, sizeClass, variantColorClass, disabledClass, className);

    if (component === 'label') {
      return (
        <label className={buttonClassName} style={style} aria-label={ariaLabel}>
          {startIcon && <span className={styles.startIcon}>{startIcon}</span>}
          {children}
        </label>
      );
    }

    return (
      <button
        className={buttonClassName}
        style={style}
        disabled={disabled}
        onClick={onClick}
        type={type}
        aria-label={ariaLabel}
        aria-owns={ariaOwns}
        aria-haspopup={ariaHaspopup}
      >
        {startIcon && <span className={styles.startIcon}>{startIcon}</span>}
        {children}
      </button>
    );
  }
}
