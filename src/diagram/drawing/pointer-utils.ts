// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * A pointerdown→pointerup whose cursor wobbled less than this many *screen*
 * pixels is a click, not a drag. Physical trackpad/mouse clicks routinely
 * move a pixel or two as the button is pressed and released, and touch taps
 * move even more; treating that jitter as a drag both nudges the element and
 * (because dragging suppresses it) leaves the variable-details panel closed.
 */
export const ClickDragThresholdPx = 5;
