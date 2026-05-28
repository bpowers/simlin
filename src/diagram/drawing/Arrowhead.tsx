// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { Point } from './common';
import { jsFormatNumber as f } from '../render-common';

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
    // Quantize SVG path coordinates -- see `Connector.tsx::renderStraightLine`
    // for the byte-identical Rust-vs-TS parity invariant the formatter enforces.
    const path = `M${f(x)},${f(y)}L${f(x - r)},${f(y + r / 2)}A${f(r * 3)},${f(r * 3)} 0 0,1 ${f(x - r)},${f(y - r / 2)}z`;
    r *= 1.5;
    const bgPath = `M${f(x + 0.5 * r)},${f(y)}L${f(x - 0.75 * r)},${f(y + r / 2)}A${f(r * 3)},${f(r * 3)} 0 0,1 ${f(x - 0.75 * r)},${f(
      y - r / 2,
    )}z`;

    let pathClassName: string;
    let staticClass: string;
    if (type === 'connector') {
      pathClassName = isSelected ? styles.arrowheadConnectorSelected : styles.arrowheadConnector;
      staticClass = isSelected ? 'simlin-arrowhead-link simlin-selected' : 'simlin-arrowhead-link';
    } else {
      pathClassName = isSelected ? styles.arrowheadFlowSelected : styles.arrowheadFlow;
      staticClass = isSelected ? 'simlin-arrowhead-flow simlin-selected' : 'simlin-arrowhead-flow';
    }

    const transform = `rotate(${f(this.props.angle)},${f(x)},${f(y)})`;

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
