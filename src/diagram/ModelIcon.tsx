// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import styles from './ModelIcon.module.css';

export class ModelIcon extends React.PureComponent<{ className?: string }> {
  render() {
    const { className } = this.props;

    return (
      <svg viewBox="0 0 55 55" className={className} version="1.0">
        <g transform="matrix(1.0162719,0,0,1.0162745,-0.0629825,-0.83188265)" id="g5942">
          <path
            className={styles.initial}
            d="M 12.362217,49.365687 43.7591,17.968803 l 4.48527,4.48527 1.121317,-16.8197603 -16.819759,1.1213179 4.485269,4.4852684 -31.396884,31.396884 6.727904,6.727904 z"
            id="path2381"
          />
          <path
            className={styles.red}
            d="m 15.745709,32.526387 -10.1113959,10.111396 6.7279039,6.727905 0,0 10.09576,-10.09576"
            id="path2389"
          />
          <path
            className={styles.red}
            d="m 36.601427,25.126478 7.157675,-7.157675 4.485268,4.48527 1.121318,-16.8197604 -16.819759,1.1213179 4.485268,4.4852685 -7.188931,7.188931"
            id="path2385"
          />
          <path
            className={styles.red}
            d="m 37.334803,28.861336 c 0,6.163892 -4.996822,11.160714 -11.160714,11.160714 -6.163892,0 -11.160714,-4.996822 -11.160714,-11.160714 0,-6.163893 4.996822,-11.160715 11.160714,-11.160715 6.163892,0 11.160714,4.996822 11.160714,11.160715 l 0,0 z"
            id="path3173"
          />
        </g>
      </svg>
    );
  }
}
