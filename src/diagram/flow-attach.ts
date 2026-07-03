// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Pure functional core for flow attachment, reattachment, and creation.
 *
 * Extracted verbatim (behavior-preserving) from `Editor.handleFlowAttach`,
 * which is the hairiest interaction path in the editor: it covers flow
 * creation, endpoint reattachment, and the full cloud lifecycle
 * (create/update/delete). The imperative shell in Editor.tsx now reads the
 * live view/model from state, calls `computeFlowAttachment`, applies the
 * returned ops via the engine, and commits the returned view + selection.
 *
 * The two endpoint branches (source = first point, sink = last point) were
 * near-duplicate ~70-line blocks in the original. They are unified here into a
 * single `reattachEndpoint` parameterized by `end`. The branches differ only
 * in:
 *   - which endpoint of the flow is operated on (first vs last point), and
 *   - which idents the bookkeeping records (source-side vs sink-side), which
 *     in turn drive outflows (source) vs inflows (sink) operations.
 * Everything else -- cloud creation/update/deletion, the move-delta math, the
 * UpdateCloudAndFlow call -- is byte-identical between the two.
 *
 * The four `updateStockFlows` op builders in the original (source attach/detach
 * touching outflows, sink attach/detach touching inflows) are unified into one
 * `stockFlowsOp` helper. See `computeFlowAttachment` for the one deliberate
 * normalization (collapsing exact-duplicate ops).
 */

import { first, last } from '@simlin/core/collections';
import { defined } from '@simlin/core/common';
import {
  CloudViewElement,
  FlowViewElement,
  NamedViewElement,
  StockFlowView,
  StockViewElement,
  UID,
  Variable,
  ViewElement,
} from '@simlin/core/datamodel';
import type { JsonModelOperation } from '@simlin/engine';

import { pinSourceToStockEdge, UpdateCloudAndFlow } from './drawing/Flow';
// The drawing-layer Point is a bare {x, y} (no attachedToUid). cursorMoveDelta
// and fauxTargetCenter are screen-space positions/deltas, not flow points, so
// they use this type -- matching what Canvas passes to onMoveFlow.
import type { Point as CanvasPoint } from './drawing/common';

// Sentinel UIDs used by Canvas during flow creation, defined once in
// drawing/creation-sentinels and re-exported here so this functional-core
// module stays free of React/DOM dependencies while callers that import them
// from `../flow-attach` keep resolving.
export { inCreationUid, inCreationCloudUid, fauxCloudTargetUid } from './drawing/creation-sentinels';
import { inCreationUid, inCreationCloudUid, fauxCloudTargetUid } from './drawing/creation-sentinels';

/**
 * Inputs to `computeFlowAttachment`, mirroring the arguments Canvas passes to
 * Editor.handleFlowAttach.
 */
export interface FlowAttachParams {
  /** The flow being attached/reattached/created (may carry a sentinel uid). */
  readonly flow: FlowViewElement;
  /** UID of the snap target stock/cloud, or 0 when released over empty space. */
  readonly targetUid: number;
  /** Pointer delta accumulated during the drag. */
  readonly cursorMoveDelta: CanvasPoint;
  /** Center of the faux target, used when a created flow ends in empty space. */
  readonly fauxTargetCenter: CanvasPoint | undefined;
  /** Whether the flow is still being created (drives flowStillBeingCreated). */
  readonly inCreation: boolean;
  /** True when the dragged endpoint is the source (first point). */
  readonly isSourceAttach: boolean;
}

/**
 * Result of `computeFlowAttachment`: a new view (elements + nextUid), the model
 * operations to apply, and the selection/creation state to commit. This is a
 * pure description of the change; the caller performs all side effects.
 */
export interface FlowAttachResult {
  readonly elements: readonly ViewElement[];
  readonly nextUid: number;
  readonly ops: readonly JsonModelOperation[];
  readonly selection: ReadonlySet<UID> | undefined;
  readonly isCreatingNew: boolean;
}

/** Mutable bookkeeping threaded through the endpoint reattachment logic. */
interface AttachState {
  // Sink-side idents (drive inflows operations).
  stockDetachingIdent: string | undefined;
  stockAttachingIdent: string | undefined;
  // Source-side idents (drive outflows operations).
  sourceStockIdent: string | undefined;
  sourceStockDetachingIdent: string | undefined;
  sourceStockAttachingIdent: string | undefined;
  // The cloud uid to remove (set when a flow detaches from a cloud).
  uidToDelete: number | undefined;
  // A cloud whose position changed (must be substituted back into elements).
  updatedCloud: ViewElement | undefined;
  // Newly created clouds to append to the element list.
  newClouds: ViewElement[];
  // The next free uid; incremented as clouds/flows are materialized.
  nextUid: number;
}

