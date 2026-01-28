// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import SvgIcon from './components/SvgIcon';

import styles from './StockIcon.module.css';

export const StockIcon: React.FunctionComponent = (props) => {
  return (
    <SvgIcon viewBox="0 0 50 50" className={styles.stockIcon} {...props}>
      <g>
        <rect x={2.5} y={7.5} width={45} height={35} />
      </g>
    </SvgIcon>
  );
};

StockIcon.displayName = 'Stock';
