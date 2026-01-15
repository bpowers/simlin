// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * JSON-compatible types for the simlin patch API.
 *
 * These types match the Rust JSON types in src/simlin-engine/src/json.rs
 * and are used for serializing patches to send to the simulation engine.
 */

// Supporting types

/**
 * Scale for graphical function axes.
 */
export interface JsonGraphicalFunctionScale {
  min: number;
  max: number;
}

/**
 * A graphical/table function (lookup table).
 */
export interface JsonGraphicalFunction {
  points?: [number, number][];
  y_points?: number[];
  kind?: string;
  x_scale?: JsonGraphicalFunctionScale;
  y_scale?: JsonGraphicalFunctionScale;
}

/**
 * An element-specific equation for arrayed variables.
 */
export interface JsonElementEquation {
  subscript: string;
  equation: string;
  initial_equation?: string;
  graphical_function?: JsonGraphicalFunction;
}

/**
 * Equation structure for arrayed/subscripted variables.
 */
export interface JsonArrayedEquation {
  dimensions: string[];
  equation?: string;
  initial_equation?: string;
  elements?: JsonElementEquation[];
}

/**
 * A reference mapping between module input/output and parent model variable.
 */
export interface JsonModuleReference {
  src: string;
  dst: string;
}

// Variable types

/**
 * A stock (level, accumulation) variable for JSON serialization.
 */
export interface JsonStock {
  name: string;
  inflows?: string[];
  outflows?: string[];
  uid?: number;
  initial_equation?: string;
  units?: string;
  non_negative?: boolean;
  documentation?: string;
  can_be_module_input?: boolean;
  is_public?: boolean;
  arrayed_equation?: JsonArrayedEquation;
}

/**
 * A flow (rate) variable for JSON serialization.
 */
export interface JsonFlow {
  name: string;
  uid?: number;
  equation?: string;
  units?: string;
  non_negative?: boolean;
  graphical_function?: JsonGraphicalFunction;
  documentation?: string;
  can_be_module_input?: boolean;
  is_public?: boolean;
  arrayed_equation?: JsonArrayedEquation;
}

/**
 * An auxiliary (intermediate calculation) variable for JSON serialization.
 */
export interface JsonAuxiliary {
  name: string;
  uid?: number;
  equation?: string;
  initial_equation?: string;
  units?: string;
  graphical_function?: JsonGraphicalFunction;
  documentation?: string;
  can_be_module_input?: boolean;
  is_public?: boolean;
  arrayed_equation?: JsonArrayedEquation;
}

/**
 * A module (submodel) variable for JSON serialization.
 */
export interface JsonModule {
  name: string;
  model_name: string;
  uid?: number;
  units?: string;
  documentation?: string;
  references?: JsonModuleReference[];
  can_be_module_input?: boolean;
  is_public?: boolean;
}

/**
 * Union type for all JSON variable types.
 */
export type JsonVariable = JsonStock | JsonFlow | JsonAuxiliary | JsonModule;

// View types

/**
 * A point in a flow's visual representation.
 */
export interface JsonFlowPoint {
  x: number;
  y: number;
  attached_to_uid?: number;
}

/**
 * A point in a link's visual representation.
 */
export interface JsonLinkPoint {
  x: number;
  y: number;
}

/**
 * A rectangle for view bounds.
 */
export interface JsonRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

/**
 * Visual element for a stock.
 */
export interface JsonStockViewElement {
  uid: number;
  name: string;
  x: number;
  y: number;
  label_side?: string;
}

/**
 * Visual element for a flow.
 */
export interface JsonFlowViewElement {
  uid: number;
  name: string;
  x: number;
  y: number;
  points?: JsonFlowPoint[];
  label_side?: string;
}

/**
 * Visual element for an auxiliary variable.
 */
export interface JsonAuxiliaryViewElement {
  uid: number;
  name: string;
  x: number;
  y: number;
  label_side?: string;
}

/**
 * Visual element for a cloud (source/sink).
 */
export interface JsonCloudViewElement {
  uid: number;
  flow_uid: number;
  x: number;
  y: number;
}

/**
 * Visual element for a causal link.
 */
export interface JsonLinkViewElement {
  uid: number;
  from_uid: number;
  to_uid: number;
  arc?: number;
  multi_points?: JsonLinkPoint[];
}

/**
 * Visual element for a module.
 */
export interface JsonModuleViewElement {
  uid: number;
  name: string;
  x: number;
  y: number;
  label_side?: string;
}

/**
 * Visual element for an alias (ghost).
 */
export interface JsonAliasViewElement {
  uid: number;
  alias_of_uid: number;
  x: number;
  y: number;
  label_side?: string;
}

/**
 * Union type for view elements.
 */
export type JsonViewElement =
  | JsonStockViewElement
  | JsonFlowViewElement
  | JsonAuxiliaryViewElement
  | JsonCloudViewElement
  | JsonLinkViewElement
  | JsonModuleViewElement
  | JsonAliasViewElement;

/**
 * A view/diagram in the model.
 */
export interface JsonView {
  elements?: JsonViewElement[];
  kind?: string;
  view_box?: JsonRect;
  zoom?: number;
}

// Simulation specs

/**
 * Simulation specification.
 */
