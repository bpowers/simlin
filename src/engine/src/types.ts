// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * High-level TypeScript types for the Simlin API.
 *
 * These types are immutable data structures representing model elements.
 * They match the schema in doc/simlin-project.schema.json for full fidelity.
 */

// ============================================================================
// Simulation Specification Types
// ============================================================================

/**
 * Simulation specification matching the schema's SimSpecs.
 */
export interface SimSpecs {
  /** Simulation start time */
  readonly startTime: number;
  /** Simulation end time */
  readonly endTime: number;
  /** Time step for simulation (as string to support expressions like "1/4") */
  readonly dt?: string;
  /** Save step - how often to record results */
  readonly saveStep?: number;
  /** Integration method (e.g., 'euler', 'rk4') */
  readonly method?: string;
  /** Time units (if specified) */
  readonly timeUnits?: string;
}

/**
 * @deprecated Use SimSpecs instead. TimeSpec is provided for backwards compatibility.
 */
export interface TimeSpec {
  /** Simulation start time */
  readonly start: number;
  /** Simulation stop time */
  readonly stop: number;
  /** Time step for simulation */
  readonly dt: number;
  /** Time units (if specified) */
  readonly units?: string;
}

// ============================================================================
// Graphical Function Types
// ============================================================================

/**
 * Scale for graphical function axes.
 */
export interface GraphicalFunctionScale {
  /** Minimum value for axis */
  readonly min: number;
  /** Maximum value for axis */
  readonly max: number;
}

/**
 * A graphical/table function (lookup table).
 *
 * Represents a piecewise function defined by data points.
 * Used in table functions and WITH LOOKUP expressions.
 */
export interface GraphicalFunction {
  /** Points as [x, y] coordinate pairs */
  readonly points?: readonly (readonly [number, number])[];
  /** Y coordinates (function values) - alternative to points */
  readonly yPoints?: readonly number[];
  /** Interpolation: 'continuous', 'discrete', or 'extrapolate' */
  readonly kind?: string;
  /** X axis scale */
  readonly xScale?: GraphicalFunctionScale;
  /** Y axis scale */
  readonly yScale?: GraphicalFunctionScale;
}

// ============================================================================
// Arrayed Variable Types
// ============================================================================

/**
 * Vensim compatibility options for a variable.
 */
export interface Compat {
  /** Active initial equation (Vensim ACTIVE INITIAL) */
  readonly activeInitial?: string;
}

/**
 * An element-specific equation for arrayed variables.
 */
export interface ElementEquation {
  /** Subscript expression for this element (e.g., "A1" or "A1, B2") */
  readonly subscript: string;
  /** Equation for this specific element */
  readonly equation: string;
  /** Active initial equation for this element */
  readonly activeInitial?: string;
  /** Graphical function for this element */
  readonly graphicalFunction?: GraphicalFunction;
}

/**
 * Equation structure for arrayed/subscripted variables.
 */
export interface ArrayedEquation {
  /** Dimension names this variable is subscripted over */
  readonly dimensions: readonly string[];
  /** Default equation for all elements (if not overridden per element) */
  readonly equation?: string;
  /** Vensim compatibility options */
  readonly compat?: Compat;
  /** Element-specific equations that override the default */
  readonly elements?: readonly ElementEquation[];
}

// ============================================================================
// Variable Types
// ============================================================================

/**
 * A stock (level, accumulation) variable.
 *
 * Stocks represent accumulations in a system dynamics model. They integrate
 * their net flow (inflows minus outflows) over time. Stock values can only
 * change through flows.
 *
 * Immutable - modifying attributes will not change the underlying model.
 */
