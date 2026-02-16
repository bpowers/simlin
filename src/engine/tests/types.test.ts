// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import {
  SimlinErrorCode,
  SimlinErrorKind,
  SimlinUnitErrorKind,
  SimlinJsonFormat,
  SimlinLinkPolarity,
  SimlinLoopPolarity,
} from '../src/internal/types';

describe('Types', () => {
  describe('SimlinErrorCode', () => {
    it('should have correct values', () => {
      expect(SimlinErrorCode.NoError).toBe(0);
      expect(SimlinErrorCode.DoesNotExist).toBe(1);
      expect(SimlinErrorCode.Generic).toBe(32);
    });
  });

  describe('SimlinErrorKind', () => {
    it('should have correct values', () => {
      expect(SimlinErrorKind.Project).toBe(0);
      expect(SimlinErrorKind.Model).toBe(1);
      expect(SimlinErrorKind.Variable).toBe(2);
      expect(SimlinErrorKind.Units).toBe(3);
      expect(SimlinErrorKind.Simulation).toBe(4);
    });
  });

  describe('SimlinUnitErrorKind', () => {
    it('should have correct values', () => {
      expect(SimlinUnitErrorKind.NotApplicable).toBe(0);
      expect(SimlinUnitErrorKind.Definition).toBe(1);
      expect(SimlinUnitErrorKind.Consistency).toBe(2);
      expect(SimlinUnitErrorKind.Inference).toBe(3);
    });
  });

  describe('SimlinJsonFormat', () => {
    it('should have correct values', () => {
      expect(SimlinJsonFormat.Native).toBe(0);
      expect(SimlinJsonFormat.Sdai).toBe(1);
    });
  });

  describe('SimlinLinkPolarity', () => {
    it('should have correct values', () => {
      expect(SimlinLinkPolarity.Positive).toBe(0);
      expect(SimlinLinkPolarity.Negative).toBe(1);
      expect(SimlinLinkPolarity.Unknown).toBe(2);
    });
  });

  describe('SimlinLoopPolarity', () => {
    it('should have correct values', () => {
      expect(SimlinLoopPolarity.Reinforcing).toBe(0);
      expect(SimlinLoopPolarity.Balancing).toBe(1);
      expect(SimlinLoopPolarity.Undetermined).toBe(2);
    });
  });
});
