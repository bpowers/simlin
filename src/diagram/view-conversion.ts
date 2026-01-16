// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import type { JsonView, JsonViewElement, JsonRect, JsonFlowPoint, JsonLinkPoint } from '@system-dynamics/engine2';

import {
  ViewElement,
  StockViewElement,
  FlowViewElement,
  AuxViewElement,
  CloudViewElement,
  LinkViewElement,
  ModuleViewElement,
  AliasViewElement,
  StockFlowView,
  Rect,
  Point,
} from '@system-dynamics/core/datamodel';

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
    attached_to_uid: point.attachedToUid,
  };
}

function pointToLinkPoint(point: Point): JsonLinkPoint {
  return {
    x: point.x,
    y: point.y,
  };
}

function elementToJson(element: ViewElement): JsonViewElement | null {
  if (element instanceof StockViewElement) {
    return {
      type: 'stock',
      uid: element.uid,
      name: element.name,
      x: element.x,
      y: element.y,
      label_side: element.labelSide,
    };
  }

  if (element instanceof FlowViewElement) {
    return {
      type: 'flow',
      uid: element.uid,
      name: element.name,
      x: element.x,
      y: element.y,
      points: element.points.map(pointToFlowPoint).toArray(),
      label_side: element.labelSide,
    };
  }

  if (element instanceof AuxViewElement) {
    return {
      type: 'aux',
      uid: element.uid,
      name: element.name,
      x: element.x,
      y: element.y,
      label_side: element.labelSide,
    };
  }

  if (element instanceof CloudViewElement) {
    return {
      type: 'cloud',
      uid: element.uid,
      flow_uid: element.flowUid,
      x: element.x,
      y: element.y,
    };
  }

  if (element instanceof LinkViewElement) {
    const result: JsonViewElement = {
      type: 'link',
      uid: element.uid,
      from_uid: element.fromUid,
      to_uid: element.toUid,
    };

    if (element.arc !== undefined) {
      (result as any).arc = element.arc;
    }

    if (element.multiPoint) {
      (result as any).multi_points = element.multiPoint.map(pointToLinkPoint).toArray();
    }

    return result;
  }

  if (element instanceof ModuleViewElement) {
    return {
      type: 'module',
      uid: element.uid,
      name: element.name,
      x: element.x,
      y: element.y,
      label_side: element.labelSide,
    };
  }

  if (element instanceof AliasViewElement) {
    return {
      type: 'alias',
      uid: element.uid,
      alias_of_uid: element.aliasOfUid,
      x: element.x,
      y: element.y,
      label_side: element.labelSide,
    };
  }

  return null;
}

/**
 * Convert a StockFlowView (datamodel) to a JsonView (engine2).
 */
export function stockFlowViewToJson(view: StockFlowView): JsonView {
  const elements: JsonViewElement[] = [];

  for (const element of view.elements) {
    const jsonElement = elementToJson(element);
    if (jsonElement) {
      elements.push(jsonElement);
    }
  }

  const result: JsonView = {
    elements,
  };

  if (view.viewBox && view.viewBox.width > 0 && view.viewBox.height > 0) {
    result.view_box = rectToJson(view.viewBox);
  }

  if (view.zoom > 0) {
    result.zoom = view.zoom;
  }

  return result;
}
