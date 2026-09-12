// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Functional Core
//
// The model operations a view edit implies. A view edit is described only by
// the view it was planned on (`baseView`) and the view it produces
// (`nextView`); this module derives every model op from the difference,
// evaluated against the COMMITTED model at dequeue, so a payload is never built
// from state read before an earlier edit landed. See "view-model-sync.ts" in
// docs/design-plans/2026-09-10-diagram-editing-core.md.

import { canonicalize } from '@simlin/core/canonicalize';
import {
  auxFromJson,
  flowFromJson,
  isNamedViewElement,
  moduleFromJson,
  stockFromJson,
  stockFlowViewToJson,
  type FlowViewElement,
  type Model,
  type NamedViewElement,
  type StockFlowView,
  type UID,
  type Variable,
  type ViewElement,
} from '@simlin/core/datamodel';
import type { JsonModelOperation } from '@simlin/engine';

/**
 * Thrown when an edit cannot be expressed against the committed model: a
 * created element names a variable that already exists, or a rename targets
 * one. Only reachable when name allocation is wrong or the committed model
 * moved underneath the edit; the controller fails the item like any engine
 * error.
 */
export class EditConflictError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'EditConflictError';
  }
}

type End = 'source' | 'sink';
type VariableKind = Variable['type'];

// Idents are derived from element NAMES, never from `ViewElement.ident`: the
// name is what the engine stores and matches canonically, while `ident` is a
// derived field a caller-built element (a staged create, a hand-built view) need
// not keep in step with it.
function identOf(element: NamedViewElement): string {
  return canonicalize(element.name);
}

function namedByUid(view: StockFlowView): Map<UID, NamedViewElement> {
  const out = new Map<UID, NamedViewElement>();
  for (const el of view.elements) {
    if (isNamedViewElement(el)) {
      out.set(el.uid, el);
    }
  }
  return out;
}

function endPoint(flow: FlowViewElement, end: End) {
  return end === 'source' ? flow.points[0] : flow.points[flow.points.length - 1];
}

/** The ident of the stock element `flow`'s `end` is attached to, if any. */
function attachedStockIdent(byUid: ReadonlyMap<UID, ViewElement>, flow: FlowViewElement | undefined, end: End) {
  if (flow === undefined || flow.points.length === 0) {
    return undefined;
  }
  const uid = endPoint(flow, end).attachedToUid;
  const el = uid === undefined ? undefined : byUid.get(uid);
  return el?.type === 'stock' ? identOf(el) : undefined;
}

function createOp(element: NamedViewElement): JsonModelOperation {
  switch (element.type) {
    case 'stock':
      return {
        type: 'upsertStock',
        payload: { stock: { name: element.name, inflows: [], outflows: [], initialEquation: '' } },
      };
    case 'flow':
      return { type: 'upsertFlow', payload: { flow: { name: element.name, equation: '' } } };
    case 'module':
      return { type: 'upsertModule', payload: { module: { name: element.name, modelName: '', references: [] } } };
    case 'aux':
      return { type: 'upsertAux', payload: { aux: { name: element.name, equation: '' } } };
  }
}

/**
 * The variable the create op for `element` makes, read as the editor's
 * datamodel reads the engine's: what the rendered model holds for an element
 * whose create is still queued (see ProjectController's render).
 */
export function createdVariable(element: NamedViewElement): Variable {
  const op = createOp(element);
  switch (op.type) {
    case 'upsertStock':
      return stockFromJson(op.payload.stock);
    case 'upsertFlow':
      return flowFromJson(op.payload.flow);
    case 'upsertModule':
      return moduleFromJson(op.payload.module);
    case 'upsertAux':
      return auxFromJson(op.payload.aux);
    default:
      throw new Error('createOp returned an operation that creates no variable');
  }
}

function sameList(a: readonly string[], b: readonly string[]): boolean {
  return a.length === b.length && a.every((entry, i) => entry === b[i]);
}

