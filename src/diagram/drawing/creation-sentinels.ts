// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Sentinel UIDs used while a flow is being created or dragged.
 *
 * During flow creation the Canvas stages placeholder elements that do not yet
 * exist in the persisted view; these negative UIDs mark them so the creation
 * logic (Canvas rendering, `flow-attach`'s `computeFlowAttachment`) can
 * recognize and later replace them with real, positive UIDs on commit.
 *
 * This is the single source of truth. `drawing/Canvas.tsx` and `flow-attach.ts`
 * both re-export these so existing import paths keep resolving, and so the
 * functional-core `flow-attach` module stays free of React/DOM imports.
 */

/** The in-creation flow itself (replaced with a real uid on commit). */
export const inCreationUid = -2;
/** The faux drag target under the cursor while reattaching an existing flow endpoint. */
export const fauxTargetUid = -3;
/** The source cloud staged when a new flow is drawn out of empty space. */
export const inCreationCloudUid = -4;
/** The faux sink target a new flow points at until it snaps to a stock/cloud. */
export const fauxCloudTargetUid = -5;
