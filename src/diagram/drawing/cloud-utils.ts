// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { CloudViewElement, FlowViewElement } from '@system-dynamics/core/datamodel';
import { first, last } from '@system-dynamics/core/collections';

/**
 * Determines if a cloud is on the source side (first point) of a flow.
 * Returns true if the flow's first point is attached to this cloud.
 * Throws if the flow has fewer than 2 points (which would be invalid).
 */
export function isCloudOnSourceSide(cloud: CloudViewElement, flow: FlowViewElement): boolean {
  if (flow.points.size < 2) {
    throw new Error(`Flow ${flow.uid} has fewer than 2 points`);
  }
  const firstPoint = first(flow.points);
  return firstPoint.attachedToUid === cloud.uid;
}

/**
 * Determines if a cloud is on the sink side (last point) of a flow.
 * Returns true if the flow's last point is attached to this cloud.
 * Throws if the flow has fewer than 2 points (which would be invalid).
 */
export function isCloudOnSinkSide(cloud: CloudViewElement, flow: FlowViewElement): boolean {
  if (flow.points.size < 2) {
    throw new Error(`Flow ${flow.uid} has fewer than 2 points`);
  }
  const lastPoint = last(flow.points);
  return lastPoint.attachedToUid === cloud.uid;
}
