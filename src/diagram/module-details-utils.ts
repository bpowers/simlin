// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Functional Core

import type { Model, Project, Variable } from '@simlin/core/datamodel';
import { STDLIB_MODEL_NAMES, STDLIB_PREFIX, isMacroModel } from './module-navigation';

/**
 * Counts how many module variables across all models in the project
 * reference the given model name. Used to determine shared model
 * editing awareness (AC4.1: "This model is used by N modules").
 */
export function countModelInstances(project: Project, modelName: string): number {
  let count = 0;
  for (const model of project.models.values()) {
    for (const variable of model.variables.values()) {
      if (variable.type === 'module' && variable.modelName === modelName) {
        count++;
      }
    }
  }
  return count;
}

/**
 * Returns true if setting a module in `fromModelName` to reference
 * `toModelName` would create a circular nesting chain. Performs DFS
 * from `toModelName` following module.modelName references; if DFS
 * reaches `fromModelName`, a cycle exists.
 *
 * Self-reference (fromModelName === toModelName) is always a cycle.
 */
export function wouldCreateCycle(project: Project, fromModelName: string, toModelName: string): boolean {
  if (fromModelName === toModelName) {
    return true;
  }

  // DFS from toModelName: if we can reach fromModelName, adding
  // fromModelName -> toModelName would close a cycle.
  const visited = new Set<string>();
  const stack: Array<string> = [toModelName];

  while (stack.length > 0) {
    const current = stack.pop()!;
    if (current === fromModelName) {
      return true;
    }
    if (visited.has(current)) {
      continue;
    }
    visited.add(current);

    const model = project.models.get(current);
    if (!model) {
      continue;
    }
    for (const variable of model.variables.values()) {
      if (variable.type === 'module' && variable.modelName) {
        stack.push(variable.modelName);
      }
    }
  }

  return false;
}

/**
 * Returns the model names available for a module's model reference,
 * split into project-defined models and stdlib models. Excludes the
 * current model (self-reference) and models that would create cycles.
 */
export function getAvailableModels(
  project: Project,
  currentModelName: string,
): { projectModels: ReadonlyArray<string>; stdlibModels: ReadonlyArray<string> } {
  const projectModels: Array<string> = [];
  // Start from the full stdlib registry so the "Standard Library"
  // group is populated even before any stdlib module is referenced.
  // Stdlib models don't need cycle checking because they never
  // contain module variables that reference user models.
  const stdlibSet = new Set<string>();
  for (const shortName of STDLIB_MODEL_NAMES) {
    const fullName = `${STDLIB_PREFIX}${shortName}`;
    if (fullName !== currentModelName) {
      stdlibSet.add(fullName);
    }
  }

  for (const [name, model] of project.models) {
    if (name === currentModelName) {
      continue;
    }
    // macros.AC6.6: a macro-marked model is a callable macro template,
    // never a selectable module-reference target -- skip it (and defend
    // the stdlib group in the unlikely case a macro shadows a stdlib
    // name). The engine materializes macro invocations directly; the
    // diagram must not let a user point a module at a macro model.
    if (isMacroModel(model)) {
      stdlibSet.delete(name);
      continue;
    }
    if (wouldCreateCycle(project, currentModelName, name)) {
      // Remove from stdlibSet too: a user-defined model that shadows
      // a stdlib name and creates a cycle must not be offered.
      stdlibSet.delete(name);
      continue;
    }
    // Use prefix check (not isStdlibModel) so user models with bare
    // stdlib names like "delay1" stay in projectModels. Only models
    // with the engine's stdlib⁚ prefix are classified as stdlib.
    if (name.startsWith(STDLIB_PREFIX)) {
      // Already in stdlibSet from the registry; nothing to do.
      continue;
    }
    projectModels.push(name);
  }

  return { projectModels, stdlibModels: [...stdlibSet] };
}

/**
 * Returns variables from a model where canBeModuleInput is true.
 * Only Aux, Stock, and Flow can be input ports; modules are excluded.
 */
export function getInputPorts(model: Model): ReadonlyArray<Variable> {
  const result: Array<Variable> = [];
  for (const variable of model.variables.values()) {
    if (variable.type === 'module') {
      continue;
    }
    if (variable.canBeModuleInput) {
      result.push(variable);
    }
  }
  return result;
}

/**
 * Returns variables from a model where isPublic is true.
 * Only Aux, Stock, and Flow can be public; modules are excluded.
 */
export function getPublicVariables(model: Model): ReadonlyArray<Variable> {
  const result: Array<Variable> = [];
  for (const variable of model.variables.values()) {
    if (variable.type === 'module') {
      continue;
    }
    if (variable.isPublic) {
      result.push(variable);
    }
  }
  return result;
}
