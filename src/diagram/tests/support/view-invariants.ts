// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * The model/view invariants M1-M3 of
 * docs/design-plans/2026-09-10-diagram-editing-core.md, plus two stock-list
 * checks the plan's defense in depth relies on: no duplicate inflow/outflow
 * entry (the engine integrates a duplicate twice with no error) and static
 * agreement between a stock's lists and the flows attached to it in the view
 * (what a generated scene, and a well-formed saved model, must hold).
 *
 * Arms are enumerated in VIEW_ARMS so tests derive their rows from it. Idents
 * are compared after canonicalization because the engine matches names
 * canonically while stock lists carry display spellings.
 */

import { canonicalize } from '@simlin/core/canonicalize';
import {
  isNamedViewElement,
  type FlowViewElement,
  type Stock,
  type StockFlowView,
  type UID,
  type Variable,
  type ViewElement,
} from '@simlin/core/datamodel';

export const VIEW_ARMS = {
  M1: ['missingVariable', 'kindMismatch', 'createdVariableMissing'],
  M2: ['notRemoved', 'newListCount', 'otherEntryChanged'],
  M3: [
    'linkFromMissing',
    'linkToMissing',
    'linkFromKind',
    'linkToKind',
    'linkSelf',
    'aliasOfMissing',
    'aliasOfKind',
    'cloudFlowMissing',
    'cloudFlowNotFlow',
    'cloudEndpointCount',
    'duplicateUid',
  ],
  stockLists: ['duplicateEntry', 'listedNotAttached', 'attachedNotListed'],
} as const;

export type ViewInvariant = keyof typeof VIEW_ARMS;
export type ViewArm = { [K in ViewInvariant]: `${K}.${(typeof VIEW_ARMS)[K][number]}` }[ViewInvariant];

export const ALL_VIEW_ARMS: readonly ViewArm[] = (Object.keys(VIEW_ARMS) as ViewInvariant[]).flatMap((invariant) =>
  VIEW_ARMS[invariant].map((arm) => `${invariant}.${arm}` as ViewArm),
);

export interface ViewViolation {
  readonly arm: ViewArm;
  /** The element the violation is about, when it is about one. */
  readonly uid: UID | undefined;
  /** The variable the violation is about, when it is about one. */
  readonly ident: string | undefined;
  readonly numbers: Readonly<Record<string, number>>;
  readonly message: string;
}

export interface ViewAndVariables {
  readonly view: StockFlowView;
  readonly variables: ReadonlyMap<string, Variable>;
}

export function formatViewViolations(violations: readonly ViewViolation[]): string {
  return violations
    .map((v) => `${v.arm} uid=${v.uid} ident=${v.ident} ${JSON.stringify(v.numbers)} ${v.message}`)
    .join('\n');
}

type End = 'source' | 'sink';
const ENDS: readonly End[] = ['source', 'sink'];

function violation(
  arm: ViewArm,
  fields: { uid?: UID; ident?: string; numbers?: Record<string, number> },
  message: string,
): ViewViolation {
  return { arm, uid: fields.uid, ident: fields.ident, numbers: fields.numbers ?? {}, message };
}

// ---------------------------------------------------------------------------
// M1 kind agreement

/**
 * Every stock/flow/aux/module element names an existing variable of its own
 * kind. The lookup key is the element's `ident`, the same key the Canvas and
 * Editor use against `Model.variables`.
 */
export function checkKindAgreement(view: StockFlowView, variables: ReadonlyMap<string, Variable>): ViewViolation[] {
  const out: ViewViolation[] = [];
  for (const el of view.elements) {
    if (el.type !== 'stock' && el.type !== 'flow' && el.type !== 'aux' && el.type !== 'module') {
      continue;
    }
    const variable = variables.get(el.ident);
    if (variable === undefined) {
      out.push(
        violation('M1.missingVariable', { uid: el.uid, ident: el.ident }, `${el.type} element names no variable`),
      );
    } else if (variable.type !== el.type) {
      out.push(
        violation('M1.kindMismatch', { uid: el.uid, ident: el.ident }, `${el.type} element names a ${variable.type}`),
      );
    }
  }
  return out;
}