export interface Stock {
  /** Variable type discriminator */
  readonly type: 'stock';
  /** Unique identifier within the model */
  readonly uid?: number;
  /** Variable name */
  readonly name: string;
  /** Initial value expression */
  readonly initialEquation?: string;
  /** Units (if specified) */
  readonly units?: string;
  /** Names of flows that increase this stock */
  readonly inflows: readonly string[];
  /** Names of flows that decrease this stock */
  readonly outflows: readonly string[];
  /** Whether this stock is constrained to be non-negative */
  readonly nonNegative?: boolean;
  /** Documentation/comments */
  readonly documentation?: string;
  /** Whether this variable can receive input from a parent model when used as a module */
  readonly canBeModuleInput?: boolean;
  /** Whether this variable is publicly accessible from parent models */
  readonly isPublic?: boolean;
  /** Arrayed equation structure for subscripted variables */
  readonly arrayedEquation?: ArrayedEquation;
  /** Vensim compatibility options */
  readonly compat?: Compat;
}

/**
 * A flow (rate) variable.
 *
 * Flows represent rates of change in a system dynamics model. They determine
 * how stocks change over time. Flows are computed at each time step based on
 * their equations.
 *
 * Immutable - modifying attributes will not change the underlying model.
 */
export interface Flow {
  /** Variable type discriminator */
  readonly type: 'flow';
  /** Unique identifier within the model */
  readonly uid?: number;
  /** Variable name */
  readonly name: string;
  /** Flow rate expression */
  readonly equation?: string;
  /** Units (if specified) */
  readonly units?: string;
  /** Whether this flow is constrained to be non-negative */
  readonly nonNegative?: boolean;
  /** Graphical/table function if this uses WITH LOOKUP */
  readonly graphicalFunction?: GraphicalFunction;
  /** Documentation/comments */
  readonly documentation?: string;
  /** Whether this variable can receive input from a parent model when used as a module */
  readonly canBeModuleInput?: boolean;
  /** Whether this variable is publicly accessible from parent models */
  readonly isPublic?: boolean;
  /** Arrayed equation structure for subscripted variables */
  readonly arrayedEquation?: ArrayedEquation;
  /** Vensim compatibility options */
  readonly compat?: Compat;
}

/**
 * An auxiliary (intermediate calculation) variable.
 *
 * Auxiliary variables are computed values that help structure models and
 * make equations more readable. They don't accumulate over time like stocks,
 * but are recalculated at each time step.
 *
 * Some auxiliaries have memory (like those using DELAY or SMOOTH), in which
 * case they have an initialEquation that sets their initial state.
 *
 * Immutable - modifying attributes will not change the underlying model.
 */
export interface Aux {
  /** Variable type discriminator */
  readonly type: 'aux';
  /** Unique identifier within the model */
  readonly uid?: number;
  /** Variable name */
  readonly name: string;
  /** Equation defining this variable */
  readonly equation?: string;
  /** Units (if specified) */
  readonly units?: string;
  /** Graphical/table function if this uses WITH LOOKUP */
  readonly graphicalFunction?: GraphicalFunction;
  /** Documentation/comments */
  readonly documentation?: string;
  /** Whether this variable can receive input from a parent model when used as a module */
  readonly canBeModuleInput?: boolean;
  /** Whether this variable is publicly accessible from parent models */
  readonly isPublic?: boolean;
  /** Arrayed equation structure for subscripted variables */
  readonly arrayedEquation?: ArrayedEquation;
  /** Vensim compatibility options */
  readonly compat?: Compat;
}

/**
 * A reference mapping between module input/output and parent model variable.
 */
export interface ModuleReference {
  /** Source variable name (in the parent model) */
  readonly src: string;
  /** Destination variable name (in the module's model) */
  readonly dst: string;
}

/**
 * A module (submodel) variable.
 *
 * Modules encapsulate other models, allowing for hierarchical model structures.
 * They have references that connect parent model variables to the module's
 * internal variables.
 */
export interface Module {
  /** Variable type discriminator */
  readonly type: 'module';
  /** Unique identifier within the model */
  readonly uid?: number;
  /** Instance name of this module */
  readonly name: string;
  /** Name of the model this module instantiates */
  readonly modelName: string;
  /** Units (if specified) */
  readonly units?: string;
  /** Documentation/comments */
  readonly documentation?: string;
  /** References connecting parent variables to module inputs/outputs */
  readonly references?: readonly ModuleReference[];
  /** Whether this variable can receive input from a parent model when used as a module */
  readonly canBeModuleInput?: boolean;
  /** Whether this variable is publicly accessible from parent models */
  readonly isPublic?: boolean;
}

