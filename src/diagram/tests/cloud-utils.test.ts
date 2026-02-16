// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { List } from 'immutable';

import { Point, FlowViewElement, CloudViewElement } from '@simlin/core/datamodel';

import { isCloudOnSourceSide, isCloudOnSinkSide } from '../drawing/cloud-utils';

function makeFlow(
  uid: number,
  x: number,
  y: number,
  points: Array<{ x: number; y: number; attachedToUid?: number }>,
): FlowViewElement {
  return new FlowViewElement({
    uid,
    name: 'TestFlow',
    ident: 'test_flow',
    var: undefined,
    x,
    y,
    labelSide: 'center',
    points: List(points.map((p) => new Point({ x: p.x, y: p.y, attachedToUid: p.attachedToUid }))),
    isZeroRadius: false,
  });
}

function makeCloud(uid: number, flowUid: number, x: number, y: number): CloudViewElement {
  return new CloudViewElement({
    uid,
    flowUid,
    x,
    y,
    isZeroRadius: false,
  });
}

describe('Cloud to stock attachment', () => {
  const stockUid = 1;
  const flowUid = 2;
  const sourceCloudUid = 3;
  const sinkCloudUid = 4;

  describe('isCloudOnSourceSide', () => {
    it('should return true when cloud is attached to first point of flow', () => {
      const cloud = makeCloud(sourceCloudUid, flowUid, 100, 100);
      const flow = makeFlow(flowUid, 150, 100, [
        { x: 100, y: 100, attachedToUid: sourceCloudUid },
        { x: 200, y: 100, attachedToUid: stockUid },
      ]);

      expect(isCloudOnSourceSide(cloud, flow)).toBe(true);
    });

    it('should return false when cloud is attached to last point of flow', () => {
      const cloud = makeCloud(sinkCloudUid, flowUid, 200, 100);
      const flow = makeFlow(flowUid, 150, 100, [
        { x: 100, y: 100, attachedToUid: stockUid },
        { x: 200, y: 100, attachedToUid: sinkCloudUid },
      ]);

      expect(isCloudOnSourceSide(cloud, flow)).toBe(false);
    });

    it('should return false when cloud is not attached to the flow', () => {
      const cloud = makeCloud(99, flowUid, 300, 100);
      const flow = makeFlow(flowUid, 150, 100, [
        { x: 100, y: 100, attachedToUid: stockUid },
        { x: 200, y: 100, attachedToUid: sinkCloudUid },
      ]);

      expect(isCloudOnSourceSide(cloud, flow)).toBe(false);
    });
  });

  describe('isCloudOnSinkSide', () => {
    it('should return true when cloud is attached to last point of flow', () => {
      const cloud = makeCloud(sinkCloudUid, flowUid, 200, 100);
      const flow = makeFlow(flowUid, 150, 100, [
        { x: 100, y: 100, attachedToUid: stockUid },
        { x: 200, y: 100, attachedToUid: sinkCloudUid },
      ]);

      expect(isCloudOnSinkSide(cloud, flow)).toBe(true);
    });

    it('should return false when cloud is attached to first point of flow', () => {
      const cloud = makeCloud(sourceCloudUid, flowUid, 100, 100);
      const flow = makeFlow(flowUid, 150, 100, [
        { x: 100, y: 100, attachedToUid: sourceCloudUid },
        { x: 200, y: 100, attachedToUid: stockUid },
      ]);

      expect(isCloudOnSinkSide(cloud, flow)).toBe(false);
    });

    it('should return false when cloud is not attached to the flow', () => {
      const cloud = makeCloud(99, flowUid, 300, 100);
      const flow = makeFlow(flowUid, 150, 100, [
        { x: 100, y: 100, attachedToUid: stockUid },
        { x: 200, y: 100, attachedToUid: sinkCloudUid },
      ]);

      expect(isCloudOnSinkSide(cloud, flow)).toBe(false);
    });
  });

  describe('invalid flow handling', () => {
    it('should throw if flow has fewer than 2 points', () => {
      const cloud = makeCloud(sourceCloudUid, flowUid, 100, 100);
      const invalidFlow = makeFlow(flowUid, 100, 100, [{ x: 100, y: 100, attachedToUid: sourceCloudUid }]);

      expect(() => isCloudOnSourceSide(cloud, invalidFlow)).toThrow('has fewer than 2 points');
      expect(() => isCloudOnSinkSide(cloud, invalidFlow)).toThrow('has fewer than 2 points');
    });
  });

  describe('cloud attached to middle point', () => {
    it('should return false for both source and sink when cloud is attached to middle point', () => {
      const middleCloudUid = 5;
      const cloud = makeCloud(middleCloudUid, flowUid, 150, 100);
      // 3-point flow with cloud attached to the middle point (not at source or sink)
      const flow = makeFlow(flowUid, 150, 100, [
        { x: 100, y: 100, attachedToUid: stockUid },
        { x: 150, y: 100, attachedToUid: middleCloudUid },
        { x: 200, y: 100, attachedToUid: sinkCloudUid },
      ]);

      expect(isCloudOnSourceSide(cloud, flow)).toBe(false);
      expect(isCloudOnSinkSide(cloud, flow)).toBe(false);
    });
  });

  describe('L-shaped flow cloud positioning', () => {
    it('should correctly identify source cloud on L-shaped flow', () => {
      const cloud = makeCloud(sourceCloudUid, flowUid, 100, 50);
      // L-shaped flow: cloud at top-left, corner in middle, stock at right
      const flow = makeFlow(flowUid, 150, 100, [
        { x: 100, y: 50, attachedToUid: sourceCloudUid },
        { x: 100, y: 100 }, // corner
        { x: 200, y: 100, attachedToUid: stockUid },
      ]);

      expect(isCloudOnSourceSide(cloud, flow)).toBe(true);
      expect(isCloudOnSinkSide(cloud, flow)).toBe(false);
    });

    it('should correctly identify sink cloud on L-shaped flow', () => {
      const cloud = makeCloud(sinkCloudUid, flowUid, 200, 150);
      // L-shaped flow: stock at left, corner in middle, cloud at bottom-right
      const flow = makeFlow(flowUid, 150, 100, [
        { x: 100, y: 100, attachedToUid: stockUid },
        { x: 200, y: 100 }, // corner
        { x: 200, y: 150, attachedToUid: sinkCloudUid },
      ]);

      expect(isCloudOnSourceSide(cloud, flow)).toBe(false);
      expect(isCloudOnSinkSide(cloud, flow)).toBe(true);
    });
  });
});