/**
 * "A created element's variable exists after the commit": named elements in
 * `next` whose uid is absent from `base` must have a variable in the model the
 * commit produced. Scoped to created elements so a base view carrying imported
 * elements without variables does not mask or pollute the edit's own result.
 */
export function checkCreatedVariables(base: StockFlowView, next: ViewAndVariables): ViewViolation[] {
  const baseUids = new Set(base.elements.map((el) => el.uid));
  const out: ViewViolation[] = [];
  for (const el of next.view.elements) {
    if (el.type !== 'stock' && el.type !== 'flow' && el.type !== 'aux' && el.type !== 'module') {
      continue;
    }
    if (!baseUids.has(el.uid) && !next.variables.has(el.ident)) {
      out.push(
        violation('M1.createdVariableMissing', { uid: el.uid, ident: el.ident }, `created ${el.type} has no variable`),
      );
    }
  }
  return out;
}

// ---------------------------------------------------------------------------
// M2 stock/flow agreement (delta)

interface AttachmentChange {
  readonly flowUid: UID;
  readonly flowIdent: string;
  readonly end: End;
  readonly from: string | undefined;
  readonly to: string | undefined;
}

/**
 * After an edit changes a flow end's attachment, the flow is removed from the
 * old stock's list and present exactly once in the new stock's, and no other
 * list entry changes.
 *
 * A flow present only in `next` (created) or only in `base` (deleted) counts as
 * unattached on the side it is absent from. A stock present only in `next` had
 * empty lists before. Renames are excluded: a named element whose uid survives
 * with a new ident is the same variable, and the engine's RenameVariable
 * rewrites every list entry naming a renamed flow (`update_stock_flow_references`
 * in src/simlin-engine/src/patch.rs), so base idents are carried through the
 * rename before anything is compared. The new-list requirement applies only when
 * both the stock and the flow variable exist in the next model, matching the
 * plan's "only existing flow variables and existing stock variables are touched".
 * Entry order is not compared: reordering changes no entry, and the same engine
 * rename sorts both lists.
 */
