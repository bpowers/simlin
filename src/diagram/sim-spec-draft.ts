// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Functional core for the sim-specs drawer fields (start/stop/dt/time units).
// The drawer holds a per-field draft string while focused and commits ONCE on
// settle (blur/Enter); these pure helpers decide what the display text is and
// whether a settled draft is worth committing. Keeping the parse/validation
// here (separate from the React shell) makes the rules exhaustively testable.

export type SimSpecField = 'startTime' | 'stopTime' | 'dt' | 'timeUnits';

export interface SimSpecCommit {
  readonly shouldCommit: boolean;
  // Present iff shouldCommit is true. A number for the three numeric fields, a
  // string for timeUnits.
  readonly value?: number | string;
}

// Renders a model value for display in a field. Numbers use the JS default
// string form, matching the previously plain controlled `<input value={n}>`.
export function formatSimSpecValue(value: number | string): string {
  return `${value}`;
}

// Decides what to do when a field edit settles. It rejects input that would
// patch garbage into the model -- empty / non-numeric / non-finite numbers, and
// a non-positive dt -- and never commits a value equal to the model's current
// one (so a focus-and-blur with no real change is a no-op). Deliberately does
// NOT enforce start < stop: the engine reports that as a model error, and the
// drawer must not invent new validation semantics beyond rejecting garbage.
// timeUnits is a free string; the empty string is a valid (units are optional).
export function resolveSimSpecDraft(field: SimSpecField, raw: string, current: number | string): SimSpecCommit {
  if (field === 'timeUnits') {
    if (raw === current) {
      return { shouldCommit: false };
    }
    return { shouldCommit: true, value: raw };
  }

  // Number('') and Number('  ') are 0, so an empty field must be rejected
  // explicitly rather than trusting Number()/isFinite alone.
  const trimmed = raw.trim();
  if (trimmed === '') {
    return { shouldCommit: false };
  }

  const value = Number(trimmed);
  if (!Number.isFinite(value)) {
    return { shouldCommit: false };
  }
  if (field === 'dt' && value <= 0) {
    return { shouldCommit: false };
  }
  if (value === current) {
    return { shouldCommit: false };
  }
  return { shouldCommit: true, value };
}