/**
 * Union type for all variable types.
 */
export type Variable = Stock | Flow | Aux | Module;

// ============================================================================
// View Element Types
// ============================================================================

/**
 * A point in a flow's visual representation.
 */
export interface FlowPoint {
  readonly x: number;
  readonly y: number;
  /** UID of the element this point is attached to (stock or cloud) */
  readonly attachedToUid?: number;
}

/**
 * A point in a link's visual representation.
 */
export interface LinkPoint {
  readonly x: number;
  readonly y: number;
}

/**
 * A rectangle for view bounds.
 */
export interface Rect {
  readonly x: number;
  readonly y: number;
  readonly width: number;
  readonly height: number;
}

/**
 * Visual element for a stock.
 */
export interface StockViewElement {
  readonly type: 'stock';
  readonly uid: number;
  readonly name: string;
  readonly x: number;
  readonly y: number;
  readonly labelSide?: string;
}

/**
 * Visual element for a flow.
 */
export interface FlowViewElement {
  readonly type: 'flow';
  readonly uid: number;
  readonly name: string;
  readonly x: number;
  readonly y: number;
  readonly points: readonly FlowPoint[];
  readonly labelSide?: string;
}

/**
 * Visual element for an auxiliary variable.
 */
export interface AuxViewElement {
  readonly type: 'aux';
  readonly uid: number;
  readonly name: string;
  readonly x: number;
  readonly y: number;
  readonly labelSide?: string;
}

/**
 * Visual element for a cloud (source/sink).
 */
export interface CloudViewElement {
  readonly type: 'cloud';
  readonly uid: number;
  /** UID of the flow this cloud is connected to */
  readonly flowUid: number;
  readonly x: number;
  readonly y: number;
}

/**
 * Visual element for a causal link arrow.
 */
export interface LinkViewElement {
  readonly type: 'link';
  readonly uid: number;
  /** UID of the source element */
  readonly fromUid: number;
  /** UID of the target element */
  readonly toUid: number;
  /** Arc curvature (positive = clockwise, negative = counter-clockwise) */
  readonly arc?: number;
  /** Control points for multi-segment links */
  readonly multiPoints?: readonly LinkPoint[];
}

/**
 * Visual element for a module.
 */
export interface ModuleViewElement {
  readonly type: 'module';
  readonly uid: number;
  readonly name: string;
  readonly x: number;
  readonly y: number;
  readonly labelSide?: string;
}

/**
 * Visual element for an alias (ghost/shadow variable).
 */
export interface AliasViewElement {
  readonly type: 'alias';
  readonly uid: number;
  /** UID of the element this is an alias of */
  readonly aliasOfUid: number;
  readonly x: number;
  readonly y: number;
  readonly labelSide?: string;
}

/**
 * Union type for all view element types.
 */
export type ViewElement =
  | StockViewElement
  | FlowViewElement
  | AuxViewElement
  | CloudViewElement
  | LinkViewElement
  | ModuleViewElement
  | AliasViewElement;

/**
 * A view/diagram in the model.
 */
export interface View {
  /** View elements (stocks, flows, links, etc.) */
  readonly elements: readonly ViewElement[];
  /** View kind (e.g., 'stock_flow') */
  readonly kind?: string;
  /** View bounding box for scrolling/zooming */
  readonly viewBox?: Rect;
  /** Zoom level */
  readonly zoom?: number;
}

// ============================================================================
// Model and Project Types
// ============================================================================

/**
 * Metadata for a feedback loop.
 */
export interface LoopMetadata {
  /** UIDs of variables in this loop */
  readonly uids: readonly number[];
  /** Loop name */
  readonly name: string;
  /** Whether this loop has been deleted */
  readonly deleted?: boolean;
  /** Loop description */
  readonly description?: string;
}

/**
 * A model in the project.
 */
