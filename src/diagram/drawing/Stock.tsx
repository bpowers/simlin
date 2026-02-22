// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import clsx from 'clsx';

import { StockViewElement, ViewElement, variableIsArrayed } from '@simlin/core/datamodel';
import { defined, Series } from '@simlin/core/common';

import { displayName, mergeBounds, Point, Rect } from './common';
import { StockWidth, StockHeight } from './default';
import { Label, labelBounds, LabelProps } from './Label';
import { Sparkline } from './Sparkline';

import styles from './Stock.module.css';

export { StockWidth, StockHeight };

export interface StockProps {
  isSelected: boolean;
  isEditingName: boolean;
  isValidTarget?: boolean;
  hasWarning?: boolean;
  series: Readonly<Array<Series>> | undefined;
  onSelection: (element: ViewElement, e: React.PointerEvent<SVGElement>, isText?: boolean) => void;
  onLabelDrag: (uid: number, e: React.PointerEvent<SVGElement>) => void;
  element: StockViewElement;
}

export function stockContains(element: ViewElement, point: Point): boolean {
  const cx = element.x;
  const cy = element.y;
  const width = StockWidth;
  const height = StockHeight;

  const dx = Math.abs(point.x - cx);
  const dy = Math.abs(point.y - cy);

  return dx <= width / 2 && dy <= height / 2;
}

export function stockBounds(element: StockViewElement): Rect {
  const { x: cx, y: cy } = element;
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

export class Stock extends React.PureComponent<StockProps> {
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

    const { element } = this.props;
    const w = StockWidth;
    const h = StockHeight;

    const cx = element.x + w / 2 - 1;
    const cy = element.y - h / 2 + 1;

    return <circle className={styles.errorIndicator} cx={cx} cy={cy} r={3} />;
  }

  sparkline(series: Readonly<Array<Series>> | undefined) {
    if (!series || series.length === 0) {
      return undefined;
    }
    const { element } = this.props;
    const isArrayed = (element.var && variableIsArrayed(element.var)) || false;
    const arrayedOffset = isArrayed ? 3 : 0;
    const cx = element.x - arrayedOffset;
    const cy = element.y - arrayedOffset;
    const w = StockWidth;
    const h = StockHeight;

    return (
      <g transform={`translate(${cx + 1 - w / 2} ${cy + 1 - h / 2})`}>
        <Sparkline series={series} width={w - 2} height={h - 2} />
      </g>
    );
  }

  render() {
    const { element, isEditingName, isSelected, isValidTarget } = this.props;
    const w = StockWidth;
    const h = StockHeight;
    const cx = element.x;
    const cy = element.y;

    const series = this.props.series;

    const isArrayed = (element.var && variableIsArrayed(element.var)) || false;
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

    const groupClassName = clsx(styles.stock, 'simlin-stock', {
      [styles.selected]: isSelected && isValidTarget === undefined,
      'simlin-selected': isSelected && isValidTarget === undefined,
      [styles.targetGood]: isValidTarget === true,
      [styles.targetBad]: isValidTarget === false,
    });

    const x = cx - w / 2;
    const y = cy - h / 2;

    let rects = [<rect key="1" x={x} y={y} width={w} height={h} />];
    if (isArrayed) {
      rects = [
        <rect key="0" x={x + arrayedOffset} y={y + arrayedOffset} width={w} height={h} />,
        <rect key="1" x={x} y={y} width={w} height={h} />,
        <rect key="2" x={x - arrayedOffset} y={y - arrayedOffset} width={w} height={h} />,
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
}