/**
 * Reattach one endpoint (source = first point, sink = last point) of an
 * existing flow. Mutates `state` for the cloud/ident bookkeeping and returns
 * the updated flow element. This preserves the original two-branch behavior
 * exactly; the only structural change is parameterizing on `end`.
 */
function reattachEndpoint(
  element: FlowViewElement,
  end: 'source' | 'sink',
  params: FlowAttachParams,
  getUid: (uid: number) => ViewElement,
  state: AttachState,
): FlowViewElement {
  const { targetUid, cursorMoveDelta, flow } = params;

  // The endpoint being moved: first point for source, last point for sink.
  const oldEnd = getUid(defined((end === 'source' ? first : last)(element.points).attachedToUid));

  let newCloud = false;
  let updateCloud = false;
  let endpoint: StockViewElement | CloudViewElement;

  if (targetUid) {
    if (oldEnd.type === 'cloud') {
      state.uidToDelete = oldEnd.uid;
    }
    const newTarget = getUid(targetUid);
    if (newTarget.type !== 'stock' && newTarget.type !== 'cloud') {
      throw new Error(`new target isn't a stock or cloud (uid ${newTarget.uid})`);
    }
    endpoint = newTarget;
  } else if (oldEnd.type === 'cloud') {
    updateCloud = true;
    endpoint = {
      ...oldEnd,
      x: oldEnd.x - cursorMoveDelta.x,
      y: oldEnd.y - cursorMoveDelta.y,
    };
  } else {
    // Detaching from a stock - create a new cloud at the release position.
    // oldEnd.x - cursorMoveDelta.x/y places the cloud where the user
    // released, not where they started.
    newCloud = true;
    endpoint = {
      type: 'cloud' as const,
      uid: state.nextUid++,
      x: oldEnd.x - cursorMoveDelta.x,
      y: oldEnd.y - cursorMoveDelta.y,
      flowUid: flow.uid,
      isZeroRadius: false,
      ident: undefined,
    };
  }

  if (oldEnd.uid !== endpoint.uid) {
    if (oldEnd.type === 'stock') {
      if (end === 'source') {
        state.sourceStockDetachingIdent = oldEnd.ident;
      } else {
        state.stockDetachingIdent = oldEnd.ident;
      }
    }
    if (endpoint.type === 'stock') {
      if (end === 'source') {
        state.sourceStockAttachingIdent = endpoint.ident;
      } else {
        state.stockAttachingIdent = endpoint.ident;
      }
    }
  }

  const moveDelta = {
    x: oldEnd.x - endpoint.x,
    y: oldEnd.y - endpoint.y,
  };
  const points = element.points.map((point) => {
    if (point.attachedToUid !== oldEnd.uid) {
      return point;
    }
    return { ...point, attachedToUid: endpoint.uid };
  });
  endpoint = {
    ...endpoint,
    x: oldEnd.x,
    y: oldEnd.y,
  } as StockViewElement | CloudViewElement;
  element = { ...element, points };

  let updatedEndpoint: StockViewElement | CloudViewElement;
  [updatedEndpoint, element] = UpdateCloudAndFlow(endpoint, element, moveDelta);
  if (newCloud) {
    state.newClouds.push(updatedEndpoint);
  } else if (updateCloud) {
    state.updatedCloud = updatedEndpoint;
  }

  return element;
}

/**
 * Build a single `updateStockFlows` operation that adds or removes `flowIdent`
 * from the named stock's inflows or outflows. Returns undefined when the
 * variable is missing or not a stock (matching the original guard).
 *
 * `list`/`action` collapse the four near-identical op builders from the
 * original (source attach/detach -> outflows add/remove; sink attach/detach
 * -> inflows add/remove). Every op carries the *full* inflow and outflow lists
 * because the engine does full replacement on updateStockFlows, so the other
 * list must be echoed back unchanged.
 */
