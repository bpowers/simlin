// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Tests of the committed model/view invariant checkers
// (tests/support/view-invariants.ts). Rows are derived from VIEW_ARMS; the
// first test fails when an arm has no row, and arms with a per-kind or per-end
// branch get one row per kind or end. Every fixture is an engine-shaped JSON
// model deserialized with the production `modelFromJson`, so element idents and
// the stock lists' display spellings are what the editor holds. Renames are the
// exception that needs the engine itself: the list rewrite a rename produces is
// taken from the real RenameVariable patch path, not written by hand.

import { describe, it, expect, beforeAll } from '@rstest/core';

import type { JsonModel, JsonProject, JsonStock, JsonViewElement } from '@simlin/engine';
import { modelFromJson, projectFromJson, type Model, type UID } from '@simlin/core/datamodel';

import { buildVariableRenameOps } from '../rename-ops';
import { describeWithEngine, editorModel, loadEngine, mainModel, type EngineModule } from './support/engine';
import {
  ALL_VIEW_ARMS,
  checkCreatedVariables,
  checkKindAgreement,
  checkReferentialIntegrity,
  checkStockFlowAgreement,
  checkStockFlowDelta,
  checkStockListDuplicates,
  formatViewViolations,
  type ViewAndVariables,
  type ViewArm,
  type ViewViolation,
} from './support/view-invariants';

// Stocks A(1) -> B(2) through Flow F (4); Flow H (5) out of A's bottom into a
// cloud; Inflow G (7) from a cloud into A; Stock C (3) unattached; an aux, a
// module, an alias of A, two links, and a group.
function baseJson(): JsonModel {
  return {
    name: 'main',
    stocks: [
      { name: 'Stock A', initialEquation: '1', inflows: ['Inflow G'], outflows: ['Flow F', 'Flow H'] },
      { name: 'Stock B', initialEquation: '1', inflows: ['Flow F'], outflows: [] },
      { name: 'Stock C', initialEquation: '1', inflows: [], outflows: [] },
    ],
    flows: [
      { name: 'Flow F', equation: '1' },
      { name: 'Flow H', equation: '1' },
      { name: 'Inflow G', equation: '1' },
    ],
    auxiliaries: [{ name: 'Aux X', equation: '1' }],
    modules: [{ name: 'Module M', modelName: 'sub' }],
    views: [
      {
        elements: [
          { type: 'stock', uid: 1, name: 'Stock A', x: 100, y: 100 },
          { type: 'stock', uid: 2, name: 'Stock B', x: 300, y: 100 },
          { type: 'stock', uid: 3, name: 'Stock C', x: 300, y: 300 },
          {
            type: 'flow',
            uid: 4,
            name: 'Flow F',
            x: 200,
            y: 100,
            points: [
              { x: 122.5, y: 100, attachedToUid: 1 },
              { x: 277.5, y: 100, attachedToUid: 2 },
            ],
          },
          {
            type: 'flow',
            uid: 5,
            name: 'Flow H',
            x: 100,
            y: 180,
            points: [
              { x: 100, y: 117.5, attachedToUid: 1 },
              { x: 100, y: 250, attachedToUid: 6 },
            ],
          },
          { type: 'cloud', uid: 6, flowUid: 5, x: 100, y: 250 },
          {
            type: 'flow',
            uid: 7,
            name: 'Inflow G',
            x: 40,
            y: 100,
            points: [
              { x: 0, y: 100, attachedToUid: 8 },
              { x: 77.5, y: 100, attachedToUid: 1 },
            ],
          },
          { type: 'cloud', uid: 8, flowUid: 7, x: 0, y: 100 },
          { type: 'aux', uid: 9, name: 'Aux X', x: 200, y: 20 },
          { type: 'module', uid: 10, name: 'Module M', x: 400, y: 20 },
          { type: 'alias', uid: 11, aliasOfUid: 1, x: 100, y: 300 },
          { type: 'link', uid: 12, fromUid: 9, toUid: 4 },
          { type: 'link', uid: 13, fromUid: 11, toUid: 7 },
          { type: 'group', uid: 14, name: 'Group Q', x: 500, y: 300, width: 100, height: 80 },
        ],
      },
    ],
  } as JsonModel;
}

