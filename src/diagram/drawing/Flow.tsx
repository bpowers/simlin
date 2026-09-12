// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import clsx from 'clsx';

import {
  Point,
  FlowViewElement,
  ViewElement,
  StockViewElement,
  CloudViewElement,
  variableIsArrayed,
} from '@simlin/core/datamodel';
import { arrayWith, defined, Series } from '@simlin/core/common';
import { at } from '@simlin/core/collections';

import { Arrowhead } from './Arrowhead';
import { displayName, Rect } from './common';
import { AuxRadius, CloudRadius, FlowArrowheadRadius } from './default';
import { Label, labelBounds, LabelProps } from './Label';
import { Sparkline } from './Sparkline';
import { jsFormatNumber as ff } from '../render-common';

import styles from './Flow.module.css';

/**
 * Pull a cloud-terminated flow's endpoint back by `radius` along the
 * direction of the final segment, so the arrowhead lands on the cloud's
 * edge rather than at its center. The retraction follows the segment's unit
 * vector: for orthogonal segments this matches a single-axis shift, while a
 * diagonal segment retracts by exactly `radius` (applying independent x and
 * y shifts would over-retract by sqrt(2)*radius). A zero-length final
 * segment is returned unchanged.
 */
export function retractFinalPointIntoCloud(pts: readonly Point[], radius: number): readonly Point[] {
  const lastPt = at(pts, pts.length - 1);
  const prevPt = at(pts, pts.length - 2);
  const dx = lastPt.x - prevPt.x;
  const dy = lastPt.y - prevPt.y;
  const len = Math.sqrt(dx * dx + dy * dy);
  if (len === 0) {
    return pts;
  }
  return arrayWith(pts, pts.length - 1, {
    ...lastPt,
    x: lastPt.x - (radius * dx) / len,
    y: lastPt.y - (radius * dy) / len,
  });
}

/**
 * Direction, in degrees [0, 360), of the last non-degenerate segment ending
 * at the flow's final point. Walks backward past coincident points --
 * atan2(0, 0) is 0, which would otherwise read a zero-length final segment
 * as "pointing right". Returns undefined when every point coincides.
 */
export function finalSegmentAngle(pts: readonly Point[]): number | undefined {
  const lastPt = at(pts, pts.length - 1);
  for (let i = pts.length - 2; i >= 0; i--) {
    const p = at(pts, i);
    const dx = lastPt.x - p.x;
    const dy = lastPt.y - p.y;
    if (dx !== 0 || dy !== 0) {
      let theta = (Math.atan2(dy, dx) * 180) / Math.PI;
      if (theta < 0) {
        theta += 360;
      }
      return theta;
    }
  }
  return undefined;
}

export function flowBounds(element: FlowViewElement): Rect {
  const cx = element.x;
  const cy = element.y;
  // Flow valve is a circle with radius 6 (FlowWidth/2 = 12/2 = 6)
  const r = 6;
  const bounds = {
    top: cy - r,
    left: cx - r,
    right: cx + r,
    bottom: cy + r,
  };

  // Include label bounds if there's a label
  if (element.name) {
    const side = element.labelSide;
    const labelProps: LabelProps = {
      cx,
      cy,
      side,
      rw: r,
      rh: r,
      text: displayName(element.name),
    };
    const lBounds = labelBounds(labelProps);

    bounds.top = Math.min(bounds.top, lBounds.top);
    bounds.left = Math.min(bounds.left, lBounds.left);
    bounds.right = Math.max(bounds.right, lBounds.right);
    bounds.bottom = Math.max(bounds.bottom, lBounds.bottom);
  }

  // Also include flow path points
  if (element.points) {
    for (const point of element.points) {
      bounds.left = Math.min(bounds.left, point.x);
      bounds.right = Math.max(bounds.right, point.x);
      bounds.top = Math.min(bounds.top, point.y);
      bounds.bottom = Math.max(bounds.bottom, point.y);
    }
  }

  return bounds;
}

export interface FlowProps {
  isSelected: boolean;
  isEditingName: boolean;
  isValidTarget?: boolean;
  hasWarning?: boolean;
  embedded?: boolean;
  series: Readonly<Array<Series>> | undefined;
  onSelection: (
    el: ViewElement,
    e: React.PointerEvent<SVGElement>,
    isText?: boolean,
    isArrowhead?: boolean,
    isSource?: boolean,
  ) => void;
  onLabelDrag: (uid: number, e: React.PointerEvent<SVGElement>) => void;
  source: StockViewElement | CloudViewElement;
  element: FlowViewElement;
  sink: StockViewElement | CloudViewElement;
}

