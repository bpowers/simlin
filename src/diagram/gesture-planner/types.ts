// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * The vocabulary of the gesture planner: which gesture a press starts, what a
 * frame of it plans, and what a press outcome asks the Canvas to do. See
 * "gesture-planner.ts" in docs/design-plans/2026-09-10-diagram-editing-core.md.
 */

import type { StockFlowView, UID, Variable, ViewElement } from '@simlin/core/datamodel';

import type { FlowEnd, XY } from '../flow-geometry';

export type Tool = 'stock' | 'flow' | 'aux' | 'link' | 'module';

export type Gesture =
  | { readonly kind: 'moveSelection' }
  | { readonly kind: 'slideValve'; readonly flow: UID }
  | { readonly kind: 'offsetSegment'; readonly flow: UID; readonly segmentIndex: number }
  | { readonly kind: 'flowEndpoint'; readonly flow: UID; readonly end: FlowEnd }
  | { readonly kind: 'linkEndpoint'; readonly link: UID }
  | { readonly kind: 'linkArc'; readonly link: UID }
  | { readonly kind: 'createFlow'; readonly from: { readonly stock: UID } | 'empty' }
  | { readonly kind: 'createLink'; readonly from: UID }
  | { readonly kind: 'createElement'; readonly type: 'aux' | 'stock' | 'module' }
  | { readonly kind: 'label'; readonly uid: UID }
  | { readonly kind: 'rubberBand' }
  | { readonly kind: 'pan' };

// A Record over the union's kinds, so adding a kind without listing it here
// fails to compile, and tables that must cover every gesture derive their rows
// from GESTURE_KINDS.
const GESTURE_KIND_TABLE: Record<Gesture['kind'], true> = {
  moveSelection: true,
  slideValve: true,
  offsetSegment: true,
  flowEndpoint: true,
  linkEndpoint: true,
  linkArc: true,
  createFlow: true,
  createLink: true,
  createElement: true,
  label: true,
  rubberBand: true,
  pan: true,
};

export const GESTURE_KINDS = Object.keys(GESTURE_KIND_TABLE) as ReadonlyArray<Gesture['kind']>;

/**
 * A press on a sole selected flow's pipe or valve, before the first move past
 * the click threshold decides between `slideValve` and `offsetSegment`
 * (`latchGesture`). The decision is state the Canvas holds for the rest of the
 * gesture: a pure function of the current pointer could not keep it once the
 * pointer comes back.
 */
export interface PipePress {
  readonly kind: 'pipe';
  readonly flow: UID;
  readonly segmentIndex: number;
}

export type PressGesture = Gesture | PipePress;

/** The default name of a new element: "New Variable", "New Variable 1", ... against everything that exists. */
export type NameAllocator = (base: string) => string;

export interface PlanInput {
  /** The rendered view this frame plans on (the Canvas aborts when it changes geometry, E5). */
  readonly view: StockFlowView;
  readonly variables: ReadonlyMap<string, Variable>;
  /** The selection in effect after the press. */
  readonly selection: ReadonlySet<UID>;
  readonly gesture: PressGesture;
  /** Model coordinates of the press and of the pointer now. */
  readonly press: XY;
  readonly current: XY;
  readonly zoom: number;
  readonly pointerType: string;
  readonly readOnly: boolean;
  readonly names: NameAllocator;
  /** The selection a click (a release within the click threshold) settles on; defaults to `selection`. */
  readonly clickSelection?: ReadonlySet<UID>;
}

export interface GesturePlan {
  /** What the preview renders and, for `commit: 'edit'`, what the commit saves. */
  readonly elements: readonly ViewElement[];
  readonly nextUid: number;
  /** The drop target under the pointer, rendered green when valid and red when not. */
  readonly target?: { readonly uid: UID; readonly valid: boolean };
  readonly commit: 'none' | 'edit' | 'select';
  /** The selection the preview renders and the release applies. */
  readonly selection: ReadonlySet<UID>;
  /** An element whose name editor opens once the release lands. */
  readonly handoff?: { readonly editName: UID };
  /** The element a creation tool staged (never part of `elements` committed by an edit). */
  readonly draft?: ViewElement;
  /** A click on an element's body, which opens its details. */
  readonly details?: boolean;
  /** The edit's name in errors and history; empty unless `commit` is 'edit'. */
  readonly label: string;
}