function load(json: JsonModel): ViewAndVariables {
  return fromModel(modelFromJson(json));
}

function fromModel(model: Model): ViewAndVariables {
  return { view: model.views[0], variables: model.variables };
}

function edit(fn: (json: JsonModel) => void): JsonModel {
  const json = baseJson();
  fn(json);
  return json;
}

function elements(json: JsonModel): JsonViewElement[] {
  return json.views![0].elements!;
}

function element(json: JsonModel, uid: UID): Record<string, unknown> {
  return elements(json).find((e) => e.uid === uid) as unknown as Record<string, unknown>;
}

function stockJson(json: JsonModel, name: string): JsonStock {
  return json.stocks!.find((s) => s.name === name)!;
}

function setPoints(
  json: JsonModel,
  uid: number,
  points: Array<{ x: number; y: number; attachedToUid?: number }>,
): void {
  element(json, uid).points = points;
}

function arms(violations: readonly ViewViolation[]): string[] {
  return violations.map((v) => v.arm).sort();
}

interface Row {
  readonly arm: ViewArm;
  readonly name: string;
  readonly check: () => ViewViolation[];
  readonly expected: readonly ViewArm[];
}

const KINDS = ['stock', 'flow', 'aux', 'module'] as const;
type Kind = (typeof KINDS)[number];
// The element of each kind a row removes or re-kinds, and a variable of a
// different kind to put under its name.
const KIND_TARGETS: Record<Kind, { name: string; uid: number; other: 'auxiliaries' | 'flows' }> = {
  stock: { name: 'Stock C', uid: 3, other: 'auxiliaries' },
  flow: { name: 'Flow H', uid: 5, other: 'auxiliaries' },
  aux: { name: 'Aux X', uid: 9, other: 'flows' },
  module: { name: 'Module M', uid: 10, other: 'auxiliaries' },
};

function removeVariable(json: JsonModel, name: string): void {
  json.stocks = json.stocks!.filter((v) => v.name !== name);
  json.flows = json.flows!.filter((v) => v.name !== name);
  json.auxiliaries = json.auxiliaries!.filter((v) => v.name !== name);
  json.modules = json.modules!.filter((v) => v.name !== name);
}

const M1_ROWS: Row[] = KINDS.flatMap((kind) => {
  const target = KIND_TARGETS[kind];
  return [
    {
      arm: 'M1.missingVariable' as const,
      name: `${kind} element with no variable`,
      check: () => {
        const { view, variables } = load(edit((json) => removeVariable(json, target.name)));
        return checkKindAgreement(view, variables);
      },
      expected: ['M1.missingVariable'] as const,
    },
    {
      arm: 'M1.kindMismatch' as const,
      name: `${kind} element naming a variable of another kind`,
      check: () => {
        const { view, variables } = load(
          edit((json) => {
            removeVariable(json, target.name);
            if (target.other === 'auxiliaries') {
              json.auxiliaries!.push({ name: target.name, equation: '1' });
            } else {
              json.flows!.push({ name: target.name, equation: '1' });
            }
          }),
        );
        return checkKindAgreement(view, variables);
      },
      expected: ['M1.kindMismatch'] as const,
    },
  ];
});

