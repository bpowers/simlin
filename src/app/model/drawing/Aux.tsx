// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { createStyles, withStyles, WithStyles } from '@material-ui/core/styles';

import { AuxViewElement, ViewElement } from '../../datamodel';

import { displayName, mergeBounds, Point, Rect, square } from './common';
import { AuxRadius } from './default';
import { Label, labelBounds, LabelProps } from './Label';
import { Sparkline } from './Sparkline';

import { defined, Series } from '../../common';

const styles = createStyles({
  aux: {
    fill: 'white',
    strokeWidth: 1,
    stroke: 'black',
  },
  targetGood: {
    '& circle': {
      stroke: 'rgb(76, 175, 80)',
      strokeWidth: 2,
    },
  },
  targetBad: {
    '& circle': {
      stroke: 'rgb(244, 67, 54)',
      strokeWidth: 2,
    },
  },
  selected: {
    '& text': {
      fill: '#4444dd',
    },
    '& circle': {
      stroke: '#4444dd',
    },
  },
  indicator: {
    fill: 'rgb(255, 152, 0)',
    strokeWidth: 0,
  },
});

export interface AuxPropsFull extends WithStyles<typeof styles> {
  isSelected: boolean;
  isEditingName: boolean;
  isValidTarget?: boolean;
  hasWarning?: boolean;
  series: Series | undefined;
  onSelection: (element: ViewElement, e: React.PointerEvent<SVGElement>, isText?: boolean) => void;
  onLabelDrag: (uid: number, e: React.PointerEvent<SVGElement>) => void;
  element: AuxViewElement;
}

export type AuxProps = Pick<
  AuxPropsFull,
  'isSelected' | 'isEditingName' | 'isValidTarget' | 'hasWarning' | 'series' | 'onSelection' | 'onLabelDrag' | 'element'
>;

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

export const Aux = withStyles(styles)(
  class AuxInner extends React.PureComponent<AuxPropsFull> {
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

      const { classes, element } = this.props;
      const r = this.radius();
      const θ = -Math.PI / 4; // 45 degrees

      const cx = element.cx + r * Math.cos(θ);
      const cy = element.cy + r * Math.sin(θ);

      return <circle className={classes.indicator} cx={cx} cy={cy} r={3} />;
    }

    sparkline(series: Series | undefined) {
      if (!series) {
        return undefined;
      }
      const { element } = this.props;
      const cx = element.cx;
      const cy = element.cy;
      const r = this.radius();

      return (
        <g transform={`translate(${cx + 1 - r / 2} ${cy + 1 - r / 2})`}>
          <Sparkline series={series} width={r - 2} height={r - 2} />
        </g>
      );
    }

    render() {
      const { classes, element, isEditingName, isSelected, isValidTarget, series } = this.props;
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

      let groupClassName = isSelected ? classes.selected : undefined;
      if (isValidTarget !== undefined) {
        groupClassName = isValidTarget ? classes.targetGood : classes.targetBad;
      }

      let circles = [<circle key="1" className={classes.aux} cx={cx} cy={cy} r={r} />];
      if (isArrayed) {
        circles = [
          <circle key="0" className={classes.aux} cx={cx + arrayedOffset} cy={cy + arrayedOffset} r={r} />,
          <circle key="1" className={classes.aux} cx={cx} cy={cy} r={r} />,
          <circle key="2" className={classes.aux} cx={cx - arrayedOffset} cy={cy - arrayedOffset} r={r} />,
        ];
      }

      return (
        <g className={groupClassName} onPointerDown={this.handlePointerDown}>
          {circles}
          {sparkline}
          {indicator}
          {label}
        </g>
      );
    }
  },
);
