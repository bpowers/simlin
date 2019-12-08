// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { createStyles, withStyles, WithStyles } from '@material-ui/core/styles';

import { createEditor, Editor, Node, Range } from 'slate';
import { withHistory } from 'slate-history';
import { Editable, Slate, withReact } from 'slate-react';

import { CommonLabelProps, LabelPadding, lineSpacing } from './CommonLabel';
import { AuxRadius } from './default';

const styles = createStyles({
  left: {
    textAnchor: 'end',
  },
  right: {
    textAnchor: 'start',
  },
  editor: {
    fontSize: 12,
    fontFamily: '"Roboto", "Open Sans", "Arial", sans-serif',
    fontWeight: 300,
    textAnchor: 'middle',
    whiteSpace: 'nowrap',
    verticalAlign: 'middle',
  },
});

interface EditingLabelPropsFull extends CommonLabelProps, WithStyles<typeof styles> {
  value: Node[];
  onChange: (value: Node[]) => void;
  onDone: (isCancel: true) => void;
}

interface EditingLabelState {
  editor: Editor;
  selection: Range | null;
}

export type EditingLabelProps = Pick<EditingLabelPropsFull, 'value' | 'onChange' | 'cx' | 'cy' | 'side' | 'rw' | 'rh'>;

export const EditableLabel = withStyles(styles)(
  class extends React.PureComponent<EditingLabelPropsFull, EditingLabelState> {
    constructor(props: EditingLabelPropsFull) {
      super(props);

      this.state = {
        editor: withHistory(withReact(createEditor())),
        selection: null,
      };
    }

    handleChange = (value: Node[], selection: Range | null): void => {
      this.setState({ selection });
      this.props.onChange(value);
    };

    handlePointerDown = (e: React.PointerEvent<HTMLDivElement>): void => {
      e.stopPropagation();
    };

    render() {
      const lines: string[] = this.props.value.map(n => Node.text(n));
      const linesCount = lines.length;

      const maxWidthChars = lines.reduce((prev, curr) => (curr.length > prev ? curr.length : prev), 0);
      const editorWidth = maxWidthChars * 6 + 10;

      const { cx, cy, side } = this.props;
      const rw: number = this.props.rw || AuxRadius;
      const rh: number = this.props.rh || AuxRadius;
      let x = cx;
      let y = cy;
      // for things on the top, we need to reverse the line-spacing we calculate
      let reverseBaseline = false;
      const textX = Math.round(x);
      let textY = y;
      let left = 0;
      let textAlign: 'center' | 'left' | 'right' = 'center';
      switch (side) {
        case 'top':
          reverseBaseline = true;
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
          textAlign = 'right';
          left = x - editorWidth;
          textY = y - (12 + (lines.length - 1) * 14) / 2 - 3;
          break;
        case 'right':
          x = cx + rw + LabelPadding - 1;
          textAlign = 'left';
          left = x;
          textY = y - (12 + (lines.length - 1) * 14) / 2 - 3;
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
        lineHeight: '14px',
        background: 'white',
        borderRadius: '3px',
        border: '1px solid #4444dd',
      };

      const { classes } = this.props;

      return (
        <div className={classes.editor} style={style} onPointerDown={this.handlePointerDown}>
          <Slate
            editor={this.state.editor}
            value={this.props.value}
            selection={this.state.selection}
            onChange={this.handleChange}
          >
            <Editable autoFocus={true} />
          </Slate>
        </div>
      );
    }
  },
);