function stockFlowsOp(
  variables: ReadonlyMap<string, Variable>,
  stockIdent: string,
  list: 'inflows' | 'outflows',
  action: 'add' | 'remove',
  flowIdent: string,
): JsonModelOperation | undefined {
  const stockVar = variables.get(stockIdent);
  if (stockVar?.type !== 'stock') {
    return undefined;
  }

  const mutate = (current: readonly string[]): string[] =>
    action === 'add' ? [...current, flowIdent] : current.filter((f) => f !== flowIdent);

  return {
    type: 'updateStockFlows',
    payload: {
      ident: stockVar.ident,
      inflows: list === 'inflows' ? mutate(stockVar.inflows) : [...stockVar.inflows],
      outflows: list === 'outflows' ? mutate(stockVar.outflows) : [...stockVar.outflows],
    },
  };
}

/**
 * Compute the full result of a flow attach/reattach/create interaction.
 *
 * This is a behavior-preserving extraction of the body of
 * Editor.handleFlowAttach up to (but not including) the engine round-trip. It
 * is pure: given the current view, the active model's variables, and the
 * interaction params, it returns the new elements, the operations to apply,
 * and the selection/creation state. The caller owns all side effects.
 *
 * Throws `unknown uid <n>` (preserved from the original `getUid`) when an
 * attachment references a uid not present in the view.
 */
