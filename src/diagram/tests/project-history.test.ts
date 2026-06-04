// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { advanceProjectHistory } from '../project-history';

const snap = (n: number): Uint8Array => new Uint8Array([n]);

describe('advanceProjectHistory', () => {
  it('prepends the new snapshot and resets the offset', () => {
    const result = advanceProjectHistory({ projectHistory: [snap(2), snap(1)], projectOffset: 0 }, snap(3), 5);
    expect(result.projectHistory).toEqual([snap(3), snap(2), snap(1)]);
    expect(result.projectOffset).toBe(0);
  });

  it('caps the history at maxSize, dropping the oldest entries', () => {
    const history = [snap(5), snap(4), snap(3), snap(2), snap(1)];
    const result = advanceProjectHistory({ projectHistory: history, projectOffset: 0 }, snap(6), 5);
    expect(result.projectHistory).toEqual([snap(6), snap(5), snap(4), snap(3), snap(2)]);
  });

  it('discards the redo branch when editing after an undo', () => {
    // History (newest -> oldest): [E, D, C, B, A]; the user undid twice and
    // is viewing C (offset 2), then edits C producing F. E and D are the
    // abandoned redo branch: the new history must be F's linear ancestry
    // [F, C, B, A], so that undoing F lands on C (its true parent), not E.
    const history = [snap(5), snap(4), snap(3), snap(2), snap(1)];
    const result = advanceProjectHistory({ projectHistory: history, projectOffset: 2 }, snap(6), 5);
    expect(result.projectHistory).toEqual([snap(6), snap(3), snap(2), snap(1)]);
    expect(result.projectOffset).toBe(0);
  });

  it('editing from the oldest snapshot keeps only that ancestry', () => {
    const history = [snap(3), snap(2), snap(1)];
    const result = advanceProjectHistory({ projectHistory: history, projectOffset: 2 }, snap(4), 5);
    expect(result.projectHistory).toEqual([snap(4), snap(1)]);
  });

  it('works from an empty history', () => {
    const result = advanceProjectHistory({ projectHistory: [], projectOffset: 0 }, snap(1), 5);
    expect(result.projectHistory).toEqual([snap(1)]);
    expect(result.projectOffset).toBe(0);
  });
});
