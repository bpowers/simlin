// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import clsx from 'clsx';

import { AliasViewElement, NamedViewElement, ViewElement } from '@simlin/core/datamodel';
import { Series } from '@simlin/core/common';

import { displayName, mergeBounds, Point, Rect, square } from './common';
import { AuxRadius } from './default';
import { Label, labelBounds, LabelProps } from './Label';
import { Sparkline } from './Sparkline';
import { jsFormatNumber as f } from '../render-common';

import styles from './Alias.module.css';

export interface AliasProps {
  isSelected: boolean;
  isValidTarget?: boolean;
  series: Readonly<Array<Series>> | undefined;
  onSelection: (element: ViewElement, e: React.PointerEvent<SVGElement>, isText?: boolean) => void;
  onLabelDrag: (uid: number, e: React.PointerEvent<SVGElement>) => void;
  element: AliasViewElement;
  aliasOf: NamedViewElement | undefined;
}

export function aliasContains(element: ViewElement, point: Point): boolean {
  const cx = element.x;
  const cy = element.y;

  const distance = Math.sqrt(square(point.x - cx) + square(point.y - cy));
  return distance <= AuxRadius;
}

export function aliasBounds(element: AliasViewElement, aliasOf: NamedViewElement | undefined): Rect {
  const { x: cx, y: cy } = element;
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

export const Alias = React.memo(function Alias(props: AliasProps): React.ReactElement {
  const { element, isSelected, isValidTarget, series, aliasOf, onSelection, onLabelDrag } = props;

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

  const cx = element.x;
  const cy = element.y;
  const r = AuxRadius;

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
      onSelection={handleLabelSelection}
      onLabelDrag={onLabelDrag}
    />
  );

  let sparkline;
  if (series && series.length > 0) {
    const sx = cx - arrayedOffset;
    const sy = cy - arrayedOffset;
    sparkline = (
      <g transform={`translate(${f(sx + 1 - r / 2)} ${f(sy + 1 - r / 2)})`}>
        <Sparkline series={series} width={r - 2} height={r - 2} />
      </g>
    );
  }

  const groupClassName = clsx(styles.alias, 'simlin-alias', {
    [styles.selected]: isSelected && isValidTarget === undefined,
    'simlin-selected': isSelected && isValidTarget === undefined,
    [styles.targetGood]: isValidTarget === true,
    [styles.targetBad]: isValidTarget === false,
  });

  let circles = [<circle key="1" cx={cx} cy={cy} r={r} />];
  if (isArrayed) {
    circles = [
      <circle key="0" cx={cx + arrayedOffset} cy={cy + arrayedOffset} r={r} />,
      <circle key="1" cx={cx} cy={cy} r={r} />,
      <circle key="2" cx={cx - arrayedOffset} cy={cy - arrayedOffset} r={r} />,
    ];
  }

  return (
    <g className={groupClassName} onPointerDown={handlePointerDown}>
      {circles}
      {sparkline}
      {label}
    </g>
  );
});
