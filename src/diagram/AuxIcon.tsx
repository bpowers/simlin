// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import SvgIcon from './components/SvgIcon';

import styles from './AuxIcon.module.css';

export const AuxIcon: React.FunctionComponent = (props) => {
  return (
    <SvgIcon viewBox="0 0 24 24" className={styles.auxIcon} {...props}>
      <g>
        <circle cx={12} cy={12} r={9} />
      </g>
    </SvgIcon>
  );
};

AuxIcon.displayName = 'Variable';
