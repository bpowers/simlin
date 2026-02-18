// Copyright 2026 The Simlin Authors. All rights reserved.
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
  yPoints?: number[];
  kind?: string;
  xScale?: JsonGraphicalFunctionScale;
  yScale?: JsonGraphicalFunctionScale;
}

/**
 * Vensim compatibility options for a variable.
 */
export interface JsonCompat {
  activeInitial?: string;
}

/**
 * An element-specific equation for arrayed variables.
 */
export interface JsonElementEquation {
  subscript: string;
  equation: string;
  compat?: JsonCompat;
  graphicalFunction?: JsonGraphicalFunction;
}

/**
 * Equation structure for arrayed/subscripted variables.
 */
export interface JsonArrayedEquation {
  dimensions: string[];
  equation?: string;
  compat?: JsonCompat;
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
  inflows: string[];
  outflows: string[];
  uid?: number;
  initialEquation?: string;
  units?: string;
  nonNegative?: boolean;
  documentation?: string;
  canBeModuleInput?: boolean;
  isPublic?: boolean;
  arrayedEquation?: JsonArrayedEquation;
  compat?: JsonCompat;
}

/**
 * A flow (rate) variable for JSON serialization.
 */
export interface JsonFlow {
  name: string;
  uid?: number;
  equation?: string;
  units?: string;
  nonNegative?: boolean;
  graphicalFunction?: JsonGraphicalFunction;
  documentation?: string;
  canBeModuleInput?: boolean;
  isPublic?: boolean;
  arrayedEquation?: JsonArrayedEquation;
  compat?: JsonCompat;
}

/**
 * An auxiliary (intermediate calculation) variable for JSON serialization.
 */
export interface JsonAuxiliary {
  name: string;
  uid?: number;
  equation?: string;
  units?: string;
  graphicalFunction?: JsonGraphicalFunction;
  documentation?: string;
  canBeModuleInput?: boolean;
  isPublic?: boolean;
  arrayedEquation?: JsonArrayedEquation;
  compat?: JsonCompat;
}

/**
 * A module (submodel) variable for JSON serialization.
 */
export interface JsonModule {
  name: string;
  modelName: string;
  uid?: number;
  units?: string;
  documentation?: string;
  references?: JsonModuleReference[];
  canBeModuleInput?: boolean;
  isPublic?: boolean;
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
  attachedToUid?: number;
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
  type: 'stock';
  uid: number;
  name: string;
  x: number;
  y: number;
  labelSide?: string;
}

/**
 * Visual element for a flow.
 */
export interface JsonFlowViewElement {
  type: 'flow';
  uid: number;
  name: string;
  x: number;
  y: number;
  points: JsonFlowPoint[];
  labelSide?: string;
}

/**
 * Visual element for an auxiliary variable.
 */
export interface JsonAuxiliaryViewElement {
  type: 'aux';
  uid: number;
  name: string;
  x: number;
  y: number;
  labelSide?: string;
}

/**
 * Visual element for a cloud (source/sink).
 */
export interface JsonCloudViewElement {
  type: 'cloud';
  uid: number;
  flowUid: number;
  x: number;
  y: number;
}

/**
 * Visual element for a causal link.
 */
export interface JsonLinkViewElement {
  type: 'link';
  uid: number;
  fromUid: number;
  toUid: number;
  arc?: number;
  multiPoints?: JsonLinkPoint[];
  polarity?: string;
}

/**
 * Visual element for a module.
 */
export interface JsonModuleViewElement {
  type: 'module';
  uid: number;
  name: string;
  x: number;
  y: number;
  labelSide?: string;
}

/**
 * Visual element for an alias (ghost).
 */
export interface JsonAliasViewElement {
  type: 'alias';
  uid: number;
  aliasOfUid: number;
  x: number;
  y: number;
  labelSide?: string;
}

/**
 * Visual element for a group/sector.
 */
