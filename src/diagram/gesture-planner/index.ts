// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * The gesture planner: every canvas gesture as a pure function of the press,
 * the pointer and the rendered view. Pure: no React, no DOM, never mutates its
 * inputs.
 *
 * `classifyPress` decides what a press starts; `latchGesture` settles a pipe
 * press into a valve slide or a segment offset on its first move past the click
 * threshold; `planGesture` plans one frame, and its `elements` are both what
 * the preview renders and what a release commits (E2 by construction);
 * `sameGeometry` is the E5 test for whether a republished view invalidates a
 * live gesture. Geometry comes from flow-geometry/, names through the host's
 * allocator, and the model ops a committed view implies from
 * view-model-sync.ts.
 *
 * Modules: `types`, `classify` (classifyPress, latchGesture), `plan`
 * (planGesture and the move, valve, segment, label, rubber-band and draft
 * gestures), `flow-ends` (flow endpoints and flow creation), `links`, `common`
 * (threshold, hit tests, merging, links following their endpoints) and `base`
 * (E5).
 */

export { sameGeometry } from './base';
export {
  classifyPress,
  isLostRelease,
  latchGesture,
  type PressHit,
  type PressInput,
  type PressOutcome,
} from './classify';
export { beyondThreshold, labelSideForPointer, type LabelSideName } from './common';
export { planGesture } from './plan';
export type { Gesture, GesturePlan, NameAllocator, PipePress, PlanInput, PressGesture, Tool } from './types';
