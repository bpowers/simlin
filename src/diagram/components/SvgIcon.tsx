// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import clsx from 'clsx';

import styles from './SvgIcon.module.css';

export interface SvgIconProps extends Omit<React.SVGProps<SVGSVGElement>, 'ref'> {
  viewBox?: string;
  className?: string;
  children?: React.ReactNode;
}

export default class SvgIcon extends React.PureComponent<SvgIconProps> {
  render() {
    const { viewBox = '0 0 24 24', className, children, ...rest } = this.props;
    return (
      <svg className={clsx(styles.svgIcon, className)} viewBox={viewBox} focusable="false" aria-hidden="true" {...rest}>
        {children}
      </svg>
    );
  }
}