// A new element of each kind (uid 20), with or without its variable.
function withNewElement(json: JsonModel, kind: Kind, withVariable: boolean): void {
  const name = `New ${kind}`;
  const at = { x: 500, y: 500 };
  switch (kind) {
    case 'stock':
      elements(json).push({ type: 'stock', uid: 20, name, ...at });
      if (withVariable) json.stocks!.push({ name, initialEquation: '1', inflows: [], outflows: [] });
      break;
    case 'flow':
      elements(json).push({ type: 'flow', uid: 20, name, ...at, points: [] });
      if (withVariable) json.flows!.push({ name, equation: '1' });
      break;
    case 'aux':
      elements(json).push({ type: 'aux', uid: 20, name, ...at });
      if (withVariable) json.auxiliaries!.push({ name, equation: '1' });
      break;
    case 'module':
      elements(json).push({ type: 'module', uid: 20, name, ...at });
      if (withVariable) json.modules!.push({ name, modelName: 'sub' });
      break;
  }
}

const CREATED_ROWS: Row[] = KINDS.map((kind) => ({
  arm: 'M1.createdVariableMissing' as const,
  name: `created ${kind} whose variable is absent after the commit`,
  check: () => checkCreatedVariables(load(baseJson()).view, load(edit((json) => withNewElement(json, kind, false)))),
  expected: ['M1.createdVariableMissing'] as const,
}));

// Move Flow F's end (source or sink) from its stock to Stock C, and let the
// caller shape the next model's lists.
type End = 'source' | 'sink';
const ENDS: readonly End[] = ['source', 'sink'];
const LIST: Record<End, 'outflows' | 'inflows'> = { source: 'outflows', sink: 'inflows' };
const OTHER_LIST: Record<End, 'outflows' | 'inflows'> = { source: 'inflows', sink: 'outflows' };
const OLD_STOCK: Record<End, string> = { source: 'Stock A', sink: 'Stock B' };

function moveEnd(end: End, lists: (json: JsonModel) => void): Row['check'] {
  return () => {
    const next = edit((json) => {
      if (end === 'source') {
        setPoints(json, 4, [
          { x: 300, y: 282.5, attachedToUid: 3 },
          { x: 277.5, y: 100, attachedToUid: 2 },
        ]);
      } else {
        setPoints(json, 4, [
          { x: 122.5, y: 100, attachedToUid: 1 },
          { x: 300, y: 282.5, attachedToUid: 3 },
        ]);
      }
      lists(json);
    });
    return checkStockFlowDelta(load(baseJson()), load(next));
  };
}

const removeFromOld = (json: JsonModel, end: End): void => {
  const old = stockJson(json, OLD_STOCK[end]);
  old[LIST[end]] = old[LIST[end]]!.filter((f) => f !== 'Flow F');
};

// Both lists updated correctly for the move.
const moved = (json: JsonModel, end: End): void => {
  removeFromOld(json, end);
  stockJson(json, 'Stock C')[LIST[end]] = ['Flow F'];
};

const M2_ROWS: Row[] = ENDS.flatMap((end) => [
  {
    arm: 'M2.notRemoved' as const,
    name: `${end} moved; the old stock still lists the flow`,
    check: moveEnd(end, (json) => {
      stockJson(json, 'Stock C')[LIST[end]] = ['Flow F'];
    }),
    expected: ['M2.notRemoved'] as const,
  },
  {
    arm: 'M2.newListCount' as const,
    name: `${end} moved; the new stock does not list the flow`,
    check: moveEnd(end, (json) => removeFromOld(json, end)),
    expected: ['M2.newListCount'] as const,
  },
  {
    arm: 'M2.newListCount' as const,
    name: `${end} moved; the new stock lists the flow twice`,
    check: moveEnd(end, (json) => {
      removeFromOld(json, end);
      stockJson(json, 'Stock C')[LIST[end]] = ['Flow F', 'Flow F'];
    }),
    expected: ['M2.newListCount'] as const,
  },
  {
    arm: 'M2.otherEntryChanged' as const,
    name: `${end} moved; an unrelated entry of a stock list was removed`,
    check: moveEnd(end, (json) => {
      moved(json, end);
      // Inflow G is A's inflow and has nothing to do with this edit.
      stockJson(json, 'Stock A').inflows = [];
    }),
    expected: ['M2.otherEntryChanged'] as const,
  },
  {
    arm: 'M2.otherEntryChanged' as const,
    name: `${end} moved; an unrelated entry was replaced by another, keeping the count`,
    check: moveEnd(end, (json) => {
      moved(json, end);
      stockJson(json, 'Stock A').inflows = ['Flow H'];
    }),
    expected: ['M2.otherEntryChanged'] as const,
  },
  {
    arm: 'M2.otherEntryChanged' as const,
    name: `${end} moved; the flow also appeared in the old stock's ${OTHER_LIST[end]}`,
    // The move exempts the flow from its old stock's ${LIST[end]} only.
    check: moveEnd(end, (json) => {
      moved(json, end);
      const old = stockJson(json, OLD_STOCK[end]);
      old[OTHER_LIST[end]] = [...old[OTHER_LIST[end]]!, 'Flow F'];
    }),
    expected: ['M2.otherEntryChanged'] as const,
  },
  {
    arm: 'M2.otherEntryChanged' as const,
    name: `${end} moved; a stock the edit created lists a flow nothing attached to it`,
    check: moveEnd(end, (json) => {
      moved(json, end);
      elements(json).push({ type: 'stock', uid: 16, name: 'Stock D', x: 100, y: 400 });
      json.stocks!.push({ name: 'Stock D', initialEquation: '1', inflows: [], outflows: [], [LIST[end]]: ['Flow H'] });
    }),
    expected: ['M2.otherEntryChanged'] as const,
  },
]);

