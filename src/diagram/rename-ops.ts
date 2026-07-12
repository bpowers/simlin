// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Functional Core -- pure construction of the variable-rename patch ops

import { canonicalize } from '@simlin/core/canonicalize';
import { isNamedViewElement, type StockFlowView, stockFlowViewToJson, type ViewElement } from '@simlin/core/datamodel';
import type { JsonModelOperation } from '@simlin/engine';

import { encodeNameNewlines } from './drawing/common';

export interface RenameOps {
  readonly updatedView: StockFlowView;
  readonly ops: readonly JsonModelOperation[];
}

/**
 * Builds the patch ops for renaming a variable: a renameVariable op plus the
 * upsertView that keeps the sketch label in sync.
 *
 * The rename `to` is the user's typed name RAW (newlines encoded to the stored
 * backslash-n form, but NOT canonicalized): the engine stores display
 * spellings verbatim and does all matching canonically (issue #890), so
 * sending `canonicalize(newName)` would downgrade the stored display name --
 * and a case-only rename ("students" -> "Students") would actively restamp a
 * preserved spelling with its canonical form (issue #906). `from` stays
 * canonical, matching how idents are keyed and compared everywhere else in
 * the TS layer.
 *
 * The view element keeps its stale `ident` here on purpose (only `name` is
 * updated), preserving the pre-extraction behavior: the engine round-trip
 * that follows the patch rebuilds every element with a fresh ident.
 */
export function buildVariableRenameOps(view: StockFlowView, oldName: string, newName: string): RenameOps {
  const oldIdent = canonicalize(oldName);
  // Encode ALL line breaks to the stored backslash-n form -- a raw newline in
  // a multi-line name would canonicalize into a malformed ident.
  const encodedName = encodeNameNewlines(newName);

  const elements = view.elements.map((element: ViewElement) => {
    if (!isNamedViewElement(element) || element.ident !== oldIdent) {
      return element;
    }
    return { ...element, name: encodedName };
  });

  const updatedView: StockFlowView = { ...view, elements };

  const ops: readonly JsonModelOperation[] = [
    {
      type: 'renameVariable',
      payload: { from: oldIdent, to: encodedName },
    },
    {
      type: 'upsertView',
      payload: { index: 0, view: stockFlowViewToJson(updatedView) },
    },
  ];

  return { updatedView, ops };
}
