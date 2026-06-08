// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import { render, fireEvent, screen } from '@testing-library/react';

import { UndoRedoBar } from '../UndoRedoBar';

describe('UndoRedoBar', () => {
  test('clicking Undo calls onUndoRedo with "undo"', () => {
    const onUndoRedo = jest.fn();
    render(<UndoRedoBar undoEnabled={true} redoEnabled={true} onUndoRedo={onUndoRedo} />);
    fireEvent.click(screen.getByRole('button', { name: /undo/i }));
    expect(onUndoRedo).toHaveBeenCalledWith('undo');
  });

  test('clicking Redo calls onUndoRedo with "redo"', () => {
    const onUndoRedo = jest.fn();
    render(<UndoRedoBar undoEnabled={true} redoEnabled={true} onUndoRedo={onUndoRedo} />);
    fireEvent.click(screen.getByRole('button', { name: /redo/i }));
    expect(onUndoRedo).toHaveBeenCalledWith('redo');
  });

  test('disables the Undo button when undoEnabled is false', () => {
    const onUndoRedo = jest.fn();
    render(<UndoRedoBar undoEnabled={false} redoEnabled={true} onUndoRedo={onUndoRedo} />);
    const undo = screen.getByRole('button', { name: /undo/i }) as HTMLButtonElement;
    expect(undo.disabled).toBe(true);
    fireEvent.click(undo);
    expect(onUndoRedo).not.toHaveBeenCalled();
  });

  test('disables the Redo button when redoEnabled is false', () => {
    const onUndoRedo = jest.fn();
    render(<UndoRedoBar undoEnabled={true} redoEnabled={false} onUndoRedo={onUndoRedo} />);
    const redo = screen.getByRole('button', { name: /redo/i }) as HTMLButtonElement;
    expect(redo.disabled).toBe(true);
    fireEvent.click(redo);
    expect(onUndoRedo).not.toHaveBeenCalled();
  });
});
