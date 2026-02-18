// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { Node, Descendant } from 'slate';

import { CustomText, CustomElement } from './SlateEditor';

export function plainDeserialize(type: 'label' | 'equation', str: string): CustomElement[] {
  return str.split('\n').map((line: string): CustomElement => {
    const text: CustomText = { text: line };
    return {
      type,
      children: [text],
    };
  });
}

export function plainSerialize(value: Descendant[]): string {
  return value.map((n) => Node.string(n)).join('\n');
}

export interface Point {
  x: number;
  y: number;
}

export interface Circle extends Point {
  r: number;
}

export interface Rect {
  top: number;
  left: number;
  right: number;
  bottom: number;
}

export interface Box {
  readonly width: number;
  readonly height: number;
}

export function mergeBounds(a: Rect, b: Rect): Rect {
  return {
    top: Math.min(a.top, b.top),
    left: Math.min(a.left, b.left),
    right: Math.max(a.right, b.right),
    bottom: Math.max(a.bottom, b.bottom),
  };
}

export function calcViewBox(elements: readonly (Rect | undefined)[]): Rect | undefined {
  if (elements.length === 0) {
    return undefined;
  }

  const initial = {
    top: Infinity,
    left: Infinity,
    right: -Infinity,
    bottom: -Infinity,
  };

  const bounds: Rect = elements.reduce<Rect>((view, box) => {
    if (box === undefined) {
      return view;
    }
    return mergeBounds(view, box);
  }, initial);

  return bounds;
}

// FIXME: this is copied from sd.js
export const displayName = (name: string): string => {
  return name.replace(/\\n/g, '\n').replace(/_/g, ' ');
};

// Convert a display name to a single-line searchable format.
// Handles both actual newlines (from XMILE parsing) and escaped newlines (from edits).
export const searchableName = (name: string): string => {
  return name.replace(/\\n|\n/g, ' ');
};

// FIXME: this is sort of gross, but works.  The main use is to check
// the result
export const isInf = (n: number): boolean => {
  return !isFinite(n) || n > 2e14;
};

export const isZero = (n: number, tolerance = 0.0000001): boolean => {
  return Math.abs(n) < tolerance;
};

export const isEqual = (a: number, b: number, tolerance = 0.0000001): boolean => {
  return isZero(a - b, tolerance);
};

export const square = (n: number): number => {
  return Math.pow(n, 2);
};

export const distance = (a: Point, b: Point): number => {
  const dx = a.x - b.x;
  const dy = a.y - b.y;
  return Math.sqrt(square(dx) + square(dy));
};

export function screenToCanvasPoint(x: number, y: number, zoom: number): Point {
  const canvas = new DOMPoint(x, y).matrixTransform(
    new DOMMatrix([
      zoom,
      0,
      0,
      zoom,
      0, // dx
      0, // dy
    ]).inverse(),
  );

  return {
    x: canvas.x,
    y: canvas.y,
  };
}
