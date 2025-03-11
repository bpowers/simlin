// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { styled } from '@mui/material/styles';

import { createEditor, Descendant, Node, Editor, Transforms } from 'slate';
import { withHistory } from 'slate-history';
import { Editable, ReactEditor, Slate, withReact } from 'slate-react';

import { CommonLabelProps, LabelPadding as baseLabelPadding, lineSpacing as baseLineSpacing } from './CommonLabel';
import { AuxRadius } from './default';

interface EditingLabelProps extends CommonLabelProps {
  value: Descendant[];
  onChange: (value: Descendant[]) => void;
  onDone: (isCancel: boolean) => void;
  zoom: number;
}

interface EditingLabelState {
  editor: ReactEditor;
}

export const EditableLabel = styled(
  class EditableLabel extends React.PureComponent<EditingLabelProps & { className?: string }, EditingLabelState> {
    constructor(props: EditingLabelProps) {
      super(props);

      const { value } = props;
      const editor = withHistory(withReact(createEditor()));
      if (value.length > 0) {
        editor.children = value;
        Transforms.select(editor, {
          anchor: {
            path: [0, 0],
            offset: 0,
          },
          focus: Editor.end(editor, [value.length - 1]),
        });
      }

      this.state = {
        editor,
      };
    }

    handleChange = (value: Descendant[]): void => {
      this.props.onChange(value);
    };

    handlePointerUpDown = (e: React.PointerEvent<HTMLDivElement>): void => {
      e.stopPropagation();
    };

    handleKeyPress = (e: React.KeyboardEvent<HTMLDivElement>): void => {
      if (e.code === 'Enter' && (e.ctrlKey || e.shiftKey || e.altKey)) {
        e.stopPropagation();
        this.props.onDone(false);
      } else if (e.code === 'Escape') {
        e.stopPropagation();
        this.props.onDone(true);
      }
    };

    render() {
      const { cx, cy, side, zoom } = this.props;
      const fontSize = 12 * zoom;

      const lines: string[] = this.props.value.map((n) => Node.string(n));
      const linesCount = lines.length;

      const maxWidthChars = lines.reduce((prev, curr) => (curr.length > prev ? curr.length : prev), 0);
      const editorWidth = (maxWidthChars * 6 + 10) * zoom;

      const rw: number = this.props.rw || AuxRadius;
      const rh: number = this.props.rh || AuxRadius;
      let x = cx;
      let y = cy;
      const textX = Math.round(x);
      let textY = y;
      let left = 0;
      let textAlign: 'center' | 'left' | 'right' = 'center';
      const labelPadding = baseLabelPadding * zoom;
      const lineSpacing = baseLineSpacing * zoom;
      switch (side) {
        case 'top':
          y = cy - rh - labelPadding - lineSpacing * linesCount;
          left = textX - editorWidth / 2;
          textY = y;
          break;
        case 'bottom':
          y = cy + rh + labelPadding;
          left = textX - editorWidth / 2;
          textY = y;
          break;
        case 'left':
          x = cx - rw - labelPadding + 1;
          textAlign = 'right';
          left = x - editorWidth;
          textY = y - (fontSize + (lines.length - 1) * 14 * zoom) / 2 - 3;
          break;
        case 'right':
          x = cx + rw + labelPadding - 1;
          textAlign = 'left';
          left = x;
          textY = y - (fontSize + (lines.length - 1) * 14 * zoom) / 2 - 3;
          break;
        default:
          // FIXME
          console.log('unknown label case ' + side);
      }

      textY = Math.round(textY);

      /*
        <circle
          cx={textX}
          cy={textY}
          r={2}
          fill={'red'}
          strokeWidth={0}
        /> */

      const style: React.CSSProperties = {
        position: 'relative',
        left,
        top: textY,
        width: editorWidth,
        textAlign,
        lineHeight: `${14 * zoom}px`,
        background: 'white',
        borderRadius: '3px',
        border: '1px solid #4444dd',
        fontSize,
      };

      const { className, value } = this.props;

      return (
        <div
          className={className}
          style={style}
          onPointerDown={this.handlePointerUpDown}
          onPointerUp={this.handlePointerUpDown}
        >
          <Slate editor={this.state.editor} initialValue={value} onChange={this.handleChange}>
            <Editable autoFocus={true} onKeyUp={this.handleKeyPress} />
          </Slate>
        </div>
      );
    }
  },
)(`
    font-family: "Roboto", "Open Sans", "Arial", sans-serif;
    font-weight: 300;
    text-anchor: middle;
    white-space: nowrap;
    vertical-align: middle;
`);
