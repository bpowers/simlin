// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import clsx from 'clsx';

import styles from './ImageList.module.css';

export interface ImageListProps {
  cols?: number;
  gap?: number;
  className?: string;
  style?: React.CSSProperties;
  children?: React.ReactNode;
}

export default function ImageList(props: ImageListProps): React.ReactElement {
  const { cols = 2, gap = 4, className, style, children } = props;

  return (
    <ul
      className={clsx(styles.imageList, className)}
      style={{
        gridTemplateColumns: `repeat(${cols}, 1fr)`,
        gap: `${gap}px`,
        ...style,
      }}
    >
      {children}
    </ul>
  );
}

export interface ImageListItemProps {
  className?: string;
  style?: React.CSSProperties;
  children?: React.ReactNode;
}

export function ImageListItem(props: ImageListItemProps): React.ReactElement {
  const { className, style, children } = props;

  return (
    <li className={clsx(styles.imageListItem, className)} style={style}>
      {children}
    </li>
  );
}