export function computeFlowAttachment(
  view: StockFlowView,
  variables: ReadonlyMap<string, Variable>,
  params: FlowAttachParams,
): FlowAttachResult {
  const { targetUid, fauxTargetCenter, isSourceAttach, cursorMoveDelta } = params;

  let selection: ReadonlySet<UID> | undefined = undefined;
  let isCreatingNew = false;

  const getUid = (uid: number): ViewElement => {
    for (const e of view.elements) {
      if (e.uid === uid) {
        return e;
      }
    }
    throw new Error(`unknown uid ${uid}`);
  };

  const state: AttachState = {
    stockDetachingIdent: undefined,
    stockAttachingIdent: undefined,
    sourceStockIdent: undefined,
    sourceStockDetachingIdent: undefined,
    sourceStockAttachingIdent: undefined,
    uidToDelete: undefined,
    updatedCloud: undefined,
    newClouds: [],
    nextUid: view.nextUid,
  };

  let flow = params.flow;

  let elements: ViewElement[] = view.elements.map((element: ViewElement) => {
    if (element.uid !== flow.uid) {
      return element;
    }
    if (element.type !== 'flow') {
      return element;
    }
    return reattachEndpoint(element, isSourceAttach ? 'source' : 'sink', params, getUid, state);
  });

  // we might have updated some clouds
  elements = elements.map((element: ViewElement) => {
    if (state.updatedCloud && state.updatedCloud.uid === element.uid) {
      return state.updatedCloud;
    }
    return element;
  });
  // if we have something to delete, do it here
  elements = elements.filter((e) => e.uid !== state.uidToDelete);

  if (flow.uid === inCreationUid) {
    flow = {
      ...flow,
      uid: state.nextUid++,
    };
    const firstPt = first(flow.points);
    const sourceUid = firstPt.attachedToUid;
    if (sourceUid === inCreationCloudUid) {
      const newCloud: CloudViewElement = {
        type: 'cloud',
        uid: state.nextUid++,
        x: firstPt.x,
        y: firstPt.y,
        flowUid: flow.uid,
        isZeroRadius: false,
        ident: undefined,
      };
      elements = [...elements, newCloud];
      flow = {
        ...flow,
        points: flow.points.map((pt) => {
          if (pt.attachedToUid === inCreationCloudUid) {
            return { ...pt, attachedToUid: newCloud.uid };
          }
          return pt;
        }),
      };
    } else if (sourceUid) {
      const sourceStock = getUid(sourceUid) as StockViewElement;
      state.sourceStockIdent = defined(sourceStock.ident);
    }
    const lastPt = last(flow.points);
    if (lastPt.attachedToUid === fauxCloudTargetUid) {
      if (targetUid) {
        // Attaching the new flow's sink to an existing stock. Route it exactly
        // the way an existing flow's endpoint reattaches (see reattachEndpoint):
        // pin the sink to the stock's EDGE and keep the flow orthogonal. The
        // in-creation flow's points are degenerate -- both sit at the press
        // point, since the drag offset is applied only at render time and never
        // committed back to the element -- so we drive UpdateCloudAndFlow from
        // the current sink point: place the target at the old sink position, then
        // move it to the stock by the resulting delta. UpdateCloudAndFlow picks
        // the axis from that delta, aligns the sink to the source's axis, and
        // clips it to the stock face (a degenerate flow stays straight). This
        // replaces an earlier snap-to-center that drew the arrowhead behind the
        // stock; a prior zero-delta route had instead collapsed the sink onto the
        // source column, which is why the center snap was tried.
        const to = getUid(targetUid) as StockViewElement | CloudViewElement;
        if (to.type === 'stock') {
          state.stockAttachingIdent = defined(to.ident);
        }
        const oldSink = last(flow.points);
        flow = {
          ...flow,
          points: flow.points.map((pt) =>
            pt.attachedToUid === fauxCloudTargetUid ? { ...pt, attachedToUid: to.uid } : pt,
          ),
        };
        const sinkDelta = { x: oldSink.x - to.x, y: oldSink.y - to.y };
        const targetAtOldSink = { ...to, x: oldSink.x, y: oldSink.y } as StockViewElement | CloudViewElement;
        // A flow sink only ever attaches to a stock (isValidTarget gates the
        // canvas to stock targets), and a stock keeps its position in the view --
        // so we discard the routed endpoint and keep only the re-routed flow.
        [, flow] = UpdateCloudAndFlow(targetAtOldSink, flow, sinkDelta);
      } else {
        let to: StockViewElement | CloudViewElement = {
          type: 'cloud' as const,
          uid: state.nextUid++,
          x: defined(fauxTargetCenter).x,
          y: defined(fauxTargetCenter).y,
          flowUid: flow.uid,
          isZeroRadius: false,
          ident: undefined,
        };
        flow = {
          ...flow,
          points: flow.points.map((pt) => {
            if (pt.attachedToUid === fauxCloudTargetUid) {
              return { ...pt, attachedToUid: to.uid };
            }
            return pt;
          }),
        };
        // The new sink cloud is materialized at the press point; the real drag
        // delta moves it (and the flow's sink) out to the release position.
        [to, flow] = UpdateCloudAndFlow(to, flow, cursorMoveDelta);
        elements = [...elements, to];
      }
    }
    // A flow drawn OUT of a stock stages its source point at the stock's
    // CENTER; now that the sink is routed, pin the source onto the facing
    // edge so the persisted endpoint honors the edge-attachment rule (it
    // otherwise hides under the stock body until the next stock drag).
    if (sourceUid !== undefined && sourceUid !== inCreationCloudUid) {
      const sourceEl = getUid(sourceUid);
      if (sourceEl.type === 'stock') {
        flow = pinSourceToStockEdge(flow, sourceEl);
      }
    }
    elements = [...elements, flow];
    selection = new Set([flow.uid]);
    isCreatingNew = true;
  }
  elements = [...elements, ...state.newClouds];

  // Build the operations. Each updateStockFlows carries the full inflow and
  // outflow lists because the engine replaces them wholesale.
  const rawOps: (JsonModelOperation | undefined)[] = [];

  if (isCreatingNew) {
    rawOps.push({
      type: 'upsertFlow',
      payload: {
        flow: {
          name: (flow as NamedViewElement).name,
          equation: '',
        },
      },
    });
  }

  // Source side -> outflows. sourceStockIdent (creation) and
  // sourceStockAttachingIdent (reattach) both ADD this flow to outflows and
  // are computed from the same pre-patch stockVar, so when both are set they
  // produce identical ops; the dedup pass below collapses the duplicate.
  if (state.sourceStockIdent) {
    rawOps.push(stockFlowsOp(variables, state.sourceStockIdent, 'outflows', 'add', flow.ident));
  }
  if (state.sourceStockAttachingIdent) {
    rawOps.push(stockFlowsOp(variables, state.sourceStockAttachingIdent, 'outflows', 'add', flow.ident));
  }
  if (state.sourceStockDetachingIdent) {
    rawOps.push(stockFlowsOp(variables, state.sourceStockDetachingIdent, 'outflows', 'remove', flow.ident));
  }
  // Sink side -> inflows.
  if (state.stockAttachingIdent) {
    rawOps.push(stockFlowsOp(variables, state.stockAttachingIdent, 'inflows', 'add', flow.ident));
  }
  if (state.stockDetachingIdent) {
    rawOps.push(stockFlowsOp(variables, state.stockDetachingIdent, 'inflows', 'remove', flow.ident));
  }

  // Drop ops whose variable was missing/non-stock (stockFlowsOp returned
  // undefined), then collapse exact-duplicate ops. The original emitted two
  // identical updateStockFlows ops when both sourceStockIdent and
  // sourceStockAttachingIdent were set; under the engine's full-replacement
  // semantics the second op rewrites the same content, so collapsing exact
  // duplicates is a no-op in effect. This is the ONE deliberate normalization
  // in this extraction -- everything else is byte-identical in effect.
  const ops: JsonModelOperation[] = [];
  const seen = new Set<string>();
  for (const op of rawOps) {
    if (op === undefined) {
      continue;
    }
    const key = JSON.stringify(op);
    if (seen.has(key)) {
      continue;
    }
    seen.add(key);
    ops.push(op);
  }

  return {
    elements,
    nextUid: state.nextUid,
    ops,
    selection,
    isCreatingNew,
  };
}