export const Flow = React.memo(function Flow(props: FlowProps): React.ReactElement {
  const { element, isEditingName, isSelected, isValidTarget, series, sink } = props;
  const { hasWarning, embedded, onSelection, onLabelDrag } = props;

  const handlePointerUp = (_e: React.PointerEvent<SVGElement>): void => {
    // e.preventDefault();
    // e.stopPropagation();
  };

  // A press on the pipe or the valve; the canvas decides from the pointer which
  // segment it is and whether the drag slides the valve or offsets the segment.
  const handlePointerDown = (e: React.PointerEvent<SVGElement>): void => {
    e.preventDefault();
    e.stopPropagation();
    onSelection(element, e);
  };

  // Memoized: passed to the memo'd Label/Arrowhead below, so a stable identity
  // (while element/onSelection are unchanged) lets them skip re-rendering.
  const handleLabelSelection = React.useCallback(
    (e: React.PointerEvent<SVGElement>): void => {
      e.preventDefault();
      e.stopPropagation();
      onSelection(element, e, true);
    },
    [onSelection, element],
  );

  const handlePointerDownArrowhead = React.useCallback(
    (e: React.PointerEvent<SVGElement>): void => {
      e.preventDefault();
      e.stopPropagation();
      onSelection(element, e, false, true);
    },
    [onSelection, element],
  );

  const handlePointerDownSource = (e: React.PointerEvent<SVGElement>): void => {
    e.preventDefault();
    e.stopPropagation();
    onSelection(element, e, false, false, true);
  };

  const isArrayed = element.var ? variableIsArrayed(element.var) : false;
  const arrayedOffset = isArrayed ? 3 : 0;

  let pts = element.points;
  if (pts.length < 2) {
    throw new Error('expected at least two points on a flow');
  }

  if (sink.type === 'cloud') {
    pts = retractFinalPointIntoCloud(pts, CloudRadius);
  }

  const finalAdjust = 7.5;
  let spath = '';
  let arrowTheta = 0;
  for (let j = 0; j < pts.length; j++) {
    let x = at(pts, j).x;
    let y = at(pts, j).y;
    if (j === pts.length - 1) {
      // Walk back past coincident points: a degenerate (zero-length) final
      // segment must not read as "pointing right" via atan2(0, 0) === 0.
      const theta = finalSegmentAngle(pts);
      if (theta === undefined) {
        arrowTheta = 0;
      } else if (theta >= 315 || theta < 45) {
        x -= finalAdjust;
        arrowTheta = 0;
      } else if (theta >= 45 && theta < 135) {
        y -= finalAdjust;
        arrowTheta = 90;
      } else if (theta >= 135 && theta < 225) {
        x += finalAdjust;
        arrowTheta = 180;
      } else {
        y += finalAdjust;
        arrowTheta = 270;
      }
    }
    const prefix = j === 0 ? 'M' : 'L';
    // Quantize flow path coordinates -- see `jsFormatNumber` in
    // `render-common.tsx` for the cross-toolchain SVG parity invariant.
    spath += `${prefix}${ff(x)},${ff(y)}`;
  }

  const cx = element.x;
  const cy = element.y;
  const r = AuxRadius;

  const lastPt = at(pts, pts.length - 1);
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
      onSelection={handleLabelSelection}
      onLabelDrag={onLabelDrag}
    />
  );

  let sparkline;
  if (series && series.length > 0) {
    const sx = cx - arrayedOffset;
    const sy = cy - arrayedOffset;
    sparkline = (
      <g transform={`translate(${ff(sx + 1 - r / 2)} ${ff(sy + 1 - r / 2)})`}>
        <Sparkline series={series} width={r - 2} height={r - 2} />
      </g>
    );
  }

  let indicator;
  if (hasWarning) {
    const theta = -Math.PI / 4; // 45 degrees
    indicator = (
      <circle className={styles.errorIndicator} cx={cx + r * Math.cos(theta)} cy={cy + r * Math.sin(theta)} r={3} />
    );
  }

  const groupClassName = clsx(styles.flow, 'simlin-flow', {
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

  const outerClassName = isSelected
    ? clsx(styles.outerSelected, 'simlin-outer-selected')
    : clsx(styles.outer, 'simlin-outer');

  // Invisible hit area at the source end for grabbing the source
  // Position it slightly into the first segment from the source point
  const firstPt = at(pts, 0);
  const secondPt = at(pts, 1);
  const sourceHitSize = 20;

  // Calculate position along the first segment, offset from the source
  let sourceHitX = firstPt.x;
  let sourceHitY = firstPt.y;

  // Move hit area slightly into the segment for better UX
  const segDx = secondPt.x - firstPt.x;
  const segDy = secondPt.y - firstPt.y;
  const segLen = Math.hypot(segDx, segDy);
  if (segLen > sourceHitSize) {
    const offsetRatio = sourceHitSize / 2 / segLen;
    sourceHitX = firstPt.x + segDx * offsetRatio;
    sourceHitY = firstPt.y + segDy * offsetRatio;
  }

  // The source grip is interactive-only: an exported diagram has nothing to grab.
  const sourceHitArea = !embedded ? (
    <rect
      x={sourceHitX - sourceHitSize / 2}
      y={sourceHitY - sourceHitSize / 2}
      width={sourceHitSize}
      height={sourceHitSize}
      fill="transparent"
      style={{ cursor: 'grab' }}
      onPointerDown={handlePointerDownSource}
    />
  ) : null;

  return (
    <g className={groupClassName}>
      <path d={spath} className={outerClassName} onPointerDown={handlePointerDown} onPointerUp={handlePointerUp} />
      {sourceHitArea}
      <Arrowhead
        point={lastPt}
        angle={arrowTheta}
        size={FlowArrowheadRadius}
        type="flow"
        isSelected={isSelected}
        onSelection={handlePointerDownArrowhead}
      />
      <path d={spath} className={clsx(styles.inner, 'simlin-inner')} />
      <g onPointerDown={handlePointerDown} onPointerUp={handlePointerUp}>
        {circles}
        {sparkline}
      </g>
      {indicator}
      {label}
    </g>
  );
});
