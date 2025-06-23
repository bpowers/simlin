// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { styled } from '@mui/material/styles';
import SvgIcon from '@mui/material/SvgIcon';

export const AuxIcon: React.FunctionComponent = styled((props) => {
  return (
    <SvgIcon viewBox="0 0 24 24" {...props}>
      <g>
        <circle cx={12} cy={12} r={9} />
      </g>
    </SvgIcon>
  );
})(`
  fill: gray;
`);

AuxIcon.displayName = 'Variable';

(AuxIcon as any).muiName = 'AuxIcon';