export function checkStockFlowDelta(base: ViewAndVariables, next: ViewAndVariables): ViewViolation[] {
  const baseById = new Map(base.view.elements.map((el) => [el.uid, el]));
  const nextById = new Map(next.view.elements.map((el) => [el.uid, el]));
  const renamed = renamedIdents(baseById, nextById);
  const toNext = (ident: string): string => renamed.get(ident) ?? ident;
  const flowUids = new Set<UID>();
  for (const el of [...base.view.elements, ...next.view.elements]) {
    if (el.type === 'flow') {
      flowUids.add(el.uid);
    }
  }

  const changes: AttachmentChange[] = [];
  for (const uid of flowUids) {
    const b = asFlow(baseById.get(uid));
    const n = asFlow(nextById.get(uid));
    const flowIdent = n?.ident ?? b!.ident;
    for (const end of ENDS) {
      const baseStock = attachedStockIdent(baseById, b, end);
      const from = baseStock === undefined ? undefined : toNext(baseStock);
      const to = attachedStockIdent(nextById, n, end);
      if (from !== to) {
        changes.push({ flowUid: uid, flowIdent, end, from, to });
      }
    }
  }

  const out: ViewViolation[] = [];
  const touched = new Map<string, Map<End, Set<string>>>();
  const touch = (stockIdent: string, end: End, flowIdent: string): void => {
    const byEnd = touched.get(stockIdent) ?? new Map<End, Set<string>>();
    const set = byEnd.get(end) ?? new Set<string>();
    set.add(flowIdent);
    byEnd.set(end, set);
    touched.set(stockIdent, byEnd);
  };

  for (const c of changes) {
    const listName = c.end === 'source' ? 'outflows' : 'inflows';
    if (c.from !== undefined) {
      touch(c.from, c.end, c.flowIdent);
      const stock = stockVariable(next.variables, c.from);
      if (stock !== undefined) {
        const count = countEntries(listOf(stock, c.end), c.flowIdent);
        if (count > 0) {
          out.push(
            violation(
              'M2.notRemoved',
              { uid: c.flowUid, ident: c.flowIdent, numbers: { count } },
              `${c.flowIdent} still listed in ${c.from}.${listName} after its ${c.end} left that stock`,
            ),
          );
        }
      }
    }
    if (c.to !== undefined) {
      touch(c.to, c.end, c.flowIdent);
      const stock = stockVariable(next.variables, c.to);
      if (stock !== undefined && next.variables.get(c.flowIdent)?.type === 'flow') {
        const count = countEntries(listOf(stock, c.end), c.flowIdent);
        if (count !== 1) {
          out.push(
            violation(
              'M2.newListCount',
              { uid: c.flowUid, ident: c.flowIdent, numbers: { count } },
              `${c.flowIdent} appears ${count} time(s) in ${c.to}.${listName} after its ${c.end} attached there`,
            ),
          );
        }
      }
    }
  }

  const baseStocks = new Map<string, Stock>();
  for (const variable of base.variables.values()) {
    if (variable.type === 'stock') {
      baseStocks.set(toNext(variable.ident), variable);
    }
  }
  // A stock the edit deleted takes its lists with it; that changes no entry of
  // a list that still exists.
  for (const [ident, nextVariable] of next.variables) {
    if (nextVariable.type !== 'stock') {
      continue;
    }
    const variable = baseStocks.get(ident);
    for (const end of ENDS) {
      const exempt = touched.get(ident)?.get(end) ?? new Set<string>();
      const before = variable === undefined ? [] : otherEntries(listOf(variable, end), exempt, toNext);
      const after = otherEntries(listOf(nextVariable, end), exempt, (entry) => entry);
      if (before.length !== after.length || before.some((entry, i) => entry !== after[i])) {
        out.push(
          violation(
            'M2.otherEntryChanged',
            { ident, numbers: { before: before.length, after: after.length } },
            `${ident}.${end === 'source' ? 'outflows' : 'inflows'} changed other entries: ${JSON.stringify(before)} -> ${JSON.stringify(after)}`,
          ),
        );
      }
    }
  }
  return out;
}

function asFlow(el: ViewElement | undefined): FlowViewElement | undefined {
  return el?.type === 'flow' ? el : undefined;
}

/** Base ident -> next ident for every named element whose uid survives the edit under a new ident. */
function renamedIdents(
  baseById: ReadonlyMap<UID, ViewElement>,
  nextById: ReadonlyMap<UID, ViewElement>,
): Map<string, string> {
  const renamed = new Map<string, string>();
  for (const [uid, b] of baseById) {
    const n = nextById.get(uid);
    if (isNamedViewElement(b) && n !== undefined && isNamedViewElement(n) && b.ident !== n.ident) {
      renamed.set(b.ident, n.ident);
    }
  }
  return renamed;
}

function attachedStockIdent(
  byUid: ReadonlyMap<UID, ViewElement>,
  flow: FlowViewElement | undefined,
  end: End,
): string | undefined {
  if (flow === undefined || flow.points.length === 0) {
    return undefined;
  }
  const p = end === 'source' ? flow.points[0] : flow.points[flow.points.length - 1];
  const el = p.attachedToUid !== undefined ? byUid.get(p.attachedToUid) : undefined;
  return el?.type === 'stock' ? el.ident : undefined;
}

function stockVariable(variables: ReadonlyMap<string, Variable>, ident: string): Stock | undefined {
  const v = variables.get(ident);
  return v?.type === 'stock' ? v : undefined;
}

