// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { styled } from '@material-ui/core/styles';

import { Rect } from './common';
import { AuxRadius } from './default';

import { CommonLabelProps, LabelPadding, lineSpacing } from './CommonLabel';

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

export const Label = styled(
  class LabelInner extends React.PureComponent<LabelPropsFull> {
    pointerId: number | undefined;
    inMove = false;

    constructor(props: LabelPropsFull) {
      super(props);

      this.state = {};
    }

    handlePointerDown = (e: React.PointerEvent<SVGElement>): void => {
      if (!e.isPrimary) {
        return;
      }
      e.preventDefault();
      e.stopPropagation();

      this.pointerId = e.pointerId;
    };

    handlePointerMove = (e: React.PointerEvent<SVGElement>): void => {
      if (this.pointerId !== e.pointerId) {
        return;
      }
      this.inMove = true;
      // eslint-disable-next-line @typescript-eslint/no-unsafe-call
      (e.target as any).setPointerCapture(e.pointerId);
      this.props.onLabelDrag?.(this.props.uid, e);
    };

    handlePointerUp = (e: React.PointerEvent<SVGElement>): void => {
      if (this.pointerId !== e.pointerId) {
        return;
      }
      this.pointerId = undefined;
      this.inMove = false;
    };

    handleDoubleClick = (e: React.MouseEvent<SVGElement>): void => {
      if (!this.inMove) {
        this.props.onSelection?.((e as unknown) as React.PointerEvent<SVGElement>);
      }
    };

    render() {
      const { textX, textY, x, lines, reverseBaseline, align } = labelLayout(this.props);
      const linesCount = lines.length;

      return (
        <g>
          <text
            x={textX}
            y={textY}
            style={align ? { textAnchor: align } : undefined}
            onPointerDown={this.handlePointerDown}
            onPointerMove={this.handlePointerMove}
            onPointerUp={this.handlePointerUp}
            onDoubleClick={this.handleDoubleClick}
            textRendering="optimizeLegibility"
          >
            {lines.map((l, i) => {
              let dy: string = i === 0 ? '1em' : `${lineSpacing}px`;
              if (reverseBaseline && i === 0) {
                dy = `${-(lineSpacing * (linesCount - 1))}px`;
              }
              return (
                <tspan key={l} x={x} dy={dy}>
                  {l}
                </tspan>
              );
            })}
          </text>
        </g>
      );
    }
  },
)(``);
