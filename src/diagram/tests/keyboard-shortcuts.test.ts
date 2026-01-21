// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { detectUndoRedo, isEditableElement } from '../keyboard-shortcuts';

describe('keyboard-shortcuts', () => {
  describe('detectUndoRedo', () => {
    describe('undo detection', () => {
      it('should detect Cmd+Z as undo on Mac', () => {
        const event = { key: 'z', metaKey: true, ctrlKey: false, shiftKey: false, altKey: false };
        expect(detectUndoRedo(event)).toBe('undo');
      });

      it('should detect Ctrl+Z as undo on Windows/Linux', () => {
        const event = { key: 'z', metaKey: false, ctrlKey: true, shiftKey: false, altKey: false };
        expect(detectUndoRedo(event)).toBe('undo');
      });

      it('should detect uppercase Z key as undo', () => {
        const event = { key: 'Z', metaKey: true, ctrlKey: false, shiftKey: false, altKey: false };
        expect(detectUndoRedo(event)).toBe('undo');
      });

      it('should detect Cmd+Z with Ctrl also pressed as undo', () => {
        const event = { key: 'z', metaKey: true, ctrlKey: true, shiftKey: false, altKey: false };
        expect(detectUndoRedo(event)).toBe('undo');
      });
    });

    describe('redo detection', () => {
      it('should detect Cmd+Shift+Z as redo on Mac', () => {
        const event = { key: 'z', metaKey: true, ctrlKey: false, shiftKey: true, altKey: false };
        expect(detectUndoRedo(event)).toBe('redo');
      });

      it('should detect Ctrl+Shift+Z as redo on Windows/Linux', () => {
        const event = { key: 'z', metaKey: false, ctrlKey: true, shiftKey: true, altKey: false };
        expect(detectUndoRedo(event)).toBe('redo');
      });

      it('should detect uppercase Z key with Shift as redo', () => {
        const event = { key: 'Z', metaKey: true, ctrlKey: false, shiftKey: true, altKey: false };
        expect(detectUndoRedo(event)).toBe('redo');
      });
    });

    describe('non-matching inputs', () => {
      it('should return null when no modifier key is pressed', () => {
        const event = { key: 'z', metaKey: false, ctrlKey: false, shiftKey: false, altKey: false };
        expect(detectUndoRedo(event)).toBeNull();
      });

      it('should return null for non-Z keys', () => {
        const event = { key: 'a', metaKey: true, ctrlKey: false, shiftKey: false, altKey: false };
        expect(detectUndoRedo(event)).toBeNull();
      });

      it('should return null for Shift+Z without Cmd/Ctrl', () => {
        const event = { key: 'z', metaKey: false, ctrlKey: false, shiftKey: true, altKey: false };
        expect(detectUndoRedo(event)).toBeNull();
      });

      it('should return null for other letter keys with modifiers', () => {
        const event = { key: 'y', metaKey: true, ctrlKey: false, shiftKey: false, altKey: false };
        expect(detectUndoRedo(event)).toBeNull();
      });

      it('should return null for number keys with modifiers', () => {
        const event = { key: '1', metaKey: true, ctrlKey: false, shiftKey: false, altKey: false };
        expect(detectUndoRedo(event)).toBeNull();
      });

      it('should return null when Alt is pressed with Cmd+Z', () => {
        const event = { key: 'z', metaKey: true, ctrlKey: false, shiftKey: false, altKey: true };
        expect(detectUndoRedo(event)).toBeNull();
      });

      it('should return null when Alt is pressed with Ctrl+Z', () => {
        const event = { key: 'z', metaKey: false, ctrlKey: true, shiftKey: false, altKey: true };
        expect(detectUndoRedo(event)).toBeNull();
      });

      it('should return null when Alt is pressed with Cmd+Shift+Z', () => {
        const event = { key: 'z', metaKey: true, ctrlKey: false, shiftKey: true, altKey: true };
        expect(detectUndoRedo(event)).toBeNull();
      });
    });
  });

  describe('isEditableElement', () => {
    describe('editable elements', () => {
      it('should return true for input elements', () => {
        const input = document.createElement('input');
        expect(isEditableElement(input)).toBe(true);
      });

      it('should return true for textarea elements', () => {
        const textarea = document.createElement('textarea');
        expect(isEditableElement(textarea)).toBe(true);
      });

      it('should return true for contentEditable elements', () => {
        const div = document.createElement('div');
        div.contentEditable = 'true';
        expect(isEditableElement(div)).toBe(true);
      });

      it('should return true when isContentEditable is true (inherited editability)', () => {
        // Test the isContentEditable code path by mocking the property.
        // In real browsers, child elements inside a contentEditable parent
        // have isContentEditable=true even without the attribute set directly.
        const span = document.createElement('span');
        Object.defineProperty(span, 'isContentEditable', { value: true });
        expect(isEditableElement(span)).toBe(true);
      });
    });

    describe('non-editable elements', () => {
      it('should return false for null', () => {
        expect(isEditableElement(null)).toBe(false);
      });

      it('should return false for regular div elements', () => {
        const div = document.createElement('div');
        expect(isEditableElement(div)).toBe(false);
      });

      it('should return false for span elements', () => {
        const span = document.createElement('span');
        expect(isEditableElement(span)).toBe(false);
      });

      it('should return false for button elements', () => {
        const button = document.createElement('button');
        expect(isEditableElement(button)).toBe(false);
      });

      it('should return false for svg elements', () => {
        const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
        expect(isEditableElement(svg)).toBe(false);
      });

      it('should return false for contentEditable="false" elements', () => {
        const div = document.createElement('div');
        div.contentEditable = 'false';
        expect(isEditableElement(div)).toBe(false);
      });
    });
  });
});
