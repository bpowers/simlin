// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import clsx from 'clsx';

import styles from './InputAdornment.module.css';

export interface InputAdornmentProps {
  position: 'start' | 'end';
  className?: string;
  style?: React.CSSProperties;
  children?: React.ReactNode;
}

export default function InputAdornment(props: InputAdornmentProps): React.ReactElement {
  const { position, className, style, children } = props;

  return (
    <div
      className={clsx(styles.adornment, position === 'start' ? styles.positionStart : styles.positionEnd, className)}
      style={style}
    >
      {children}
    </div>
  );
}
