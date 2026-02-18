// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import type { JsonView, JsonViewElement, JsonRect, JsonFlowPoint, JsonLinkPoint } from '@simlin/engine';

import {
  ViewElement,
  StockFlowView,
  Rect,
  Point,
} from '@simlin/core/datamodel';

function rectToJson(rect: Rect): JsonRect {
  return {
    x: rect.x,
    y: rect.y,
    width: rect.width,
    height: rect.height,
  };
}

function pointToFlowPoint(point: Point): JsonFlowPoint {
  return {
    x: point.x,
    y: point.y,
    attachedToUid: point.attachedToUid,
  };
}

function pointToLinkPoint(point: Point): JsonLinkPoint {
  return {
    x: point.x,
    y: point.y,
  };
}

function elementToJson(element: ViewElement): JsonViewElement {
  switch (element.type) {
    case 'stock':
      return {
        type: 'stock',
        uid: element.uid,
        name: element.name,
        x: element.x,
        y: element.y,
        labelSide: element.labelSide,
      };

    case 'flow':
      return {
        type: 'flow',
        uid: element.uid,
        name: element.name,
        x: element.x,
        y: element.y,
        points: element.points.map(pointToFlowPoint),
        labelSide: element.labelSide,
      };

    case 'aux':
      return {
        type: 'aux',
        uid: element.uid,
        name: element.name,
        x: element.x,
        y: element.y,
        labelSide: element.labelSide,
      };

    case 'cloud':
      return {
        type: 'cloud',
        uid: element.uid,
        flowUid: element.flowUid,
        x: element.x,
        y: element.y,
      };

    case 'link': {
      const result: JsonViewElement = {
        type: 'link',
        uid: element.uid,
        fromUid: element.fromUid,
        toUid: element.toUid,
      };

      if (element.arc !== undefined) {
        (result as any).arc = element.arc;
      }

      if (element.multiPoint) {
        (result as any).multiPoints = element.multiPoint.map(pointToLinkPoint);
      }

      return result;
    }

    case 'module':
      return {
        type: 'module',
        uid: element.uid,
        name: element.name,
        x: element.x,
        y: element.y,
        labelSide: element.labelSide,
      };

    case 'alias':
      return {
        type: 'alias',
        uid: element.uid,
        aliasOfUid: element.aliasOfUid,
        x: element.x,
        y: element.y,
        labelSide: element.labelSide,
      };

    case 'group':
      // GroupViewElement stores center-based x/y internally, convert to top-left for JSON
      return {
        type: 'group',
        uid: element.uid,
        name: element.name,
        x: element.x - element.width / 2,
        y: element.y - element.height / 2,
        width: element.width,
        height: element.height,
      };
  }
}

/**
 * Convert a StockFlowView (datamodel) to a JsonView (engine).
 */
export function stockFlowViewToJson(view: StockFlowView): JsonView {
  const elements: JsonViewElement[] = [];

  for (const element of view.elements) {
    elements.push(elementToJson(element));
  }

  const result: JsonView = {
    elements,
  };

  if (view.viewBox && view.viewBox.width > 0 && view.viewBox.height > 0) {
    result.viewBox = rectToJson(view.viewBox);
  }

  if (view.zoom > 0) {
    result.zoom = view.zoom;
  }

  return result;
}
