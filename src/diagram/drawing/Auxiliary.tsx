// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import clsx from 'clsx';
import { styled } from '@mui/material/styles';

import { AuxViewElement, ViewElement } from '@system-dynamics/core/datamodel';
import { defined, Series } from '@system-dynamics/core/common';

import { displayName, mergeBounds, Point, Rect, square } from './common.js';
import { AuxRadius } from './default.js';
import { Label, labelBounds, LabelProps } from './Label.js';
import { Sparkline } from './Sparkline.js';

export interface AuxProps {
  isSelected: boolean;
  isEditingName: boolean;
  isValidTarget?: boolean;
  hasWarning?: boolean;
  series: Readonly<Array<Series>> | undefined;
  onSelection: (element: ViewElement, e: React.PointerEvent<SVGElement>, isText?: boolean) => void;
  onLabelDrag: (uid: number, e: React.PointerEvent<SVGElement>) => void;
  element: AuxViewElement;
}

export function auxContains(element: ViewElement, point: Point): boolean {
  const cx = element.cx;
  const cy = element.cy;

  const distance = Math.sqrt(square(point.x - cx) + square(point.y - cy));
  return distance <= AuxRadius;
}

export function auxBounds(element: AuxViewElement): Rect {
  const { cx, cy } = element;
  const r = AuxRadius;

  const bounds = {
    top: cy - r,
    left: cx - r,
    right: cx + r,
    bottom: cy + r,
  };

  const side = element.labelSide;
  const labelProps: LabelProps = {
    cx,
    cy,
    side,
    text: displayName(defined(element.name)),
  };

  return mergeBounds(bounds, labelBounds(labelProps));
}

export const Aux = styled(
  class AuxInner extends React.PureComponent<AuxProps & { className?: string }> {
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

    radius(): number {
      return AuxRadius;
    }

    indicators() {
      if (!this.props.hasWarning) {
        return undefined;
      }

      const { element } = this.props;
      const r = this.radius();
      const θ = -Math.PI / 4; // 45 degrees

      const cx = element.cx + r * Math.cos(θ);
      const cy = element.cy + r * Math.sin(θ);

      return <circle className="simlin-error-indicator" cx={cx} cy={cy} r={3} />;
    }

    sparkline(series: Readonly<Array<Series>> | undefined) {
      if (!series || series.length === 0) {
        return undefined;
      }
      const { element } = this.props;
      const isArrayed = element.var?.isArrayed || false;
      const arrayedOffset = isArrayed ? 3 : 0;
      const cx = element.cx - arrayedOffset;
      const cy = element.cy - arrayedOffset;
      const r = this.radius();

      return (
        <g transform={`translate(${cx + 1 - r / 2} ${cy + 1 - r / 2})`}>
          <Sparkline series={series} width={r - 2} height={r - 2} />
        </g>
      );
    }

    render() {
      const { className, element, isEditingName, isSelected, isValidTarget, series } = this.props;
      const cx = element.cx;
      const cy = element.cy;
      const r = this.radius();

      const isArrayed = element.var?.isArrayed || false;
      const arrayedOffset = isArrayed ? 3 : 0;

      const side = element.labelSide;
      const label = isEditingName ? undefined : (
        <Label
          uid={element.uid}
          cx={cx}
          cy={cy}
          side={side}
          rw={r + arrayedOffset}
          rh={r + arrayedOffset}
          text={displayName(defined(element.name))}
          onSelection={this.handleLabelSelection}
          onLabelDrag={this.props.onLabelDrag}
        />
      );

      const sparkline = this.sparkline(series);
      const indicator = this.indicators();

      let groupClassName = isSelected ? 'simlin-selected' : undefined;
      if (isValidTarget !== undefined) {
        groupClassName = isValidTarget ? 'simlin-target-good' : 'simlin-target-bad';
      }

      let circles = [<circle key="1" cx={cx} cy={cy} r={r} />];
      if (isArrayed) {
        circles = [
          <circle key="0" cx={cx + arrayedOffset} cy={cy + arrayedOffset} r={r} />,
          <circle key="1" cx={cx} cy={cy} r={r} />,
          <circle key="2" cx={cx - arrayedOffset} cy={cy - arrayedOffset} r={r} />,
        ];
      }

      return (
        <g className={clsx(className, groupClassName)} onPointerDown={this.handlePointerDown}>
          {circles}
          {sparkline}
          {indicator}
          {label}
        </g>
      );
    }
  },
)(
  ({ theme }) => `
  & circle {
    stroke-width: 1px;
    stroke: ${theme.palette.common.black};
    fill: ${theme.palette.common.white};
  }
  &.simlin-target-good circle {
    stroke: rgb(76, 175, 80);
    stroke-width: 2px;
  }
  &.simlin-target-bad circle {
    stroke: rgb(244, 67, 54);
    stroke-width: 2px;
  }
  &.simlin-selected {
    text {
      fill: #4444dd;
    }
    circle {
      stroke: #4444dd;
    }
  }
  & .simlin-error-indicator {
    stroke-width: 0px;
    fill: rgb(255, 152, 0);
  }
  `,
);