/**
 * The model operations implied by replacing `baseView` with `nextView`,
 * followed by the `upsertView` of `nextView`, in the order one patch applies
 * them:
 *
 * 1. `renameVariable` for every named element whose uid survives with a
 *    different name (`from` is the committed ident, `to` the new name as typed,
 *    which the engine stores verbatim).
 * 2. `deleteVariable` for every named element removed from the view whose
 *    variable exists in the committed model and names no remaining element;
 *    then an upsert for every named element created by the edit (new uid). A
 *    created element naming an existing variable throws EditConflictError.
 * 3. One `updateStockFlows` per stock whose lists the edit changes: per flow
 *    end whose attached stock differs between base and next, the flow leaves
 *    the old stock's list and joins the new stock's (deduped). Only flows and
 *    stocks that exist after the edit (committed, not deleted, or created by
 *    it) are touched; each op carries both full lists from the committed model
 *    with renames applied, deleted variables omitted, and the deltas applied.
 * 4. `upsertView` of `nextView` (index 0), exactly as given: the caller decides
 *    which viewport it carries.
 */
export function buildEditOps(
  committedModel: Model,
  baseView: StockFlowView,
  nextView: StockFlowView,
): JsonModelOperation[] {
  const variables = committedModel.variables;
  const baseNamed = namedByUid(baseView);
  const nextNamed = namedByUid(nextView);

  // --- renames
  const renameOps: JsonModelOperation[] = [];
  // committed ident -> ident after the edit
  const renamed = new Map<string, string>();
  for (const [uid, b] of baseNamed) {
    const n = nextNamed.get(uid);
    if (n === undefined || n.name === b.name) {
      continue;
    }
    const from = identOf(b);
    const to = identOf(n);
    if (!variables.has(from) || renamed.has(from)) {
      continue;
    }
    if (to !== from && variables.has(to)) {
      throw new EditConflictError(`cannot rename '${b.name}' to '${n.name}': a variable with that name exists`);
    }
    renamed.set(from, to);
    renameOps.push({ type: 'renameVariable', payload: { from, to: n.name } });
  }
  const afterRename = (ident: string): string => renamed.get(ident) ?? ident;
  const beforeRename = new Map<string, string>();
  for (const [from, to] of renamed) {
    beforeRename.set(to, from);
  }

  // --- deletes
  const remainingIdents = new Set<string>();
  for (const n of nextNamed.values()) {
    remainingIdents.add(identOf(n));
  }
  const deleteOps: JsonModelOperation[] = [];
  const deleted = new Set<string>();
  for (const [uid, b] of baseNamed) {
    if (nextNamed.has(uid)) {
      continue;
    }
    const ident = identOf(b);
    const variable = variables.get(ident);
    if (variable === undefined || remainingIdents.has(afterRename(ident)) || deleted.has(ident)) {
      continue;
    }
    deleted.add(ident);
    deleteOps.push({ type: 'deleteVariable', payload: { ident: variable.ident } });
  }

  // --- creates
  const createOps: JsonModelOperation[] = [];
  const created = new Map<string, VariableKind>();
  for (const [uid, n] of nextNamed) {
    if (baseNamed.has(uid)) {
      continue;
    }
    const ident = identOf(n);
    if (created.has(ident)) {
      continue;
    }
    const existing = afterRenameVariable(variables, beforeRename, ident);
    if (existing !== undefined && !deleted.has(existing.ident)) {
      throw new EditConflictError(`cannot create '${n.name}': a variable with that name exists`);
    }
    created.set(ident, n.type);
    createOps.push(createOp(n));
  }

  // The kind of `ident` (an after-edit ident) once the patch's renames,
  // deletes and creates have applied, or undefined when no variable exists.
  const kindAfterEdit = (ident: string): VariableKind | undefined => {
    const createdKind = created.get(ident);
    if (createdKind !== undefined) {
      return createdKind;
    }
    const committedIdent = beforeRename.get(ident) ?? ident;
    if (deleted.has(committedIdent)) {
      return undefined;
    }
    if (beforeRename.get(ident) === undefined && renamed.has(ident)) {
      // renamed away: the old ident no longer names a variable
      return undefined;
    }
    return variables.get(committedIdent)?.type;
  };

  // --- stock/flow deltas
  const baseByUid = new Map<UID, ViewElement>(baseView.elements.map((el) => [el.uid, el]));
  const nextByUid = new Map<UID, ViewElement>(nextView.elements.map((el) => [el.uid, el]));
  const flowUids = new Set<UID>();
  for (const el of [...baseView.elements, ...nextView.elements]) {
    if (el.type === 'flow') {
      flowUids.add(el.uid);
    }
  }

  interface ListDelta {
    readonly add: Set<string>;
    readonly remove: Set<string>;
  }
  const deltas = new Map<string, { inflows: ListDelta; outflows: ListDelta }>();
  const deltaFor = (stockIdent: string, end: End): ListDelta => {
    let entry = deltas.get(stockIdent);
    if (entry === undefined) {
      entry = {
        inflows: { add: new Set(), remove: new Set() },
        outflows: { add: new Set(), remove: new Set() },
      };
      deltas.set(stockIdent, entry);
    }
    return end === 'source' ? entry.outflows : entry.inflows;
  };

  for (const uid of flowUids) {
    const b = baseByUid.get(uid);
    const n = nextByUid.get(uid);
    const baseFlow = b?.type === 'flow' ? b : undefined;
    const nextFlow = n?.type === 'flow' ? n : undefined;
    // A next element's name is already the after-edit spelling; only a flow the
    // edit removed is named through the base view and carried through renames.
    const flowIdent = nextFlow !== undefined ? identOf(nextFlow) : afterRename(identOf(baseFlow!));
    if (kindAfterEdit(flowIdent) !== 'flow') {
      continue;
    }
    for (const end of ['source', 'sink'] as const) {
      const fromBase = attachedStockIdent(baseByUid, baseFlow, end);
      const from = fromBase === undefined ? undefined : afterRename(fromBase);
      const to = attachedStockIdent(nextByUid, nextFlow, end);
      if (from === to) {
        continue;
      }
      if (from !== undefined && kindAfterEdit(from) === 'stock') {
        deltaFor(from, end).remove.add(flowIdent);
      }
      if (to !== undefined && kindAfterEdit(to) === 'stock') {
        deltaFor(to, end).add.add(flowIdent);
      }
    }
  }

  const stockOps: JsonModelOperation[] = [];
  for (const stockIdent of [...deltas.keys()].sort()) {
    const delta = deltas.get(stockIdent)!;
    const committedIdent = beforeRename.get(stockIdent) ?? stockIdent;
    const committedStock = created.has(stockIdent) ? undefined : variables.get(committedIdent);
    const committedLists =
      committedStock?.type === 'stock'
        ? { inflows: committedStock.inflows, outflows: committedStock.outflows }
        : { inflows: [], outflows: [] };
    const echo = (list: readonly string[]): string[] =>
      list
        .map((entry) => {
          const ident = canonicalize(entry);
          return renamed.has(ident) ? renamed.get(ident)! : entry;
        })
        .filter((entry) => !deleted.has(canonicalize(entry)));
    const apply = (list: readonly string[], listDelta: ListDelta): string[] => {
      const kept = list.filter((entry) => !listDelta.remove.has(canonicalize(entry)));
      const present = new Set(kept.map((entry) => canonicalize(entry)));
      for (const flowIdent of listDelta.add) {
        if (!present.has(flowIdent)) {
          kept.push(flowIdent);
          present.add(flowIdent);
        }
      }
      return kept;
    };
    const inflowsBefore = echo(committedLists.inflows);
    const outflowsBefore = echo(committedLists.outflows);
    const inflows = apply(inflowsBefore, delta.inflows);
    const outflows = apply(outflowsBefore, delta.outflows);
    if (sameList(inflows, inflowsBefore) && sameList(outflows, outflowsBefore)) {
      continue;
    }
    stockOps.push({ type: 'updateStockFlows', payload: { ident: stockIdent, inflows, outflows } });
  }

  return [
    ...renameOps,
    ...deleteOps,
    ...createOps,
    ...stockOps,
    { type: 'upsertView', payload: { index: 0, view: stockFlowViewToJson(nextView) } },
  ];
}

// The committed variable an after-edit ident names, looking through renames:
// a created element whose name is a rename TARGET collides with the renamed
// variable, and one whose name is a rename SOURCE does not (that name is free
// once the rename applies).
function afterRenameVariable(
  variables: ReadonlyMap<string, Variable>,
  beforeRename: ReadonlyMap<string, string>,
  ident: string,
): Variable | undefined {
  const renamedFrom = beforeRename.get(ident);
  if (renamedFrom !== undefined) {
    return variables.get(renamedFrom);
  }
  for (const from of beforeRename.values()) {
    if (from === ident) {
      return undefined;
    }
  }
  return variables.get(ident);
}