// Element kinds a link end may not be, and an element of each kind in the base
// fixture. An alias may not be an alias target either.
const NOT_LINK_ENDS: Record<'cloud' | 'link' | 'group', UID> = { cloud: 6, link: 13, group: 14 };
const NOT_ALIAS_TARGETS: Record<'cloud' | 'link' | 'group' | 'alias', UID> = { ...NOT_LINK_ENDS, alias: 11 };

const referential = (fn: (json: JsonModel) => void): ViewViolation[] => checkReferentialIntegrity(load(edit(fn)).view);

const M3_ROWS: Row[] = [
  {
    arm: 'M3.linkFromMissing',
    name: 'link from a missing uid',
    check: () => referential((json) => (element(json, 12).fromUid = 99)),
    expected: ['M3.linkFromMissing'],
  },
  {
    arm: 'M3.linkToMissing',
    name: 'link to a missing uid',
    check: () => referential((json) => (element(json, 12).toUid = 99)),
    expected: ['M3.linkToMissing'],
  },
  ...Object.entries(NOT_LINK_ENDS).flatMap(([kind, uid]) => [
    {
      arm: 'M3.linkFromKind' as const,
      name: `link from a ${kind}`,
      check: () => referential((json) => (element(json, 12).fromUid = uid)),
      expected: ['M3.linkFromKind'] as const,
    },
    {
      arm: 'M3.linkToKind' as const,
      name: `link to a ${kind}`,
      check: () => referential((json) => (element(json, 12).toUid = uid)),
      expected: ['M3.linkToKind'] as const,
    },
  ]),
  {
    arm: 'M3.linkSelf',
    name: 'link from an aux to itself',
    check: () => referential((json) => (element(json, 12).toUid = 9)),
    expected: ['M3.linkSelf'],
  },
  {
    arm: 'M3.aliasOfMissing',
    name: 'alias of a missing uid',
    check: () => referential((json) => (element(json, 11).aliasOfUid = 99)),
    expected: ['M3.aliasOfMissing'],
  },
  ...Object.entries(NOT_ALIAS_TARGETS).map(([kind, uid]) => ({
    arm: 'M3.aliasOfKind' as const,
    name: `alias of a ${kind}`,
    check: () => referential((json) => (element(json, 11).aliasOfUid = uid)),
    expected: ['M3.aliasOfKind'] as const,
  })),
  {
    arm: 'M3.cloudFlowMissing',
    name: 'cloud of a missing uid',
    check: () => referential((json) => (element(json, 6).flowUid = 99)),
    expected: ['M3.cloudFlowMissing'],
  },
  {
    arm: 'M3.cloudFlowNotFlow',
    name: 'cloud owned by an aux',
    check: () => referential((json) => (element(json, 6).flowUid = 9)),
    expected: ['M3.cloudFlowNotFlow'],
  },
  {
    arm: 'M3.cloudEndpointCount',
    name: 'cloud its flow no longer reaches',
    check: () =>
      referential((json) =>
        setPoints(json, 5, [
          { x: 100, y: 117.5, attachedToUid: 1 },
          { x: 100, y: 250 },
        ]),
      ),
    expected: ['M3.cloudEndpointCount'],
  },
  {
    arm: 'M3.cloudEndpointCount',
    name: 'cloud at both ends of its flow',
    check: () =>
      referential((json) =>
        setPoints(json, 5, [
          { x: 100, y: 250, attachedToUid: 6 },
          { x: 100, y: 250, attachedToUid: 6 },
        ]),
      ),
    expected: ['M3.cloudEndpointCount'],
  },
  {
    arm: 'M3.duplicateUid',
    name: 'two elements share a uid',
    check: () => referential((json) => elements(json).push({ type: 'aux', uid: 10, name: 'Aux X', x: 600, y: 20 })),
    expected: ['M3.duplicateUid'],
  },
];

