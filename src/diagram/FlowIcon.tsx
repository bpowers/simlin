// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { styled } from '@mui/material/styles';
import SvgIcon from '@mui/material/SvgIcon';

export const FlowIcon = styled(
  class Flow extends React.PureComponent<{ className?: string }> {
    render() {
      const { className } = this.props;
      return (
        <SvgIcon viewBox="0 0 44 44">
          <g transform="translate(0,-1008.3622)" className={className}>
            <rect y="1027.3622" x="2" height="5" width="31" />
            <path
              transform="matrix(0.56482908,0,0,0.68009598,15.213828,1010.9895)"
              d="m 24.502534,14.516574 11.460483,6.616713 11.460484,6.616713 -11.460483,6.616713 -11.460484,6.616713 0,-13.233426 z"
            />
            <path
              transform="matrix(0.75505621,0,0,0.75505621,19.865169,1011.842)"
              d="m 8.1249995,23.866072 a 11.919642,11.919642 0 1 1 -23.8392845,0 11.919642,11.919642 0 1 1 23.8392845,0 z"
            />
          </g>
        </SvgIcon>
      );
    }
  },
)(`
  stroke-width: 1px;
  stroke-linejoin: round;
  stroke: gray;
  fill: gray;
  opacity: 1;
`);

(FlowIcon as any).muiName = 'FlowIcon';
