// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Functional Core
//
// Sketch-connector / equation consistency check. In a system-dynamics model the
// EQUATIONS are the source of truth for causal dependencies; the CONNECTORS
// (the arrows in the sketch) are the visual representation of those
// dependencies. They can drift apart -- an equation edited to reference a new
// variable without drawing a connector, or a connector left behind after an
// equation stops referencing something. This module computes, per target
// variable, the connectors that are out of sync with its equation.
//
// This is a pure function: given the view elements, the model's variables, and
// each target's equation-derived dependency idents (the engine's
// `getIncomingLinks`, which already excludes builtins/TIME, structural
// flow<->stock edges, and dotted module-output references), it returns the
// per-variable issues. The imperative shell (ProjectController) fetches the
// dependency idents from the engine and attaches the results to variables.

import type { ConnectorError, UID, Variable, ViewElement } from '@simlin/core/datamodel';

export interface ConnectorSyncInput {
  /** All elements of the active model's primary view (nodes, links, aliases). */
  readonly elements: readonly ViewElement[];
  /** The active model's variables, keyed by canonical ident. */
  readonly variables: ReadonlyMap<string, Variable>;
  /**
   * Per-target equation-derived dependency idents, from the engine's
   * `getIncomingLinks`. Only variables that should be CHECKED appear as keys
   * (aux/flow/stock with a primary element on the view); the caller decides
   * which to include. Each value is the set of in-model variable idents the
   * target's equation directly references.
   */
  readonly dependencies: ReadonlyMap<string, readonly string[]>;
}

// A view element that stands in for a variable (its primary node or an alias),
// resolved to the variable's canonical ident plus whether that variable is a
// module (modules are exempt as connector sources -- see below).
interface ResolvedEndpoint {
  readonly ident: string;
  readonly isModule: boolean;
}

/**
 * Resolve a view element (by uid) to the variable ident it represents, or
 * undefined when it is not a variable endpoint (a cloud, link, or group, or a
 * dangling alias). An alias is followed one hop via `aliasOfUid` to its target;
 * aliases never point at other aliases.
 */
function resolveEndpoint(uid: UID, byUid: ReadonlyMap<UID, ViewElement>): ResolvedEndpoint | undefined {
  const el = byUid.get(uid);
  if (!el) {
    return undefined;
  }
  switch (el.type) {
    case 'aux':
    case 'stock':
    case 'flow':
      return { ident: el.ident, isModule: false };
    case 'module':
      return { ident: el.ident, isModule: true };
    case 'alias': {
      const target = byUid.get(el.aliasOfUid);
      if (!target) {
        return undefined;
      }
      switch (target.type) {
        case 'aux':
        case 'stock':
        case 'flow':
          return { ident: target.ident, isModule: false };
        case 'module':
          return { ident: target.ident, isModule: true };
        default:
          return undefined;
      }
    }
    default:
      return undefined;
  }
}

/**
 * Compute per-variable connector/equation-sync issues for one model view.
 *
 * For each checked target T (a key of `dependencies` that has a primary
 * aux/flow/stock element on the view):
 *  - MISSING: T's equation references X (a real variable), X has a node or alias
 *    on this view (so a connector is drawable), but no connector runs from X (or
 *    an alias of X) into T.
 *  - STALE: a connector runs from X into T but T's equation does not reference X.
 *    Connectors whose source is a MODULE are exempt (a reader of a module output
 *    depends on `module.output`, a dotted ident the engine deliberately drops
 *    from `getIncomingLinks`, so we cannot verify it and never flag it).
 *
 * Self references (X === T) are ignored in both directions. The returned map
 * omits variables with no issues.
 */
export function computeConnectorErrors(input: ConnectorSyncInput): ReadonlyMap<string, readonly ConnectorError[]> {
  const { elements, variables, dependencies } = input;

  const byUid = new Map<UID, ViewElement>();
  // Canonical ident -> display name, sourced from the primary node's `name`.
  const displayName = new Map<string, string>();
  // Idents that have a PRIMARY (non-alias) aux/flow/stock/module node on the view.
  const primaryIdents = new Set<string>();
  // Idents that have ANY representation on the view (primary node or alias),
  // i.e. a connector to/from them is drawable.
  const presentIdents = new Set<string>();

  for (const el of elements) {
    byUid.set(el.uid, el);
    if (el.type === 'aux' || el.type === 'stock' || el.type === 'flow' || el.type === 'module') {
      primaryIdents.add(el.ident);
      presentIdents.add(el.ident);
      if (!displayName.has(el.ident)) {
        displayName.set(el.ident, el.name);
      }
    }
  }
  // A second pass records alias-only presence: an alias makes its target ident
  // "present" (drawable) even when the target's primary node lives on another
  // view. `byUid` must be fully populated first so aliasOfUid resolves.
  for (const el of elements) {
    if (el.type === 'alias') {
      const resolved = resolveEndpoint(el.uid, byUid);
      if (resolved) {
        presentIdents.add(resolved.ident);
      }
    }
  }

  // Connectors grouped by their resolved target ident.
  const incomingByTarget = new Map<string, ResolvedEndpoint[]>();
  for (const el of elements) {
    if (el.type !== 'link') {
      continue;
    }
    const from = resolveEndpoint(el.fromUid, byUid);
    const to = resolveEndpoint(el.toUid, byUid);
    if (!from || !to) {
      continue;
    }
    const existing = incomingByTarget.get(to.ident);
    if (existing) {
      existing.push(from);
    } else {
      incomingByTarget.set(to.ident, [from]);
    }
  }

  const nameFor = (ident: string): string => displayName.get(ident) ?? ident;

  const result = new Map<string, readonly ConnectorError[]>();

  for (const [ident, deps] of dependencies) {
    const variable = variables.get(ident);
    if (!variable || variable.type === 'module' || !primaryIdents.has(ident)) {
      continue;
    }

    const expected = new Set<string>();
    for (const dep of deps) {
      if (dep !== ident) {
        expected.add(dep);
      }
    }

    const incoming = incomingByTarget.get(ident) ?? [];
    const connectedSources = new Set<string>();
    for (const src of incoming) {
      connectedSources.add(src.ident);
    }

    const issues: ConnectorError[] = [];

    // MISSING: an expected dependency that is drawable but not connected.
    for (const dep of expected) {
      if (presentIdents.has(dep) && !connectedSources.has(dep)) {
        issues.push({ kind: 'missingConnector', ident: dep, name: nameFor(dep) });
      }
    }

    // STALE: a connector into T whose source T's equation does not reference.
    // Module sources are exempt (unverifiable dotted module-output deps), and
    // duplicate connectors from the same source collapse to one issue.
    const reportedStale = new Set<string>();
    for (const src of incoming) {
      if (src.isModule || src.ident === ident || expected.has(src.ident) || reportedStale.has(src.ident)) {
        continue;
      }
      reportedStale.add(src.ident);
      issues.push({ kind: 'staleConnector', ident: src.ident, name: nameFor(src.ident) });
    }

    if (issues.length > 0) {
      result.set(ident, issues);
    }
  }

  return result;
}
