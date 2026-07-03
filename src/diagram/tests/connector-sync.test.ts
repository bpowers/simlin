// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { computeConnectorErrors } from '../connector-sync';
import type {
  Aux,
  AuxViewElement,
  AliasViewElement,
  CloudViewElement,
  Flow,
  FlowViewElement,
  GroupViewElement,
  LinkViewElement,
  Module,
  ModuleViewElement,
  Stock,
  StockViewElement,
  Variable,
  ViewElement,
} from '@simlin/core/datamodel';

// --- element builders ---

const ident = (name: string): string => name.toLowerCase().replace(/ /g, '_');

const aux = (uid: number, name: string): AuxViewElement => ({
  type: 'aux',
  uid,
  var: undefined,
  x: 0,
  y: 0,
  name,
  ident: ident(name),
  labelSide: 'right',
  isZeroRadius: false,
});

const stock = (uid: number, name: string): StockViewElement => ({
  type: 'stock',
  uid,
  var: undefined,
  x: 0,
  y: 0,
  name,
  ident: ident(name),
  labelSide: 'center',
  isZeroRadius: false,
  inflows: [],
  outflows: [],
});

const flow = (uid: number, name: string): FlowViewElement => ({
  type: 'flow',
  uid,
  var: undefined,
  x: 0,
  y: 0,
  name,
  ident: ident(name),
  labelSide: 'center',
  points: [],
  isZeroRadius: false,
});

const moduleEl = (uid: number, name: string): ModuleViewElement => ({
  type: 'module',
  uid,
  var: undefined,
  x: 0,
  y: 0,
  name,
  ident: ident(name),
  labelSide: 'center',
  isZeroRadius: false,
});

const alias = (uid: number, aliasOfUid: number): AliasViewElement => ({
  type: 'alias',
  uid,
  aliasOfUid,
  x: 0,
  y: 0,
  labelSide: 'center',
  isZeroRadius: false,
  ident: undefined,
});

const cloud = (uid: number, flowUid: number): CloudViewElement => ({
  type: 'cloud',
  uid,
  flowUid,
  x: 0,
  y: 0,
  isZeroRadius: false,
  ident: undefined,
});

const group = (uid: number, name: string): GroupViewElement => ({
  type: 'group',
  uid,
  name,
  x: 0,
  y: 0,
  width: 10,
  height: 10,
  isZeroRadius: false,
  ident: undefined,
});

const link = (uid: number, fromUid: number, toUid: number): LinkViewElement => ({
  type: 'link',
  uid,
  fromUid,
  toUid,
  arc: undefined,
  isStraight: true,
  multiPoint: undefined,
  polarity: undefined,
  x: NaN,
  y: NaN,
  isZeroRadius: false,
  ident: undefined,
});

// --- variable builders ---

const auxVar = (name: string): Aux => ({
  type: 'aux',
  ident: ident(name),
  equation: { type: 'scalar', equation: '0' },
  documentation: '',
  units: '',
  gf: undefined,
  canBeModuleInput: false,
  isPublic: false,
  activeInitial: undefined,
  dataSource: undefined,
  data: undefined,
  errors: undefined,
  unitErrors: undefined,
  connectorErrors: undefined,
  uid: undefined,
});

const stockVar = (name: string): Stock => ({
  type: 'stock',
  ident: ident(name),
  equation: { type: 'scalar', equation: '0' },
  documentation: '',
  units: '',
  inflows: [],
  outflows: [],
  nonNegative: false,
  canBeModuleInput: false,
  isPublic: false,
  activeInitial: undefined,
  dataSource: undefined,
  data: undefined,
  errors: undefined,
  unitErrors: undefined,
  connectorErrors: undefined,
  uid: undefined,
});

const flowVar = (name: string): Flow => ({
  type: 'flow',
  ident: ident(name),
  equation: { type: 'scalar', equation: '0' },
  documentation: '',
  units: '',
  gf: undefined,
  nonNegative: false,
  canBeModuleInput: false,
  isPublic: false,
  activeInitial: undefined,
  dataSource: undefined,
  data: undefined,
  errors: undefined,
  unitErrors: undefined,
  connectorErrors: undefined,
  uid: undefined,
});

const moduleVar = (name: string): Module => ({
  type: 'module',
  ident: ident(name),
  modelName: 'sub',
  documentation: '',
  units: '',
  references: [],
  canBeModuleInput: false,
  isPublic: false,
  dataSource: undefined,
  data: undefined,
  errors: undefined,
  unitErrors: undefined,
  connectorErrors: undefined,
  uid: undefined,
});

