// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import SvgIcon from '@material-ui/core/SvgIcon';

export const AuxIcon: React.FunctionComponent = (props) => {
  return (
    <SvgIcon viewBox="0 0 24 24" {...props}>
      <g>
        <circle cx={12} cy={12} r={9} />
      </g>
    </SvgIcon>
  );
};

AuxIcon.displayName = 'Variable';
(AuxIcon as any).muiName = 'AuxIcon';