const LIST_ROWS: Row[] = [
  ...ENDS.map((end) => ({
    arm: 'stockLists.duplicateEntry' as const,
    name: `duplicate ${LIST[end]} entry`,
    check: () => {
      const json = edit((j) => {
        const s = stockJson(j, OLD_STOCK[end]);
        s[LIST[end]] = [...s[LIST[end]]!, 'Flow F'];
      });
      return checkStockListDuplicates(load(json).variables);
    },
    expected: ['stockLists.duplicateEntry'] as const,
  })),
  {
    arm: 'stockLists.duplicateEntry',
    name: 'duplicate spelled differently (canonical match)',
    check: () => {
      const json = edit((j) => {
        stockJson(j, 'Stock B').inflows = ['Flow F', 'flow_f'];
      });
      return checkStockListDuplicates(load(json).variables);
    },
    expected: ['stockLists.duplicateEntry'],
  },
  {
    arm: 'stockLists.listedNotAttached',
    name: 'a stock lists a flow attached elsewhere (XMILE imports list one flow in two stocks)',
    check: () => {
      const { view, variables } = load(edit((j) => (stockJson(j, 'Stock C').outflows = ['Flow F'])));
      return checkStockFlowAgreement(view, variables);
    },
    expected: ['stockLists.listedNotAttached'],
  },
  {
    arm: 'stockLists.attachedNotListed',
    name: 'a flow attached to a stock that does not list it',
    check: () => {
      const { view, variables } = load(edit((j) => (stockJson(j, 'Stock A').outflows = ['Flow F'])));
      return checkStockFlowAgreement(view, variables);
    },
    expected: ['stockLists.attachedNotListed'],
  },
];

const ROWS: readonly Row[] = [...M1_ROWS, ...CREATED_ROWS, ...M2_ROWS, ...M3_ROWS, ...LIST_ROWS];

describe('view invariant arm table', () => {
  it('covers every enumerated arm', () => {
    expect([...new Set(ROWS.map((r) => r.arm))].sort()).toEqual([...ALL_VIEW_ARMS].sort());
  });

  it('the base fixture passes every check', () => {
    const base = load(baseJson());
    const all = [
      ...checkKindAgreement(base.view, base.variables),
      ...checkCreatedVariables(base.view, base),
      ...checkStockFlowDelta(base, base),
      ...checkReferentialIntegrity(base.view),
      ...checkStockListDuplicates(base.variables),
      ...checkStockFlowAgreement(base.view, base.variables),
    ];
    expect(formatViewViolations(all)).toBe('');
  });

  for (const row of ROWS) {
    it(`${row.arm}: ${row.name}`, () => {
      expect(arms(row.check())).toEqual([...row.expected].sort());
    });
  }
});

