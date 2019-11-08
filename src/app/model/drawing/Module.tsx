// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { createStyles, withStyles, WithStyles } from '@material-ui/core/styles';

import { ViewElement } from '../../../engine/xmile';

import { displayName, Rect } from './common';
import { AuxRadius, ModuleRadius } from './default';
import { findSide, Label } from './Label';

import { defined } from '../../common';

const styles = createStyles({
  module: {
    fill: 'white',
    strokeWidth: 1,
    stroke: 'black',
  },
});

interface ModulePropsFull extends WithStyles<typeof styles> {
  isSelected: boolean;
  element: ViewElement;
}

export type ModuleProps = Pick<ModulePropsFull, 'isSelected' | 'element'>;

export function moduleBounds(props: ModuleProps): Rect {
  const { element } = props;
  const { cx, cy } = element;
  const width = element.width ? element.width : 55;
  const height = element.height ? element.height : 45;
  return {
    top: cy - height / 2,
    left: cx - width / 2,
    right: cx + width / 2,
    bottom: cy + height / 2,
  };
}

export const Module = withStyles(styles)(
  class extends React.PureComponent<ModulePropsFull> {
    constructor(props: ModulePropsFull) {
      super(props);
    }

    render() {
      const { element, classes } = this.props;
      const w = element.width ? element.width : 55;
      const h = element.height ? element.height : 45;
      const cx = element.cx;
      const cy = element.cy;

      const side = findSide(element);

      return (
        <g>
          <rect
            className={classes.module}
            x={Math.ceil(cx - w / 2)}
            y={Math.ceil(cy - h / 2)}
            width={w}
            height={h}
            rx={ModuleRadius}
            ry={ModuleRadius}
          />
          <Label
            uid={element.uid}
            cx={cx}
            cy={cy}
            side={side}
            text={displayName(defined(element.name))}
            rw={w / 2}
            rh={h / 2}
          />
        </g>
      );
    }
  },
);
