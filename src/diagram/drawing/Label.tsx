// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { Rect } from './common';
import { AuxRadius } from './default';
import { ClickDragThresholdPx } from './pointer-utils';

import { CommonLabelProps, LabelPadding, lineSpacing } from './CommonLabel';

// Font properties applied as inline styles on <text> elements rather than
// relying on CSS <style> blocks alone.  resvg-wasm >= 0.4 doesn't apply
// CSS class-based font properties to text, so we duplicate them here.
// Single quotes avoid &quot; encoding issues from React's renderToString,
// which resvg-wasm >= 0.4 doesn't decode when parsing inline CSS.
const textStyle: React.CSSProperties = {
  fill: '#000000',
  fontSize: '12px',
  fontFamily: "'Roboto Light', 'Roboto', 'Open Sans', 'Arial', sans-serif",
  fontWeight: 300,
  whiteSpace: 'nowrap',
};

interface LabelLayout {
  textX: number;
  textY: number;
  x: number;
  y: number;
  lines: string[];
  reverseBaseline: boolean;
  align: 'end' | 'start' | 'middle';
}

function labelLayout(props: LabelPropsFull): LabelLayout {
  const lines = props.text.split('\n');
  // TODO: figure this out dynamically
  // const maxH = 13;

  const { cx, cy, side } = props;
  const rw: number = props.rw || AuxRadius;
  const rh: number = props.rh || AuxRadius;
  let x = cx;
  let y = cy;
  let className: 'end' | 'start' | 'middle' = 'middle';
  // for things on the top, we need to reverse the line-spacing we calculate
  let reverseBaseline = false;
  const textX = x;
  let textY = y;
  switch (side) {
    case 'top':
      reverseBaseline = true;
      y = cy - rh - LabelPadding - 2;
      textY = y;
      break;
    case 'bottom':
      y = cy + rh + LabelPadding;
      textY = y;
      break;
    case 'left':
      x = cx - rw - LabelPadding;
      className = 'end'; // left
      textY = y - (12 + (lines.length - 1) * 14) / 2 - 3;
      break;
    case 'right':
      x = cx + rw + LabelPadding;
      className = 'start'; // right
      textY = y - (12 + (lines.length - 1) * 14) / 2 - 3;
      break;
    default:
      // FIXME
      console.log('unknown label case ' + side);
  }

  return {
    textX,
    textY,
    x,
    y,
    lines,
    reverseBaseline,
    align: className,
  };
}

export function labelBounds(props: LabelProps): Rect {
  const lines = props.text.split('\n');

  const linesCount = lines.length;

  const maxWidthChars = lines.reduce((prev, curr) => (curr.length > prev ? curr.length : prev), 0);
  const editorWidth = maxWidthChars * 6 + 10;

  const { cx, cy, side } = props;
  const rw: number = props.rw || AuxRadius;
  const rh: number = props.rh || AuxRadius;
  let x = cx;
  let y = cy;
  const textX = x;
  let textY = y;
  let left = 0;
  switch (side) {
    case 'top':
      y = cy - rh - LabelPadding - lineSpacing * linesCount;
      left = textX - editorWidth / 2;
      textY = y;
      break;
    case 'bottom':
      y = cy + rh + LabelPadding;
      left = textX - editorWidth / 2;
      textY = y;
      break;
    case 'left':
      x = cx - rw - LabelPadding + 1;
      left = x - editorWidth;
      textY = y - (12 + (lines.length - 1) * 14) / 2 - 3;
      break;
    case 'right':
      x = cx + rw + LabelPadding - 1;
      left = x;
      textY = y - (12 + (lines.length - 1) * 14) / 2 - 3;
      break;
    default:
      // FIXME
      console.log('unknown label case ' + side);
  }

  textY = Math.round(textY);

  return {
    top: textY,
    left,
    right: left + editorWidth,
    bottom: textY + 14 * linesCount,
  };
}

interface LabelPropsFull extends CommonLabelProps {
  text: string;
  onSelection?: (e: React.PointerEvent<SVGElement>) => void;
  onLabelDrag?: (uid: number, e: React.PointerEvent<SVGElement>) => void;
}