describe('M1 created-element scoping', () => {
  for (const kind of KINDS) {
    it(`a created ${kind} with its variable passes`, () => {
      expect(
        checkCreatedVariables(load(baseJson()).view, load(edit((json) => withNewElement(json, kind, true)))),
      ).toEqual([]);
    });
  }

  it('an imported element without a variable in the base view is not a created-element violation', () => {
    const ghost = (json: JsonModel): void => {
      elements(json).push({ type: 'aux', uid: 21, name: 'Ghost', x: 700, y: 700 });
    };
    const base = load(edit(ghost));
    const next = load(edit(ghost));
    expect(checkCreatedVariables(base.view, next)).toEqual([]);
    // The static arm still sees it, which is why created elements have their own arm.
    expect(arms(checkKindAgreement(next.view, next.variables))).toEqual(['M1.missingVariable']);
  });
});

describe('M2 delta semantics', () => {
  for (const end of ENDS) {
    it(`${end} moved with both lists updated passes`, () => {
      expect(moveEnd(end, (json) => moved(json, end))()).toEqual([]);
    });
  }

  it('a created stock-to-stock flow must be listed once at each end', () => {
    const base = edit((json) => {
      json.views![0].elements = elements(json).filter((e) => e.uid !== 4);
      stockJson(json, 'Stock A').outflows = ['Flow H'];
      stockJson(json, 'Stock B').inflows = [];
    });
    const unlisted = edit((json) => {
      stockJson(json, 'Stock A').outflows = ['Flow H'];
      stockJson(json, 'Stock B').inflows = [];
    });
    expect(checkStockFlowDelta(load(base), load(baseJson()))).toEqual([]);
    expect(arms(checkStockFlowDelta(load(base), load(unlisted)))).toEqual(['M2.newListCount', 'M2.newListCount']);
  });

  it('a deleted flow element must be removed from both lists', () => {
    const deleted = (json: JsonModel): void => {
      json.views![0].elements = elements(json).filter((e) => e.uid !== 4);
    };
    const cleaned = edit((json) => {
      deleted(json);
      stockJson(json, 'Stock A').outflows = ['Flow H'];
      stockJson(json, 'Stock B').inflows = [];
    });
    expect(checkStockFlowDelta(load(baseJson()), load(cleaned))).toEqual([]);
    expect(arms(checkStockFlowDelta(load(baseJson()), load(edit(deleted))))).toEqual([
      'M2.notRemoved',
      'M2.notRemoved',
    ]);
  });

  it('a flow moved onto a stock the edit created passes when that stock lists it', () => {
    const next = edit((json) => {
      elements(json).push({ type: 'stock', uid: 16, name: 'Stock D', x: 100, y: 400 });
      json.stocks!.push({ name: 'Stock D', initialEquation: '1', inflows: [], outflows: ['Flow F'] });
      setPoints(json, 4, [
        { x: 100, y: 382.5, attachedToUid: 16 },
        { x: 277.5, y: 100, attachedToUid: 2 },
      ]);
      stockJson(json, 'Stock A').outflows = ['Flow H'];
    });
    expect(checkStockFlowDelta(load(baseJson()), load(next))).toEqual([]);
  });

  it('a stale imported entry on an uninvolved stock must be left alone', () => {
    // Imported shape: Flow F is also listed in Stock C's outflows while attached to A.
    const stale = (json: JsonModel): void => {
      stockJson(json, 'Stock C').outflows = ['Flow F'];
      elements(json).push({ type: 'stock', uid: 16, name: 'Stock D', x: 100, y: 400 });
      json.stocks!.push({ name: 'Stock D', initialEquation: '1', inflows: [], outflows: [] });
    };
    const movedToD = (json: JsonModel): void => {
      stale(json);
      setPoints(json, 4, [
        { x: 100, y: 382.5, attachedToUid: 16 },
        { x: 277.5, y: 100, attachedToUid: 2 },
      ]);
      stockJson(json, 'Stock A').outflows = ['Flow H'];
      stockJson(json, 'Stock D').outflows = ['Flow F'];
    };
    expect(checkStockFlowDelta(load(edit(stale)), load(edit(movedToD)))).toEqual([]);
    const cleanedUp = edit((json) => {
      movedToD(json);
      stockJson(json, 'Stock C').outflows = [];
    });
    expect(arms(checkStockFlowDelta(load(edit(stale)), load(cleanedUp)))).toEqual(['M2.otherEntryChanged']);
  });

  it('a flow leaving a renamed stock must still leave its list', () => {
    // A stock rename leaves the stock's lists untouched (the engine rewrites
    // list entries only for renamed flows); the move is judged under the new name.
    const renamedAndMoved = (outflows: string[]): JsonModel =>
      edit((json) => {
        stockJson(json, 'Stock A').name = 'Stock Renamed';
        stockJson(json, 'Stock Renamed').outflows = outflows;
        element(json, 1).name = 'Stock Renamed';
        setPoints(json, 4, [
          { x: 300, y: 282.5, attachedToUid: 3 },
          { x: 277.5, y: 100, attachedToUid: 2 },
        ]);
        stockJson(json, 'Stock C').outflows = ['Flow F'];
      });
    expect(checkStockFlowDelta(load(baseJson()), load(renamedAndMoved(['Flow H'])))).toEqual([]);
    expect(arms(checkStockFlowDelta(load(baseJson()), load(renamedAndMoved(['Flow F', 'Flow H']))))).toEqual([
      'M2.notRemoved',
    ]);
  });

  it('a flow renamed and moved in one commit is judged under its new name', () => {
    // The rename half has the shape the engine rename path produces (entries naming
    // the flow rewritten to the new name; see the engine-backed tests below); the
    // move takes Flow F's source from Stock A to Stock C.
    const renamedAndMoved = (aOutflows: string[]): JsonModel =>
      edit((json) => {
        json.flows!.find((f) => f.name === 'Flow F')!.name = 'Flow Renamed';
        element(json, 4).name = 'Flow Renamed';
        setPoints(json, 4, [
          { x: 300, y: 282.5, attachedToUid: 3 },
          { x: 277.5, y: 100, attachedToUid: 2 },
        ]);
        stockJson(json, 'Stock A').outflows = aOutflows;
        stockJson(json, 'Stock B').inflows = ['flow_renamed'];
        stockJson(json, 'Stock C').outflows = ['flow_renamed'];
      });
    expect(checkStockFlowDelta(load(baseJson()), load(renamedAndMoved(['Flow H'])))).toEqual([]);
    expect(arms(checkStockFlowDelta(load(baseJson()), load(renamedAndMoved(['Flow H', 'flow_renamed']))))).toEqual([
      'M2.notRemoved',
    ]);
  });

  it('reordering a list is not a change', () => {
    const reordered = edit((json) => {
      stockJson(json, 'Stock A').outflows = ['Flow H', 'Flow F'];
    });
    expect(checkStockFlowDelta(load(baseJson()), load(reordered))).toEqual([]);
  });

  it('a canonical spelling of the new entry counts as the entry', () => {
    expect(
      moveEnd('source', (json) => {
        removeFromOld(json, 'source');
        stockJson(json, 'Stock C').outflows = ['flow_f'];
      })(),
    ).toEqual([]);
  });

  it('no list requirement at the new stock when the flow has no variable', () => {
    const next = edit((json) => {
      json.flows = json.flows!.filter((f) => f.name !== 'Flow F');
      setPoints(json, 4, [
        { x: 300, y: 282.5, attachedToUid: 3 },
        { x: 277.5, y: 100, attachedToUid: 2 },
      ]);
      removeFromOld(json, 'source');
    });
    expect(checkStockFlowDelta(load(baseJson()), load(next))).toEqual([]);
  });
});

