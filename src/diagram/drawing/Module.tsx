// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { createStyles, withStyles, WithStyles } from '@material-ui/styles';

import { ModuleViewElement } from '@system-dynamics/core/datamodel';

import { displayName, Rect } from './common';
import { ModuleRadius } from './default';
import { Label } from './Label';

import { defined } from '@system-dynamics/core/common';

export const ModuleWidth = 55;
export const ModuleHeight = 45;

const styles = createStyles({
  module: {
    fill: 'white',
    strokeWidth: 1,
    stroke: 'black',
  },
});

interface ModulePropsFull extends WithStyles<typeof styles> {
  isSelected: boolean;
  element: ModuleViewElement;
}

export type ModuleProps = Pick<ModulePropsFull, 'isSelected' | 'element'>;

export function moduleBounds(props: ModuleProps): Rect {
  const { element } = props;
  const { cx, cy } = element;
  const width = ModuleWidth;
  const height = ModuleHeight;
  return {
    top: cy - height / 2,
    left: cx - width / 2,
    right: cx + width / 2,
    bottom: cy + height / 2,
  };
}

export const Module = withStyles(styles)(
  class Module extends React.PureComponent<ModulePropsFull> {
    render() {
      const { element, classes } = this.props;
      const w = ModuleWidth;
      const h = ModuleHeight;
      const cx = element.cx;
      const cy = element.cy;
      const side = element.labelSide;

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
