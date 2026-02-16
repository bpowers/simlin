// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import styles from './Status.module.css';

interface StatusProps {
  status: 'ok' | 'error' | 'disabled';
  onClick: () => void;
}

export class Status extends React.PureComponent<StatusProps> {
  handleClick = () => {
    this.props.onClick();
  };

  render() {
    const { status } = this.props;
    const fill = status === 'ok' ? '#81c784' : status === 'error' ? 'rgb(255, 152, 0)' : '#DCDCDC';
    return (
      <svg className={styles.status}>
        <circle cx={12} cy={12} r={12} fill={fill} onClick={this.handleClick} />
      </svg>
    );
  }
}
