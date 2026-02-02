// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import clsx from 'clsx';

import styles from './IconButton.module.css';

interface IconButtonProps {
  color?: 'default' | 'inherit';
  edge?: 'start' | 'end' | false;
  size?: 'small' | 'medium' | 'large';
  disabled?: boolean;
  onClick?: (event: React.MouseEvent<HTMLButtonElement>) => void;
  className?: string;
  'aria-label'?: string;
  style?: React.CSSProperties;
  children?: React.ReactNode;
}

export default class IconButton extends React.PureComponent<IconButtonProps> {
  render() {
    const { color, edge = false, size = 'medium', disabled, onClick, className, style, children, ...rest } = this.props;

    const sizeClassMap: Record<NonNullable<IconButtonProps['size']>, string> = {
      small: styles.sizeSmall,
      medium: styles.sizeMedium,
      large: styles.sizeLarge,
    };
    const sizeClass = sizeClassMap[size];

    return (
      <button
        className={clsx(
          styles.iconButton,
          color === 'inherit' && styles.colorInherit,
          edge === 'start' && styles.edgeStart,
          edge === 'end' && styles.edgeEnd,
          sizeClass,
          disabled && styles.disabled,
          className,
        )}
        disabled={disabled}
        onClick={onClick}
        style={style}
        type="button"
        {...rest}
      >
        {children}
      </button>
    );
  }
}