/**
 * Compute the GROWN geometry of a flow whose cloud-terminated endpoint is being
 * dragged, for the live drag preview.
 *
 * This is the shared routing behind two visually-identical interactions:
 *   - flow CREATION (the flow tool stages a degenerate flow and records the drag
 *     only as `moveDelta`; the sink follows the cursor), and
 *   - dragging an EXISTING flow's cloud endpoint (source or sink) to move or
 *     reattach it.
 * In both cases the dragged endpoint follows the cursor over empty space, or
 * pins to a hovered stock's edge, while the OTHER endpoint stays fixed and the
 * flow stays orthogonal. Routing both previews through `UpdateCloudAndFlow` --
 * the same function the commit (`computeFlowAttachment`/`reattachEndpoint`)
 * uses -- keeps the preview identical to the committed result and makes an
 * existing-cloud drag feel the same as creation (the flow line, arrowhead, and
 * valve all track the cursor, not just the valve).
 *
 * `isSource` selects which endpoint moves (source = first point, sink = last).
 * `target` is the valid stock under the cursor (or undefined over empty space).
 * `moveDelta` is the Canvas convention (= press - cursor). Pure: returns a new
 * flow element; the caller swaps it into the render.
 */
export function growEndpointDrag(
  flow: FlowViewElement,
  isSource: boolean,
  moveDelta: CanvasPoint,
  target: StockViewElement | CloudViewElement | undefined,
): FlowViewElement {
  const endIndex = isSource ? 0 : flow.points.length - 1;
  const endPt = isSource ? first(flow.points) : last(flow.points);
  if (target !== undefined) {
    // Hovering a valid stock: pin the endpoint to its edge -- the same routing
    // the commit (computeFlowAttachment) and reattachEndpoint use. Temporarily
    // attach the endpoint to the target so UpdateCloudAndFlow can route and clip
    // it; the real attachment is (re)computed on release.
    const attached: FlowViewElement = {
      ...flow,
      points: flow.points.map((pt, i) => (i === endIndex ? { ...pt, attachedToUid: target.uid } : pt)),
    };
    const endDelta = { x: endPt.x - target.x, y: endPt.y - target.y };
    const targetAtOldEnd = { ...target, x: endPt.x, y: endPt.y } as StockViewElement | CloudViewElement;
    return UpdateCloudAndFlow(targetAtOldEnd, attached, endDelta)[1];
  }
  // Over empty space: the endpoint follows the cursor. Move the cloud (the
  // endpoint's current attachment, real or faux) from its current position by
  // moveDelta; UpdateCloudAndFlow picks the axis from moveDelta and keeps the
  // opposite endpoint fixed.
  const endCloud: CloudViewElement = {
    type: 'cloud',
    uid: endPt.attachedToUid ?? fauxCloudTargetUid,
    flowUid: flow.uid,
    x: endPt.x,
    y: endPt.y,
    isZeroRadius: false,
    ident: undefined,
  };
  return UpdateCloudAndFlow(endCloud, flow, moveDelta)[1];
}

/**
 * Live drag preview for flow CREATION: the sink follows the cursor (or snaps to
 * a hovered stock's edge) with the source fixed. A thin wrapper over
 * `growEndpointDrag` (the sink is the last point, so `isSource: false`) kept as
 * a named entry point for the creation call site and its tests.
 */
export function growInCreationFlow(
  flow: FlowViewElement,
  moveDelta: CanvasPoint,
  target: StockViewElement | CloudViewElement | undefined,
): FlowViewElement {
  return growEndpointDrag(flow, false, moveDelta, target);
}
