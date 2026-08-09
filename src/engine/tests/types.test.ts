// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { describe, it, expect } from '@rstest/core';

import { SimlinLoopPolarity } from '../src/internal/types';
import { LoopPolarity } from '../src/types';

describe('Types', () => {
  it('public LoopPolarity matches the FFI numeric values', () => {
    // The public enum and the internal FFI enum are declared independently, so
    // they must be pinned against each other: direct-backend's
    // `polarity as unknown as LoopPolarity` cast is only sound while they
    // agree. Includes the Rux/Bux mixed-sign runtime variants (GH #495).
    expect(LoopPolarity.Reinforcing).toBe(SimlinLoopPolarity.Reinforcing);
    expect(LoopPolarity.Balancing).toBe(SimlinLoopPolarity.Balancing);
    expect(LoopPolarity.Undetermined).toBe(SimlinLoopPolarity.Undetermined);
    expect(LoopPolarity.MostlyReinforcing).toBe(SimlinLoopPolarity.MostlyReinforcing);
    expect(LoopPolarity.MostlyBalancing).toBe(SimlinLoopPolarity.MostlyBalancing);
  });
});
