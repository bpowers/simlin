// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import clsx from 'clsx';

import styles from './CircularProgress.module.css';

export interface CircularProgressProps {
  /** Outer diameter in px. */
  size?: number;
  /** Ring thickness in px. */
  thickness?: number;
  className?: string;
  style?: React.CSSProperties;
  /** Accessible label; the spinner is an indeterminate progressbar. */
  label?: string;
}

// A minimal indeterminate spinner used for in-flight loads (project list,
// project open, auth handshake). Indeterminate: role=progressbar with no
// aria-valuenow. The visual is a single CSS-animated ring so it carries no
// dependencies and respects prefers-reduced-motion (see the CSS module).
export default function CircularProgress(props: CircularProgressProps): React.ReactElement {
  const { size = 40, thickness = 4, className, style, label = 'Loading' } = props;
  return (
    <span
      role="progressbar"
      aria-label={label}
      className={clsx(styles.spinner, className)}
      style={{ width: size, height: size, borderWidth: thickness, ...style }}
    />
  );
}
