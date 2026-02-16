// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import clsx from 'clsx';

import styles from './Toolbar.module.css';

export interface ToolbarProps {
  variant?: 'dense' | 'regular';
  className?: string;
  style?: React.CSSProperties;
  children?: React.ReactNode;
}

export default function Toolbar(props: ToolbarProps): React.ReactElement {
  const { variant = 'regular', className, style, children } = props;

  return (
    <div className={clsx(styles.toolbar, variant === 'dense' ? styles.dense : styles.regular, className)} style={style}>
      {children}
    </div>
  );
}
