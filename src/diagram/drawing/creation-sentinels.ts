// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Sentinel UIDs for elements a gesture draws that are not in the view.
 *
 * A live gesture renders placeholders the persisted view never holds; these
 * negative UIDs mark them so they can never collide with an allocated uid and
 * are never committed. A committed gesture allocates real uids from `nextUid`.
 */

/** The element an aux/stock/module tool press places until its name is typed. */
export const inCreationUid = -2;
/** The zero-radius point a link previews toward while it is over no valid target. */
export const fauxTargetUid = -3;
