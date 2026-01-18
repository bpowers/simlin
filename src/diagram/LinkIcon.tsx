// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import SvgIcon from '@mui/material/SvgIcon';

import styles from './LinkIcon.module.css';

export class LinkIcon extends React.PureComponent {
  render() {
    return (
      <SvgIcon viewBox="0 0 44 44">
        <g transform="translate(0,-1008.3622)">
          <path
            id="path3927"
            d="m 5.8298463,1048.4011 c 2.380511,-13.7575 14.9296827,-24.9072 24.2268437,-30.2027"
            className={styles.connector}
          />
          <path
            transform="matrix(1.3443263,-0.74535776,0.70135805,1.0714971,9.929532,1015.4334)"
            d="m 16.433926,9.9821424 -5.026338,2.9019576 -5.0263374,2.901957 0,-5.8039147 0,-5.8039147 5.0263384,2.9019575 z"
            className={styles.arrowhead}
          />
        </g>
      </SvgIcon>
    );
  }
}

(LinkIcon as any).muiName = 'LinkIcon';
