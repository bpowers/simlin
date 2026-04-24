// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { Model, Project, stockFlowViewFromJson, stockFlowViewToJson } from '@simlin/core/datamodel';
import { mapSet } from '@simlin/core/common';

/**
 * Merge an incoming Project (built from the engine's serialized state) with
 * the current live Project (from React state), preserving the live model's
 * view for the active model.
 *
 * Why: Editor.updateProject() awaits the async engine round-trip
 * (applyPatch + serialize). During that round-trip the user can keep
 * panning/moving via setView() optimistic updates. When updateProject
 * finally commits, the engine view it serialized may be older than the
 * latest setView, so simply overwriting activeProject snaps the diagram
 * back to the engine's older view. Preserving the live view keeps those
 * optimistic updates intact.
 *
 * The live view is round-tripped through JSON so that ViewElement.var
 * references and Stock inflow/outflow UIDs are re-linked against the
 * incoming model's variables -- the live view may carry stale or
 * undefined refs from setView calls that ran before variable changes
 * propagated, but its positions, viewBox, and zoom are the latest user
 * intent.
 */
export function preserveLiveView(incoming: Project, live: Project | undefined, modelName: string): Project {
  if (!live) {
    return incoming;
  }
  const liveModel = live.models.get(modelName);
  const newModel = incoming.models.get(modelName);
  if (!liveModel || !newModel) {
    return incoming;
  }
  if (liveModel.views.length === 0 || newModel.views.length === 0) {
    return incoming;
  }

  const liveViewJson = stockFlowViewToJson(liveModel.views[0]);
  const preservedView = stockFlowViewFromJson(liveViewJson, newModel.variables);

  const updatedModel: Model = {
    ...newModel,
    views: [preservedView, ...newModel.views.slice(1)],
  };
  return {
    ...incoming,
    models: mapSet(incoming.models, modelName, updatedModel),
  };
}