function listOf(stock: Stock, end: End): readonly string[] {
  return end === 'source' ? stock.outflows : stock.inflows;
}

function countEntries(list: readonly string[], flowIdent: string): number {
  return list.filter((entry) => canonicalize(entry) === flowIdent).length;
}

function otherEntries(
  list: readonly string[],
  exempt: ReadonlySet<string>,
  rename: (ident: string) => string,
): string[] {
  return list
    .map((entry) => rename(canonicalize(entry)))
    .filter((entry) => !exempt.has(entry))
    .sort();
}

// ---------------------------------------------------------------------------
// M3 referential integrity

/**
 * Links, aliases and clouds reference existing elements; a cloud's reference is
 * to its owning flow, so a cloud pointing at a non-flow is broken too. A
 * duplicate uid is reported under M3 because a reference to it does not name
 * one existing element.
 *
 * Beyond existence: a link joins two distinct elements, each a named element
 * (stock, flow, aux, module) or an alias of one, never a cloud, link or group;
 * an alias stands for a named element; and a cloud is exactly one endpoint of
 * its owning flow (a cloud no endpoint reaches renders detached, one at both
 * ends makes a flow its own source and sink).
 */
export function checkReferentialIntegrity(view: StockFlowView): ViewViolation[] {
  const out: ViewViolation[] = [];
  const byUid = new Map<UID, ViewElement>();
  const counts = new Map<UID, number>();
  for (const el of view.elements) {
    byUid.set(el.uid, el);
    counts.set(el.uid, (counts.get(el.uid) ?? 0) + 1);
  }
  for (const [uid, count] of counts) {
    if (count > 1) {
      out.push(violation('M3.duplicateUid', { uid, numbers: { count } }, `uid ${uid} is used by ${count} elements`));
    }
  }
  for (const el of view.elements) {
    if (el.type === 'link') {
      const from = byUid.get(el.fromUid);
      if (from === undefined) {
        out.push(
          violation(
            'M3.linkFromMissing',
            { uid: el.uid, numbers: { fromUid: el.fromUid } },
            `link from missing uid ${el.fromUid}`,
          ),
        );
      } else if (!isNamedViewElement(from) && from.type !== 'alias') {
        out.push(
          violation(
            'M3.linkFromKind',
            { uid: el.uid, numbers: { fromUid: el.fromUid } },
            `link from a ${from.type} (uid ${el.fromUid})`,
          ),
        );
      }
      const to = byUid.get(el.toUid);
      if (to === undefined) {
        out.push(
          violation(
            'M3.linkToMissing',
            { uid: el.uid, numbers: { toUid: el.toUid } },
            `link to missing uid ${el.toUid}`,
          ),
        );
      } else if (!isNamedViewElement(to) && to.type !== 'alias') {
        out.push(
          violation(
            'M3.linkToKind',
            { uid: el.uid, numbers: { toUid: el.toUid } },
            `link to a ${to.type} (uid ${el.toUid})`,
          ),
        );
      }
      if (el.fromUid === el.toUid) {
        out.push(
          violation(
            'M3.linkSelf',
            { uid: el.uid, numbers: { fromUid: el.fromUid } },
            `link from uid ${el.fromUid} to itself`,
          ),
        );
      }
    } else if (el.type === 'alias') {
      const target = byUid.get(el.aliasOfUid);
      if (target === undefined) {
        out.push(
          violation(
            'M3.aliasOfMissing',
            { uid: el.uid, numbers: { aliasOfUid: el.aliasOfUid } },
            `alias of missing uid ${el.aliasOfUid}`,
          ),
        );
      } else if (!isNamedViewElement(target)) {
        out.push(
          violation(
            'M3.aliasOfKind',
            { uid: el.uid, numbers: { aliasOfUid: el.aliasOfUid } },
            `alias of a ${target.type} (uid ${el.aliasOfUid})`,
          ),
        );
      }
    } else if (el.type === 'cloud') {
      const owner = byUid.get(el.flowUid);
      if (owner === undefined) {
        out.push(
          violation(
            'M3.cloudFlowMissing',
            { uid: el.uid, numbers: { flowUid: el.flowUid } },
            `cloud of missing uid ${el.flowUid}`,
          ),
        );
      } else if (owner.type !== 'flow') {
        out.push(
          violation(
            'M3.cloudFlowNotFlow',
            { uid: el.uid, numbers: { flowUid: el.flowUid } },
            `cloud owner ${el.flowUid} is a ${owner.type}`,
          ),
        );
      } else {
        const endIndexes = owner.points.length === 0 ? [] : [...new Set([0, owner.points.length - 1])];
        const count = endIndexes.filter((i) => owner.points[i].attachedToUid === el.uid).length;
        if (count !== 1) {
          out.push(
            violation(
              'M3.cloudEndpointCount',
              { uid: el.uid, numbers: { flowUid: el.flowUid, count } },
              `cloud is ${count} endpoint(s) of its flow ${el.flowUid}`,
            ),
          );
        }
      }
    }
  }
  return out;
}

