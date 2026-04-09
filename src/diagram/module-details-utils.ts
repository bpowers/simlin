// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Functional Core

import type { Model, Project, Variable } from '@simlin/core/datamodel';
import { isStdlibModel } from './module-navigation';

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
  const stdlibModels: Array<string> = [];
  for (const name of project.models.keys()) {
    if (name === currentModelName) {
      continue;
    }
    if (wouldCreateCycle(project, currentModelName, name)) {
      continue;
    }
    if (isStdlibModel(name)) {
      stdlibModels.push(name);
    } else {
      projectModels.push(name);
    }
  }

  return { projectModels, stdlibModels };
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
