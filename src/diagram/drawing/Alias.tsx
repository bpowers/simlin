// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { createStyles, withStyles, WithStyles } from '@material-ui/styles';

import { AliasViewElement, NamedViewElement, ViewElement } from '@system-dynamics/core/datamodel';

import { displayName, mergeBounds, Point, Rect, square } from './common';
import { AuxRadius } from './default';
import { Label, labelBounds, LabelProps } from './Label';
import { Sparkline } from './Sparkline';

import { Series } from '@system-dynamics/core/common';

const styles = createStyles({
  aux: {
    fill: 'white',
    strokeWidth: 1,
    stroke: 'black',
    strokeDasharray: 2,
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

export interface AliasPropsFull extends WithStyles<typeof styles> {
  isSelected: boolean;
  isValidTarget?: boolean;
  series: Readonly<Array<Series>> | undefined;
  onSelection: (element: ViewElement, e: React.PointerEvent<SVGElement>, isText?: boolean) => void;
  onLabelDrag: (uid: number, e: React.PointerEvent<SVGElement>) => void;
  element: AliasViewElement;
  aliasOf: NamedViewElement | undefined;
}

export type AliasProps = Pick<
  AliasPropsFull,
  'isSelected' | 'isValidTarget' | 'series' | 'onSelection' | 'onLabelDrag' | 'element' | 'aliasOf'
>;

export function aliasContains(element: ViewElement, point: Point): boolean {
  const cx = element.cx;
  const cy = element.cy;

  const distance = Math.sqrt(square(point.x - cx) + square(point.y - cy));
  return distance <= AuxRadius;
}

export function aliasBounds(element: AliasViewElement, aliasOf: NamedViewElement | undefined): Rect {
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
    text: displayName(aliasOf?.name || 'unknown alias'),
  };

  return mergeBounds(bounds, labelBounds(labelProps));
}

export const Alias = withStyles(styles)(
  class AliasInner extends React.PureComponent<AliasPropsFull> {
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

    sparkline(series: Readonly<Array<Series>> | undefined) {
      if (!series || series.length === 0) {
        return undefined;
      }
      const { element } = this.props;
      const isArrayed = false; // element.var?.isArrayed || false;
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
      const { classes, element, isSelected, isValidTarget, series, aliasOf } = this.props;
      const cx = element.cx;
      const cy = element.cy;
      const r = this.radius();

      const isArrayed = false; // element.var?.isArrayed || false;
      const arrayedOffset = isArrayed ? 3 : 0;

      const side = element.labelSide;
      const label = (
        <Label
          uid={element.uid}
          cx={cx}
          cy={cy}
          side={side}
          rw={r + arrayedOffset}
          rh={r + arrayedOffset}
          text={displayName(aliasOf?.name || 'unknown alias')}
          onSelection={this.handleLabelSelection}
          onLabelDrag={this.props.onLabelDrag}
        />
      );

      const sparkline = this.sparkline(series);

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
          {label}
        </g>
      );
    }
  },
);
