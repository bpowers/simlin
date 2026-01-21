// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { detectUndoRedo } from '../keyboard-shortcuts';

describe('keyboard-shortcuts', () => {
  describe('detectUndoRedo', () => {
    describe('undo detection', () => {
      it('should detect Cmd+Z as undo on Mac', () => {
        const event = { key: 'z', metaKey: true, ctrlKey: false, shiftKey: false };
        expect(detectUndoRedo(event)).toBe('undo');
      });

      it('should detect Ctrl+Z as undo on Windows/Linux', () => {
        const event = { key: 'z', metaKey: false, ctrlKey: true, shiftKey: false };
        expect(detectUndoRedo(event)).toBe('undo');
      });

      it('should detect uppercase Z key as undo', () => {
        const event = { key: 'Z', metaKey: true, ctrlKey: false, shiftKey: false };
        expect(detectUndoRedo(event)).toBe('undo');
      });

      it('should detect Cmd+Z with Ctrl also pressed as undo', () => {
        const event = { key: 'z', metaKey: true, ctrlKey: true, shiftKey: false };
        expect(detectUndoRedo(event)).toBe('undo');
      });
    });

    describe('redo detection', () => {
      it('should detect Cmd+Shift+Z as redo on Mac', () => {
        const event = { key: 'z', metaKey: true, ctrlKey: false, shiftKey: true };
        expect(detectUndoRedo(event)).toBe('redo');
      });

      it('should detect Ctrl+Shift+Z as redo on Windows/Linux', () => {
        const event = { key: 'z', metaKey: false, ctrlKey: true, shiftKey: true };
        expect(detectUndoRedo(event)).toBe('redo');
      });

      it('should detect uppercase Z key with Shift as redo', () => {
        const event = { key: 'Z', metaKey: true, ctrlKey: false, shiftKey: true };
        expect(detectUndoRedo(event)).toBe('redo');
      });
    });

    describe('non-matching inputs', () => {
      it('should return null when no modifier key is pressed', () => {
        const event = { key: 'z', metaKey: false, ctrlKey: false, shiftKey: false };
        expect(detectUndoRedo(event)).toBeNull();
      });

      it('should return null for non-Z keys', () => {
        const event = { key: 'a', metaKey: true, ctrlKey: false, shiftKey: false };
        expect(detectUndoRedo(event)).toBeNull();
      });

      it('should return null for Shift+Z without Cmd/Ctrl', () => {
        const event = { key: 'z', metaKey: false, ctrlKey: false, shiftKey: true };
        expect(detectUndoRedo(event)).toBeNull();
      });

      it('should return null for other letter keys with modifiers', () => {
        const event = { key: 'y', metaKey: true, ctrlKey: false, shiftKey: false };
        expect(detectUndoRedo(event)).toBeNull();
      });

      it('should return null for number keys with modifiers', () => {
        const event = { key: '1', metaKey: true, ctrlKey: false, shiftKey: false };
        expect(detectUndoRedo(event)).toBeNull();
      });
    });
  });
});
