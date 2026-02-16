// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import clsx from 'clsx';

import styles from './Avatar.module.css';

export interface AvatarProps {
  src?: string;
  alt?: string;
  className?: string;
  style?: React.CSSProperties;
  children?: React.ReactNode;
}

export default function Avatar(props: AvatarProps): React.ReactElement {
  const { src, alt, className, style, children } = props;

  return (
    <div className={clsx(styles.avatar, className)} style={style}>
      {src ? <img src={src} alt={alt || ''} className={styles.image} /> : children}
    </div>
  );
}
