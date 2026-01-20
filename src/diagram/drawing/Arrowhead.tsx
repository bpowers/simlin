// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { Point } from './common';

import styles from './Arrowhead.module.css';

export interface ArrowheadProps {
  isSelected: boolean;
  point: Point;
  angle: number;
  size: number;
  type: 'flow' | 'connector';
  onSelection?: (e: React.PointerEvent<SVGElement>) => void;
}

export class Arrowhead extends React.PureComponent<ArrowheadProps> {
  handlePointerDown = (e: React.PointerEvent<SVGElement>): void => {
    if (this.props.onSelection) {
      this.props.onSelection(e);
    }
  };

  render() {
    const { type, isSelected } = this.props;
    const { x, y } = this.props.point;
    let r = this.props.size;
    const path = `M${x},${y}L${x - r},${y + r / 2}A${r * 3},${r * 3} 0 0,1 ${x - r},${y - r / 2}z`;
    r *= 1.5;
    const bgPath = `M${x + 0.5 * r},${y}L${x - 0.75 * r},${y + r / 2}A${r * 3},${r * 3} 0 0,1 ${x - 0.75 * r},${
      y - r / 2
    }z`;

    let pathClassName: string;
    let staticClass: string;
    if (type === 'connector') {
      pathClassName = isSelected ? styles.arrowheadConnectorSelected : styles.arrowheadConnector;
      staticClass = isSelected ? 'simlin-arrowhead-link simlin-selected' : 'simlin-arrowhead-link';
    } else {
      pathClassName = isSelected ? styles.arrowheadFlowSelected : styles.arrowheadFlow;
      staticClass = isSelected ? 'simlin-arrowhead-flow simlin-selected' : 'simlin-arrowhead-flow';
    }

    const transform = `rotate(${this.props.angle},${x},${y})`;

    return (
      <g>
        <path
          d={bgPath}
          className={`${styles.arrowheadBg} simlin-arrowhead-bg`}
          transform={transform}
          onPointerDown={this.handlePointerDown}
        />
        <path
          d={path}
          className={`${pathClassName} ${staticClass}`}
          transform={transform}
          onPointerDown={this.handlePointerDown}
        />
      </g>
    );
  }
}
