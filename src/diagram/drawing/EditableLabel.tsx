// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { createEditor, Descendant, Node, Editor, Transforms } from 'slate';
import { withHistory } from 'slate-history';
import { Editable, ReactEditor, Slate, withReact } from 'slate-react';

import { CommonLabelProps, LabelPadding as baseLabelPadding, lineSpacing as baseLineSpacing } from './CommonLabel';
import { AuxRadius } from './default';

import styles from './EditableLabel.module.css';

interface EditingLabelProps extends CommonLabelProps {
  value: Descendant[];
  onChange: (value: Descendant[]) => void;
  onDone: (isCancel: boolean) => void;
  zoom: number;
}

export const EditableLabel = React.memo(function EditableLabel(props: EditingLabelProps): React.ReactElement {
  const { value, onChange, onDone } = props;

  // The Slate editor is created exactly once per mount (Canvas remounts this
  // component for each edit session), seeded from the initial value with the
  // full text selected -- mirroring the class component's constructor.
  const [editor] = React.useState<ReactEditor>(() => {
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
    return editor;
  });

  const handleChange = (value: Descendant[]): void => {
    onChange(value);
  };

  const handlePointerUpDown = (e: React.PointerEvent<HTMLDivElement>): void => {
    e.stopPropagation();
  };

  const isEnter = (code: string): boolean => code === 'Enter' || code === 'NumpadEnter';

  // Standard editor convention: plain Enter commits, shift+Enter inserts a
  // line break (labels are multi-line capable), Escape cancels. The commit
  // fires on keyUp; keyDown must block Slate's default insertBreak for the
  // commit case, otherwise the committed name picks up a trailing newline --
  // which canonicalized into garbage idents like `drain_` and made renames
  // appear to silently fail.
  const handleKeyDown = (e: React.KeyboardEvent<HTMLDivElement>): void => {
    if (isEnter(e.code) && !e.shiftKey) {
      e.preventDefault();
    }
  };

  const handleKeyPress = (e: React.KeyboardEvent<HTMLDivElement>): void => {
    if (isEnter(e.code) && !e.shiftKey) {
      e.stopPropagation();
      onDone(false);
    } else if (e.code === 'Escape') {
      e.stopPropagation();
      onDone(true);
    }
  };

  const { cx, cy, side, zoom } = props;
  const fontSize = 12 * zoom;

  const lines: string[] = value.map((n) => Node.string(n));
  const linesCount = lines.length;

  const maxWidthChars = lines.reduce((prev, curr) => (curr.length > prev ? curr.length : prev), 0);
  const editorWidth = (maxWidthChars * 6 + 10) * zoom;

  const rw: number = props.rw || AuxRadius;
  const rh: number = props.rh || AuxRadius;
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

  return (
    <div
      className={styles.editableLabel}
      style={style}
      onPointerDown={handlePointerUpDown}
      onPointerUp={handlePointerUpDown}
    >
      <Slate editor={editor} initialValue={value} onChange={handleChange}>
        <Editable autoFocus={true} onKeyDown={handleKeyDown} onKeyUp={handleKeyPress} />
      </Slate>
    </div>
  );
});
