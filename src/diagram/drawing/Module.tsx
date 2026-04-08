// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import clsx from 'clsx';

import { ModuleViewElement, ViewElement } from '@simlin/core/datamodel';
import { defined } from '@simlin/core/common';

import { displayName, mergeBounds, Point, Rect } from './common';
import { ModuleRadius, ModuleWidth, ModuleHeight } from './default';
import { Label, labelBounds, LabelProps } from './Label';

import styles from './Module.module.css';

export { ModuleWidth, ModuleHeight };

export interface ModuleProps {
  isSelected: boolean;
  isEditingName: boolean;
  isValidTarget?: boolean;
  hasWarning?: boolean;
  onSelection: (element: ViewElement, e: React.PointerEvent<SVGElement>, isText?: boolean) => void;
  onLabelDrag: (uid: number, e: React.PointerEvent<SVGElement>) => void;
  onDoubleClick?: (element: ModuleViewElement) => void;
  element: ModuleViewElement;
}

export function moduleContains(element: ViewElement, point: Point): boolean {
  const cx = element.x;
  const cy = element.y;
  const dx = Math.abs(point.x - cx);
  const dy = Math.abs(point.y - cy);
  return dx <= ModuleWidth / 2 && dy <= ModuleHeight / 2;
}

export function moduleBounds(element: ModuleViewElement): Rect {
  const { x: cx, y: cy } = element;
  const width = ModuleWidth;
  const height = ModuleHeight;
  const bounds = {
    top: cy - height / 2,
    left: cx - width / 2,
    right: cx + width / 2,
    bottom: cy + height / 2,
  };

  const side = element.labelSide;
  const labelProps: LabelProps = {
    cx,
    cy,
    side,
    rw: width / 2,
    rh: height / 2,
    text: displayName(defined(element.name)),
  };

  return mergeBounds(bounds, labelBounds(labelProps));
}

export class Module extends React.PureComponent<ModuleProps> {
  handlePointerDown = (e: React.PointerEvent<SVGElement>): void => {
    e.preventDefault();
    e.stopPropagation();
    this.props.onSelection(this.props.element, e);
  };

  handleLabelSelection = (e: React.PointerEvent<SVGElement>): void => {
    e.preventDefault();
    e.stopPropagation();
    this.props.onSelection(this.props.element, e, true);
  };

  handleDoubleClick = (e: React.MouseEvent<SVGElement>): void => {
    e.preventDefault();
    e.stopPropagation();
    if (this.props.onDoubleClick) {
      this.props.onDoubleClick(this.props.element);
    }
  };

  indicators() {
    if (!this.props.hasWarning) {
      return undefined;
    }

    const { element } = this.props;
    const w = ModuleWidth;
    const h = ModuleHeight;
    const cx = element.x + w / 2 - 1;
    const cy = element.y - h / 2 + 1;

    return <circle className={styles.errorIndicator} cx={cx} cy={cy} r={3} />;
  }

  render() {
    const { element, isEditingName, isSelected, isValidTarget } = this.props;
    const w = ModuleWidth;
    const h = ModuleHeight;
    const cx = element.x;
    const cy = element.y;
    const side = element.labelSide;

    const label = isEditingName ? undefined : (
      <Label
        uid={element.uid}
        cx={cx}
        cy={cy}
        side={side}
        text={displayName(defined(element.name))}
        rw={w / 2}
        rh={h / 2}
        onSelection={this.handleLabelSelection}
        onLabelDrag={this.props.onLabelDrag}
      />
    );

    const indicator = this.indicators();

    const groupClassName = clsx(styles.module, 'simlin-module', {
      [styles.selected]: isSelected && isValidTarget === undefined,
      'simlin-selected': isSelected && isValidTarget === undefined,
      [styles.targetGood]: isValidTarget === true,
      [styles.targetBad]: isValidTarget === false,
    });

    return (
      <g className={groupClassName} onPointerDown={this.handlePointerDown} onDoubleClick={this.handleDoubleClick}>
        <rect
          x={Math.ceil(cx - w / 2)}
          y={Math.ceil(cy - h / 2)}
          width={w}
          height={h}
          rx={ModuleRadius}
          ry={ModuleRadius}
        />
        {indicator}
        {label}
      </g>
    );
  }
}