export interface Model {
  /** Model name */
  readonly name: string;
  /** Stock variables */
  readonly stocks: readonly Stock[];
  /** Flow variables */
  readonly flows: readonly Flow[];
  /** Auxiliary variables */
  readonly auxiliaries: readonly Aux[];
  /** Module (submodel) instances */
  readonly modules?: readonly Module[];
  /** Model-specific simulation specs (overrides project-level) */
  readonly simSpecs?: SimSpecs;
  /** Visual diagrams */
  readonly views?: readonly View[];
  /** Feedback loop metadata */
  readonly loopMetadata?: readonly LoopMetadata[];
}

/**
 * A dimension for subscripted variables.
 */
export interface Dimension {
  /** Dimension name */
  readonly name: string;
  /** Element names in this dimension */
  readonly elements?: readonly string[];
  /** Size of the dimension (if elements not specified) */
  readonly size?: number;
  /** Name of another dimension this maps to */
  readonly mapsTo?: string;
}

/**
 * A unit definition.
 */
export interface Unit {
  /** Unit name */
  readonly name: string;
  /** Equation defining this unit in terms of base units */
  readonly equation?: string;
  /** Whether this unit is disabled */
  readonly disabled?: boolean;
  /** Alternative names for this unit */
  readonly aliases?: readonly string[];
}

/**
 * A complete system dynamics project.
 */
export interface Project {
  /** Project name */
  readonly name: string;
  /** Default simulation specifications */
  readonly simSpecs: SimSpecs;
  /** Models in this project */
  readonly models: readonly Model[];
  /** Dimension definitions for subscripted variables */
  readonly dimensions?: readonly Dimension[];
  /** Unit definitions */
  readonly units?: readonly Unit[];
}

// ============================================================================
// Analysis and Issue Types
// ============================================================================

/**
 * An issue found during model checking.
 */
export interface ModelIssue {
  /** Issue severity: 'error', 'warning', or 'info' */
  readonly severity: 'error' | 'warning' | 'info';
  /** Human-readable description of the issue */
  readonly message: string;
  /** Name of the variable with the issue (if applicable) */
  readonly variable?: string;
  /** Suggested fix for the issue (if available) */
  readonly suggestion?: string;
}

/**
 * A dimensional analysis issue.
 */
export interface UnitIssue {
  /** Variable name with the unit issue */
  readonly variable: string;
  /** Description of the unit issue */
  readonly message: string;
  /** Expected units for this variable */
  readonly expectedUnits?: string;
  /** Actual units computed for this variable */
  readonly actualUnits?: string;
}

/**
 * Link polarity indicating direction of causal influence.
 */
export enum LinkPolarity {
  /** Positive: input increase causes output increase */
  Positive = 0,
  /** Negative: input increase causes output decrease */
  Negative = 1,
  /** Unknown: polarity could not be determined */
  Unknown = 2,
}

/**
 * Loop polarity indicating reinforcing or balancing behavior.
 */
export enum LoopPolarity {
  /** Reinforcing: amplifies changes (positive feedback) */
  Reinforcing = 0,
  /** Balancing: counteracts changes (negative feedback) */
  Balancing = 1,
  /** Undetermined: polarity cannot be determined or changes during simulation */
  Undetermined = 2,
}

/**
 * A causal link between two variables.
 */
export interface Link {
  /** Source variable name */
  readonly from: string;
  /** Target variable name */
  readonly to: string;
  /** Polarity of the causal relationship */
  readonly polarity: LinkPolarity;
  /** LTM (Loops That Matter) scores over time, if available */
  readonly score?: Float64Array;
}

/**
 * A feedback loop in the model.
 */
export interface Loop {
  /** Loop identifier */
  readonly id: string;
  /** Variables in the loop (in order) */
  readonly variables: readonly string[];
  /** Loop polarity (reinforcing or balancing) */
  readonly polarity: LoopPolarity;
}

/**
 * A period of time where specific loops are dominant.
 */
export interface DominantPeriod {
  /** IDs of dominant loops during this period */
  readonly dominantLoops: readonly string[];
  /** Start time of this period */
  readonly startTime: number;
  /** End time of this period */
  readonly endTime: number;
}
