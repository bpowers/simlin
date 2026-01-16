// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { styled } from '@mui/material/styles';

import { Point } from './common.js';

export interface ArrowheadProps {
  isSelected: boolean;
  point: Point;
  angle: number;
  size: number;
  type: 'flow' | 'connector';
  onSelection?: (e: React.PointerEvent<SVGElement>) => void;
}

export const Arrowhead = styled(
  class Arrowhead extends React.PureComponent<ArrowheadProps & { className?: string }> {
    handlePointerDown = (e: React.PointerEvent<SVGElement>): void => {
      if (this.props.onSelection) {
        this.props.onSelection(e);
      }
    };

    render() {
      const { className, type, isSelected } = this.props;
      const { x, y } = this.props.point;
      let r = this.props.size;
      const path = `M${x},${y}L${x - r},${y + r / 2}A${r * 3},${r * 3} 0 0,1 ${x - r},${y - r / 2}z`;
      r *= 1.5;
      const bgPath = `M${x + 0.5 * r},${y}L${x - 0.75 * r},${y + r / 2}A${r * 3},${r * 3} 0 0,1 ${x - 0.75 * r},${
        y - r / 2
      }z`;

      let pathClassName: string;
      if (type === 'connector') {
        pathClassName = isSelected ? 'simlin-arrowhead-connector-selected' : 'simlin-arrowhead-connector';
      } else {
        pathClassName = isSelected ? 'simlin-arrowhead-flow-selected' : 'simlin-arrowhead-flow';
      }

      const transform = `rotate(${this.props.angle},${x},${y})`;

      return (
        <g className={className}>
          <path
            d={bgPath}
            className="simlin-arrowhead-bg"
            transform={transform}
            onPointerDown={this.handlePointerDown}
          />
          <path d={path} className={pathClassName} transform={transform} onPointerDown={this.handlePointerDown} />
        </g>
      );
    }
  },
)(
  ({ theme }) => `
  & .simlin-arrowhead-flow {
    stroke-width: 1px;
    stroke-linejoin: round;
    stroke: ${theme.palette.common.black};
    fill: ${theme.palette.common.white};
  }
  & .simlin-arrowhead-flow-selected {
    stroke-width: 1;
    stroke-linejoin: round;
    stroke: #4444dd;
    fill: white;
  }
  & .simlin-arrowhead-connector {
    stroke-width: 1,
    stroke-linejoin: round;
    stroke: ${theme.palette.mode === 'dark' ? '#777777' : 'gray'};
    fill: ${theme.palette.mode === 'dark' ? '#777777' : 'gray'};
  }
  & .simlin-arrowhead-connector-selected {
    stroke-width: 1,
    stroke-linejoin: round;
    stroke: #4444dd;
    fill: #4444dd;
  }
  & .simlin-arrowhead-bg {
    fill: white;
    opacity: 0;
  }
`,
);
