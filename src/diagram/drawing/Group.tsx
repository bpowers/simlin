// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { GroupViewElement } from '@system-dynamics/core/datamodel';

import { displayName, Rect } from './common';

import styles from './Group.module.css';

// Corner radius for the rounded rectangle
const GroupRadius = 8;

// Padding between the label and the edge of the group
const LabelPadding = 8;

export interface GroupProps {
  isSelected: boolean;
  element: GroupViewElement;
}

export function groupBounds(element: GroupViewElement): Rect {
  // x/y is the center, compute bounds from center
  const { x, y, width, height } = element;
  const left = x - width / 2;
  const top = y - height / 2;
  return {
    top,
    left,
    right: left + width,
    bottom: top + height,
  };
}

export class Group extends React.PureComponent<GroupProps> {
  render() {
    const { element, isSelected } = this.props;
    const { x, y, width, height, name } = element;

    // x/y is the center, compute top-left for SVG rect
    const left = x - width / 2;
    const top = y - height / 2;

    const className = isSelected ? `${styles.group} ${styles.selected}` : styles.group;

    return (
      <g className={`${className} simlin-group`}>
        <rect
          x={left}
          y={top}
          width={width}
          height={height}
          rx={GroupRadius}
          ry={GroupRadius}
        />
        <text
          x={left + LabelPadding}
          y={top + LabelPadding}
          dominantBaseline="hanging"
          className={styles.label}
        >
          {displayName(name)}
        </text>
      </g>
    );
  }
}