// ---------------------------------------------------------------------------
// Stock lists

export function checkStockListDuplicates(variables: ReadonlyMap<string, Variable>): ViewViolation[] {
  const out: ViewViolation[] = [];
  for (const variable of variables.values()) {
    if (variable.type !== 'stock') {
      continue;
    }
    for (const end of ENDS) {
      const counts = new Map<string, number>();
      for (const entry of listOf(variable, end)) {
        const id = canonicalize(entry);
        counts.set(id, (counts.get(id) ?? 0) + 1);
      }
      for (const [flowIdent, count] of counts) {
        if (count > 1) {
          out.push(
            violation(
              'stockLists.duplicateEntry',
              { ident: variable.ident, numbers: { count } },
              `${variable.ident}.${end === 'source' ? 'outflows' : 'inflows'} lists ${flowIdent} ${count} times`,
            ),
          );
        }
      }
    }
  }
  return out;
}

/**
 * A stock's outflows (inflows) name exactly the flows whose source (sink) is
 * attached to that stock's element. Entries naming flows with no element on
 * this view are ignored: another view, or no view, may carry them.
 */
export function checkStockFlowAgreement(
  view: StockFlowView,
  variables: ReadonlyMap<string, Variable>,
): ViewViolation[] {
  const out: ViewViolation[] = [];
  const flows = view.elements.filter((el): el is FlowViewElement => el.type === 'flow');
  const onView = new Set(flows.map((f) => f.ident));
  for (const el of view.elements) {
    if (el.type !== 'stock') {
      continue;
    }
    const stock = stockVariable(variables, el.ident);
    if (stock === undefined) {
      continue;
    }
    for (const end of ENDS) {
      const attached = new Set(
        flows
          .filter((f) => {
            const p = end === 'source' ? f.points[0] : f.points[f.points.length - 1];
            return p?.attachedToUid === el.uid;
          })
          .map((f) => f.ident),
      );
      const listed = new Set(
        listOf(stock, end)
          .map((entry) => canonicalize(entry))
          .filter((id) => onView.has(id)),
      );
      const listName = end === 'source' ? 'outflows' : 'inflows';
      for (const id of listed) {
        if (!attached.has(id)) {
          out.push(
            violation(
              'stockLists.listedNotAttached',
              { uid: el.uid, ident: id },
              `${el.ident}.${listName} lists unattached ${id}`,
            ),
          );
        }
      }
      for (const id of attached) {
        if (!listed.has(id)) {
          out.push(
            violation(
              'stockLists.attachedNotListed',
              { uid: el.uid, ident: id },
              `${id} attaches to ${el.ident} but is not in ${listName}`,
            ),
          );
        }
      }
    }
  }
  return out;
}
