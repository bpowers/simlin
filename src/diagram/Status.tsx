// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { styled } from '@mui/material/styles';

interface StatusProps {
  status: 'ok' | 'error' | 'disabled';
  onClick: () => void;
}

export const Status = styled(
  class Status extends React.PureComponent<StatusProps & { className?: string }> {
    handleClick = () => {
      this.props.onClick();
    };

    render() {
      const { className, status } = this.props;
      const fill = status === 'ok' ? '#81c784' : status === 'error' ? 'rgb(255, 152, 0)' : '#DCDCDC';
      return (
        <svg className={className}>
          <circle cx={12} cy={12} r={12} fill={fill} onClick={this.handleClick} />
        </svg>
      );
    }
  },
)(`
    position: absolute;
    top: 0px;
    right: 0px;
    height: 24px;
    width: 24px;
    margin: 12px;
    marginRight: 16px;
`);