export interface JsonGroupViewElement {
  type: 'group';
  uid: number;
  name: string;
  x: number;
  y: number;
  width: number;
  height: number;
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
  | JsonAliasViewElement
  | JsonGroupViewElement;

/**
 * A view/diagram in the model.
 */
export interface JsonView {
  elements: JsonViewElement[];
  kind?: string;
  viewBox?: JsonRect;
  zoom?: number;
  useLetteredPolarity?: boolean;
}

// Simulation specs

/**
 * Simulation specification.
 */
export interface JsonSimSpecs {
  startTime: number;
  endTime: number;
  dt?: string;
  saveStep?: number;
  method?: string;
  timeUnits?: string;
}

// Project structure types

/**
 * A dimension for subscripted variables.
 */
export interface JsonDimension {
  name: string;
  elements?: string[];
  size?: number;
  mapsTo?: string;
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
  uids: number[];
  name: string;
  deleted?: boolean;
  description?: string;
}

/**
 * Semantic/organizational group for categorizing model variables.
 * This is distinct from visual diagram groups (JsonGroupViewElement).
 */
export interface JsonModelGroup {
  name: string;
  doc?: string;
  parent?: string;
  members: string[];
  runEnabled?: boolean;
}

/**
 * Source information for imported projects.
 */
export interface JsonSource {
  extension?: 'xmile' | 'vensim';
  content?: string;
}

/**
 * A model in the project.
 */
export interface JsonModel {
  name: string;
  stocks: JsonStock[];
  flows: JsonFlow[];
  auxiliaries: JsonAuxiliary[];
  modules?: JsonModule[];
  simSpecs?: JsonSimSpecs;
  views?: JsonView[];
  loopMetadata?: JsonLoopMetadata[];
  groups?: JsonModelGroup[];
}

/**
 * A complete system dynamics project.
 */
export interface JsonProject {
  name: string;
  simSpecs: JsonSimSpecs;
  models: JsonModel[];
  dimensions?: JsonDimension[];
  units?: JsonUnit[];
  source?: JsonSource;
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
 * Payload for update stock flows operation.
 * Updates only the inflows/outflows of an existing stock, preserving all other fields.
 */
export interface UpdateStockFlowsPayload {
  ident: string;
  inflows: string[];
  outflows: string[];
}

/**
 * Payload for set sim specs operation.
 */
export interface SetSimSpecsPayload {
  simSpecs: JsonSimSpecs;
}

// Operation types with type discriminator

export interface UpsertStockOp {
  type: 'upsertStock';
  payload: UpsertStockPayload;
}

export interface UpsertFlowOp {
  type: 'upsertFlow';
  payload: UpsertFlowPayload;
}

export interface UpsertAuxOp {
  type: 'upsertAux';
  payload: UpsertAuxPayload;
}

export interface UpsertModuleOp {
  type: 'upsertModule';
  payload: UpsertModulePayload;
}

export interface DeleteVariableOp {
  type: 'deleteVariable';
  payload: DeleteVariablePayload;
}

export interface RenameVariableOp {
  type: 'renameVariable';
  payload: RenameVariablePayload;
}

export interface UpsertViewOp {
  type: 'upsertView';
  payload: UpsertViewPayload;
}

export interface DeleteViewOp {
  type: 'deleteView';
  payload: DeleteViewPayload;
}

export interface SetSimSpecsOp {
  type: 'setSimSpecs';
  payload: SetSimSpecsPayload;
}

export interface UpdateStockFlowsOp {
  type: 'updateStockFlows';
  payload: UpdateStockFlowsPayload;
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
  | DeleteViewOp
  | UpdateStockFlowsOp;

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
  projectOps?: JsonProjectOperation[];
  models?: JsonModelPatch[];
}

// Type guards for discriminating operations

export function isUpsertStock(op: JsonModelOperation): op is UpsertStockOp {
  return op.type === 'upsertStock';
}

export function isUpsertFlow(op: JsonModelOperation): op is UpsertFlowOp {
  return op.type === 'upsertFlow';
}

export function isUpsertAux(op: JsonModelOperation): op is UpsertAuxOp {
  return op.type === 'upsertAux';
}

export function isUpsertModule(op: JsonModelOperation): op is UpsertModuleOp {
  return op.type === 'upsertModule';
}

export function isDeleteVariable(op: JsonModelOperation): op is DeleteVariableOp {
  return op.type === 'deleteVariable';
}

export function isRenameVariable(op: JsonModelOperation): op is RenameVariableOp {
  return op.type === 'renameVariable';
}

export function isUpsertView(op: JsonModelOperation): op is UpsertViewOp {
  return op.type === 'upsertView';
}

export function isDeleteView(op: JsonModelOperation): op is DeleteViewOp {
  return op.type === 'deleteView';
}

export function isUpdateStockFlows(op: JsonModelOperation): op is UpdateStockFlowsOp {
  return op.type === 'updateStockFlows';
}