// Renames through the path the Editor takes (`buildVariableRenameOps` ->
// `applyPatch` with the controller's options -> serializeJson -> projectFromJson),
// so the list rewrite being excluded is the one the engine produces.
describeWithEngine('M2 renames through the engine rename path', () => {
  let engine: EngineModule;

  beforeAll(async () => {
    engine = await loadEngine();
  });

  async function rename(
    oldName: string,
    newName: string,
    adjust: (json: JsonModel) => void = () => {},
  ): Promise<{ base: ViewAndVariables; next: ViewAndVariables }> {
    const projectJson: JsonProject = {
      name: 'rename',
      simSpecs: { startTime: 0, endTime: 1, dt: '1' },
      models: [baseJson()],
    };
    const project = await engine.Project.openJson(JSON.stringify(projectJson));
    try {
      const base = await editorModel(project);
      const { ops } = buildVariableRenameOps(base.views[0], oldName, newName);
      await project.applyPatch({ models: [{ name: 'main', ops: [...ops] }] }, { allowErrors: true });
      const serialized = JSON.parse(await project.serializeJson()) as JsonProject;
      adjust(serialized.models.find((m) => m.name === 'main')!);
      return { base: fromModel(base), next: fromModel(mainModel(projectFromJson(serialized).models)) };
    } finally {
      await project.dispose();
    }
  }

  it('a renamed flow: the engine rewrites and re-sorts its list entries, which is not a change', async () => {
    const { base, next } = await rename('Flow F', 'Flow Renamed');
    const lists = (vv: ViewAndVariables, ident: string): unknown => {
      const s = vv.variables.get(ident);
      return s?.type === 'stock' ? { inflows: s.inflows, outflows: s.outflows } : undefined;
    };
    // What the rename actually produced, so the exclusion below is exercised on it.
    expect(lists(next, 'stock_a')).toEqual({ inflows: ['Inflow G'], outflows: ['Flow H', 'flow_renamed'] });
    expect(lists(next, 'stock_b')).toEqual({ inflows: ['flow_renamed'], outflows: [] });
    expect(checkStockFlowDelta(base, next)).toEqual([]);
  });

  it('a renamed stock keeps its lists, which is not a change', async () => {
    const { base, next } = await rename('Stock A', 'Stock Renamed');
    expect(next.variables.has('stock_renamed')).toBe(true);
    expect(checkStockFlowDelta(base, next)).toEqual([]);
  });

  it('an entry left under the old name after a flow rename is a change', async () => {
    const { base, next } = await rename('Flow F', 'Flow Renamed', (json) => {
      json.stocks!.find((s) => s.name === 'Stock B')!.inflows = ['Flow F'];
    });
    expect(arms(checkStockFlowDelta(base, next))).toEqual(['M2.otherEntryChanged']);
  });

  it('an unrelated entry changed alongside a rename still reports', async () => {
    const { base, next } = await rename('Flow F', 'Flow Renamed', (json) => {
      json.stocks!.find((s) => s.name === 'Stock A')!.inflows = [];
    });
    expect(arms(checkStockFlowDelta(base, next))).toEqual(['M2.otherEntryChanged']);
  });
});

describe('M3 cloud endpoints', () => {
  it("a one-point flow's lone point is one endpoint, not two", () => {
    const view = load(edit((json) => setPoints(json, 5, [{ x: 100, y: 250, attachedToUid: 6 }]))).view;
    expect(checkReferentialIntegrity(view)).toEqual([]);
  });
});

describe('stock-list agreement scoping', () => {
  it('ignores entries naming flows with no element on the view', () => {
    const json = edit((j) => {
      j.flows!.push({ name: 'Offscreen', equation: '1' });
      stockJson(j, 'Stock A').outflows = ['Flow F', 'Flow H', 'Offscreen'];
    });
    const { view, variables } = load(json);
    expect(checkStockFlowAgreement(view, variables)).toEqual([]);
  });
});
