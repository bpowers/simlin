// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { createStyles, withStyles, WithStyles } from '@material-ui/core/styles';

import { Point } from './common';

const styles = createStyles({
  flowArrowhead: {
    strokeWidth: 1,
    strokeLinejoin: 'round',
    stroke: 'black',
    fill: 'white',
  },
  flowArrowheadSelected: {
    strokeWidth: 1,
    strokeLinejoin: 'round',
    stroke: '#4444dd',
    fill: 'white',
  },
  connArrowhead: {
    strokeWidth: 1,
    strokeLinejoin: 'round',
    stroke: 'gray',
    fill: 'gray',
  },
  connArrowheadSelected: {
    strokeWidth: 1,
    strokeLinejoin: 'round',
    stroke: '#4444dd',
    fill: '#4444dd',
  },
  arrowheadBg: {
    fill: 'white',
    opacity: 0,
  },
});

interface ArrowheadProps extends WithStyles<typeof styles> {
  isSelected: boolean;
  point: Point;
  angle: number;
  size: number;
  type: 'flow' | 'connector';
  onSelection?: (e: React.PointerEvent<SVGElement>) => void;
}

export const Arrowhead = withStyles(styles)(
  class extends React.PureComponent<ArrowheadProps> {
    handlePointerDown = (e: React.PointerEvent<SVGElement>): void => {
      if (this.props.onSelection) {
        this.props.onSelection(e);
      }
    };

    render() {
      const { classes, type, isSelected } = this.props;
      const { x, y } = this.props.point;
      let r = this.props.size;
      const path = `M${x},${y}L${x - r},${y + r / 2}A${r * 3},${r * 3} 0 0,1 ${x - r},${y - r / 2}z`;
      r *= 1.5;
      const bgPath = `M${x + 0.5 * r},${y}L${x - 0.75 * r},${y + r / 2}A${r * 3},${r * 3} 0 0,1 ${x - 0.75 * r},${y -
        r / 2}z`;

      let className: string;
      if (type === 'connector') {
        className = isSelected ? classes.connArrowheadSelected : classes.connArrowhead;
      } else {
        className = isSelected ? classes.flowArrowheadSelected : classes.flowArrowhead;
      }

      const transform = `rotate(${this.props.angle},${x},${y})`;

      return (
        <g>
          <path
            d={bgPath}
            className={classes.arrowheadBg}
            transform={transform}
            onPointerDown={this.handlePointerDown}
          />
          <path d={path} className={className} transform={transform} onPointerDown={this.handlePointerDown} />
        </g>
      );
    }
  },
);
