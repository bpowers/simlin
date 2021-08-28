// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { styled } from '@material-ui/core/styles';
import SvgIcon from '@material-ui/core/SvgIcon';

export const StockIcon: React.FunctionComponent = styled((props) => {
  return (
    <SvgIcon viewBox="0 0 50 50" {...props}>
      <g>
        <rect x={2.5} y={7.5} width={45} height={35} />
      </g>
    </SvgIcon>
  );
})(`
  fill: gray;
`);

StockIcon.displayName = 'Stock';
// eslint-disable-next-line @typescript-eslint/no-unsafe-member-access
(StockIcon as any).muiName = 'StockIcon';