export type LabelProps = Pick<LabelPropsFull, 'text' | 'onSelection' | 'cx' | 'cy' | 'side' | 'rw' | 'rh'>;

export const Label = React.memo(function Label(props: LabelPropsFull): React.ReactElement {
  const { uid, onSelection, onLabelDrag } = props;

  // Transient pointer-gesture tracking: deliberately refs, not state -- a
  // drag-in-progress must not trigger re-renders of the label itself.
  const pointerId = React.useRef<number | undefined>(undefined);
  const inMove = React.useRef(false);
  // Screen-pixel position of the press, so a label drag only begins once the
  // pointer has moved past the click/drag threshold. Without this, the
  // incidental 1-2px wobble of a physical double-click on the name started a
  // label drag (selecting + moving the label) instead of opening the name
  // editor, which is one reason double-click-to-rename was unreliable.
  const downPoint = React.useRef<{ x: number; y: number } | undefined>(undefined);

  const handlePointerDown = (e: React.PointerEvent<SVGElement>): void => {
    if (!e.isPrimary) {
      return;
    }
    e.preventDefault();
    e.stopPropagation();

    pointerId.current = e.pointerId;
    downPoint.current = { x: e.clientX, y: e.clientY };
    // Capture on press (not once the drag starts): a short label has a tiny
    // hit box, so a grip near its edge can leave the <text> bbox while still
    // sub-threshold -- without capture those moves would never reach us and the
    // label could not be dragged from that point. Capture on `currentTarget`
    // (the <text> the handler is bound to), not `e.target` (which may be a child
    // <tspan> the press landed on). Because it is the SAME node the dblclick
    // handler lives on, capturing here does not change click/dblclick targeting;
    // the load-bearing part of the double-click fix is purely that a
    // sub-threshold wobble fires no onLabelDrag (no selection / node churn
    // between the two clicks), which the threshold gate below still guarantees.
    e.currentTarget.setPointerCapture(e.pointerId);
  };

  const handlePointerMove = (e: React.PointerEvent<SVGElement>): void => {
    if (pointerId.current !== e.pointerId) {
      return;
    }
    // Ignore sub-threshold jitter until it clearly becomes a drag. Pointer
    // coords are already in screen pixels, so compare directly against the
    // screen-pixel threshold (no zoom conversion needed). Once a drag has
    // started, every subsequent move keeps feeding onLabelDrag.
    if (!inMove.current) {
      const start = downPoint.current;
      if (start && Math.hypot(e.clientX - start.x, e.clientY - start.y) < ClickDragThresholdPx) {
        return;
      }
      inMove.current = true;
    }

    onLabelDrag?.(uid, e);
  };

  const handlePointerUp = (e: React.PointerEvent<SVGElement>): void => {
    if (pointerId.current !== e.pointerId) {
      return;
    }
    pointerId.current = undefined;
    inMove.current = false;
    downPoint.current = undefined;
  };

  const handleDoubleClick = (e: React.MouseEvent<SVGElement>): void => {
    if (!inMove.current) {
      onSelection?.(e as unknown as React.PointerEvent<SVGElement>);
    }
  };

  const { textX, textY, x, lines, reverseBaseline, align } = labelLayout(props);
  const linesCount = lines.length;

  return (
    <g>
      <text
        x={textX}
        y={textY}
        style={{ ...textStyle, textAnchor: align, filter: 'url(#labelBackground)' }}
        onPointerDown={handlePointerDown}
        onPointerMove={handlePointerMove}
        onPointerUp={handlePointerUp}
        onDoubleClick={handleDoubleClick}
        textRendering="optimizeLegibility"
      >
        {lines.map((l, i) => {
          let dy: string = i === 0 ? '1em' : `${lineSpacing}px`;
          if (reverseBaseline && i === 0) {
            dy = `${-(lineSpacing * (linesCount - 1))}px`;
          }
          return (
            // Keyed by index, not line text: repeated lines (e.g. a label
            // edited to "x\nx") would otherwise produce duplicate keys.
            // Index keys are safe here -- a flat, fully-rebuilt list with
            // no per-line state.
            <tspan key={i} x={x} dy={dy}>
              {l}
            </tspan>
          );
        })}
      </text>
    </g>
  );
});
