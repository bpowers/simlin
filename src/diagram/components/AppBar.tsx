// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import clsx from 'clsx';

import styles from './AppBar.module.css';

export interface AppBarProps {
  position?: 'fixed' | 'static' | 'sticky';
  className?: string;
  style?: React.CSSProperties;
  children?: React.ReactNode;
}

export default function AppBar(props: AppBarProps): React.ReactElement {
  const { position = 'static', className, style, children } = props;

  const positionClass =
    position === 'fixed' ? styles.positionFixed : position === 'sticky' ? styles.positionSticky : styles.positionStatic;

  return (
    <header className={clsx(styles.appBar, positionClass, className)} style={style}>
      {children}
    </header>
  );
}
