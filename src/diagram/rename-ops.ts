// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Functional Core -- the next view of a variable rename

import { canonicalize } from '@simlin/core/canonicalize';
import { isNamedViewElement, type StockFlowView, type ViewElement } from '@simlin/core/datamodel';

import { encodeNameNewlines } from './drawing/common';

/**
 * The view a rename produces: every named element whose name is `oldName`
 * (compared canonically) relabeled `newName`. The engine's RenameVariable
 * renames equations, module references and group members but never view
 * elements, so a rename is an edit WITH a next view, and buildEditOps derives
 * the renameVariable op from the relabeled element (`from` its committed ident,
 * `to` its new name).
 *
 * The new name is the user's typed name RAW, with line breaks encoded to the
 * stored backslash-n form (a raw newline would canonicalize into a malformed
 * ident) but NOT canonicalized: the engine stores display spellings verbatim
 * and does all matching canonically (issue #890), so canonicalizing would
 * downgrade the stored display name, and a case-only rename ("students" ->
 * "Students") would restamp a preserved spelling (issue #906).
 *
 * The element takes the new name's ident too. Elements are matched by name,
 * not by `ident`, so a second rename while the first is pending (or a rename of
 * a pending create) finds the element under the name it renders with. While the
 * rename is pending, the controller's rendered model names the committed
 * variable by the new ident, so the canvas and the details panel still resolve
 * the element to it.
 */
export function relabelVariable(view: StockFlowView, oldName: string, newName: string): StockFlowView {
  const oldIdent = canonicalize(encodeNameNewlines(oldName));
  const encodedName = encodeNameNewlines(newName);
  const newIdent = canonicalize(encodedName);
  const elements = view.elements.map((element: ViewElement) => {
    if (!isNamedViewElement(element) || canonicalize(element.name) !== oldIdent) {
      return element;
    }
    return { ...element, name: encodedName, ident: newIdent };
  });
  return { ...view, elements };
}
