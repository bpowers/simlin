// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { List } from 'immutable';

import { createStyles, withStyles, WithStyles } from '@material-ui/core/styles';

import { StockViewElement, ViewElement } from '../../datamodel';

import { displayName, mergeBounds, Point, Rect } from './common';
import { Label, labelBounds, LabelProps } from './Label';
import { Sparkline } from './Sparkline';

import { defined, Series } from '../../common';

const styles = createStyles({
  stock: {
    strokeWidth: 1,
    stroke: 'black',
    fill: 'white',
  },
  stockSelected: {
    strokeWidth: 1,
    stroke: '#4444dd',
    fill: 'white',
  },
  targetGood: {
    '& rect': {
      stroke: 'rgb(76, 175, 80)',
      strokeWidth: 2,
    },
  },
  targetBad: {
    '& rect': {
      stroke: 'rgb(244, 67, 54)',
      strokeWidth: 2,
    },
  },
  selected: {
    '& text': {
      fill: '#4444dd',
    },
    '& rect': {
      stroke: '#4444dd',
    },
  },
  indicator: {
    fill: 'rgb(255, 152, 0)',
    strokeWidth: 0,
  },
});

export const StockWidth = 45;
export const StockHeight = 35;

interface StockPropsFull extends WithStyles<typeof styles> {
  isSelected: boolean;
  isEditingName: boolean;
  isValidTarget?: boolean;
  hasWarning?: boolean;
  series: List<Series> | undefined;
  onSelection: (element: ViewElement, e: React.PointerEvent<SVGElement>, isText?: boolean) => void;
  onLabelDrag: (uid: number, e: React.PointerEvent<SVGElement>) => void;
  element: StockViewElement;
}

export type StockProps = Pick<
  StockPropsFull,
  'element' | 'series' | 'isSelected' | 'isValidTarget' | 'isEditingName' | 'hasWarning' | 'onSelection' | 'onLabelDrag'
>;

export function stockContains(element: ViewElement, point: Point): boolean {
  const cx = element.cx;
  const cy = element.cy;
  const width = StockWidth;
  const height = StockHeight;

  const dx = Math.abs(point.x - cx);
  const dy = Math.abs(point.y - cy);

  return dx <= width / 2 && dy <= height / 2;
}

export function stockBounds(element: StockViewElement): Rect {
  const { cx, cy } = element;
  const width = StockWidth;
  const height = StockHeight;
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

export const Stock = withStyles(styles)(
  class StockInner extends React.PureComponent<StockPropsFull> {
    handlePointerUp = (_e: React.PointerEvent<SVGElement>): void => {
      // e.preventDefault();
      // e.stopPropagation();
    };

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

    indicators() {
      if (!this.props.hasWarning) {
        return undefined;
      }

      const { classes, element } = this.props;
      const w = StockWidth;
      const h = StockHeight;

      const cx = element.cx + w / 2 - 1;
      const cy = element.cy - h / 2 + 1;

      return <circle className={classes.indicator} cx={cx} cy={cy} r={3} />;
    }

    sparkline(series: List<Series> | undefined) {
      if (!series || series.isEmpty()) {
        return undefined;
      }
      const { element } = this.props;
      const isArrayed = element.var?.isArrayed || false;
      const arrayedOffset = isArrayed ? 3 : 0;
      const cx = element.cx - arrayedOffset;
      const cy = element.cy - arrayedOffset;
      const w = StockWidth;
      const h = StockHeight;

      return (
        <g transform={`translate(${cx + 1 - w / 2} ${cy + 1 - h / 2})`}>
          <Sparkline series={series} width={w - 2} height={h - 2} />
        </g>
      );
    }

    render() {
      const { element, isEditingName, isSelected, isValidTarget, classes } = this.props;
      const w = StockWidth;
      const h = StockHeight;
      const cx = element.cx;
      const cy = element.cy;

      const series = this.props.series;

      const isArrayed = element.var?.isArrayed || false;
      const arrayedOffset = isArrayed ? 3 : 0;

      const side = element.labelSide;
      const label = isEditingName ? undefined : (
        <Label
          uid={element.uid}
          cx={cx}
          cy={cy}
          side={side}
          rw={w / 2 + arrayedOffset}
          rh={h / 2 + arrayedOffset}
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

      let rects = [
        <rect
          key="1"
          className={classes.stock}
          x={Math.ceil(cx - w / 2)}
          y={Math.ceil(cy - h / 2)}
          width={w}
          height={h}
        />,
      ];
      if (isArrayed) {
        rects = [
          <rect
            key="0"
            className={classes.stock}
            x={Math.ceil(cx - w / 2) + arrayedOffset}
            y={Math.ceil(cy - h / 2) + arrayedOffset}
            width={w}
            height={h}
          />,
          <rect
            key="1"
            className={classes.stock}
            x={Math.ceil(cx - w / 2)}
            y={Math.ceil(cy - h / 2)}
            width={w}
            height={h}
          />,
          <rect
            key="2"
            className={classes.stock}
            x={Math.ceil(cx - w / 2) - arrayedOffset}
            y={Math.ceil(cy - h / 2) - arrayedOffset}
            width={w}
            height={h}
          />,
        ];
      }

      return (
        <g className={groupClassName} onPointerDown={this.handlePointerDown} onPointerUp={this.handlePointerUp}>
          {rects}
          {sparkline}
          {indicator}
          {label}
        </g>
      );
    }
  },
);
