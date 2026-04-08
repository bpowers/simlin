// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Functional Core

import type { Variable } from '@simlin/core/datamodel';

/**
 * Returns true if any module variable in the given variables map has a non-empty modelName.
 * Used to determine whether to show warning indicators on unconfigured modules.
 *
 * When no modules have model references yet (new model scenario where the user
 * is rapidly sketching module structure), warnings are suppressed to avoid a
 * wall of warning dots on every freshly placed module.
 */
export function anyModuleHasModelReference(variables: ReadonlyMap<string, Variable>): boolean {
  for (const variable of variables.values()) {
    if (variable.type === 'module' && variable.modelName !== '') {
      return true;
    }
  }
  return false;
}
