// Copyright 2026 The Simlin Authors. All rights reserved.
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
  onClick?: (event: React.MouseEvent<HTMLElement>) => void;
  className?: string;
  'aria-label'?: string;
  style?: React.CSSProperties;
  // When set, render an <a> with the icon-button styling instead of a
  // <button>. This exists for navigation affordances (e.g. a router Link with
  // asChild injecting href/onClick): nesting a <button> inside an anchor is
  // invalid interactive content, so the anchor must BE the styled element.
  // `disabled` is ignored in this mode -- links have no disabled state.
  href?: string;
  children?: React.ReactNode;
}

export default function IconButton(props: IconButtonProps): React.ReactElement {
  const { color, edge = false, size = 'medium', disabled, onClick, className, style, href, children, ...rest } = props;

  const sizeClassMap: Record<NonNullable<IconButtonProps['size']>, string> = {
    small: styles.sizeSmall,
    medium: styles.sizeMedium,
    large: styles.sizeLarge,
  };
  const sizeClass = sizeClassMap[size];

  const classes = clsx(
    styles.iconButton,
    color === 'inherit' && styles.colorInherit,
    edge === 'start' && styles.edgeStart,
    edge === 'end' && styles.edgeEnd,
    sizeClass,
    // In href mode `disabled` is ignored entirely -- including the styling:
    // a greyed pointer-events:none anchor would still be keyboard-focusable
    // and Enter would navigate, i.e. it would LOOK disabled without being so.
    disabled && href === undefined && styles.disabled,
    className,
  );

  if (href !== undefined) {
    return (
      <a className={classes} href={href} onClick={onClick} style={style} {...rest}>
        {children}
      </a>
    );
  }

  return (
    <button className={classes} disabled={disabled} onClick={onClick} style={style} type="button" {...rest}>
      {children}
    </button>
  );
}
