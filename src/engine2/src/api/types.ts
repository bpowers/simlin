// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * High-level TypeScript types for the Simlin API.
 *
 * These types are immutable data structures representing model elements.
 * They match the semantics of pysimlin types for cross-language consistency.
 */

/**
 * Time specification for simulation.
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
  /** X coordinates. If undefined, uses implicit x scale from 0 to yPoints.length-1 */
  readonly xPoints?: readonly number[];
  /** Y coordinates (function values) */
  readonly yPoints: readonly number[];
  /** X axis scale */
  readonly xScale: GraphicalFunctionScale;
  /** Y axis scale */
  readonly yScale: GraphicalFunctionScale;
  /** Interpolation: 'continuous', 'discrete', or 'extrapolate' */
  readonly kind: string;
}

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
  /** Variable name */
  readonly name: string;
  /** Initial value expression */
  readonly initialEquation: string;
  /** Names of flows that increase this stock */
  readonly inflows: readonly string[];
  /** Names of flows that decrease this stock */
  readonly outflows: readonly string[];
  /** Units (if specified) */
  readonly units?: string;
  /** Documentation/comments */
  readonly documentation?: string;
  /** Dimension names for arrayed variables (empty if scalar) */
  readonly dimensions: readonly string[];
  /** Whether this stock is constrained to be non-negative */
  readonly nonNegative: boolean;
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
  /** Variable name */
  readonly name: string;
  /** Flow rate expression */
  readonly equation: string;
  /** Units (if specified) */
  readonly units?: string;
  /** Documentation/comments */
  readonly documentation?: string;
  /** Dimension names for arrayed variables (empty if scalar) */
  readonly dimensions: readonly string[];
  /** Whether this flow is constrained to be non-negative */
  readonly nonNegative: boolean;
  /** Graphical/table function if this uses WITH LOOKUP */
  readonly graphicalFunction?: GraphicalFunction;
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
  /** Variable name */
  readonly name: string;
  /** Equation defining this variable */
  readonly equation: string;
  /** Initial value equation (for variables with memory like DELAY, SMOOTH) */
  readonly initialEquation?: string;
  /** Units (if specified) */
  readonly units?: string;
  /** Documentation/comments */
  readonly documentation?: string;
  /** Dimension names for arrayed variables (empty if scalar) */
  readonly dimensions: readonly string[];
  /** Graphical/table function if this uses WITH LOOKUP */
  readonly graphicalFunction?: GraphicalFunction;
}

/**
 * Union type for all variable types.
 */
export type Variable = Stock | Flow | Aux;

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