export interface JsonSimSpecs {
  start_time: number;
  end_time: number;
  dt?: string;
  save_step?: number;
  method?: string;
  time_units?: string;
}

// Project structure types

/**
 * A dimension for subscripted variables.
 */
export interface JsonDimension {
  name: string;
  elements?: string[];
  size?: number;
  maps_to?: string;
}

/**
 * A unit definition.
 */
export interface JsonUnit {
  name: string;
  equation?: string;
  disabled?: boolean;
  aliases?: string[];
}

/**
 * Metadata for a feedback loop.
 */
export interface JsonLoopMetadata {
  uids?: number[];
  deleted?: boolean;
  name?: string;
  description?: string;
}

/**
 * A model in the project.
 */
export interface JsonModel {
  name: string;
  stocks?: JsonStock[];
  flows?: JsonFlow[];
  auxiliaries?: JsonAuxiliary[];
  modules?: JsonModule[];
  sim_specs?: JsonSimSpecs;
  views?: JsonView[];
  loop_metadata?: JsonLoopMetadata[];
}

/**
 * A complete system dynamics project.
 */
export interface JsonProject {
  name: string;
  sim_specs: JsonSimSpecs;
  models?: JsonModel[];
  dimensions?: JsonDimension[];
  units?: JsonUnit[];
}

// Patch operation payloads

/**
 * Payload for upsert stock operation.
 */
export interface UpsertStockPayload {
  stock: JsonStock;
}

/**
 * Payload for upsert flow operation.
 */
export interface UpsertFlowPayload {
  flow: JsonFlow;
}

/**
 * Payload for upsert auxiliary operation.
 */
export interface UpsertAuxPayload {
  aux: JsonAuxiliary;
}

/**
 * Payload for upsert module operation.
 */
export interface UpsertModulePayload {
  module: JsonModule;
}

/**
 * Payload for delete variable operation.
 */
export interface DeleteVariablePayload {
  ident: string;
}

/**
 * Payload for rename variable operation.
 */
export interface RenameVariablePayload {
  from: string;
  to: string;
}

/**
 * Payload for upsert view operation.
 */
export interface UpsertViewPayload {
  index: number;
  view: JsonView;
}

/**
 * Payload for delete view operation.
 */
export interface DeleteViewPayload {
  index: number;
}

/**
 * Payload for set sim specs operation.
 */
export interface SetSimSpecsPayload {
  sim_specs: JsonSimSpecs;
}

// Operation types with type discriminator

export interface UpsertStockOp {
  type: 'upsert_stock';
  payload: UpsertStockPayload;
}

export interface UpsertFlowOp {
  type: 'upsert_flow';
  payload: UpsertFlowPayload;
}

export interface UpsertAuxOp {
  type: 'upsert_aux';
  payload: UpsertAuxPayload;
}

export interface UpsertModuleOp {
  type: 'upsert_module';
  payload: UpsertModulePayload;
}

export interface DeleteVariableOp {
  type: 'delete_variable';
  payload: DeleteVariablePayload;
}

export interface RenameVariableOp {
  type: 'rename_variable';
  payload: RenameVariablePayload;
}

export interface UpsertViewOp {
  type: 'upsert_view';
  payload: UpsertViewPayload;
}

export interface DeleteViewOp {
  type: 'delete_view';
  payload: DeleteViewPayload;
}

export interface SetSimSpecsOp {
  type: 'set_sim_specs';
  payload: SetSimSpecsPayload;
}

/**
 * Union type for model operations.
 */
export type JsonModelOperation =
  | UpsertStockOp
  | UpsertFlowOp
  | UpsertAuxOp
  | UpsertModuleOp
  | DeleteVariableOp
  | RenameVariableOp
  | UpsertViewOp
  | DeleteViewOp;

/**
 * Union type for project operations.
 */
export type JsonProjectOperation = SetSimSpecsOp;

// Patch structures

/**
 * A patch containing operations for a specific model.
 */
export interface JsonModelPatch {
  name: string;
  ops: JsonModelOperation[];
}

/**
 * A patch containing project-level and model-level operations.
 */
export interface JsonProjectPatch {
  project_ops?: JsonProjectOperation[];
  models?: JsonModelPatch[];
}

// Type guards for discriminating operations

export function isUpsertStock(op: JsonModelOperation): op is UpsertStockOp {
  return op.type === 'upsert_stock';
}

export function isUpsertFlow(op: JsonModelOperation): op is UpsertFlowOp {
  return op.type === 'upsert_flow';
}

export function isUpsertAux(op: JsonModelOperation): op is UpsertAuxOp {
  return op.type === 'upsert_aux';
}

export function isUpsertModule(op: JsonModelOperation): op is UpsertModuleOp {
  return op.type === 'upsert_module';
}

export function isDeleteVariable(op: JsonModelOperation): op is DeleteVariableOp {
  return op.type === 'delete_variable';
}

export function isRenameVariable(op: JsonModelOperation): op is RenameVariableOp {
  return op.type === 'rename_variable';
}

export function isUpsertView(op: JsonModelOperation): op is UpsertViewOp {
  return op.type === 'upsert_view';
}

export function isDeleteView(op: JsonModelOperation): op is DeleteViewOp {
  return op.type === 'delete_view';
}
