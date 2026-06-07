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
import { jsFormatNumber as f } from '../render-common';

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

export const Stock = React.memo(function Stock(props: StockProps): React.ReactElement {
  const { element, isEditingName, isSelected, isValidTarget, hasWarning, series, onSelection, onLabelDrag } = props;

  const handlePointerUp = (_e: React.PointerEvent<SVGElement>): void => {
    // e.preventDefault();
    // e.stopPropagation();
  };

  const handlePointerDown = (e: React.PointerEvent<SVGElement>): void => {
    e.preventDefault();
    e.stopPropagation();
    onSelection(element, e);
  };

  // Memoized: passed to the memo'd Label below, so a stable identity (while
  // element/onSelection are unchanged) lets Label skip re-rendering.
  const handleLabelSelection = React.useCallback(
    (e: React.PointerEvent<SVGElement>): void => {
      e.preventDefault();
      e.stopPropagation();
      onSelection(element, e, true);
    },
    [onSelection, element],
  );

  const w = StockWidth;
  const h = StockHeight;
  const cx = element.x;
  const cy = element.y;

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
      onSelection={handleLabelSelection}
      onLabelDrag={onLabelDrag}
    />
  );

  let sparkline;
  if (series && series.length > 0) {
    const sx = cx - arrayedOffset;
    const sy = cy - arrayedOffset;
    sparkline = (
      <g transform={`translate(${f(sx + 1 - w / 2)} ${f(sy + 1 - h / 2)})`}>
        <Sparkline series={series} width={w - 2} height={h - 2} />
      </g>
    );
  }

  let indicator;
  if (hasWarning) {
    indicator = <circle className={styles.errorIndicator} cx={cx + w / 2 - 1} cy={cy - h / 2 + 1} r={3} />;
  }

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
    <g className={groupClassName} onPointerDown={handlePointerDown} onPointerUp={handlePointerUp}>
      {rects}
      {sparkline}
      {indicator}
      {label}
    </g>
  );
});