const variablesOf = (...vars: Variable[]): ReadonlyMap<string, Variable> => new Map(vars.map((v) => [v.ident, v]));

describe('computeConnectorErrors', () => {
  it('flags a missing connector when the equation references a drawn-but-unconnected variable', () => {
    const elements: ViewElement[] = [aux(1, 'a'), aux(2, 'b')];
    const result = computeConnectorErrors({
      elements,
      variables: variablesOf(auxVar('a'), auxVar('b')),
      // b's equation uses a, but there is no connector a -> b.
      dependencies: new Map([['b', ['a']]]),
    });
    expect(result.get('b')).toEqual([{ kind: 'missingConnector', ident: 'a', name: 'a' }]);
    expect(result.has('a')).toBe(false);
  });

  it('reports no issue when the connector matches the equation dependency', () => {
    const elements: ViewElement[] = [aux(1, 'a'), aux(2, 'b'), link(3, 1, 2)];
    const result = computeConnectorErrors({
      elements,
      variables: variablesOf(auxVar('a'), auxVar('b')),
      dependencies: new Map([['b', ['a']]]),
    });
    expect(result.size).toBe(0);
  });

  it('flags a stale connector when a connector source is unused by the equation', () => {
    const elements: ViewElement[] = [aux(1, 'a'), aux(2, 'b'), link(3, 1, 2)];
    const result = computeConnectorErrors({
      elements,
      variables: variablesOf(auxVar('a'), auxVar('b')),
      // b does not reference a, yet a -> b is drawn.
      dependencies: new Map([['b', []]]),
    });
    expect(result.get('b')).toEqual([{ kind: 'staleConnector', ident: 'a', name: 'a' }]);
  });

  it('resolves a connector originating from an alias to its aliased variable', () => {
    const elements: ViewElement[] = [aux(1, 'a'), aux(2, 'b'), alias(3, 1), link(4, 3, 2)];
    const result = computeConnectorErrors({
      elements,
      variables: variablesOf(auxVar('a'), auxVar('b')),
      // The connector comes from an alias of a; a -> b is satisfied.
      dependencies: new Map([['b', ['a']]]),
    });
    expect(result.size).toBe(0);
  });

  it('does not flag a missing connector when the dependency has no element on the view', () => {
    // a is referenced by b's equation but is not placed on this view, so no
    // connector is drawable -- not a sketch-hygiene issue.
    const elements: ViewElement[] = [aux(2, 'b')];
    const result = computeConnectorErrors({
      elements,
      variables: variablesOf(auxVar('a'), auxVar('b')),
      dependencies: new Map([['b', ['a']]]),
    });
    expect(result.size).toBe(0);
  });

  it('still flags a missing connector when only an (unconnected) alias of the source is present', () => {
    // a's node and an alias of a both sit on the view, but no connector runs
    // from either into b. The alias existing does not satisfy the dependency.
    const elements: ViewElement[] = [aux(1, 'a'), aux(2, 'b'), alias(3, 1)];
    const result = computeConnectorErrors({
      elements,
      variables: variablesOf(auxVar('a'), auxVar('b')),
      dependencies: new Map([['b', ['a']]]),
    });
    expect(result.get('b')).toEqual([{ kind: 'missingConnector', ident: 'a', name: 'a' }]);
  });

  it('exempts a connector whose source is a module (unverifiable module-output dependency)', () => {
    const elements: ViewElement[] = [moduleEl(1, 'sub'), aux(2, 'reader'), link(3, 1, 2)];
    const result = computeConnectorErrors({
      elements,
      variables: variablesOf(moduleVar('sub'), auxVar('reader')),
      // getIncomingLinks drops the dotted `sub.output`, so reader shows no dep;
      // the connector from the module must NOT be flagged stale.
      dependencies: new Map([['reader', []]]),
    });
    expect(result.size).toBe(0);
  });

  it('does not check module variables as targets', () => {
    const elements: ViewElement[] = [aux(1, 'a'), moduleEl(2, 'sub')];
    const result = computeConnectorErrors({
      elements,
      variables: variablesOf(auxVar('a'), moduleVar('sub')),
      // Even if a dependency map is supplied for the module, it is skipped.
      dependencies: new Map([['sub', ['a']]]),
    });
    expect(result.size).toBe(0);
  });

  it('handles a stock whose expected connectors are its initial-equation dependencies', () => {
    const elements: ViewElement[] = [aux(1, 'init_level'), stock(2, 'level')];
    const result = computeConnectorErrors({
      elements,
      variables: variablesOf(auxVar('init_level'), stockVar('level')),
      // level's initial value references init_level, but no connector is drawn.
      dependencies: new Map([['level', ['init_level']]]),
    });
    expect(result.get('level')).toEqual([{ kind: 'missingConnector', ident: 'init_level', name: 'init_level' }]);
  });

  it('flags a flow that reads a variable without a connector', () => {
    const elements: ViewElement[] = [aux(1, 'rate'), flow(2, 'net_flow')];
    const result = computeConnectorErrors({
      elements,
      variables: variablesOf(auxVar('rate'), flowVar('net_flow')),
      dependencies: new Map([['net_flow', ['rate']]]),
    });
    expect(result.get('net_flow')).toEqual([{ kind: 'missingConnector', ident: 'rate', name: 'rate' }]);
  });

  it('ignores self-references in both directions', () => {
    const elements: ViewElement[] = [aux(1, 'a'), link(2, 1, 1)];
    const result = computeConnectorErrors({
      elements,
      variables: variablesOf(auxVar('a')),
      // a depends on itself and has a self-connector; neither is flagged.
      dependencies: new Map([['a', ['a']]]),
    });
    expect(result.size).toBe(0);
  });

  it('reports both a missing and a stale connector on the same variable', () => {
    const elements: ViewElement[] = [aux(1, 'a'), aux(2, 'b'), aux(3, 'c'), link(4, 3, 2)];
    const result = computeConnectorErrors({
      elements,
      variables: variablesOf(auxVar('a'), auxVar('b'), auxVar('c')),
      // b uses a (no connector) but has a connector from c (unused).
      dependencies: new Map([['b', ['a']]]),
    });
    const issues = result.get('b');
    expect(issues).toEqual([
      { kind: 'missingConnector', ident: 'a', name: 'a' },
      { kind: 'staleConnector', ident: 'c', name: 'c' },
    ]);
  });

  it('collapses duplicate connectors from the same source into one stale issue', () => {
    const elements: ViewElement[] = [aux(1, 'a'), aux(2, 'b'), link(3, 1, 2), link(4, 1, 2)];
    const result = computeConnectorErrors({
      elements,
      variables: variablesOf(auxVar('a'), auxVar('b')),
      dependencies: new Map([['b', []]]),
    });
    expect(result.get('b')).toEqual([{ kind: 'staleConnector', ident: 'a', name: 'a' }]);
  });

  it('ignores links whose endpoints do not resolve to variables (clouds, groups, dangling aliases)', () => {
    const elements: ViewElement[] = [
      flow(1, 'f'),
      cloud(2, 1),
      group(3, 'grp'),
      aux(4, 'b'),
      alias(5, 999), // dangling: aliasOfUid points nowhere
      link(6, 2, 4), // from a cloud
      link(7, 5, 4), // from a dangling alias
    ];
    const result = computeConnectorErrors({
      elements,
      variables: variablesOf(flowVar('f'), auxVar('b')),
      dependencies: new Map([['b', []]]),
    });
    // Neither unresolved link produces a stale issue.
    expect(result.size).toBe(0);
  });

  it('uses the original-case display name from the view element', () => {
    const elements: ViewElement[] = [aux(1, 'Birth Rate'), aux(2, 'b')];
    const result = computeConnectorErrors({
      elements,
      variables: variablesOf(auxVar('Birth Rate'), auxVar('b')),
      dependencies: new Map([['b', ['birth_rate']]]),
    });
    expect(result.get('b')).toEqual([{ kind: 'missingConnector', ident: 'birth_rate', name: 'Birth Rate' }]);
  });

  it('skips a dependency-map target that has no primary node on the view', () => {
    // Only an alias of b is on the view (no primary node), so b is not a
    // checkable target even though a dependency entry exists.
    const elements: ViewElement[] = [aux(1, 'a')];
    const result = computeConnectorErrors({
      elements,
      variables: variablesOf(auxVar('a'), auxVar('b')),
      dependencies: new Map([['b', ['a']]]),
    });
    expect(result.size).toBe(0);
  });

  it('skips a dependency-map target absent from the variables map', () => {
    const elements: ViewElement[] = [aux(1, 'a')];
    const result = computeConnectorErrors({
      elements,
      variables: variablesOf(auxVar('a')),
      dependencies: new Map([['ghost', ['a']]]),
    });
    expect(result.size).toBe(0);
  });

  it('returns an empty map when there are no dependencies to check', () => {
    const elements: ViewElement[] = [aux(1, 'a')];
    const result = computeConnectorErrors({
      elements,
      variables: variablesOf(auxVar('a')),
      dependencies: new Map(),
    });
    expect(result.size).toBe(0);
  });
});
