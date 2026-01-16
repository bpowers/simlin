// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * ModelPatchBuilder for accumulating model edit operations.
 *
 * Use Model.edit() to get a patch builder and apply batched edits.
 */

import {
  JsonStock,
  JsonFlow,
  JsonAuxiliary,
  JsonModule,
  JsonView,
  JsonModelPatch,
  JsonModelOperation,
  UpsertStockOp,
  UpsertFlowOp,
  UpsertAuxOp,
  UpsertModuleOp,
  DeleteVariableOp,
  RenameVariableOp,
  UpsertViewOp,
  DeleteViewOp,
} from './json-types';

/**
 * Builder for accumulating model operations before applying them as a JSON patch.
 *
 * ModelPatchBuilder collects operations (upsert, delete, rename) and
 * builds them into a JsonModelPatch that can be applied to a project.
 */
export class ModelPatchBuilder {
  private _modelName: string;
  private _ops: JsonModelOperation[] = [];

  /**
   * Create a new patch builder for the specified model.
   * @param modelName Name of the model to patch
   */
  constructor(modelName: string) {
    this._modelName = modelName;
  }

  /**
   * Get the model name this patch is for.
   */
  get modelName(): string {
    return this._modelName;
  }

  /**
   * Check if any operations have been added.
   */
  hasOperations(): boolean {
    return this._ops.length > 0;
  }

  /**
   * Get the number of operations added.
   */
  get operationCount(): number {
    return this._ops.length;
  }

  /**
   * Build the JSON patch structure.
   * @returns JsonModelPatch ready to be applied
   */
  build(): JsonModelPatch {
    return {
      name: this._modelName,
      ops: [...this._ops],
    };
  }

  /**
   * Insert or update a stock variable.
   * @param stock The stock to upsert
   * @returns The stock (for chaining)
   */
  upsertStock(stock: JsonStock): JsonStock {
    const op: UpsertStockOp = { type: 'upsert_stock', payload: { stock } };
    this._ops.push(op);
    return stock;
  }

  /**
   * Insert or update a flow variable.
   * @param flow The flow to upsert
   * @returns The flow (for chaining)
   */
  upsertFlow(flow: JsonFlow): JsonFlow {
    const op: UpsertFlowOp = { type: 'upsert_flow', payload: { flow } };
    this._ops.push(op);
    return flow;
  }

  /**
   * Insert or update an auxiliary variable.
   * @param aux The auxiliary to upsert
   * @returns The auxiliary (for chaining)
   */
  upsertAux(aux: JsonAuxiliary): JsonAuxiliary {
    const op: UpsertAuxOp = { type: 'upsert_aux', payload: { aux } };
    this._ops.push(op);
    return aux;
  }

  /**
   * Insert or update a module.
   * @param module The module to upsert
   * @returns The module (for chaining)
   */
  upsertModule(module: JsonModule): JsonModule {
    const op: UpsertModuleOp = { type: 'upsert_module', payload: { module } };
    this._ops.push(op);
    return module;
  }

  /**
   * Delete a variable by name.
   * @param ident Variable identifier to delete
   */
  deleteVariable(ident: string): void {
    const op: DeleteVariableOp = { type: 'delete_variable', payload: { ident } };
    this._ops.push(op);
  }

  /**
   * Rename a variable.
   * @param currentIdent Current variable name
   * @param newIdent New variable name
   */
  renameVariable(currentIdent: string, newIdent: string): void {
    const op: RenameVariableOp = { type: 'rename_variable', payload: { from: currentIdent, to: newIdent } };
    this._ops.push(op);
  }

  /**
   * Insert or update a view at a specific index.
   * @param index View index
   * @param view The view to upsert
   * @returns The view (for chaining)
   */
  upsertView(index: number, view: JsonView): JsonView {
    const op: UpsertViewOp = { type: 'upsert_view', payload: { index, view } };
    this._ops.push(op);
    return view;
  }

  /**
   * Delete a view at a specific index.
   * @param index View index to delete
   */
  deleteView(index: number): void {
    const op: DeleteViewOp = { type: 'delete_view', payload: { index } };
    this._ops.push(op);
  }
}
