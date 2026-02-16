// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Updates an arc angle by a given difference, preserving undefined values.
 *
 * When arc is undefined, the link is a straight line between endpoints.
 * Moving endpoints of a straight line should keep it straight, so we
 * preserve the undefined value rather than converting to a curved arc.
 *
 * @param arc The current arc angle in degrees, or undefined for a straight line
 * @param angleDiff The angle difference to subtract (in degrees)
 * @returns The updated arc angle, or undefined if the input was undefined
 */
export function updateArcAngle(arc: number | undefined, angleDiff: number): number | undefined {
  if (arc === undefined) {
    return undefined;
  }
  return arc - angleDiff;
}

/**
 * Converts radians to degrees.
 */
export function radToDeg(radians: number): number {
  return (radians * 180) / Math.PI;
}
