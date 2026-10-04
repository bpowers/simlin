// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! `run_tests`: the validation battery, the model tests of system dynamics
//! practice (Forrester and Senge, 1980; Sterman, *Business Dynamics*, ch. 21)
//! run mechanically.
//!
//! Each test is a set of checks, and each check one run of the model with one
//! change, compared with the model's current run:
//!
//! - `units`: the engine's unit diagnostics, by id; skipped, saying so, for a
//!   model that declares no units and has no unit diagnostic, where nothing
//!   is checked.
//! - `extreme_conditions`: each targeted constant at its low extreme and at
//!   its high one, each check saying which rule chose its value
//!   ([`ExtremeRule`]). An extreme is a condition of the system, never a
//!   value outside the constant's domain or an artifact of the integration:
//!   - a constant some equation divides by has zero outside its domain, so
//!     its low extreme is a tenth of its value, as is that of a constant
//!     nothing reads as the run goes, which only sets where the run starts (a
//!     stock that starts empty is rarely a condition the system is meant to
//!     survive); any other's is zero;
//!   - a time constant's low extreme is a tenth of its value, and at least
//!     four DTs for each of its stages, in its own unit of time: below that
//!     the run shows the integration, not the system;
//!   - a date (a constant used as a point in time) is tried at the run's
//!     start and past its stop;
//!   - the high extreme is ten times the value, except a share's, which is
//!     the whole, and a fractional rate's, which is at most one over two DTs;
//!   - the caller's own `low` or `high` for a target replaces the rule.
//!
//!   A check fails when a value becomes NaN or infinite that is not so in the
//!   model's own run. It is flagged when a stock's element, or a series the
//!   model marks non-negative, goes below zero where the model's own run
//!   keeps it at or above zero (a stock with an element below zero there is a
//!   quantity with a sign, and is not judged). A check says what happened --
//!   which series, from when, how far -- and no cause. The model's own run is
//!   a check of its own: the series not a number in it, and the series the
//!   model marks non-negative below zero in it, a marking this engine does not
//!   enforce. Values that grow very large are not judged: growth is what ten
//!   times a growth rate should do.
//! - `integration_error`: one measurement, the run at half and at a quarter
//!   of its DT. The two differences give the order the runs converge at and,
//!   by Richardson extrapolation, an estimate of each stock's error at the
//!   model's DT: under [`INTEGRATION_TOLERANCE`] of its scale the check
//!   passes, up to [`INTEGRATION_FAILURE`] it is flagged for the modeler to
//!   weigh, and above it it fails, naming the time constant DT is too large
//!   for when there is one. Where the runs do not converge, or an equation
//!   reads DT, the model is discrete in time or chaotic, and the check is
//!   flagged saying which.
//! - `sensitivity`: each targeted constant at half and at double (within its
//!   extremes); a check is flagged when a recorded variable's behavior
//!   changes family materially, and the strongest responses are reported
//!   however they come out. A family is the classifier's mode with pace set
//!   aside ([`BehaviorFamily`]); a change of it needs the series to move, and
//!   the series' turns at [`MATERIAL_CHANGE`] of its range to change too, so a
//!   wiggle near one of the classifier's thresholds is no change of behavior.
//!   Checks that flag the same change of the same series are listed once.
//! - `loop_knockout`: a targeted variable held at its initial value (each
//!   element at its own): the links and loops that cuts, and how the recorded
//!   variables respond.
//! - `disturbance`: a 10% step in a targeted constant a tenth of the way into
//!   the run: how the recorded variables respond, and the loops that lead
//!   after it. A model at rest hides its loops, and this is how the battery
//!   sees its structure.
//!
//! The targeted tests' default targets are the constants that reach a stock,
//! by any read (`analysis::model_reads`): as the run goes, or as it starts,
//! where a stock starts from one. They leave out the model's unit
//! conversions: a constant whose value is one of its units (`1e6 tons/Mton`,
//! `one_year = 1 year`) changes the units a quantity is counted in, not the
//! quantity ([`Roles`]). Sensitivity and disturbance leave out dates too,
//! whose half, double and step are no conditions of the system, and a
//! disturbance the constants nothing reads as the run goes, which a step
//! after the start moves nothing of. A time constant is recognized by its
//! units where it has them, and by its role where it has none
//! (`time_constants`); each check of one says which.
//!
//! A model that does not simulate says why once, and every test is skipped;
//! so is a test that would compare nothing (a model whose stocks are all in
//! modules), saying so. A check that passed is counted and not listed, except
//! sensitivity's strongest responses. The model's own run is listed first,
//! and the others under ids (`T1`, ...) keyed by test, target and condition,
//! so the same check keeps its id when it is run again after an edit. The
//! list is shared between the tests in turn, so the checks of one test cannot
//! crowd out another's.

use std::collections::{HashMap, HashSet, VecDeque};

use serde::{Deserialize, Serialize};

#[cfg(feature = "schema")]
use schemars::JsonSchema;

use crate::ast::{BinaryOp, Expr0, IndexExpr0, Literal, UnaryOp};
use crate::builtins::UntypedBuiltinFn;
use crate::common::{Canonical, Ident};
use crate::datamodel::{self, Equation, UnitMap, Variable};
use crate::lexer::LexerType;
use crate::results::Results;

use super::behavior::{
    BehaviorMode, Damping, Direction, ModeKind, Shape, magnitude, residue_bound, shape_at,
};
use super::evidence::Evidence;
use super::experiment::{settable, value_change};
use super::loops::{CutLink, analysis_of, cut_of, leaders_after};
use super::runs::{
    self, EquationChange, Replacement, Run, RunFailure, RunPlan, RunStore, SpecsChange, ValueChange,
};
use super::series::{
    KeyedSeries, MAX_ELEMENTS, compared_scales, element_series_upto, keyed_series_upto, round,
    scale_in_run,
};
use super::variables::NotFound;
use super::{
    DiagnosticCategoryName, ResolvedModel, Session, ToolError, Workspace, names, resolve_model,
};

/// The most targets and recorded variables a call names.
pub(crate) const MAX_TARGETS: usize = 12;

/// The most constants a disturbance test steps by default, those that reach
/// the most stocks first: each is a loop analysis, which compiles under the
/// LTM overlay.
pub(crate) const MAX_DEFAULT_DISTURBANCES: usize = 3;

/// A stock's estimated integration error, as a fraction of its scale, up to
/// which the integration error test passes; above it, the check is flagged
/// for the modeler to weigh.
pub(crate) const INTEGRATION_TOLERANCE: f64 = 0.01;

/// The estimated integration error, as a fraction of a stock's scale, above
/// which the integration error test fails: DT is too large for the model.
pub(crate) const INTEGRATION_FAILURE: f64 = 0.05;

/// The least order the half- and quarter-DT runs must converge at for their
/// differences to estimate an error: each halving of DT must shrink the
/// difference by `2^0.5` at least. Euler converges at order one, so a model
/// under it is well clear of this unless its DT is far outside the range the
/// method's error is proportional to DT in.
const CONVERGING_ORDER: f64 = 0.5;

/// How many DTs each stage of a time constant must span for the run to show
/// the system rather than the integration: the low end of the rule of thumb
/// that DT be a quarter to a tenth of the smallest time constant (Sterman,
/// *Business Dynamics*, appendix A).
pub(crate) const DTS_PER_STAGE: f64 = 4.0;

/// The order of magnitude an extreme that is not zero or a whole is from the
/// constant's value: the high extreme is this many times the value, and the
/// low extreme of a time constant, or of a constant some equation divides
/// by, is the value over it.
const EXTREME_FACTOR: f64 = 10.0;

/// How many times finer than the model's DT a check that found something is
/// run again at: an extreme makes a term [`EXTREME_FACTOR`] times as fast,
/// and at a DT as many times finer the integration is as accurate as the
/// model's own run is.
const CONFIRMING_DT_DIVISOR: f64 = EXTREME_FACTOR;

/// The most extreme conditions checks one call runs again at a finer DT:
/// each is a run of its own, [`EXTREME_FACTOR`] times as long.
const MAX_CONFIRMATIONS: usize = 12;

/// How many DTs the time constant of a fractional rate at its high extreme
/// spans at least. Euler's step of a first-order drain at rate `r` is
/// `x * (1 - r * DT)`: it passes zero once `r * DT` is over one and grows
/// without bound past two. At two DTs a step takes at most half the stock,
/// so the extreme shows the drain and not Euler's step.
const DTS_PER_FASTEST_RATE: f64 = 2.0;

/// The step a disturbance adds, as a fraction of the constant's value.
pub(crate) const STEP_FRACTION: f64 = 0.1;

/// The most checks an answer lists, and of those the most that passed
/// sensitivity checks with the strongest responses.
pub(crate) const MAX_RESULTS: usize = 20;
const MAX_STRONGEST: usize = 5;

/// The most responses, problems and differences a check lists, and the most
/// names a test's note gives.
const MAX_DETAILS: usize = 3;

/// The largest change, as a fraction of a series' scale, below which a change
/// of its behavior is the classifier's boundary, not a change of behavior: a
/// series a few percent from where it was has not changed mode.
pub(crate) const MATERIAL_CHANGE: f64 = 0.05;

/// How near one a unit conversion's value, in its units' scales, must be.
const CONVERSION_TOLERANCE: f64 = 1e-6;

/// The stdlib input port a delay's, a smooth's or a trend's time is wired to
/// (`module_functions::stdlib_args`).
const TIME_PORT: &str = "delay_time";

/// The units of time a constant's units may be, besides the model's own,
/// each in seconds: a month is a twelfth and a quarter a fourth of a year of
/// 365.2425 days.
const TIME_UNITS: [(&str, f64); 11] = [
    ("nanosecond", 1e-9),
    ("microsecond", 1e-6),
    ("millisecond", 1e-3),
    ("second", 1.0),
    ("minute", 60.0),
    ("hour", 3_600.0),
    ("day", 86_400.0),
    ("week", 604_800.0),
    ("month", 2_629_746.0),
    ("quarter", 7_889_238.0),
    ("year", 31_556_952.0),
];

/// How near two units of time's known ratio a conversion between them must
/// be: calendars differ by more than rounding (a 360- or 365-day year, a
/// 30-day month, a 52-week year).
const TIME_RATIO_TOLERANCE: f64 = 0.02;

/// The scales a unit's name can carry before the unit it scales, as
/// `billion_people`, `kilogram` and `Mton` spell them: number words and SI
/// prefixes, the letter prefixes (canonical, so lowercase: `m` is mega or
/// milli) last.
const SCALES: [(&str, f64); 18] = [
    ("hundred", 1e2),
    ("thousand", 1e3),
    ("million", 1e6),
    ("billion", 1e9),
    ("trillion", 1e12),
    ("kilo", 1e3),
    ("mega", 1e6),
    ("giga", 1e9),
    ("tera", 1e12),
    ("milli", 1e-3),
    ("micro", 1e-6),
    ("nano", 1e-9),
    ("k", 1e3),
    ("m", 1e6),
    ("m", 1e-3),
    ("g", 1e9),
    ("t", 1e12),
    ("u", 1e-6),
];

/// Units that are pure numbers at a scale. `ppt` is parts per trillion or per
/// thousand.
const NUMBER_UNITS: [(&str, f64); 7] = [
    ("percent", 1e-2),
    ("pct", 1e-2),
    ("permille", 1e-3),
    ("ppm", 1e-6),
    ("ppb", 1e-9),
    ("ppt", 1e-12),
    ("ppt", 1e-3),
];

/// Number words a constant's name may state its value in: one, and each
/// that is one of a unit in [`NUMBER_UNITS`] (`hundred_percent`).
const NUMBER_WORDS: [(&str, f64); 6] = [
    ("one", 1.0),
    ("hundred", 1e2),
    ("thousand", 1e3),
    ("million", 1e6),
    ("billion", 1e9),
    ("trillion", 1e12),
];

/// Words in a dimensionless constant's name that make it a share of a whole.
const SHARE_WORDS: [&str; 5] = ["share", "fraction", "proportion", "percent", "percentage"];

/// A test of the battery.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum TestName {
    Units,
    ExtremeConditions,
    IntegrationError,
    Sensitivity,
    LoopKnockout,
    Disturbance,
}

impl TestName {
    pub const ALL: [TestName; 6] = [
        TestName::Units,
        TestName::ExtremeConditions,
        TestName::IntegrationError,
        TestName::Sensitivity,
        TestName::LoopKnockout,
        TestName::Disturbance,
    ];
}

/// How a check came out.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// The model fails the check: fix the model, or say why the check does
    /// not apply.
    Failed,
    /// The check could not run; its reason says why.
    NotRun,
    /// A result for judgment: it may be right, and a modeler should say.
    Flagged,
    /// What happened, for an explanation: a knockout's or a disturbance's.
    Observed,
    Passed,
}

impl Outcome {
    pub const ALL: [Outcome; 5] = [
        Outcome::Failed,
        Outcome::NotRun,
        Outcome::Flagged,
        Outcome::Observed,
        Outcome::Passed,
    ];
}

/// What a check changed.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Condition {
    /// A constant at its low extreme; the check's `extreme` says which.
    Low,
    /// A constant at its high extreme; the check's `extreme` says which.
    High,
    /// The model's own run, with nothing changed.
    OwnRun,
    /// A constant at half its value (a time constant no lower than its low
    /// extreme).
    Half,
    /// A constant at double its value (a share or a fractional rate no
    /// higher than its high extreme).
    Double,
    /// The run at half and at a quarter of its DT.
    FinerDt,
    /// A variable held at its initial value, each element of an arrayed one
    /// at its own.
    Held,
    /// A constant stepped up by a tenth, a tenth of the way into the run.
    Step,
}

impl Condition {
    pub const ALL: [Condition; 8] = [
        Condition::Low,
        Condition::High,
        Condition::OwnRun,
        Condition::Half,
        Condition::Double,
        Condition::FinerDt,
        Condition::Held,
        Condition::Step,
    ];
}

/// The rule that chose an extreme conditions check's value.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ExtremeRule {
    /// Low: zero, for a constant no equation divides by.
    Zero,
    /// Low: a tenth of its value, for a constant some equation divides
    /// by, whose domain zero is outside, or one nothing reads as the run
    /// goes, which only sets where the run starts.
    Tenth,
    /// Low: a tenth of a time constant's value, and at least four DTs for
    /// each of its stages (in its own unit of time).
    ShortTime,
    /// Low: the run's start, for a date.
    RunStart,
    /// High: ten times its value.
    TenTimes,
    /// High: the whole, for a share or fraction: 1, or 100 in percent.
    Whole,
    /// High: ten times a fractional rate's value, and at most one over two
    /// DTs (in its own unit of time).
    FastestRate,
    /// High: past the run's stop, for a date.
    PastStop,
    /// The value the call gave.
    Given,
}

impl ExtremeRule {
    pub const ALL: [ExtremeRule; 9] = [
        ExtremeRule::Zero,
        ExtremeRule::Tenth,
        ExtremeRule::ShortTime,
        ExtremeRule::RunStart,
        ExtremeRule::TenTimes,
        ExtremeRule::Whole,
        ExtremeRule::FastestRate,
        ExtremeRule::PastStop,
        ExtremeRule::Given,
    ];
}

/// Why the battery took a constant for a time constant, whose low extreme is
/// a short time rather than zero.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum TimeConstantEvidence {
    /// Its units are a unit of time.
    TimeUnits,
    /// It is the time of a delay, smooth or trend.
    DelayTime,
    /// It has no units, and divides a quantity a stock moves into a rate:
    /// `(goal - level) / adjustment_time`.
    DividesARate,
    /// It has no units, and its reciprocal multiplies a quantity a stock
    /// moves into a rate: `population * (1 / lifetime)`, or `population *
    /// death_fraction` with `death_fraction = 1 / lifetime`.
    ReciprocalInARate,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunTestsInput {
    /// The tests to run: all of them when absent.
    #[serde(default)]
    pub tests: Vec<TestName>,
    /// The variables the targeted tests change, in place of their defaults:
    /// extreme conditions, sensitivity and disturbance take constants (by
    /// default the constants that reach a stock, where it starts or as the
    /// run goes, less the model's unit conversions); a loop knockout holds
    /// variables at their initial values, and runs only on targets named
    /// here. At most 12.
    #[serde(default)]
    #[cfg_attr(feature = "schema", schemars(length(max = 12)))]
    pub targets: Vec<String>,
    /// The variables whose responses sensitivity, knockouts and disturbances
    /// report: the model's stocks when absent. At most 12.
    #[serde(default)]
    #[cfg_attr(feature = "schema", schemars(length(max = 12)))]
    pub record: Vec<String>,
    /// Extremes for the extreme conditions test to give a constant in place
    /// of the ones its rules choose, for a constant whose meaningful range
    /// the model's author knows. A constant named here is a target of that
    /// test whether or not `targets` names it. At most 12.
    #[serde(default)]
    #[cfg_attr(feature = "schema", schemars(length(max = 12)))]
    pub extremes: Vec<ExtremeInput>,
}

/// The extremes a call gives one constant; the one it leaves out is the
/// battery's.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ExtremeInput {
    pub variable: String,
    /// The low extreme (for an arrayed constant, every element's).
    #[serde(default)]
    pub low: Option<f64>,
    /// The high extreme (for an arrayed constant, every element's).
    #[serde(default)]
    pub high: Option<f64>,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct RunTestsOutput {
    pub revision: u64,
    /// Each test run, with how its checks came out.
    pub tests: Vec<TestSummary>,
    /// The checks worth reading, each under its id: every one that did not
    /// pass, and sensitivity's strongest; failures first.
    pub results: Vec<TestResult>,
    /// Checks worth reading that the answer leaves out, past its limit or
    /// its budget.
    #[serde(skip_serializing_if = "is_zero")]
    pub omitted: usize,
    /// Why the model's own run fails, when it does: every test that runs the
    /// model is skipped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_fails: Option<String>,
    /// What the default targets of the tests leave out, and why: the unit
    /// conversions, and the dates sensitivity and disturbance do not scale.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub left_out: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub not_found: Vec<NotFound>,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct TestSummary {
    pub test: TestName,
    pub checks: usize,
    #[serde(skip_serializing_if = "is_zero")]
    pub passed: usize,
    #[serde(skip_serializing_if = "is_zero")]
    pub failed: usize,
    #[serde(skip_serializing_if = "is_zero")]
    pub flagged: usize,
    #[serde(skip_serializing_if = "is_zero")]
    pub observed: usize,
    #[serde(skip_serializing_if = "is_zero")]
    pub not_run: usize,
    /// Why the test made no checks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped: Option<String>,
    /// What the test's outcomes rest on that its checks do not say: the
    /// checks that found something only at the model's DT, or that were not
    /// run again at a finer one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// One check: what it changed, how it came out, and what it found.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct TestResult {
    /// The session's id for this check (`T1`, ...), the same when it is run
    /// again.
    pub id: String,
    pub test: TestName,
    pub outcome: Outcome,
    /// The variable it changed, as the model names it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variable: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub condition: Option<Condition>,
    /// The rule that chose an extreme conditions check's value: if the rule
    /// does not fit the constant, the check tested the wrong extreme, and the
    /// call can give its own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extreme: Option<ExtremeRule>,
    /// Why a check at a short time took its constant for a time constant.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_constant: Option<TimeConstantEvidence>,
    /// The extreme a sensitivity check's half or double would have passed,
    /// and was held at: the value tried is not half or double the
    /// constant's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub held_at: Option<ExtremeRule>,
    /// The value it gave the variable (a scalar).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    /// When the change took effect (a disturbance).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_time: Option<f64>,
    /// The unit diagnostics, by id (units).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<String>,
    /// What went wrong in the run (extreme conditions), first first.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub problems: Vec<Problem>,
    /// The stocks (or elements) whose values depend most on DT (integration
    /// error).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub differences: Vec<Difference>,
    /// The order the runs at finer DTs converge at (integration error): about
    /// 1 for Euler and 4 for RK4 on a smooth model, and near or below 0 where
    /// refining DT does not converge.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order: Option<f64>,
    /// How recorded variables responded (sensitivity, knockout,
    /// disturbance): those the check made undefined, then those whose
    /// behavior changed materially, then the largest changes; at most three.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub responses: Vec<Response>,
    /// How many more recorded series the check made undefined or changed
    /// the behavior of materially than `responses` lists: `record` names the
    /// ones to read.
    #[serde(skip_serializing_if = "is_zero")]
    pub more_changes: usize,
    /// The other sensitivity checks that flag the same change of the same
    /// series, folded into this one.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub same_change: Vec<SameChange>,
    /// The links a knockout removed.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub cut_links: Vec<CutLink>,
    /// Loop ids: those a knockout cut, or those that led after a
    /// disturbance.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub loops: Vec<String>,
    /// Why a check did not run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// What the outcome rests on that its numbers do not show: a
    /// non-negative marking this engine does not enforce, why runs at finer
    /// DTs do not converge, the time constant DT is too large for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// A sensitivity check folded into another that flags the same change.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct SameChange {
    pub variable: String,
    pub condition: Condition,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ProblemKind {
    /// A value became NaN or infinite.
    NonFinite,
    /// A value went below zero that does not in the model's run.
    GoesNegative,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct Problem {
    pub kind: ProblemKind,
    /// The variable (or element), as the model names it.
    pub variable: String,
    /// When it first happened.
    pub time: f64,
    /// Its least value (goes negative).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    /// Whether the model marks it non-negative. The engine does not enforce
    /// the marking, so it goes negative here where a tool that enforces it
    /// would hold it at zero.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub non_negative: bool,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct Difference {
    pub variable: String,
    /// Its estimated error at the model's DT, as a fraction of its scale (the
    /// larger of its range and its largest magnitude): its largest
    /// difference from the run at half the DT, extrapolated by the order the
    /// runs converge at; where they do not converge, that difference itself.
    pub difference: f64,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct Response {
    pub variable: String,
    /// How far its final value moved from the model's run, as a fraction of
    /// its scale there (of what it is computed from, for a series the
    /// model's run holds at zero): positive up, negative down. Left out when either run
    /// of the series is not a number somewhere, which its mode says.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change: Option<f64>,
    /// Its largest difference from the model's run at any time, as a
    /// fraction of its scale there. Left out as `change` is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub largest_change: Option<f64>,
    /// Its behavior mode in the check's run.
    pub mode: ModeKind,
    /// Its behavior mode in the model's run, when the check changed it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub was: Option<ModeKind>,
    /// Whether its swings die out, hold or grow in the check's run, when it
    /// oscillates there: a loss of stability shows here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub damping: Option<Damping>,
    /// The same in the model's run, when the check changed it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub was_damping: Option<Damping>,
    /// The family of its behavior in the check's run, when the check changed
    /// it materially: its mode with pace set aside, so a label can change
    /// while the family stays (exponential growth that reads as linear over
    /// a shorter horizon, a goal seeker a few percent past its goal).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub family: Option<BehaviorFamily>,
    /// The family of its behavior in the model's run, when the check
    /// changed it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub was_family: Option<BehaviorFamily>,
    /// Whether the series is not a number somewhere in the check's run where
    /// it is a number throughout the model's: what the check did to it.
    #[serde(skip)]
    went_undefined: bool,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// The checks a session has listed, by id: what each checked, and how it last
/// came out at which revision, so a citation of one is verified without the
/// battery running again while the model is as it was.
#[derive(Default)]
pub(crate) struct CheckLog {
    checks: HashMap<String, CheckRecord>,
}

struct CheckRecord {
    key: TestKey,
    /// The recorded variables' canonical names; the stocks when empty.
    record: Vec<String>,
    /// The extremes the call gave the check's variable.
    given: GivenExtremes,
    revision: u64,
    outcome: Outcome,
}

/// The extremes a call gave a constant; the one it left out is the battery's.
#[derive(Clone, Copy, Default, PartialEq)]
struct GivenExtremes {
    low: Option<f64>,
    high: Option<f64>,
}

/// What the tests read of a model besides its run: its links, its equations,
/// and what its constants are.
struct Reading<'m> {
    graph: Graph,
    parsed: Parsed<'m>,
    roles: Roles,
}

impl<'m> Reading<'m> {
    fn of(ws: &Workspace<'_>, resolved: &ResolvedModel<'m>, base: &Run) -> Reading<'m> {
        let graph = Graph::of(ws.db, resolved);
        let units = Units::of(ws, resolved);
        let parsed = Parsed::of(resolved.model);
        let roles = Roles::of(resolved.model, &graph, &units, &parsed, base);
        Reading {
            graph,
            parsed,
            roles,
        }
    }
}

/// The outcome of the check the session listed as `id`: as it came out when
/// the model was as it is, else run again.
pub(crate) fn recheck(
    session: &mut Session,
    ws: &mut Workspace<'_>,
    resolved: &ResolvedModel<'_>,
    id: &str,
) -> Result<Outcome, String> {
    let Some(record) = session.checks.checks.get(id) else {
        return Err(format!(
            "no battery check has the id '{id}' in this session: check ids come from run_tests"
        ));
    };
    if record.revision == ws.revision {
        return Ok(record.outcome);
    }
    let (key, record_names, given) = (record.key.clone(), record.record.clone(), record.given);
    let model = resolved.model;
    let target = key
        .variable
        .as_deref()
        .map(|name| {
            model
                .get_variable(name)
                .ok_or_else(|| format!("{id} changed a variable the model no longer has, {name}"))
        })
        .transpose()?;
    let record: Vec<&Variable> = if record_names.is_empty() {
        model
            .variables
            .iter()
            .filter(|v| matches!(v, Variable::Stock(_)))
            .collect()
    } else {
        record_names
            .iter()
            .filter_map(|name| model.get_variable(name))
            .collect()
    };
    let base = session.runs.current(ws, model).map_err(|err| err.error)?;
    let targets: Vec<&Variable> = target.into_iter().collect();
    let given: HashMap<String, GivenExtremes> = key
        .variable
        .iter()
        .map(|name| (name.clone(), given))
        .collect();
    let reading = Reading::of(ws, resolved, &base);
    // A check that stopped for other work answers with the stop's words;
    // the verifier stops at the next citation, while the work still waits.
    let stopped = |err: ToolError| err.error;
    let checks = match key.test {
        TestName::Units => vec![units_check(&mut session.evidence, ws, resolved)],
        TestName::ExtremeConditions => {
            extreme_conditions(ws, model, &base, &reading, &targets, &given)
                .map_err(stopped)?
                .checks
        }
        TestName::IntegrationError => {
            integration_error(ws, model, &base, &reading).map_err(stopped)?
        }
        TestName::Sensitivity => {
            sensitivity(ws, model, &base, &reading.roles, &targets, &record).map_err(stopped)?
        }
        TestName::LoopKnockout => loop_knockout(
            &mut session.runs,
            &mut session.evidence,
            ws,
            resolved,
            &base,
            &targets,
            &record,
        )
        .map_err(stopped)?,
        TestName::Disturbance => disturbance(
            &mut session.runs,
            &mut session.evidence,
            ws,
            resolved,
            &base,
            &reading.roles,
            &targets,
            &record,
        )
        .map_err(stopped)?,
    };
    let outcome = checks
        .into_iter()
        .find(|check| check.key == key)
        .map(|check| check.result.outcome)
        .ok_or_else(|| format!("{id} no longer applies to the model as it is"))?;
    if let Some(record) = session.checks.checks.get_mut(id) {
        record.revision = ws.revision;
        record.outcome = outcome;
    }
    Ok(outcome)
}

/// What identifies a check across calls and revisions.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct TestKey {
    test: TestName,
    variable: Option<String>,
    condition: Option<Condition>,
}

/// A check before the session names it.
struct Check {
    key: TestKey,
    result: TestResult,
    /// How strong its responses were, for ranking sensitivity's.
    strength: f64,
}

impl Check {
    fn new(test: TestName, variable: Option<&Variable>, condition: Option<Condition>) -> Check {
        Check {
            key: TestKey {
                test,
                variable: variable.map(|v| crate::canonicalize(v.get_ident()).into_owned()),
                condition,
            },
            result: TestResult {
                id: String::new(),
                test,
                outcome: Outcome::Passed,
                variable: variable.map(|v| v.get_ident().to_string()),
                condition,
                extreme: None,
                time_constant: None,
                held_at: None,
                value: None,
                from_time: None,
                diagnostics: vec![],
                problems: vec![],
                differences: vec![],
                order: None,
                responses: vec![],
                more_changes: 0,
                same_change: vec![],
                cut_links: vec![],
                loops: vec![],
                reason: None,
                note: None,
            },
            strength: 0.0,
        }
    }

    fn not_run(mut self, reason: impl Into<String>) -> Check {
        self.result.outcome = Outcome::NotRun;
        self.result.reason = Some(reason.into());
        self
    }
}

/// A test's checks, and what the test says of what it left out.
struct Made {
    checks: Vec<Check>,
    note: Option<String>,
}

impl From<Vec<Check>> for Made {
    fn from(checks: Vec<Check>) -> Made {
        Made { checks, note: None }
    }
}

/// The model's variables `names` name, in order; a name the model lacks goes
/// to `not_found` with what it may have meant.
fn resolve_all<'m>(
    model: &'m datamodel::Model,
    names: &[String],
    not_found: &mut Vec<NotFound>,
) -> Vec<&'m Variable> {
    names
        .iter()
        .filter_map(|name| match names::resolve(model, name) {
            Ok(var) => Some(var),
            Err(suggestions) => {
                not_found.push(NotFound {
                    name: name.clone(),
                    suggestions,
                    reason: None,
                });
                None
            }
        })
        .collect()
}

/// Whether the model has stocks of its own, which the tests that compare
/// runs read by default.
fn has_stocks(model: &datamodel::Model) -> bool {
    model
        .variables
        .iter()
        .any(|v| matches!(v, Variable::Stock(_)))
}

/// Why a test that compares the model's stocks between runs compares none:
/// they are inside modules, which the battery does not read yet, or the
/// model has none.
fn nothing_read(model: &datamodel::Model) -> String {
    if model
        .variables
        .iter()
        .any(|v| matches!(v, Variable::Module(_)))
    {
        "the model's stocks are inside modules, which this test does not read yet, so it \
         would compare nothing"
            .to_string()
    } else {
        "the model has no stocks for this test to compare".to_string()
    }
}

/// Whether the model declares units on any variable. Unit checking is opt-in
/// by declaring them (`db::units::check_model_units`), so a model that
/// declares none has no unit diagnostics from its equations, whatever they
/// are; a broken unit definition is the project's and is reported anyway.
fn declares_units(model: &datamodel::Model) -> bool {
    model.variables.iter().any(|var| {
        var.get_units()
            .is_some_and(|units| !units.trim().is_empty())
    })
}

pub(crate) fn run_tests(
    session: &mut Session,
    ws: &mut Workspace<'_>,
    input: RunTestsInput,
) -> Result<RunTestsOutput, ToolError> {
    for (field, count) in [
        ("targets", input.targets.len()),
        ("record", input.record.len()),
        ("extremes", input.extremes.len()),
    ] {
        if count > MAX_TARGETS {
            return Err(ToolError::new(format!(
                "{field} names at most {MAX_TARGETS} variables (this call names {count})"
            )));
        }
    }
    for extreme in &input.extremes {
        let name = &extreme.variable;
        if extreme.low.is_none() && extreme.high.is_none() {
            return Err(ToolError::new(format!(
                "extremes gives '{name}' neither a low nor a high: give one or both, or name it \
                 in targets for the battery's own"
            )));
        }
        if [extreme.low, extreme.high]
            .iter()
            .flatten()
            .any(|value| !value.is_finite())
        {
            return Err(ToolError::new(format!(
                "the extremes of '{name}' must be numbers a run can hold"
            )));
        }
        if let (Some(low), Some(high)) = (extreme.low, extreme.high)
            && low > high
        {
            return Err(ToolError::new(format!(
                "the low extreme of '{name}', {low}, is above its high one, {high}"
            )));
        }
    }
    let resolved = resolve_model(ws.project, ws.db, &session.model_name)?;
    let model = resolved.model;
    let mut not_found = Vec::new();
    let targets = resolve_all(model, &input.targets, &mut not_found);
    let record = if input.record.is_empty() {
        model
            .variables
            .iter()
            .filter(|v| matches!(v, Variable::Stock(_)))
            .collect()
    } else {
        resolve_all(model, &input.record, &mut not_found)
    };
    // The constants the call gives extremes are targets of the extreme
    // conditions test beside the ones it names.
    let mut given: HashMap<String, GivenExtremes> = HashMap::new();
    let mut extreme_targets = targets.clone();
    for extreme in &input.extremes {
        let named = resolve_all(
            model,
            std::slice::from_ref(&extreme.variable),
            &mut not_found,
        );
        for var in named {
            let name = var.get_ident();
            let canonical = crate::canonicalize(name).into_owned();
            if given.contains_key(&canonical) {
                return Err(ToolError::new(format!(
                    "extremes names '{name}' twice: give its low and high in one entry"
                )));
            }
            let entry = given.entry(canonical).or_insert(GivenExtremes {
                low: extreme.low,
                high: extreme.high,
            });
            if let (Some(low), Some(high)) = (entry.low, entry.high)
                && low > high
            {
                return Err(ToolError::new(format!(
                    "the low extreme of '{name}', {low}, is above its high one, {high}"
                )));
            }
            if !extreme_targets
                .iter()
                .any(|target| std::ptr::eq(*target, var))
            {
                extreme_targets.push(var);
            }
        }
    }
    let tests: Vec<TestName> = TestName::ALL
        .into_iter()
        .filter(|t| input.tests.is_empty() || input.tests.contains(t))
        .collect();

    let current = match session.runs.current(ws, model) {
        Err(err) if err.is_interrupted() => return Err(err),
        current => current
            .map(|base| {
                let reading = Reading::of(ws, &resolved, &base);
                (base, reading)
            })
            .map_err(|err| err.error),
    };
    let mut summaries = Vec::new();
    let mut checks: Vec<Check> = Vec::new();
    let mut left_out = LeftOut::default();
    for test in tests {
        let made: Result<Made, String> = match (test, &current) {
            // A unit definition is the project's: one that is broken is
            // an error whether or not a variable declares units.
            (TestName::Units, _) => {
                let check = units_check(&mut session.evidence, ws, &resolved);
                if check.result.diagnostics.is_empty() && !declares_units(model) {
                    Err(
                        "the model declares no units, so there is nothing to check: units are \
                         checked once variables declare them"
                            .to_string(),
                    )
                } else {
                    Ok(vec![check].into())
                }
            }
            // Said once, as the answer's `runFails`.
            (_, Err(_)) => Err("the model does not simulate (runFails says why)".to_string()),
            // A test that would compare nothing says so, rather than pass.
            (TestName::ExtremeConditions | TestName::IntegrationError, Ok(_))
                if !has_stocks(model) =>
            {
                Err(nothing_read(model))
            }
            (TestName::Sensitivity | TestName::LoopKnockout | TestName::Disturbance, Ok(_))
                if record.is_empty() =>
            {
                Err(nothing_read(model))
            }
            (TestName::ExtremeConditions, Ok((base, reading))) => {
                let targets = targets_or(&extreme_targets, &mut left_out, || {
                    reading
                        .roles
                        .defaults(model, &reading.graph, Targeted::Extremes)
                });
                Ok(extreme_conditions(
                    ws, model, base, reading, &targets, &given,
                )?)
            }
            (TestName::IntegrationError, Ok((base, reading))) => {
                Ok(integration_error(ws, model, base, reading)?.into())
            }
            (TestName::Sensitivity, Ok((base, reading))) => {
                let targets = targets_or(&targets, &mut left_out, || {
                    reading
                        .roles
                        .defaults(model, &reading.graph, Targeted::Changes)
                });
                Ok(sensitivity(ws, model, base, &reading.roles, &targets, &record)?.into())
            }
            (TestName::LoopKnockout, Ok(_)) if targets.is_empty() => {
                Err("name the variables to hold at their initial values in targets".to_string())
            }
            (TestName::LoopKnockout, Ok((base, _))) => Ok(loop_knockout(
                &mut session.runs,
                &mut session.evidence,
                ws,
                &resolved,
                base,
                &targets,
                &record,
            )?
            .into()),
            (TestName::Disturbance, Ok((base, reading))) => {
                let targets = targets_or(&targets, &mut left_out, || {
                    let (defaults, left_out) =
                        reading
                            .roles
                            .defaults(model, &reading.graph, Targeted::Changes);
                    // A step a tenth of the way into the run is past every
                    // start, and moves nothing a constant only starts.
                    let defaults = defaults
                        .into_iter()
                        .filter(|var| {
                            !is_zero_valued(base, model, var) && !reading.roles.only_starts(var)
                        })
                        .take(MAX_DEFAULT_DISTURBANCES)
                        .collect();
                    (defaults, left_out)
                });
                Ok(disturbance(
                    &mut session.runs,
                    &mut session.evidence,
                    ws,
                    &resolved,
                    base,
                    &reading.roles,
                    &targets,
                    &record,
                )?
                .into())
            }
        };
        let mut summary = TestSummary {
            test,
            checks: 0,
            passed: 0,
            failed: 0,
            flagged: 0,
            observed: 0,
            not_run: 0,
            skipped: None,
            note: None,
        };
        match made {
            Ok(made) => {
                summary.checks = made.checks.len();
                summary.note = made.note;
                for check in &made.checks {
                    *match check.result.outcome {
                        Outcome::Passed => &mut summary.passed,
                        Outcome::Failed => &mut summary.failed,
                        Outcome::Flagged => &mut summary.flagged,
                        Outcome::Observed => &mut summary.observed,
                        Outcome::NotRun => &mut summary.not_run,
                    } += 1;
                }
                if made.checks.is_empty() {
                    summary.skipped = Some("the model has nothing this test changes".to_string());
                }
                checks.extend(made.checks);
            }
            Err(reason) => summary.skipped = Some(reason),
        }
        summaries.push(summary);
    }

    let (listed, omitted) = listed(same_changes_folded(checks));
    let output = fitted(
        &mut session.evidence,
        RunTestsOutput {
            revision: ws.revision,
            tests: summaries,
            results: vec![],
            omitted,
            run_fails: current.as_ref().err().cloned(),
            left_out: left_out.note(),
            not_found,
        },
        &listed,
        session.outline_budget,
    );
    let record_names: Vec<String> = if input.record.is_empty() {
        vec![]
    } else {
        record
            .iter()
            .map(|v| crate::canonicalize(v.get_ident()).into_owned())
            .collect()
    };
    for (result, check) in output.results.iter().zip(&listed) {
        let given = check
            .key
            .variable
            .as_ref()
            .filter(|_| check.key.test == TestName::ExtremeConditions)
            .and_then(|name| given.get(name))
            .copied()
            .unwrap_or_default();
        session.checks.checks.insert(
            result.id.clone(),
            CheckRecord {
                key: check.key.clone(),
                record: record_names.clone(),
                given,
                revision: ws.revision,
                outcome: result.outcome,
            },
        );
    }
    Ok(output)
}

/// `targets`, or when the call names none `default()`, with what it left
/// out added to `left_out`.
fn targets_or<'a>(
    targets: &[&'a Variable],
    left_out: &mut LeftOut<'a>,
    default: impl FnOnce() -> (Vec<&'a Variable>, LeftOut<'a>),
) -> Vec<&'a Variable> {
    if !targets.is_empty() {
        return targets.to_vec();
    }
    let (defaults, left) = default();
    left_out.add(left);
    defaults
}

/// What the default targets of the tests a call ran leave out: the unit
/// conversions, and the dates of the tests that change a constant by a
/// factor. It is said once in an answer, whichever tests left each out.
#[derive(Default)]
struct LeftOut<'m> {
    conversions: Vec<&'m Variable>,
    dates: Vec<&'m Variable>,
}

impl<'m> LeftOut<'m> {
    fn add(&mut self, other: LeftOut<'m>) {
        for (into, from) in [
            (&mut self.conversions, other.conversions),
            (&mut self.dates, other.dates),
        ] {
            for var in from {
                if !into.iter().any(|known| std::ptr::eq(*known, var)) {
                    into.push(var);
                }
            }
        }
    }

    /// The answer's note of what was left out and why, if anything was.
    fn note(&self) -> Option<String> {
        let named = |vars: &[&Variable]| {
            let names: Vec<&str> = vars
                .iter()
                .take(MAX_DETAILS)
                .map(|var| var.get_ident())
                .collect();
            let more = vars.len().saturating_sub(MAX_DETAILS);
            if more > 0 {
                format!("{} and {more} more", names.join(", "))
            } else {
                names.join(", ")
            }
        };
        let plural = |n: usize| if n == 1 { "" } else { "s" };
        let mut notes: Vec<String> = Vec::new();
        if !self.conversions.is_empty() {
            let n = self.conversions.len();
            notes.push(format!(
                "The default targets leave out {n} unit conversion{} ({}): a constant whose \
                 value is one of its units changes the units a quantity is counted in, not the \
                 quantity. Name one in targets to test it anyway.",
                plural(n),
                named(&self.conversions),
            ));
        }
        if !self.dates.is_empty() {
            let n = self.dates.len();
            notes.push(format!(
                "Sensitivity and disturbance leave out {n} date{} ({}): a multiple of a point \
                 in time is no condition of the system. Extreme conditions tries each at the \
                 run's start and past its stop; run_experiment moves one.",
                plural(n),
                named(&self.dates),
            ));
        }
        (!notes.is_empty()).then(|| notes.join(" "))
    }
}

/// The checks an answer lists, in order, and how many past its limit it
/// leaves out: the checks that did not pass, then sensitivity's strongest.
///
/// The list is shared between the tests in turn -- each test's checks in the
/// order they are worth reading, failures first and the strongest responses
/// first among equals -- so one test with many findings leaves room for every
/// other test's. What is taken is then put failures first, by test.
fn listed(checks: Vec<Check>) -> (Vec<Check>, usize) {
    let (worth, passed): (Vec<Check>, Vec<Check>) = checks
        .into_iter()
        .partition(|c| c.result.outcome != Outcome::Passed);
    let total = worth.len();
    // What the model's own run shows is the first thing a reader hears.
    let (mut taken, worth): (Vec<Check>, Vec<Check>) = worth
        .into_iter()
        .partition(|c| c.key.condition == Some(Condition::OwnRun));
    let test_order = |test: TestName| TestName::ALL.iter().position(|&t| t == test);
    let mut by_test: Vec<VecDeque<Check>> = TestName::ALL.iter().map(|_| VecDeque::new()).collect();
    for check in worth {
        if let Some(index) = test_order(check.key.test) {
            by_test[index].push_back(check);
        }
    }
    for queue in by_test.iter_mut() {
        // A stable sort keeps the rest in target order.
        queue.make_contiguous().sort_by(|a, b| {
            a.result
                .outcome
                .cmp(&b.result.outcome)
                .then(b.strength.total_cmp(&a.strength))
        });
    }
    while taken.len() < MAX_RESULTS && by_test.iter().any(|queue| !queue.is_empty()) {
        for queue in by_test.iter_mut() {
            if taken.len() == MAX_RESULTS {
                break;
            }
            taken.extend(queue.pop_front());
        }
    }
    // Stable, so each test's checks keep the order they were taken in.
    let own = |c: &Check| c.key.condition == Some(Condition::OwnRun);
    taken.sort_by(|a, b| {
        own(b)
            .cmp(&own(a))
            .then(a.result.outcome.cmp(&b.result.outcome))
            .then(test_order(a.key.test).cmp(&test_order(b.key.test)))
    });
    let mut strongest: Vec<Check> = passed
        .into_iter()
        .filter(|c| c.key.test == TestName::Sensitivity && c.strength > 0.0)
        .collect();
    strongest.sort_by(|a, b| b.strength.total_cmp(&a.strength));
    strongest.truncate(MAX_STRONGEST);
    let room = MAX_RESULTS - taken.len();
    let omitted = (total - taken.len()) + strongest.len().saturating_sub(room);
    strongest.truncate(room);
    taken.extend(strongest);
    (taken, omitted)
}

/// `checks` with each flagged sensitivity check whose first changed series
/// changes as a stronger one's does -- the same series into the same family
/// (the family it changes from is the model's run's) -- folded into that one
/// (`same_change`): one change of behavior is one finding, whichever
/// constants make it.
fn same_changes_folded(checks: Vec<Check>) -> Vec<Check> {
    let change_of = |check: &Check| {
        (check.key.test == TestName::Sensitivity)
            .then(|| {
                check
                    .result
                    .responses
                    .iter()
                    .find(|r| material(r))
                    .map(|r| (r.variable.clone(), r.family))
            })
            .flatten()
    };
    let mut order: Vec<usize> = (0..checks.len()).collect();
    order.sort_by(|&a, &b| checks[b].strength.total_cmp(&checks[a].strength));
    let mut first: Vec<(usize, (String, Option<BehaviorFamily>))> = Vec::new();
    let mut into: Vec<Option<usize>> = vec![None; checks.len()];
    for i in order {
        let Some(change) = change_of(&checks[i]) else {
            continue;
        };
        match first.iter().find(|(_, seen)| *seen == change) {
            Some(&(j, _)) => into[i] = Some(j),
            None => first.push((i, change)),
        }
    }
    let mut folded: Vec<(usize, SameChange)> = Vec::new();
    for (i, check) in checks.iter().enumerate() {
        if let (Some(j), Some(variable), Some(condition)) =
            (into[i], &check.result.variable, check.result.condition)
        {
            folded.push((
                j,
                SameChange {
                    variable: variable.clone(),
                    condition,
                },
            ));
        }
    }
    let mut kept: Vec<Check> = Vec::new();
    for (i, mut check) in checks.into_iter().enumerate() {
        if into[i].is_some() {
            continue;
        }
        check.result.same_change = folded
            .iter()
            .filter(|(j, _)| *j == i)
            .map(|(_, same)| same.clone())
            .collect();
        kept.push(check);
    }
    kept
}

/// `output` with as many of `listed`, in order, as fit `budget` bytes, each
/// named by `evidence`; the rest counted as omitted. The fit runs on a copy of
/// the evidence, so a check left out gets no id.
fn fitted(
    evidence: &mut Evidence,
    mut output: RunTestsOutput,
    listed: &[Check],
    budget: usize,
) -> RunTestsOutput {
    let mut trial = evidence.clone();
    output.results = listed
        .iter()
        .map(|check| TestResult {
            id: trial.test_id(&check.key),
            ..check.result.clone()
        })
        .collect();
    while output.results.len() > 1
        && serde_json::to_string(&output).map_or(0, |json| json.len()) > budget
    {
        output.results.pop();
        output.omitted += 1;
    }
    for (result, check) in output.results.iter_mut().zip(listed) {
        result.id = evidence.test_id(&check.key);
    }
    output
}

/// What reads each of a model's variables, by canonical ident
/// (`analysis::model_reads`, the one owner of it): the reads made as the run
/// goes and those made only as it starts.
pub(crate) struct Graph {
    readers: HashMap<String, Vec<Read>>,
    stocks: HashSet<String>,
}

/// One read of a variable: its reader, and whether every read of the pair is
/// made only as the model starts (a stock's initial value, `INIT`).
struct Read {
    reader: String,
    start_only: bool,
}

impl Graph {
    fn of(db: &crate::db::SimlinDb, resolved: &ResolvedModel<'_>) -> Graph {
        let mut readers: HashMap<String, Vec<Read>> = HashMap::new();
        for read in crate::analysis::model_reads(db, resolved.source_model, resolved.source_project)
        {
            readers.entry(read.from).or_default().push(Read {
                reader: read.to,
                start_only: read.start_only,
            });
        }
        let stocks = resolved
            .model
            .variables
            .iter()
            .filter(|v| matches!(v, Variable::Stock(_)))
            .map(|v| crate::canonicalize(v.get_ident()).into_owned())
            .collect();
        Graph { readers, stocks }
    }

    /// Every variable `from` reaches, itself excluded: through every read,
    /// or only through those made as the run goes.
    fn reach(&self, from: &str, as_it_runs: bool) -> HashSet<String> {
        let mut seen: HashSet<String> = HashSet::new();
        let mut queue: VecDeque<&str> = VecDeque::from([from]);
        while let Some(node) = queue.pop_front() {
            for read in self.readers.get(node).into_iter().flatten() {
                if !(as_it_runs && read.start_only) && seen.insert(read.reader.clone()) {
                    queue.push_back(&read.reader);
                }
            }
        }
        seen.remove(from);
        seen
    }

    /// Every variable `from` reaches through any read: what the model does
    /// depends on it, from the start or as the run goes.
    fn downstream(&self, from: &str) -> HashSet<String> {
        self.reach(from, false)
    }

    /// Every variable `from` moves as the run goes: a quantity frozen from it
    /// at the start (`INIT(level)`, a stock started from it) is no part of it.
    fn moved_by(&self, from: &str) -> HashSet<String> {
        self.reach(from, true)
    }

    /// Whether `from` moves no stock as the run goes: whatever it does, it
    /// does where the run starts. A flow it moves moves its stock.
    fn only_starts(&self, from: &str) -> bool {
        !self
            .moved_by(from)
            .iter()
            .any(|name| self.stocks.contains(name))
    }

    /// The model's constants that reach a stock, where it starts or as the
    /// run goes, those that reach the most stocks first (then by name).
    fn reaching_stocks<'m>(&self, model: &'m datamodel::Model) -> Vec<&'m Variable> {
        let mut constants: Vec<(usize, &Variable)> = model
            .variables
            .iter()
            .filter(|v| settable(v).is_ok())
            .filter_map(|v| {
                let stocks = self
                    .downstream(&crate::canonicalize(v.get_ident()))
                    .iter()
                    .filter(|name| self.stocks.contains(*name))
                    .count();
                (stocks > 0).then_some((stocks, v))
            })
            .collect();
        constants.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.get_ident().cmp(b.1.get_ident())));
        constants.into_iter().map(|(_, v)| v).collect()
    }
}

/// The units of a model's variables, as the project's unit definitions read
/// them.
struct Units {
    ctx: crate::units::Context,
    /// Each unit of time a constant's units may be: the model's own time
    /// units, and the units of time the engine knows.
    time: Vec<UnitMap>,
    /// The model's own unit of time, as the unit pass reads it
    /// (`units_check::model_time_units`): the one DT is in.
    model_time: UnitMap,
    /// The units of time the engine knows, each with its length in seconds.
    seconds: Vec<(UnitMap, f64)>,
    /// The model's variables, by canonical name, whose equations carry a
    /// unit warning: their units do not balance.
    warned: HashSet<String>,
}

impl Units {
    fn of(ws: &Workspace<'_>, resolved: &ResolvedModel<'_>) -> Units {
        let ctx = crate::db::project_units_context(ws.db, resolved.source_project).clone();
        let seconds: Vec<(UnitMap, f64)> = TIME_UNITS
            .iter()
            .map(|&(unit, seconds)| (ctx.resolve_name(unit), seconds))
            .collect();
        let mut time: Vec<UnitMap> = seconds.iter().map(|(unit, _)| unit.clone()).collect();
        let model_time = crate::units_check::model_time_units(&ctx);
        // A clock declared dimensionless is no unit a constant can be in.
        if !model_time.is_empty() && !time.contains(&model_time) {
            time.push(model_time.clone());
        }
        let model_name = crate::canonicalize(&resolved.model.name).into_owned();
        let warned = crate::db::collect_all_diagnostics(
            ws.db,
            resolved.source_project,
            crate::db::LtmOverlay::Off,
        )
        .into_iter()
        .filter(|d| {
            crate::canonicalize(&d.model) == model_name
                && matches!(
                    d.category(),
                    crate::db::DiagnosticCategory::UnitConsistency
                        | crate::db::DiagnosticCategory::UnitInference
                )
        })
        .filter_map(|d| d.owner.or(d.variable))
        .map(|name| crate::canonicalize(&name).into_owned())
        .collect();
        Units {
            ctx,
            time,
            model_time,
            seconds,
            warned,
        }
    }

    /// How many of `units`, a unit of time, one of the model's units of time
    /// is: what DT is multiplied by to be in `units`. One for the model's
    /// own unit; the ratio of the two lengths where the engine knows both;
    /// `None` otherwise (a constant in days in a model whose clock has a unit
    /// of its own).
    fn per_model_time(&self, units: &UnitMap) -> Option<f64> {
        if *units == self.model_time {
            return Some(1.0);
        }
        let seconds = |map: &UnitMap| {
            self.seconds
                .iter()
                .find(|(unit, _)| unit == map)
                .map(|&(_, seconds)| seconds)
        };
        Some(seconds(&self.model_time)? / seconds(units)?)
    }

    /// The unit of time `units` is one over, when it is: a fractional rate's
    /// (`1/year`).
    fn rate_time(&self, units: &UnitMap) -> Option<UnitMap> {
        let time = units.clone().reciprocal();
        self.is_time(&time).then_some(time)
    }

    /// Whether the equation of the variable named `canonical` carries a unit
    /// warning.
    fn warned(&self, canonical: &str) -> bool {
        self.warned.contains(canonical)
    }

    /// A variable's units, when it declares units that parse.
    fn of_variable(&self, var: &Variable) -> Option<UnitMap> {
        let units = var.get_units()?;
        if units.trim().is_empty() {
            return None;
        }
        crate::units::parse_units(&self.ctx, Some(units))
            .ok()
            .flatten()
    }

    fn is_time(&self, units: &UnitMap) -> bool {
        self.time.contains(units)
    }

    /// How many seconds the unit named `name` is, when it is a unit of time
    /// the engine knows, or a scale of one.
    fn seconds(&self, name: &str) -> Option<f64> {
        self.readings(name).into_iter().find_map(|(scale, map)| {
            self.seconds
                .iter()
                .find(|(unit, _)| *unit == map)
                .map(|&(_, seconds)| scale * seconds)
        })
    }

    /// The ways a unit's name can be read: itself, or a scale of another
    /// unit (`mton` a million or a thousandth of `ton`, `billion_people` a
    /// billion of `people`), or a pure number at a scale (`percent`), each
    /// with a trailing `s` or without.
    fn readings(&self, name: &str) -> Vec<(f64, UnitMap)> {
        let resolve = |text: &str| -> Vec<UnitMap> {
            let mut maps = vec![self.ctx.resolve_name(text)];
            if let Some(singular) = text.strip_suffix('s')
                && !singular.is_empty()
            {
                maps.push(self.ctx.resolve_name(singular));
            }
            maps
        };
        let mut readings: Vec<(f64, UnitMap)> =
            resolve(name).into_iter().map(|map| (1.0, map)).collect();
        for &(unit, scale) in &NUMBER_UNITS {
            if name == unit {
                readings.push((scale, UnitMap::new()));
            }
        }
        for &(prefix, scale) in &SCALES {
            if let Some(rest) = name.strip_prefix(prefix) {
                let rest = rest.trim_start_matches('_');
                if rest.len() > 1 {
                    readings.extend(resolve(rest).into_iter().map(|map| (scale, map)));
                }
            }
        }
        readings
    }
}

/// Each variable's parsed equations (one per element for per-element
/// equations), by canonical ident, with the algebra a role reads through
/// undone ([`normalized`]).
struct Parsed<'m> {
    equations: HashMap<String, (&'m Variable, Vec<Expr0>)>,
}

impl<'m> Parsed<'m> {
    fn of(model: &'m datamodel::Model) -> Parsed<'m> {
        let mut equations = HashMap::new();
        for var in &model.variables {
            let texts: Vec<&str> = match var.get_equation() {
                Some(Equation::Scalar(text)) | Some(Equation::ApplyToAll(_, text)) => vec![text],
                Some(Equation::Arrayed(_, elements, default, _)) => elements
                    .iter()
                    .map(|(_, text, _, _)| text.as_str())
                    .chain(default.as_deref())
                    .collect(),
                None => vec![],
            };
            let parsed: Vec<Expr0> = texts
                .into_iter()
                .filter_map(|text| Expr0::new(text, LexerType::Equation).ok().flatten())
                .map(|expr| normalized(&expr))
                .collect();
            equations.insert(
                crate::canonicalize(var.get_ident()).into_owned(),
                (var, parsed),
            );
        }
        Parsed { equations }
    }
}

/// `expr` with the algebra that hides a role undone, so a role is judged by
/// what an equation computes rather than how it is spelled: a power of -1 is
/// a division, and multiplying or dividing by one, or raising to it, is
/// nothing. `x * c ^ -1` and `x / c / 1` read as `x / c`.
fn normalized(expr: &Expr0) -> Expr0 {
    let one = |expr: &Expr0| number(expr) == Some(1.0);
    match expr {
        Expr0::Op2(op, l, r, loc) => {
            let (l, r) = (normalized(l), normalized(r));
            let reciprocal = |expr: &Expr0| match expr {
                Expr0::Op2(BinaryOp::Div, numerator, divisor, _) if one(numerator) => {
                    Some(divisor.as_ref().clone())
                }
                _ => None,
            };
            match op {
                BinaryOp::Exp if number(&r) == Some(-1.0) => Expr0::Op2(
                    BinaryOp::Div,
                    Box::new(Expr0::Const("1".to_string(), Literal::new(1.0), *loc)),
                    Box::new(l),
                    *loc,
                ),
                BinaryOp::Exp | BinaryOp::Div if one(&r) => l,
                BinaryOp::Mul if one(&l) => r,
                BinaryOp::Mul if one(&r) => l,
                BinaryOp::Mul => match (reciprocal(&l), reciprocal(&r)) {
                    (_, Some(divisor)) => {
                        Expr0::Op2(BinaryOp::Div, Box::new(l), Box::new(divisor), *loc)
                    }
                    (Some(divisor), None) => {
                        Expr0::Op2(BinaryOp::Div, Box::new(r), Box::new(divisor), *loc)
                    }
                    (None, None) => Expr0::Op2(BinaryOp::Mul, Box::new(l), Box::new(r), *loc),
                },
                op => Expr0::Op2(*op, Box::new(l), Box::new(r), *loc),
            }
        }
        Expr0::Op1(op, inner, loc) => Expr0::Op1(*op, Box::new(normalized(inner)), *loc),
        Expr0::App(UntypedBuiltinFn(name, args), loc) => Expr0::App(
            UntypedBuiltinFn(name.clone(), args.iter().map(normalized).collect()),
            *loc,
        ),
        Expr0::If(c, t, f, loc) => Expr0::If(
            Box::new(normalized(c)),
            Box::new(normalized(t)),
            Box::new(normalized(f)),
            *loc,
        ),
        other => other.clone(),
    }
}

/// The number `expr` is, signed or not.
fn number(expr: &Expr0) -> Option<f64> {
    match expr {
        Expr0::Const(_, literal, _) => Some(literal.value()),
        Expr0::Op1(UnaryOp::Negative, inner, _) => number(inner).map(|n| -n),
        Expr0::Op1(UnaryOp::Positive, inner, _) => number(inner),
        _ => None,
    }
}

/// A time constant: why the battery takes it for one, how many stages it is
/// spread over, and its unit of time against the model's.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq)]
struct TimeConstant {
    evidence: TimeConstantEvidence,
    /// The order of the delay it is the time of: each stage takes that share
    /// of it, so its low extreme is that many times a single stage's.
    stages: f64,
    /// How many of its unit of time one of the model's is ([`Units::per_model_time`]):
    /// what DT is multiplied by to be in the constant's unit. `None` when
    /// the two units' lengths are not both known, and DT cannot be put in
    /// its unit.
    per_model_time: Option<f64>,
    /// Whether it sets how fast a stock moves: a delay's time, or a divisor
    /// of a rate, whatever its units. A constant in units of time that is
    /// neither (a capital-output ratio, an effort in hours) is a time
    /// constant for its extremes, and no time DT has to be short beside.
    paces_a_stock: bool,
}

impl TimeConstant {
    /// The least value the run shows the system at rather than the
    /// integration: [`DTS_PER_STAGE`] DTs for each stage, in the constant's
    /// unit; zero where DT cannot be put in that unit.
    fn floor(&self, dt: f64) -> f64 {
        self.per_model_time
            .map_or(0.0, |per| DTS_PER_STAGE * self.stages * dt * per)
    }
}

/// Which default targets a test takes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Targeted {
    /// Extreme conditions: every constant with an extreme, dates included.
    Extremes,
    /// Sensitivity and disturbance, which change a constant by a factor: a
    /// factor of a date is no condition of the system.
    Changes,
}

/// What the battery knows of the model's constants: which are time
/// constants, dates, fractional rates and divisors, which convert units, and
/// which are shares of a whole.
struct Roles {
    time_constants: HashMap<String, TimeConstant>,
    /// Numbers an equation divides a stock-dependent quantity by in a rate,
    /// times written into the equation rather than named
    /// (`backlog / 0.1`).
    literal_times: Vec<LiteralTime>,
    /// Constants whose value is one of their units.
    conversions: HashSet<String>,
    /// Shares and fractions, with their whole: 1, or 100 in percent.
    shares: HashMap<String, f64>,
    /// Constants some equation divides by, whose domain zero is outside.
    divisors: HashSet<String>,
    /// Constants nothing reads as the run goes ([`Graph::only_starts`]).
    starts: HashSet<String>,
    /// Constants used as points in time.
    dates: HashSet<String>,
    /// Fractional rates, constants in units of one over a unit of time, each
    /// with how many of that unit one of the model's units of time is, when
    /// known ([`Units::per_model_time`]).
    rates: HashMap<String, Option<f64>>,
}

/// An extreme for a constant: the rule that chose it, and each element's
/// value there given its value in the model.
struct Extreme {
    rule: ExtremeRule,
    to: Box<dyn Fn(f64) -> f64>,
}

impl Extreme {
    fn new(rule: ExtremeRule, to: impl Fn(f64) -> f64 + 'static) -> Extreme {
        Extreme {
            rule,
            to: Box::new(to),
        }
    }
}

impl Roles {
    fn of(
        model: &datamodel::Model,
        graph: &Graph,
        units: &Units,
        parsed: &Parsed<'_>,
        base: &Run,
    ) -> Roles {
        let mut conversions = HashSet::new();
        let mut shares = HashMap::new();
        let mut rates = HashMap::new();
        let complements = complements(parsed);
        let (mut time_constants, literal_times) = time_constants(model, graph, units, parsed);
        let used_as_dates = points_in_time(parsed);
        let mut dates = HashSet::new();
        let mut starts = HashSet::new();
        for var in model.variables.iter().filter(|v| settable(v).is_ok()) {
            let canonical = crate::canonicalize(var.get_ident()).into_owned();
            if graph.only_starts(&canonical) {
                starts.insert(canonical.clone());
            }
            let declared = units.of_variable(var);
            // A date is in units of time, or in none: a constant in other
            // units that an equation compares with the time is a threshold
            // on something else. A delay's time, or a divisor of a rate, is a
            // duration however else it is used.
            let is_time = declared.as_ref().is_none_or(|u| units.is_time(u));
            let is_duration = time_constants
                .get(&canonical)
                .is_some_and(|tc| tc.evidence != TimeConstantEvidence::TimeUnits);
            if used_as_dates.contains(&canonical) && is_time && !is_duration {
                time_constants.remove(&canonical);
                dates.insert(canonical);
                continue;
            }
            if let Some(time) = declared.as_ref().and_then(|u| units.rate_time(u)) {
                rates.insert(canonical.clone(), units.per_model_time(&time));
            }
            let values = element_values(base, model, var);
            if values.is_empty() {
                continue;
            }
            if let Some(declared) = &declared
                && is_unit_conversion(&canonical, declared, &values, units)
            {
                conversions.insert(canonical);
                continue;
            }
            if let Some(whole) =
                share_whole(&canonical, var, declared.as_ref(), &values, &complements)
            {
                shares.insert(canonical, whole);
            }
        }
        Roles {
            time_constants,
            literal_times,
            conversions,
            shares,
            divisors: divisors(parsed),
            starts,
            dates,
            rates,
        }
    }

    /// The constants a targeted test changes by default: those that reach a
    /// stock, where it starts or as the run goes, less the model's unit
    /// conversions and, for a test that changes a constant by a factor, its
    /// dates; and what was left out.
    fn defaults<'m>(
        &self,
        model: &'m datamodel::Model,
        graph: &Graph,
        targeted: Targeted,
    ) -> (Vec<&'m Variable>, LeftOut<'m>) {
        let is_in = |set: &HashSet<String>, var: &Variable| {
            set.contains(crate::canonicalize(var.get_ident()).as_ref())
        };
        let (conversions, rest): (Vec<&Variable>, Vec<&Variable>) = graph
            .reaching_stocks(model)
            .into_iter()
            .partition(|var| is_in(&self.conversions, var));
        let scaled = targeted == Targeted::Changes;
        let (dates, targets): (Vec<&Variable>, Vec<&Variable>) = rest
            .into_iter()
            .partition(|var| scaled && is_in(&self.dates, var));
        (targets, LeftOut { conversions, dates })
    }

    fn time_constant(&self, var: &Variable) -> Option<TimeConstant> {
        self.time_constants
            .get(crate::canonicalize(var.get_ident()).as_ref())
            .copied()
    }

    fn share_whole(&self, var: &Variable) -> Option<f64> {
        self.shares
            .get(crate::canonicalize(var.get_ident()).as_ref())
            .copied()
    }

    fn only_starts(&self, var: &Variable) -> bool {
        self.starts
            .contains(crate::canonicalize(var.get_ident()).as_ref())
    }

    fn is_date(&self, var: &Variable) -> bool {
        self.dates
            .contains(crate::canonicalize(var.get_ident()).as_ref())
    }

    /// The highest value a fractional rate takes: one over
    /// [`DTS_PER_FASTEST_RATE`] DTs, in the rate's unit of time; `None` for a
    /// constant that is no fractional rate, or whose unit of time DT cannot
    /// be put in.
    fn fastest_rate(&self, var: &Variable, dt: f64) -> Option<f64> {
        let per = (*self
            .rates
            .get(crate::canonicalize(var.get_ident()).as_ref())?)?;
        Some(1.0 / (DTS_PER_FASTEST_RATE * dt * per))
    }

    /// `var`'s low extreme, in a run with `specs`: the call's, else by what
    /// the constant is.
    fn low(
        &self,
        var: &Variable,
        given: GivenExtremes,
        specs: &crate::results::Specs,
    ) -> (Extreme, Option<TimeConstantEvidence>) {
        if let Some(low) = given.low {
            return (Extreme::new(ExtremeRule::Given, move |_| low), None);
        }
        if self.is_date(var) {
            let start = specs.start;
            return (Extreme::new(ExtremeRule::RunStart, move |_| start), None);
        }
        if let Some(tc) = self.time_constant(var) {
            let floor = tc.floor(specs.dt);
            let short = move |value: f64| within(value, floor.max(value / EXTREME_FACTOR));
            return (
                Extreme::new(ExtremeRule::ShortTime, short),
                Some(tc.evidence),
            );
        }
        let canonical = crate::canonicalize(var.get_ident());
        if self.divisors.contains(canonical.as_ref()) || self.starts.contains(canonical.as_ref()) {
            return (
                Extreme::new(ExtremeRule::Tenth, |value| value / EXTREME_FACTOR),
                None,
            );
        }
        (Extreme::new(ExtremeRule::Zero, |_| 0.0), None)
    }

    /// `var`'s high extreme, in a run with `specs`: the call's, else by what
    /// the constant is.
    fn high(&self, var: &Variable, given: GivenExtremes, specs: &crate::results::Specs) -> Extreme {
        if let Some(high) = given.high {
            return Extreme::new(ExtremeRule::Given, move |_| high);
        }
        if self.is_date(var) {
            let past = specs.stop + (specs.stop - specs.start) * STEP_FRACTION;
            return Extreme::new(ExtremeRule::PastStop, move |_| past);
        }
        if let Some(whole) = self.share_whole(var) {
            return Extreme::new(ExtremeRule::Whole, move |_| whole);
        }
        if let Some(fastest) = self.fastest_rate(var, specs.dt) {
            return Extreme::new(ExtremeRule::FastestRate, move |value| {
                beyond(value, (value * EXTREME_FACTOR).min(fastest))
            });
        }
        Extreme::new(ExtremeRule::TenTimes, |value| value * EXTREME_FACTOR)
    }

    /// What sensitivity gives each element of `var` at half and at double:
    /// within the constant's extremes, so a check that reads the system at
    /// its extremes does not read the integration between them.
    fn half_and_double(
        &self,
        var: &Variable,
        dt: f64,
    ) -> (impl Fn(f64) -> f64 + use<>, impl Fn(f64) -> f64 + use<>) {
        let floor = self.time_constant(var).map(|tc| tc.floor(dt));
        let whole = self.share_whole(var);
        let fastest = self.fastest_rate(var, dt);
        let half = move |value: f64| match floor {
            Some(floor) => within(value, floor.max(value * 0.5)),
            None => value * 0.5,
        };
        let double = move |value: f64| {
            let doubled = value * 2.0;
            match (whole, fastest) {
                (Some(whole), _) => doubled.min(whole),
                (None, Some(fastest)) => beyond(value, doubled.min(fastest)),
                (None, None) => doubled,
            }
        };
        (half, double)
    }
}

/// `low` when it is below `value`, else `value`: a low extreme is never
/// above the value it is an extreme of.
fn within(value: f64, low: f64) -> f64 {
    if low < value { low } else { value }
}

/// `high` when it is above `value`, else `value`: a high extreme is never
/// below the value it is an extreme of.
fn beyond(value: f64, high: f64) -> f64 {
    if high > value { high } else { value }
}

/// The builtin `name` calls, by its canonical name (`BuiltinSig::by_name`,
/// which knows the aliases).
fn builtin_called(name: &str) -> Option<&'static str> {
    crate::builtins::BuiltinSig::by_name(&name.to_lowercase()).map(|sig| sig.name)
}

/// The variable `expr` is a bare reference to.
fn reference(expr: &Expr0) -> Option<String> {
    match expr {
        Expr0::Var(raw, _) | Expr0::Subscript(raw, _, _) => {
            Some(raw.canonicalize().as_str().to_string())
        }
        _ => None,
    }
}

/// The variables the model uses as points in time: one an equation compares
/// with the time, or gives STEP, RAMP or PULSE as the time it starts (or a
/// ramp ends) at, and those an auxiliary that is so used copies or selects
/// between (`start = IF policy THEN early_year ELSE late_year`,
/// `start = INIT(first_year)`).
///
/// Only a bare reference counts: in `TIME > start + delay` neither operand is
/// known to be the date. A subtraction from the time is no evidence either,
/// since `TIME - lag` and `TIME - base_year` read alike.
fn points_in_time(parsed: &Parsed<'_>) -> HashSet<String> {
    fn is_time(expr: &Expr0) -> bool {
        matches!(expr, Expr0::App(UntypedBuiltinFn(name, args), _)
            if args.is_empty() && builtin_called(name) == Some("time"))
    }
    fn used(expr: &Expr0, found: &mut Vec<String>) {
        match expr {
            Expr0::Const(..) | Expr0::Var(..) => {}
            Expr0::Subscript(_, indices, _) => {
                for index in indices.iter() {
                    if let IndexExpr0::Expr(e) = index {
                        used(e, found);
                    }
                }
            }
            Expr0::App(UntypedBuiltinFn(name, args), _) => {
                // The arguments that are times: STEP(height, time),
                // RAMP(slope, start, end), PULSE(volume, first, interval),
                // as `vm::step`, `vm::ramp` and `vm::pulse` read them.
                let times: &[usize] = match builtin_called(name) {
                    Some("step") | Some("pulse") => &[1],
                    Some("ramp") => &[1, 2],
                    _ => &[],
                };
                found.extend(
                    times
                        .iter()
                        .filter_map(|&i| args.get(i).and_then(reference)),
                );
                for arg in args.iter() {
                    used(arg, found);
                }
            }
            Expr0::Op1(_, inner, _) => used(inner, found),
            Expr0::Op2(op, l, r, _) => {
                let compares = matches!(
                    op,
                    BinaryOp::Gt
                        | BinaryOp::Lt
                        | BinaryOp::Gte
                        | BinaryOp::Lte
                        | BinaryOp::Eq
                        | BinaryOp::Neq
                );
                if compares && is_time(l) {
                    found.extend(reference(r));
                }
                if compares && is_time(r) {
                    found.extend(reference(l));
                }
                used(l, found);
                used(r, found);
            }
            Expr0::If(c, t, f, _) => {
                used(c, found);
                used(t, found);
                used(f, found);
            }
        }
    }
    /// What an auxiliary that is a point in time takes its value from: a
    /// reference, the branches of an IF, and what INIT holds.
    fn carried(expr: &Expr0, found: &mut Vec<String>) {
        match expr {
            Expr0::Var(..) | Expr0::Subscript(..) => found.extend(reference(expr)),
            Expr0::If(_, t, f, _) => {
                carried(t, found);
                carried(f, found);
            }
            Expr0::App(UntypedBuiltinFn(name, args), _) if builtin_called(name) == Some("init") => {
                for arg in args.iter() {
                    carried(arg, found);
                }
            }
            Expr0::Const(..) | Expr0::App(..) | Expr0::Op1(..) | Expr0::Op2(..) => {}
        }
    }
    let mut pending: Vec<String> = Vec::new();
    for (_, exprs) in parsed.equations.values() {
        for expr in exprs {
            used(expr, &mut pending);
        }
    }
    let mut found: HashSet<String> = HashSet::new();
    while let Some(name) = pending.pop() {
        if !found.insert(name.clone()) {
            continue;
        }
        if let Some((var @ Variable::Aux(aux), exprs)) = parsed.equations.get(&name)
            && aux.gf.is_none()
            && settable(var).is_err()
        {
            for expr in exprs {
                carried(expr, &mut pending);
            }
        }
    }
    found
}

/// The variables some equation divides by: a factor of a divisor (`x / c`,
/// `x / (c * d)`, `x MOD c`, `c ^ -2`), and the factors of an auxiliary that
/// is one (`x / scale` with `scale = c * d`). A term of a sum in a divisor
/// (`x / (a + c)`) is not: the sum is not zero where the term is. Nor is the
/// divisor of a guarded division (`SAFEDIV`, which the importers read Vensim's
/// ZIDZ and XIDZ as): the equation says what it is at zero.
fn divisors(parsed: &Parsed<'_>) -> HashSet<String> {
    /// The variables `expr` is zero wherever one of is: the factors of a
    /// product, the numerator of a quotient, the base of a positive power.
    fn factors_of(expr: &Expr0, found: &mut Vec<String>) {
        match expr {
            Expr0::Var(..) | Expr0::Subscript(..) => found.extend(reference(expr)),
            Expr0::Op2(BinaryOp::Mul, l, r, _) => {
                factors_of(l, found);
                factors_of(r, found);
            }
            Expr0::Op2(BinaryOp::Div, l, _, _) => factors_of(l, found),
            Expr0::Op2(BinaryOp::Exp, base, exponent, _)
                if number(exponent).is_none_or(|n| n > 0.0) =>
            {
                factors_of(base, found)
            }
            Expr0::Op1(UnaryOp::Negative | UnaryOp::Positive, inner, _) => factors_of(inner, found),
            Expr0::If(_, t, f, _) => {
                factors_of(t, found);
                factors_of(f, found);
            }
            // An extreme gives every element of an arrayed constant its
            // value, and a sum of zeros is zero.
            Expr0::App(UntypedBuiltinFn(name, args), _) if builtin_called(name) == Some("sum") => {
                for arg in args.iter() {
                    factors_of(arg, found);
                }
            }
            Expr0::Const(..) | Expr0::App(..) | Expr0::Op1(..) | Expr0::Op2(..) => {}
        }
    }
    fn divided_by(expr: &Expr0, found: &mut Vec<String>) {
        match expr {
            Expr0::Const(..) | Expr0::Var(..) => {}
            Expr0::Subscript(_, indices, _) => {
                for index in indices.iter() {
                    if let IndexExpr0::Expr(e) = index {
                        divided_by(e, found);
                    }
                }
            }
            Expr0::App(UntypedBuiltinFn(_, args), _) => {
                for arg in args.iter() {
                    divided_by(arg, found);
                }
            }
            Expr0::Op1(_, inner, _) => divided_by(inner, found),
            Expr0::Op2(op, l, r, _) => {
                match op {
                    BinaryOp::Div | BinaryOp::Mod => factors_of(r, found),
                    BinaryOp::Exp if number(r).is_some_and(|n| n < 0.0) => factors_of(l, found),
                    _ => {}
                }
                divided_by(l, found);
                divided_by(r, found);
            }
            Expr0::If(c, t, f, _) => {
                divided_by(c, found);
                divided_by(t, found);
                divided_by(f, found);
            }
        }
    }
    let mut pending: Vec<String> = Vec::new();
    for (_, exprs) in parsed.equations.values() {
        for expr in exprs {
            divided_by(expr, &mut pending);
        }
    }
    let mut found: HashSet<String> = HashSet::new();
    while let Some(name) = pending.pop() {
        if !found.insert(name.clone()) {
            continue;
        }
        // An auxiliary that is divided by is zero where a factor of its
        // equation is.
        let computed = match parsed.equations.get(&name) {
            Some((var @ Variable::Aux(_), exprs)) => settable(var).is_err().then_some(exprs),
            _ => None,
        };
        for expr in computed.into_iter().flatten() {
            factors_of(expr, &mut pending);
        }
    }
    found
}

/// Whether a constant named `canonical`, with `values` (one per element) in
/// its `declared` units, converts units: its value is one of its units, so
/// multiplying by it changes the units a quantity is counted in and not the
/// quantity.
///
/// Either its units are a ratio of one unit at two scales and its value is
/// the ratio (`1e6 tons/Mton`, `1e9 people/billion_people`, `1000 ppt/ppb`),
/// or they are one unit and its name states the value and the unit
/// (`one_year = 1 year`, `100_percent = 100 percent`), or they are a ratio of
/// two units of time and its value their known ratio (`hours_per_week = 168
/// hour/week`, `days_per_year = 365 day/year`, within
/// [`TIME_RATIO_TOLERANCE`]). A ratio of one unit at two scales that is not
/// their ratio is a quantity (`0.3 Mton/Gton`, `40 hour/week` of work), and
/// a constant of one unit that its name does not state is a parameter
/// (`adjustment_time = 1 year`).
fn is_unit_conversion(canonical: &str, declared: &UnitMap, values: &[f64], units: &Units) -> bool {
    let factors: Vec<(&String, i32)> = declared.map.iter().map(|(n, &e)| (n, e)).collect();
    if factors.is_empty() || factors.len() > 4 {
        return false;
    }
    let numerator = factors.iter().find(|(_, e)| *e == 1);
    let denominator = factors.iter().find(|(_, e)| *e == -1);
    if let (2, Some((numerator, _)), Some((denominator, _))) =
        (factors.len(), numerator, denominator)
        && let (Some(numerator), Some(denominator)) =
            (units.seconds(numerator), units.seconds(denominator))
    {
        let ratio = denominator / numerator;
        if values
            .iter()
            .all(|&v| (v / ratio - 1.0).abs() <= TIME_RATIO_TOLERANCE)
        {
            return true;
        }
    }
    let readings: Vec<Vec<(f64, UnitMap)>> = factors
        .iter()
        .map(|(name, _)| units.readings(name))
        .collect();
    let is_one = |x: f64| (x - 1.0).abs() <= CONVERSION_TOLERANCE;
    let words: Vec<&str> = canonical.split('_').collect();
    let stated = |value: f64| {
        words.iter().any(|word| {
            word.parse::<f64>().ok().or_else(|| {
                NUMBER_WORDS
                    .iter()
                    .find(|(w, _)| w == word)
                    .map(|&(_, n)| n)
            }) == Some(value)
        })
    };
    // A word of the name names the unit when it spells it, in the singular
    // or the plural, or when the project's unit definitions read the word as
    // that unit: `year` where the project calls the unit `yr`.
    let named = |unit: &str| {
        let singular = |text: &str| text.strip_suffix('s').unwrap_or(text).to_string();
        let is_unit = |text: &str| {
            let map = units.ctx.resolve_name(text);
            map.map.len() == 1 && map.map.get(unit) == Some(&1)
        };
        words.iter().any(|word| {
            singular(word) == singular(unit) || is_unit(word) || is_unit(&singular(word))
        })
    };
    // Every way to read the factors, one reading each.
    let mut choice = vec![0usize; factors.len()];
    loop {
        let mut stem = UnitMap::new();
        let mut scale = 1.0;
        for (i, (_, exp)) in factors.iter().enumerate() {
            let (s, map) = &readings[i][choice[i]];
            stem = stem * map.clone().exp(*exp);
            scale *= s.powi(*exp);
        }
        let ratio = factors.len() > 1
            && factors.iter().any(|(_, e)| *e > 0)
            && factors.iter().any(|(_, e)| *e < 0)
            && scale != 1.0;
        let one_named_unit = factors.len() == 1
            && factors[0].1 == 1
            && named(factors[0].0)
            && values.iter().all(|&v| stated(v));
        if stem.is_empty() || one_named_unit {
            let reads_one = values.iter().all(|&v| {
                let in_units = if stem.is_empty() { v * scale } else { v };
                is_one(in_units)
            });
            if reads_one && (ratio || one_named_unit) {
                return true;
            }
        }
        // The next choice of readings.
        let mut i = 0;
        loop {
            if i == choice.len() {
                return false;
            }
            choice[i] += 1;
            if choice[i] < readings[i].len() {
                break;
            }
            choice[i] = 0;
            i += 1;
        }
    }
}

/// The constants some equation takes the complement of, `1 - c`: the rest of
/// a share of one. (A share in percent is one by its units, whatever is done
/// with it.)
fn complements(parsed: &Parsed<'_>) -> HashSet<String> {
    fn walk(expr: &Expr0, found: &mut HashSet<String>) {
        match expr {
            Expr0::Const(..) => {}
            Expr0::Var(..) => {}
            Expr0::Op2(BinaryOp::Sub, l, r, _) => {
                if let (Expr0::Const(_, literal, _), Expr0::Var(raw, _)) = (l.as_ref(), r.as_ref())
                    && literal.value() == 1.0
                {
                    found.insert(raw.canonicalize().as_str().to_string());
                }
                walk(l, found);
                walk(r, found);
            }
            Expr0::Op2(_, l, r, _) => {
                walk(l, found);
                walk(r, found);
            }
            Expr0::Op1(_, inner, _) => walk(inner, found),
            Expr0::App(UntypedBuiltinFn(_, args), _) => {
                for arg in args.iter() {
                    walk(arg, found);
                }
            }
            Expr0::Subscript(_, indices, _) => {
                for index in indices.iter() {
                    if let IndexExpr0::Expr(e) = index {
                        walk(e, found);
                    }
                }
            }
            Expr0::If(c, t, f, _) => {
                walk(c, found);
                walk(t, found);
                walk(f, found);
            }
        }
    }
    let mut found = HashSet::new();
    for (_, exprs) in parsed.equations.values() {
        for expr in exprs {
            walk(expr, &mut found);
        }
    }
    found
}

/// The whole a constant is a share or fraction of, when it is one: 100 for a
/// constant in percent, else 1. It is one when its units are a fraction or a
/// percentage, or (without units, or dimensionless) when its name calls it a
/// share, fraction or proportion, or an equation takes its complement (`1 -
/// c`); and every element is within its whole.
fn share_whole(
    canonical: &str,
    var: &Variable,
    declared: Option<&UnitMap>,
    values: &[f64],
    complements: &HashSet<String>,
) -> Option<f64> {
    let raw = var
        .get_units()
        .map(|units| crate::canonicalize(units.trim()).into_owned());
    let in_percent = matches!(raw.as_deref(), Some("percent" | "%" | "pct"));
    let whole = if in_percent { 100.0 } else { 1.0 };
    let dimensionless = declared.is_none_or(UnitMap::is_empty) || in_percent;
    let marked = matches!(raw.as_deref(), Some("fraction")) || in_percent;
    let named = canonical.split('_').any(|word| SHARE_WORDS.contains(&word));
    let complemented = !in_percent && complements.contains(canonical);
    let within = values.iter().all(|&v| v > 0.0 && v <= whole);
    (dimensionless && within && (marked || named || complemented)).then_some(whole)
}

/// The model's time constants, by canonical ident: why each is one, and how
/// many DTs its low extreme is.
///
/// - The time of a delay, smooth or trend is one, whatever its units. A
///   third-order delay's or smooth's time is three DTs at least, since each
///   of its stages takes a third of it; any other's is one.
/// - A constant with units is one when they are a unit of time, and is not
///   otherwise, whatever its role: a divisor in units of what it divides is a
///   scale.
/// - A constant without units is one by its role in a rate: a flow, or what a
///   rate adds up (an auxiliary that is a term of a rate's sum, through its IF
///   branches and the arguments of MAX and MIN). In a term `x / c` of a rate,
///   or a factor `x / c` of one, `x` depending on a stock, `c` is a time
///   constant (`(goal - level) / adjustment_time`, `population / (lifetime *
///   1)`, and `workforce / average_tenure` times an effect), unless `c` is a
///   term of a sum in `x`, `(capacity - population) / capacity`, which makes
///   it a scale. A reciprocal factor `1 / c` (a number over `c`) of a term
///   with a factor depending on a stock makes `c` a time constant too, as does
///   one of a fractional rate the term multiplies (`population *
///   death_fraction`, `death_fraction = 1 / lifetime`); a variable over `c`
///   does not (`-gravity / length * angle`).
/// - A constant declared dimensionless (`1`, `dmnl`) is one without units
///   where the rate it divides carries a unit warning, the rate's own
///   equation or its flow's: a stock over it there cannot balance, so the
///   declaration is the modeler's slip (`experts / average_tenure`). Where the
///   rate's units balance, the declaration is right, and the constant is not
///   one by its role (`pipeline / delay_time / number_of_stages`).
/// - A divisor elsewhere -- in a sum within a factor, `r * p * (1 - p /
///   capacity)`, or in a lookup's input -- is a scale, whose low extreme is
///   zero.
fn time_constants(
    model: &datamodel::Model,
    graph: &Graph,
    units: &Units,
    parsed: &Parsed<'_>,
) -> (HashMap<String, TimeConstant>, Vec<LiteralTime>) {
    let constants: HashMap<String, Option<UnitMap>> = model
        .variables
        .iter()
        .filter(|v| settable(v).is_ok())
        .map(|v| {
            (
                crate::canonicalize(v.get_ident()).into_owned(),
                units.of_variable(v),
            )
        })
        .collect();
    let mut found: HashMap<String, TimeConstant> = HashMap::new();
    for (name, declared) in &constants {
        if let Some(declared) = declared
            && units.is_time(declared)
        {
            found.insert(
                name.clone(),
                TimeConstant {
                    evidence: TimeConstantEvidence::TimeUnits,
                    stages: 1.0,
                    per_model_time: units.per_model_time(declared),
                    paces_a_stock: false,
                },
            );
        }
    }
    for (_, exprs) in parsed.equations.values() {
        for expr in exprs {
            delay_times(expr, &mut found);
        }
    }

    // What depends on a stock: everything a stock moves as the run goes. A
    // quantity frozen from one at the start is a constant of the run.
    let mut stock_dependent: HashSet<String> = HashSet::new();
    for var in &model.variables {
        if matches!(var, Variable::Stock(_)) {
            let stock = crate::canonicalize(var.get_ident()).into_owned();
            stock_dependent.extend(graph.moved_by(&stock));
            stock_dependent.insert(stock);
        }
    }
    // A role counts for a constant without units, and never overrides a
    // delay's time. A constant declared dimensionless (`1`, `dmnl`) has no
    // units to go by either where the rate it divides carries a unit warning
    // (its own equation, or its flow's): a stock over it there cannot
    // balance, so the declaration is the modeler's slip, and the role
    // decides. Where the rate's units balance, the declaration is right.
    let mut pacing: HashSet<String> = HashSet::new();
    let mut by_role = |name: &str, evidence: TimeConstantEvidence, rate: [&str; 2]| {
        pacing.insert(name.to_string());
        let undeclared = match constants.get(name) {
            Some(None) => true,
            Some(Some(declared)) => declared.is_empty() && rate.iter().any(|var| units.warned(var)),
            None => false,
        };
        if undeclared {
            // A constant taken for a time by its role is in the model's own
            // unit of time: the rate it divides is per that unit.
            found.entry(name.to_string()).or_insert(TimeConstant {
                evidence,
                stages: 1.0,
                per_model_time: Some(1.0),
                paces_a_stock: true,
            });
        }
    };
    let is_aux = |name: &str| matches!(parsed.equations.get(name), Some((Variable::Aux(_), _)));
    // A table's equation is its input, not a rate.
    let has_table = |var: &Variable| match var {
        Variable::Aux(aux) => aux.gf.is_some(),
        Variable::Flow(flow) => flow.gf.is_some(),
        _ => false,
    };

    // Each rate with the flow it is a term of (a flow's own is itself).
    let mut rates: VecDeque<(String, String)> = parsed
        .equations
        .iter()
        .filter(|(_, (var, _))| matches!(var, Variable::Flow(_)))
        .map(|(name, _)| (name.clone(), name.clone()))
        .collect();
    let mut seen: HashSet<String> = rates.iter().map(|(rate, _)| rate.clone()).collect();
    // Fractional rates, auxiliaries a rate's term multiplies by, to read the
    // reciprocals of, each with its flow.
    let mut fractions: VecDeque<(String, String)> = VecDeque::new();
    let mut seen_fractions: HashSet<String> = HashSet::new();
    let mut literals: Vec<LiteralTime> = Vec::new();
    while let Some((rate, flow)) = rates.pop_front() {
        let Some((var, exprs)) = parsed.equations.get(&rate) else {
            continue;
        };
        if has_table(var) {
            continue;
        }
        for expr in exprs {
            additive_terms(expr, &mut |term| match term {
                Expr0::Var(raw, _) => {
                    let name = raw.canonicalize().as_str().to_string();
                    if is_aux(&name) && seen.insert(name.clone()) {
                        rates.push_back((name, flow.clone()));
                    }
                }
                product => {
                    let factors = factors(product);
                    let moved = |i: usize| {
                        factors
                            .iter()
                            .enumerate()
                            .any(|(j, f)| j != i && reads_any(f, &stock_dependent))
                    };
                    for (i, factor) in factors.iter().enumerate() {
                        match factor {
                            Expr0::Op2(BinaryOp::Div, quantity, divisor, _) => {
                                let Some(c) = single_variable(divisor) else {
                                    if let Some(value) = number(divisor)
                                        && value > 0.0
                                        && reads_any(quantity, &stock_dependent)
                                        && let Some((var, _)) = parsed.equations.get(&rate)
                                    {
                                        literals.push(LiteralTime {
                                            variable: var.get_ident().to_string(),
                                            value,
                                        });
                                    }
                                    continue;
                                };
                                if reads_any(quantity, &stock_dependent) {
                                    if !in_a_sum(quantity, &c) {
                                        by_role(
                                            &c,
                                            TimeConstantEvidence::DividesARate,
                                            [&rate, &flow],
                                        );
                                    }
                                } else if is_number(quantity) && moved(i) {
                                    by_role(
                                        &c,
                                        TimeConstantEvidence::ReciprocalInARate,
                                        [&rate, &flow],
                                    );
                                }
                            }
                            Expr0::Var(raw, _) if moved(i) => {
                                let name = raw.canonicalize().as_str().to_string();
                                if is_aux(&name) && seen_fractions.insert(name.clone()) {
                                    fractions.push_back((name, flow.clone()));
                                }
                            }
                            _ => {}
                        }
                    }
                }
            });
        }
    }
    // A fractional rate's reciprocals, through the fractional rates it
    // multiplies in turn.
    while let Some((fraction, flow)) = fractions.pop_front() {
        let Some((var, exprs)) = parsed.equations.get(&fraction) else {
            continue;
        };
        if has_table(var) {
            continue;
        }
        for expr in exprs {
            additive_terms(expr, &mut |term| {
                for factor in factors(term) {
                    match factor {
                        Expr0::Op2(BinaryOp::Div, quantity, divisor, _) if is_number(quantity) => {
                            if let Some(c) = single_variable(divisor) {
                                by_role(
                                    &c,
                                    TimeConstantEvidence::ReciprocalInARate,
                                    [&fraction, &flow],
                                );
                            }
                        }
                        Expr0::Var(raw, _) => {
                            let name = raw.canonicalize().as_str().to_string();
                            if is_aux(&name) && seen_fractions.insert(name.clone()) {
                                fractions.push_back((name, flow.clone()));
                            }
                        }
                        _ => {}
                    }
                }
            });
        }
    }
    for name in pacing {
        if let Some(tc) = found.get_mut(&name) {
            tc.paces_a_stock = true;
        }
    }
    (found, literals)
}

/// A time a rate's equation is written with: the number it divides a
/// quantity a stock moves by (`backlog / 0.1`), in the model's own unit of
/// time, as a named time constant there would be.
#[derive(Clone)]
struct LiteralTime {
    /// The variable whose equation it is in, as the model names it.
    variable: String,
    value: f64,
}

/// Call `term` with each term of `expr`'s sum: through addition and
/// subtraction, a sign, IF branches, and the arguments of MAX and MIN.
fn additive_terms(expr: &Expr0, term: &mut impl FnMut(&Expr0)) {
    match expr {
        Expr0::Op2(BinaryOp::Add | BinaryOp::Sub, l, r, _) => {
            additive_terms(l, term);
            additive_terms(r, term);
        }
        Expr0::Op1(_, inner, _) => additive_terms(inner, term),
        Expr0::If(_, t, f, _) => {
            additive_terms(t, term);
            additive_terms(f, term);
        }
        Expr0::App(UntypedBuiltinFn(name, args), _)
            if matches!(name.to_lowercase().as_str(), "max" | "min") =>
        {
            for arg in args.iter() {
                additive_terms(arg, term);
            }
        }
        other => term(other),
    }
}

/// The factors of a product, `expr` itself when it is not one; a division is
/// a factor, its divisor not split.
fn factors(expr: &Expr0) -> Vec<&Expr0> {
    match expr {
        Expr0::Op2(BinaryOp::Mul, l, r, _) => {
            let mut all = factors(l);
            all.extend(factors(r));
            all
        }
        Expr0::Op2(BinaryOp::Div, l, _, _)
            if matches!(l.as_ref(), Expr0::Op2(BinaryOp::Mul, ..)) =>
        {
            vec![expr]
        }
        other => vec![other],
    }
}

/// Whether `expr` is a number, signed or not.
fn is_number(expr: &Expr0) -> bool {
    match expr {
        Expr0::Const(..) => true,
        Expr0::Op1(_, inner, _) => is_number(inner),
        _ => false,
    }
}

/// The one variable a divisor is, alone or times numbers: `c`, `(c * 1)`.
fn single_variable(divisor: &Expr0) -> Option<String> {
    let mut variables = factors(divisor)
        .into_iter()
        .filter_map(|factor| match factor {
            Expr0::Var(raw, _) => Some(Some(raw.canonicalize().as_str().to_string())),
            Expr0::Const(..) => None,
            _ => Some(None),
        });
    let only = variables.next()??;
    variables.next().is_none().then_some(only)
}

/// Whether `c` is a term of a sum in `quantity`: of `quantity`'s own, or of
/// one of its factors.
fn in_a_sum(quantity: &Expr0, c: &str) -> bool {
    let term_of = |expr: &Expr0| {
        let mut found = false;
        additive_terms(expr, &mut |term| {
            if let Expr0::Var(raw, _) = term
                && raw.canonicalize().as_str() == c
            {
                found = true;
            }
        });
        found && matches!(expr, Expr0::Op2(BinaryOp::Add | BinaryOp::Sub, ..))
    };
    term_of(quantity) || factors(quantity).into_iter().any(term_of)
}

/// Record that `name` is the time of a delay of `stages` stages. A constant
/// in a unit of time keeps that unit; any other is in the model's own, the
/// unit the delay reads its time in.
fn note_stages(found: &mut HashMap<String, TimeConstant>, name: &str, stages: f64) {
    let entry = found.entry(name.to_string()).or_insert(TimeConstant {
        evidence: TimeConstantEvidence::DelayTime,
        stages,
        per_model_time: Some(1.0),
        paces_a_stock: true,
    });
    entry.evidence = TimeConstantEvidence::DelayTime;
    entry.paces_a_stock = true;
    entry.stages = entry.stages.max(stages);
}

/// Which argument of a call to `function` (lowercase) is a time a delay, a
/// smooth or a trend spreads its input over, and over how many stages; `None`
/// for any other function.
///
/// The engine's own tables say which: a stdlib module-function's time is the
/// argument wired to the [`TIME_PORT`] port (`module_functions::stdlib_args`),
/// and its stages are its model's stocks. The aliases the engine rewrites to
/// one of those (`builtins::is_stdlib_module_function` without a descriptor:
/// `DELAY`, `DELAYN`, `SMTHN`) take `(input, time, order, initial)`, as
/// `builtins_visitor::rewrite_alias_module_call` reads them; `DELAY` has no
/// order and is first order there.
fn time_argument(function: &str, args: &[Expr0]) -> Option<(usize, f64)> {
    if let Some(ports) = crate::module_functions::stdlib_args(function) {
        let index = ports.iter().position(|port| *port == TIME_PORT)?;
        let stages = crate::stdlib::get(function).map_or(1, |model| {
            model
                .variables
                .iter()
                .filter(|var| matches!(var, Variable::Stock(_)))
                .count()
        });
        return Some((index, stages.max(1) as f64));
    }
    if !crate::builtins::is_stdlib_module_function(function) {
        return None;
    }
    let order = match function {
        "delay" => None,
        _ => args.get(2).and_then(number).filter(|order| *order >= 1.0),
    };
    Some((1, order.unwrap_or(1.0)))
}

/// Add to `found` each variable `expr` gives a delay, smooth or trend as its
/// time, with the delay's order: its number of stages.
fn delay_times(expr: &Expr0, found: &mut HashMap<String, TimeConstant>) {
    match expr {
        Expr0::Const(..) | Expr0::Var(..) => {}
        Expr0::App(UntypedBuiltinFn(name, args), _) => {
            if let Some((index, stages)) = time_argument(&name.to_lowercase(), args)
                && let Some(Expr0::Var(raw, _)) = args.get(index)
            {
                note_stages(found, raw.canonicalize().as_str(), stages);
            }
            for arg in args.iter() {
                delay_times(arg, found);
            }
        }
        Expr0::Subscript(_, indices, _) => {
            for index in indices.iter() {
                match index {
                    IndexExpr0::Expr(e) => delay_times(e, found),
                    IndexExpr0::Range(l, r, _) => {
                        delay_times(l, found);
                        delay_times(r, found);
                    }
                    _ => {}
                }
            }
        }
        Expr0::Op1(_, inner, _) => delay_times(inner, found),
        Expr0::Op2(_, l, r, _) => {
            delay_times(l, found);
            delay_times(r, found);
        }
        Expr0::If(c, t, f, _) => {
            delay_times(c, found);
            delay_times(t, found);
            delay_times(f, found);
        }
    }
}

/// Whether `expr` reads any of `names` (canonical idents).
fn reads_any(expr: &Expr0, names: &HashSet<String>) -> bool {
    match expr {
        Expr0::Const(..) => false,
        Expr0::Var(raw, _) | Expr0::Subscript(raw, _, _) => {
            names.contains(raw.canonicalize().as_str())
        }
        Expr0::App(UntypedBuiltinFn(_, args), _) => args.iter().any(|a| reads_any(a, names)),
        Expr0::Op1(_, inner, _) => reads_any(inner, names),
        Expr0::Op2(_, l, r, _) => reads_any(l, names) || reads_any(r, names),
        Expr0::If(c, t, f, _) => reads_any(c, names) || reads_any(t, names) || reads_any(f, names),
    }
}

/// The units check: the engine's unit diagnostics, by id.
fn units_check(evidence: &mut Evidence, ws: &Workspace<'_>, resolved: &ResolvedModel<'_>) -> Check {
    let mut check = Check::new(TestName::Units, None, None);
    check.result.diagnostics = evidence
        .report_diagnostics(ws, resolved)
        .into_iter()
        .filter(|d| {
            matches!(
                d.category,
                DiagnosticCategoryName::UnitDefinition
                    | DiagnosticCategoryName::UnitConsistency
                    | DiagnosticCategoryName::UnitInference
            )
        })
        .map(|d| d.id)
        .collect();
    if !check.result.diagnostics.is_empty() {
        check.result.outcome = Outcome::Failed;
    }
    check
}

/// A value change for every element of `var`, `to(what it was)`, from
/// `from_time` (the start when `None`), and the scalar value it gives.
fn change(
    base: &Run,
    var: &Variable,
    from_time: Option<f64>,
    to: impl Fn(f64) -> f64,
) -> Result<(RunPlan, Option<f64>), String> {
    let (values, applied) = value_change(base, var, from_time, to).map_err(|err| err.error)?;
    let plan = RunPlan {
        values: vec![ValueChange {
            variable: crate::canonicalize(var.get_ident()).into_owned(),
            from_time,
            values,
        }],
        ..RunPlan::default()
    };
    Ok((plan, applied.value))
}

/// Whether every element of constant `var` is zero in `base`.
fn is_zero_valued(base: &Run, model: &datamodel::Model, var: &Variable) -> bool {
    element_values(base, model, var).iter().all(|&v| v == 0.0)
}

/// The values `var`'s elements start `base` with, each by its subscript
/// (empty for a scalar), in results order.
fn element_start_values(
    base: &Run,
    model: &datamodel::Model,
    var: &Variable,
) -> Vec<(String, f64)> {
    let canonical = crate::canonicalize(var.get_ident()).into_owned();
    let Some(first) = base.results.iter().next() else {
        return vec![];
    };
    let declared = super::series::declared(model);
    let mut values: Vec<(usize, String, f64)> = base
        .results
        .offsets
        .iter()
        .filter_map(|(key, &offset)| {
            let key = key.as_str();
            let owner = crate::save_check::column_variable(key, &declared)?;
            (owner == canonical).then(|| {
                let element = key[owner.len()..]
                    .strip_prefix('[')
                    .and_then(|rest| rest.strip_suffix(']'))
                    .unwrap_or_default();
                (offset, element.to_string(), first[offset])
            })
        })
        .collect();
    values.sort_by_key(|(offset, _, _)| *offset);
    values
        .into_iter()
        .map(|(_, element, value)| (element, value))
        .collect()
}

/// The values `var`'s elements start `base` with.
fn element_values(base: &Run, model: &datamodel::Model, var: &Variable) -> Vec<f64> {
    element_start_values(base, model, var)
        .into_iter()
        .map(|(_, value)| value)
        .collect()
}

/// Extreme conditions: the model's own run read for stocks and flows below a
/// zero they should not pass, then each target at its low extreme and at its
/// high one ([`Roles::low`], [`Roles::high`]; `given` holds the call's own).
fn extreme_conditions(
    ws: &mut Workspace<'_>,
    model: &datamodel::Model,
    base: &Run,
    reading: &Reading<'_>,
    targets: &[&Variable],
    given: &HashMap<String, GivenExtremes>,
) -> Result<Made, ToolError> {
    let watched = Watched::of(base, model);
    let specs = &base.results.specs;
    let mut planned: Vec<(Check, RunPlan)> = Vec::new();
    let mut checks = vec![watched.own_run()];
    for &var in targets {
        let given = given
            .get(crate::canonicalize(var.get_ident()).as_ref())
            .copied()
            .unwrap_or_default();
        let current = element_values(base, model, var);
        let (low, evidence) = reading.roles.low(var, given, specs);
        let high = reading.roles.high(var, given, specs);
        for (condition, extreme, evidence) in [
            (Condition::Low, low, evidence),
            (Condition::High, high, None),
        ] {
            // A constant at its extreme already has no condition to try
            // there: zero at zero, a share at the whole, a time constant
            // within four DTs.
            if settable(var).is_ok()
                && !current.is_empty()
                && current.iter().all(|&value| (extreme.to)(value) == value)
            {
                continue;
            }
            let mut check = Check::new(TestName::ExtremeConditions, Some(var), Some(condition));
            check.result.extreme = Some(extreme.rule);
            check.result.time_constant = evidence;
            match change(base, var, None, extreme.to) {
                Err(reason) => checks.push(check.not_run(reason)),
                Ok((plan, value)) => {
                    check.result.value = value;
                    planned.push((check, plan));
                }
            }
        }
    }
    let plans: Vec<RunPlan> = planned.iter().map(|(_, plan)| plan.clone()).collect();
    let problems = runs::execute_values(ws, model, &plans, |plan, results| {
        watched.problems(model, plan, &results)
    })?;
    // What a check found may be the integration's, not the equations': a
    // term made ten times as fast outruns the model's DT, and a stock
    // overshoots zero or a product overflows. A problem that arises as the
    // run goes is tried again at a DT as many times finer, and judged by
    // that run; one in the run's first values is no matter of integration.
    let start = base.times().first().copied().map(round);
    let (mut confirmed, mut artifacts, mut unconfirmed) = (0, 0, 0);
    for ((mut check, plan), problems) in planned.into_iter().zip(problems) {
        match problems {
            Err(reason) => {
                check.result.outcome = Outcome::Failed;
                check.result.reason = Some(format!("the check's run fails: {reason}"));
            }
            Ok(mut problems) => {
                let integrated = problems
                    .first()
                    .is_some_and(|first| Some(first.time) != start);
                if integrated && confirmed == MAX_CONFIRMATIONS {
                    unconfirmed += 1;
                } else if integrated {
                    confirmed += 1;
                    let finer = RunPlan {
                        specs: finer_specs(specs, CONFIRMING_DT_DIVISOR),
                        ..plan
                    };
                    ws.yield_point()?;
                    match runs::execute(ws, model, &finer) {
                        Ok(results) => {
                            problems = watched.problems(model, &finer, &results);
                            if problems.is_empty() {
                                artifacts += 1;
                            }
                        }
                        Err(RunFailure::Stopped) => return Err(ToolError::interrupted()),
                        // A finer run that costs more than a run may leaves
                        // the check as the model's DT has it.
                        Err(RunFailure::Failed(_)) => unconfirmed += 1,
                    }
                }
                check.result.outcome = outcome_of(&problems);
                check.result.note = marked_note(&problems);
                check.result.problems = problems.into_iter().take(MAX_DETAILS).collect();
            }
        }
        checks.push(check);
    }
    let plural = |n: usize| if n == 1 { "" } else { "s" };
    let notes = [
        (artifacts > 0).then(|| {
            format!(
                "{artifacts} check{} found something at the model's DT and nothing at a tenth \
                 of it: at those extremes the model wants a finer DT, which is the \
                 integration's failure and not the equations'.",
                plural(artifacts)
            )
        }),
        (unconfirmed > 0).then(|| {
            format!(
                "{unconfirmed} check{} that found something {} not run again at a finer DT, so \
                 what {} found may be the integration's.",
                plural(unconfirmed),
                if unconfirmed == 1 { "was" } else { "were" },
                if unconfirmed == 1 { "it" } else { "they" },
            )
        }),
    ];
    Ok(Made {
        checks,
        note: notes
            .into_iter()
            .flatten()
            .reduce(|a, b| format!("{a} {b}")),
    })
}

/// How a run with `problems` came out: failed by a value that is no number,
/// flagged for judgment by one below zero.
fn outcome_of(problems: &[Problem]) -> Outcome {
    if problems.iter().any(|p| p.kind == ProblemKind::NonFinite) {
        Outcome::Failed
    } else if problems.is_empty() {
        Outcome::Passed
    } else {
        Outcome::Flagged
    }
}

/// The model's series an extreme conditions check watches, labeled once for
/// every check: each variable's (or element's) results key, its label, and
/// whether going negative counts for it. A series that is not a number
/// somewhere in the model's own run is not watched: a check cannot make it
/// so, and the own run says it once.
struct Watched {
    series: Vec<WatchedSeries>,
    /// What the model's own run shows, with nothing changed: series that are
    /// not a number, and series the model marks non-negative below zero.
    own: Vec<Problem>,
}

struct WatchedSeries {
    key: Ident<Canonical>,
    label: String,
    /// Whether the model marks its variable non-negative.
    marked: bool,
    /// Whether a check judges it for going negative: a marked series never
    /// below zero in the model's own run, or a stock's element where no
    /// element of the stock is below zero there.
    judged: bool,
}

impl Watched {
    fn of(base: &Run, model: &datamodel::Model) -> Watched {
        let by_ident: HashMap<String, &Variable> = model
            .variables
            .iter()
            .map(|v| (crate::canonicalize(v.get_ident()).into_owned(), v))
            .collect();
        let times = base.times();
        let mut keys: Vec<(&Ident<Canonical>, usize)> = base
            .results
            .offsets
            .iter()
            .map(|(key, &offset)| (key, offset))
            .collect();
        keys.sort_by_key(|(_, offset)| *offset);
        let declared = super::series::declared(model);
        fn column_owner<'k>(
            key: &'k Ident<Canonical>,
            declared: &std::collections::BTreeSet<String>,
        ) -> Option<&'k str> {
            crate::save_check::column_variable(key.as_str(), declared)
        }
        let owner = |key| column_owner(key, &declared);
        // A stock below zero at any element in the model's own run is a
        // quantity with a sign (a heat anomaly, a balance): none of its
        // elements is judged for going negative.
        let signed: HashSet<&str> = keys
            .iter()
            .filter(|(key, offset)| {
                matches!(
                    owner(key).and_then(|ident| by_ident.get(ident)),
                    Some(Variable::Stock(_))
                ) && negative_in_run(
                    &base.results,
                    model,
                    &base.plan,
                    key.as_str(),
                    &base.series(*offset),
                )
                .is_some()
            })
            .filter_map(|(key, _)| owner(key))
            .collect();
        let mut series = Vec::new();
        let mut own = Vec::new();
        for (key, offset) in keys {
            let Some((ident, var)) =
                owner(key).and_then(|ident| Some((ident, by_ident.get(ident)?)))
            else {
                continue;
            };
            let label = format!("{}{}", var.get_ident(), &key.as_str()[ident.len()..]);
            let values = base.series(offset);
            if let Some(row) = values.iter().position(|v| !v.is_finite()) {
                own.push(Problem {
                    kind: ProblemKind::NonFinite,
                    variable: label,
                    time: round(times[row]),
                    value: None,
                    non_negative: false,
                });
                continue;
            }
            let marked = match var {
                Variable::Stock(stock) => stock.compat.non_negative,
                Variable::Flow(flow) => flow.compat.non_negative,
                Variable::Aux(_) | Variable::Module(_) => false,
            };
            let negative = negative_in_run(&base.results, model, &base.plan, key.as_str(), &values);
            if marked && let Some(row) = negative {
                own.push(Problem {
                    kind: ProblemKind::GoesNegative,
                    variable: label.clone(),
                    time: round(times[row]),
                    value: Some(round(values.iter().copied().fold(f64::INFINITY, f64::min))),
                    non_negative: true,
                });
            }
            let judged = negative.is_none()
                && (marked || (matches!(var, Variable::Stock(_)) && !signed.contains(ident)));
            series.push(WatchedSeries {
                key: key.clone(),
                label,
                marked,
                judged,
            });
        }
        own.sort_by(|a, b| {
            (a.kind != ProblemKind::NonFinite)
                .cmp(&(b.kind != ProblemKind::NonFinite))
                .then(a.time.total_cmp(&b.time))
        });
        Watched { series, own }
    }

    /// The check of the model's own run, with nothing changed.
    fn own_run(&self) -> Check {
        let mut check = Check::new(TestName::ExtremeConditions, None, Some(Condition::OwnRun));
        check.result.outcome = outcome_of(&self.own);
        check.result.note = marked_note(&self.own);
        check.result.problems = self.own.iter().take(MAX_DETAILS).cloned().collect();
        check
    }

    /// What went wrong in `results` that did not in the model's run: values
    /// that became NaN or infinite, and judged series that went below zero;
    /// non-finite values first, then by time.
    fn problems(
        &self,
        model: &datamodel::Model,
        plan: &RunPlan,
        results: &Results,
    ) -> Vec<Problem> {
        let rows: Vec<&[f64]> = results.iter().collect();
        let time = |row: usize| round(rows[row][crate::results::TIME_OFF]);
        let mut problems = Vec::new();
        for watched in &self.series {
            let Some(&offset) = results.offsets.get(&watched.key) else {
                continue;
            };
            let values: Vec<f64> = rows.iter().map(|r| r[offset]).collect();
            if let Some(row) = values.iter().position(|v| !v.is_finite()) {
                problems.push(Problem {
                    kind: ProblemKind::NonFinite,
                    variable: watched.label.clone(),
                    time: time(row),
                    value: None,
                    non_negative: false,
                });
            } else if watched.judged
                && let Some(row) =
                    negative_in_run(results, model, plan, watched.key.as_str(), &values)
            {
                problems.push(Problem {
                    kind: ProblemKind::GoesNegative,
                    variable: watched.label.clone(),
                    time: time(row),
                    value: Some(round(values.iter().copied().fold(f64::INFINITY, f64::min))),
                    non_negative: watched.marked,
                });
            }
        }
        problems.sort_by(|a, b| {
            (a.kind != ProblemKind::NonFinite)
                .cmp(&(b.kind != ProblemKind::NonFinite))
                .then(a.time.total_cmp(&b.time))
        });
        problems
    }
}

/// What `problems` rest on that their numbers do not show: a non-negative
/// marking, which this engine does not enforce.
fn marked_note(problems: &[Problem]) -> Option<String> {
    let marked: Vec<&str> = problems
        .iter()
        .filter(|p| p.kind == ProblemKind::GoesNegative && p.non_negative)
        .map(|p| p.variable.as_str())
        .collect();
    if marked.is_empty() {
        return None;
    }
    let more = marked.len().saturating_sub(MAX_DETAILS);
    let shown = marked[..marked.len().min(MAX_DETAILS)].join(", ");
    let one = marked.len() == 1;
    Some(format!(
        "{shown}{} {} marked non-negative, which this engine does not enforce: a tool that \
         enforces the marking would hold {} at zero.",
        if more > 0 {
            format!(" and {more} more")
        } else {
            String::new()
        },
        if one { "is" } else { "are" },
        if one { "it" } else { "them" },
    ))
}

/// The first row at which `values` is below zero by more than the residue
/// floating point leaves at `scale` (`behavior::residue_bound`): quantities
/// of that magnitude that cancel -- a stock drained to empty, a balance of
/// two flows -- leave no more than that below zero, which is the arithmetic,
/// not a stock passing zero. A scale of zero counts every value below zero.
fn goes_negative(values: &[f64], scale: f64) -> Option<usize> {
    let bound = residue_bound(scale);
    values.iter().position(|&v| v < -bound)
}

/// [`goes_negative`] for the series under results key `key` in `results`,
/// at the scale of what it is computed from in that run, or of its own
/// magnitude where that is larger (`series::scale_in_run`, as every tool
/// reads a series at).
fn negative_in_run(
    results: &Results,
    model: &datamodel::Model,
    plan: &RunPlan,
    key: &str,
    values: &[f64],
) -> Option<usize> {
    // The scale is a walk of the series' flows' equations: read it only for
    // a series with a value below zero to judge.
    if !values.iter().any(|&v| v < 0.0) {
        return None;
    }
    goes_negative(
        values,
        scale_in_run(results, model, plan, key).max(magnitude(values)),
    )
}

/// The specs of the model's run at one `divisor`th (a whole number) of its
/// DT, saving the times the model's run saves among its rows, so the finer
/// run is read at the model's own times ([`rows_at`]).
///
/// The model's run saves row `m` at the first of its steps at or after the
/// row's save time (`results::Specs::saved_row_step`). A save step that is a
/// whole number of DTs is one of the finer DT too, and the finer run saves
/// exactly the model's rows. One that is not lands each row on a step of the
/// model's grid that the finer grid reaches earlier, so the finer run saves
/// every one of the model's steps instead, among which are its rows; it
/// holds more rows than the model's run, and the cost check of a run
/// (`runs::over_budget`) says when that is too many.
fn finer_specs(specs: &crate::results::Specs, divisor: f64) -> SpecsChange {
    let on_the_grid = crate::results::save_step_is_on_the_step_grid(specs.dt, specs.save_step);
    SpecsChange {
        dt: Some(specs.dt / divisor),
        save_step: Some(if on_the_grid {
            specs.save_step
        } else {
            specs.dt
        }),
        ..SpecsChange::default()
    }
}

/// For each of `times` (increasing), the row of a run saved at `saved`
/// (increasing) that is at that time: the last saved within a fraction of
/// `dt` after it, the model's DT, which the two clocks' rounding is far
/// inside and the next row (a DT or more later) far outside.
fn rows_at(saved: &[f64], times: &[f64], dt: f64) -> Vec<usize> {
    let margin = dt / 8.0;
    let mut row = 0;
    times
        .iter()
        .map(|&t| {
            while row + 1 < saved.len() && saved[row + 1] <= t + margin {
                row += 1;
            }
            row
        })
        .collect()
}

/// The order a method's error shrinks at as DT does, on a smooth model: what
/// an estimate assumes where the runs cannot show it.
fn nominal_order(method: crate::results::Method) -> f64 {
    match method {
        crate::results::Method::Euler => 1.0,
        crate::results::Method::RungeKutta2 => 2.0,
        crate::results::Method::RungeKutta4 => 4.0,
    }
}

/// The variables whose equations read DT, as the model names them.
fn reading_dt<'m>(parsed: &Parsed<'m>) -> Vec<&'m str> {
    fn reads(expr: &Expr0) -> bool {
        match expr {
            Expr0::Const(..) | Expr0::Var(..) => false,
            Expr0::Subscript(_, indices, _) => indices
                .iter()
                .any(|index| matches!(index, IndexExpr0::Expr(e) if reads(e))),
            Expr0::App(UntypedBuiltinFn(name, args), _) => {
                (args.is_empty() && builtin_called(name) == Some("time_step"))
                    || args.iter().any(reads)
            }
            Expr0::Op1(_, inner, _) => reads(inner),
            Expr0::Op2(_, l, r, _) => reads(l) || reads(r),
            Expr0::If(c, t, f, _) => reads(c) || reads(t) || reads(f),
        }
    }
    let mut names: Vec<&str> = parsed
        .equations
        .values()
        .filter(|(_, exprs)| exprs.iter().any(reads))
        .map(|(var, _)| var.get_ident())
        .collect();
    names.sort_unstable();
    names
}

/// The model's time constant with the fewest DTs to each of its stages, when
/// that is fewer than [`DTS_PER_STAGE`]: how a note names it, its stages and
/// that number of DTs. Only a time constant that paces a stock counts, named
/// (`adjustment_time = 2`) or written into a rate's equation (`the 0.1
/// order_fulfillment divides by`).
fn shortest_time_constant(
    model: &datamodel::Model,
    base: &Run,
    roles: &Roles,
) -> Option<(String, f64, f64)> {
    let dt = base.results.specs.dt;
    let mut shortest: Option<(String, f64, f64)> = None;
    let mut consider = |named: &dyn Fn() -> String, stages: f64, dts: f64| {
        if dts < DTS_PER_STAGE && shortest.as_ref().is_none_or(|(_, _, least)| dts < *least) {
            shortest = Some((named(), stages, dts));
        }
    };
    let mut names: Vec<&String> = roles.time_constants.keys().collect();
    names.sort_unstable();
    for name in names {
        let tc = roles.time_constants[name];
        // DT is put in the constant's own unit of time, which it cannot be
        // where the two lengths are not both known.
        let (Some(var), Some(per)) = (model.get_variable(name), tc.per_model_time) else {
            continue;
        };
        if !tc.paces_a_stock {
            continue;
        }
        for value in element_values(base, model, var)
            .into_iter()
            .filter(|&v| v > 0.0)
        {
            consider(
                &|| format!("{} = {}", var.get_ident(), round(value)),
                tc.stages,
                value / (dt * per * tc.stages),
            );
        }
    }
    for literal in &roles.literal_times {
        consider(
            &|| {
                format!(
                    "the {} {} divides by",
                    round(literal.value),
                    literal.variable
                )
            },
            1.0,
            literal.value / dt,
        );
    }
    shortest
}

/// Integration error, one measurement: the model's run against the same run
/// at half and at a quarter of its DT, at every element of every stock.
///
/// The two differences say how the run converges as DT shrinks. Where each
/// halving shrinks the difference (by [`CONVERGING_ORDER`] at least),
/// Richardson extrapolation gives the error at the model's DT: for a method
/// of order `p`, `error(DT) = (x(DT) - x(DT/2)) * 2^p / (2^p - 1)`, with `p`
/// the order the runs show. An estimate above [`INTEGRATION_TOLERANCE`] of a
/// stock's scale fails. Where the differences do not shrink, there is no
/// error to estimate: the model is discrete in time, or chaotic over the
/// horizon, and the check is flagged saying so, as it is where an equation
/// reads DT and the finer runs are of another model.
fn integration_error(
    ws: &mut Workspace<'_>,
    model: &datamodel::Model,
    base: &Run,
    reading: &Reading<'_>,
) -> Result<Vec<Check>, ToolError> {
    let specs = &base.results.specs;
    let mut check = Check::new(TestName::IntegrationError, None, Some(Condition::FinerDt));
    let mut finer = |divisor: f64| -> Result<Result<Run, String>, ToolError> {
        let plan = RunPlan {
            specs: finer_specs(specs, divisor),
            ..base.plan.clone()
        };
        ws.yield_point()?;
        match runs::execute(ws, model, &plan) {
            Ok(results) => Ok(Ok(Run::new(String::new(), 0, 0, plan, results))),
            Err(RunFailure::Stopped) => Err(ToolError::interrupted()),
            Err(RunFailure::Failed(reason)) => Ok(Err(reason)),
        }
    };
    let half = match finer(2.0)? {
        Ok(run) => run,
        Err(reason) => {
            return Ok(vec![check.not_run(format!(
                "the run at half the model's DT was not made: {reason}"
            ))]);
        }
    };
    // A run at a quarter of DT that costs more than a run may is done
    // without: the method's own order stands in for the one it would show.
    let quarter = finer(4.0)?.ok();

    let stocks: Vec<&Variable> = model
        .variables
        .iter()
        .filter(|v| matches!(v, Variable::Stock(_)))
        .collect();
    let base_times = base.times();
    let half_rows = rows_at(&half.times(), &base_times, specs.dt);
    let quarter_rows = quarter
        .as_ref()
        .map(|quarter| rows_at(&quarter.times(), &base_times, specs.dt));
    // Each stock's (or element's) largest difference between the model's run
    // and the run at half its DT, and between that and the run at a quarter,
    // each as a fraction of its scale in the model's run.
    let mut differences: Vec<(String, f64, f64)> = Vec::new();
    for var in &stocks {
        let every = usize::MAX;
        let series = |run: &Run| element_series_upto(run, model, var.get_ident(), None, every).0;
        let ours = series(base);
        let halved = series(&half);
        let quartered = quarter.as_ref().map(series);
        for (i, ((label, a), (_, b))) in ours.into_iter().zip(halved).enumerate() {
            let scale = scale(&a);
            let c = quartered.as_ref().and_then(|series| series.get(i));
            let (mut first, mut second) = (0.0_f64, 0.0_f64);
            for (row, &at) in half_rows.iter().enumerate() {
                let at_half = b[at];
                first = first.max((a[row] - at_half).abs() / scale);
                // The quarter-DT run saves at the times the half-DT run
                // does, so `rows[row]` and `at` index the same row: reading
                // either is the same measurement.
                if let (Some((_, c)), Some(rows)) = (c, &quarter_rows) {
                    second = second.max((at_half - c[rows[row]]).abs() / scale);
                }
            }
            differences.push((label, first, second));
        }
    }
    differences.sort_by(|a, b| b.1.total_cmp(&a.1));
    let Some((_, first, second)) = differences.first().cloned() else {
        return Ok(vec![check]);
    };
    // The order the runs converge at, by the stock that moved most: `None`
    // where they are the same run, which has no error to speak of.
    let observed = (quarter.is_some() && first > 0.0).then(|| {
        if second > 0.0 {
            (first / second).log2()
        } else {
            f64::INFINITY
        }
    });
    let order = match (&quarter, observed) {
        (None, _) => nominal_order(specs.method),
        (Some(_), Some(order)) => order,
        (Some(_), None) => f64::INFINITY,
    };
    let converges = order >= CONVERGING_ORDER;
    // Richardson's factor from a difference to an error; one where the runs
    // do not converge, and the difference is all there is to report.
    let factor = if !converges {
        1.0
    } else if order.is_finite() {
        let ratio = 2.0_f64.powf(order);
        ratio / (ratio - 1.0)
    } else {
        1.0
    };
    let worst = first * factor;
    // An order is an order of convergence: where the runs do not converge
    // there is none to give.
    check.result.order = observed
        .filter(|order| order.is_finite() && converges)
        .map(round);
    check.result.differences = differences
        .into_iter()
        .take(MAX_DETAILS)
        .map(|(variable, first, _)| Difference {
            variable,
            difference: round(first * factor),
        })
        .collect();
    // Runs that do not converge bound no error, however small the first
    // difference: they are said before the tolerance is.
    if converges && worst <= INTEGRATION_TOLERANCE {
        return Ok(vec![check]);
    }

    let mut notes: Vec<String> = Vec::new();
    let dt_readers = reading_dt(&reading.parsed);
    if !converges && !dt_readers.is_empty() {
        check.result.outcome = Outcome::Flagged;
        let more = dt_readers.len().saturating_sub(MAX_DETAILS);
        notes.push(format!(
            "The runs at finer DTs do not converge, and {}{} read{} DT: the model's equations \
             change with it, so a run at a finer DT is a run of another model, and the \
             difference is no error of integration.",
            dt_readers[..dt_readers.len().min(MAX_DETAILS)].join(", "),
            if more > 0 {
                format!(" and {more} more")
            } else {
                String::new()
            },
            if dt_readers.len() == 1 { "s" } else { "" },
        ));
    } else if !converges {
        check.result.outcome = Outcome::Flagged;
        notes.push(
            "Halving DT again changes the run as much as halving it did, so the runs do not \
             converge and there is no error to estimate: the model is discrete in time (a \
             pulse, a fixed delay, a sampled value), its behavior is chaotic over this horizon, \
             or DT is far too large for it."
                .to_string(),
        );
    } else {
        check.result.outcome = if worst <= INTEGRATION_FAILURE {
            Outcome::Flagged
        } else {
            Outcome::Failed
        };
        if quarter.is_none() {
            notes.push(format!(
                "A run at a quarter of DT costs more than a run may, so the estimate assumes \
                 the method's own order, {}.",
                nominal_order(specs.method)
            ));
        }
    }
    if let Some((named, stages, dts)) = shortest_time_constant(model, base, &reading.roles) {
        notes.push(format!(
            "The shortest time constant is {named}, {} DT{}: DT should be at most a quarter of \
             {}.",
            round(dts),
            if stages > 1.0 {
                format!(" for each of its {stages} stages")
            } else {
                String::new()
            },
            if stages > 1.0 { "a stage" } else { "it" },
        ));
    }
    check.result.note = (!notes.is_empty()).then(|| notes.join(" "));
    Ok(vec![check])
}

/// A series' scale: the larger of its range and its largest magnitude.
fn scale(series: &[f64]) -> f64 {
    let (min, max) = series
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &v| {
            (lo.min(v), hi.max(v))
        });
    (max - min)
        .max(min.abs())
        .max(max.abs())
        .max(f64::MIN_POSITIVE)
}

/// A behavior's family: what a change of behavior means, as the classifier's
/// label does not. Which of linear, exponential, goal seeking or S-shaped
/// growth a series reads as depends on the horizon as much as on the
/// structure: they are one family, which a change of pace does not leave.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum BehaviorFamily {
    /// It does not move.
    Still,
    /// It rises, without a material turn.
    Rising,
    /// It falls, without a material turn.
    Falling,
    /// It rises, turns once, and falls.
    RisesThenFalls,
    /// It falls, turns once, and rises.
    FallsThenRises,
    /// It turns twice or more.
    Oscillating,
}

impl BehaviorFamily {
    pub const ALL: [BehaviorFamily; 6] = [
        BehaviorFamily::Still,
        BehaviorFamily::Rising,
        BehaviorFamily::Falling,
        BehaviorFamily::RisesThenFalls,
        BehaviorFamily::FallsThenRises,
        BehaviorFamily::Oscillating,
    ];

    /// The family of a series of `values` the classifier named `mode`: the
    /// movements it named it for, with pace set aside. An overshoot is a
    /// turn when it comes back [`MATERIAL_CHANGE`] of the series' range or
    /// more; one that comes back less is the movement it ends, a series a
    /// few percent past where it settles. None for a series that is not a
    /// number somewhere.
    fn of(mode: &BehaviorMode, values: &[f64]) -> Option<BehaviorFamily> {
        use BehaviorFamily::*;
        let falling = mode.direction == Some(Direction::Falling);
        let way =
            |rising: BehaviorFamily, fall: BehaviorFamily| if falling { fall } else { rising };
        // Per-variant semantics: the family each of the classifier's modes
        // is of.
        Some(match mode.kind {
            ModeKind::AtRest => Still,
            ModeKind::Undefined => return None,
            ModeKind::Linear
            | ModeKind::Exponential
            | ModeKind::GoalSeeking
            | ModeKind::SShaped
            | ModeKind::Other => way(Rising, Falling),
            ModeKind::Overshoot if come_back(values, falling) >= MATERIAL_CHANGE => {
                way(RisesThenFalls, FallsThenRises)
            }
            ModeKind::Overshoot => way(Rising, Falling),
            ModeKind::RiseAndFall => RisesThenFalls,
            ModeKind::FallAndRise => FallsThenRises,
            ModeKind::Oscillation => Oscillating,
        })
    }

    /// The family a series' material turns make it: its legs between its
    /// start, its turning points (the classifier's, `Shape::turns`) and its
    /// end, those under `material` of its range folded away
    /// ([`material_legs`]); none for a series that is not a number
    /// somewhere.
    fn of_turns(shape: &Shape, values: &[f64], material: f64) -> Option<BehaviorFamily> {
        if shape.mode.kind == ModeKind::Undefined {
            return None;
        }
        Some(
            match material_legs(values, &shape.turns, material).as_slice() {
                [] => BehaviorFamily::Still,
                [only] if *only >= 0.0 => BehaviorFamily::Rising,
                [_] => BehaviorFamily::Falling,
                [first, _] if *first >= 0.0 => BehaviorFamily::RisesThenFalls,
                [_, _] => BehaviorFamily::FallsThenRises,
                [..] => BehaviorFamily::Oscillating,
            },
        )
    }
}

/// How far an overshoot comes back from the furthest it went, as a fraction
/// of the series' range: from its greatest value to its last for one that
/// rose, from its least for one that `fell`.
fn come_back(values: &[f64], fell: bool) -> f64 {
    let (lo, hi) = values
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &v| {
            (lo.min(v), hi.max(v))
        });
    let (Some(&last), true) = (values.last(), hi > lo) else {
        return 0.0;
    };
    if fell { last - lo } else { hi - last }.abs() / (hi - lo)
}

/// The legs of a series between its start, its turning points (`turns`, the
/// classifier's) and its end, each as a signed fraction of the series' range,
/// with every leg under `material` folded into its neighbors: an inner leg
/// goes with its two ends, joining the legs either side of it, which run the
/// same way; a leg at the start or the end goes with its turning point. What
/// is left is the series' material turns.
fn material_legs(values: &[f64], turns: &[usize], material: f64) -> Vec<f64> {
    let Some(last) = values.len().checked_sub(1) else {
        return vec![];
    };
    let (lo, hi) = values
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &v| {
            (lo.min(v), hi.max(v))
        });
    let range = hi - lo;
    if range.is_nan() || range <= 0.0 {
        return vec![];
    }
    let mut points: Vec<usize> = std::iter::once(0)
        .chain(turns.iter().copied())
        .chain(std::iter::once(last))
        .collect();
    loop {
        let legs: Vec<f64> = points
            .windows(2)
            .map(|w| (values[w[1]] - values[w[0]]) / range)
            .collect();
        let smallest = legs
            .iter()
            .enumerate()
            .min_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
            .map(|(i, leg)| (i, leg.abs()));
        match smallest {
            Some((i, size)) if legs.len() > 1 && size < material => {
                if i == 0 {
                    points.remove(1);
                } else if i == legs.len() - 1 {
                    points.remove(points.len() - 2);
                } else {
                    points.remove(i + 1);
                    points.remove(i);
                }
            }
            _ => return legs,
        }
    }
}

/// How a check's recorded variables responded ([`responses`]).
struct Responded {
    /// The responses the check lists, the most telling first, at most
    /// [`MAX_DETAILS`].
    listed: Vec<Response>,
    /// How many series the check made undefined or changed the behavior of
    /// materially beyond those listed.
    more_changes: usize,
    /// How strong the strongest response was: its largest change, infinite
    /// for a series made undefined, so a check that did that is never passed
    /// over.
    strength: f64,
}

/// How each recorded variable of `run` responded, against `base`: the series
/// the check made undefined, then the material changes of behavior, then the
/// largest changes, as many as a check lists, with how many more series
/// changed behavior.
///
/// A change of mode is reported when the classifier's label changes; it is
/// material when the behavior changes family ([`changed_family`]) and the
/// series moved at least [`MATERIAL_CHANGE`] of its scale somewhere in the
/// run, and the response then says both families. A series not a number
/// somewhere in either run has no change to report, and its numbers are left
/// out.
///
/// The two runs are compared at the model's own times (a check run again at
/// a finer DT saves more rows), and each series is classified in each run at
/// that run's scale, the two sharing the larger of their magnitudes
/// (`series::compared_scales`), so a check that only changes the series'
/// scale does not change its mode.
fn responses(run: &Run, base: &Run, model: &datamodel::Model, record: &[&Variable]) -> Responded {
    let base_times = base.times();
    let rows = rows_at(&run.times(), &base_times, base.results.specs.dt);
    let mut all: Vec<Response> = Vec::new();
    for var in record {
        let (theirs, _) = keyed_series_upto(run, model, var.get_ident(), None, MAX_ELEMENTS);
        let (ours, _) = keyed_series_upto(base, model, var.get_ident(), None, MAX_ELEMENTS);
        for KeyedSeries { label, key, values } in theirs {
            let Some(base_values) = ours
                .iter()
                .find(|series| series.key == key)
                .map(|series| series.values.as_slice())
            else {
                continue;
            };
            let (this_scale, base_scale) =
                compared_scales(model, &key, (run, &values), Some((base, base_values)));
            let base_scale = base_scale.unwrap_or(this_scale);
            // A difference is residue at the larger of the two scales.
            let shared = this_scale.max(base_scale);
            let values: Vec<f64> = rows.iter().map(|&row| values[row]).collect();
            let (Some(&last), Some(&base_last)) = (values.last(), base_values.last()) else {
                continue;
            };
            let now = shape_at(&base_times, &values, this_scale);
            let was = shape_at(&base_times, base_values, base_scale);
            let defined = |series: &[f64]| series.iter().all(|v| v.is_finite());
            let (defined, base_defined) = (defined(&values), defined(base_values));
            let (change, largest_change) = if defined && base_defined {
                // A series the model's run holds at zero (residue aside) has
                // no range or magnitude of its own to measure a change by: it
                // is measured by what it is computed from.
                let scale = if magnitude(base_values) <= residue_bound(shared) {
                    shared.max(f64::MIN_POSITIVE)
                } else {
                    scale(base_values)
                };
                // A difference within the residue of what the series is
                // computed from is the arithmetic's, not a change.
                let residue = residue_bound(shared);
                let moved = |a: f64, b: f64| if (a - b).abs() <= residue { 0.0 } else { a - b };
                let largest = values
                    .iter()
                    .zip(base_values)
                    .map(|(&a, &b)| moved(a, b).abs() / scale)
                    .fold(0.0_f64, f64::max);
                let finite = |x: f64| x.is_finite().then_some(x);
                (
                    finite(round(moved(last, base_last) / scale)),
                    finite(round(largest)),
                )
            } else {
                (None, None)
            };
            // A series that barely moves beside its level in both runs has
            // no behavior to change: its family is its rounding's.
            let changed = material_change(
                changed_family((&now, &values), (&was, base_values)),
                largest_change,
            )
            .filter(|_| moves(&values) || moves(base_values));
            all.push(Response {
                variable: label,
                change,
                largest_change,
                mode: now.mode.kind,
                was: (now.mode.kind != was.mode.kind).then_some(was.mode.kind),
                damping: now.mode.damping,
                was_damping: was
                    .mode
                    .damping
                    .filter(|_| now.mode.damping != was.mode.damping),
                family: changed.map(|(family, _)| family),
                was_family: changed.map(|(_, was)| was),
                went_undefined: !defined && base_defined,
            });
        }
    }
    let strongest = if all.iter().any(|r| r.went_undefined) {
        f64::INFINITY
    } else {
        all.iter()
            .filter_map(|r| r.largest_change)
            .fold(0.0_f64, f64::max)
    };
    let largest = |r: &Response| r.largest_change.unwrap_or(f64::NEG_INFINITY);
    all.sort_by(|a, b| {
        b.went_undefined
            .cmp(&a.went_undefined)
            .then(material(b).cmp(&material(a)))
            .then(largest(b).total_cmp(&largest(a)))
    });
    let found = all
        .iter()
        .filter(|r| r.went_undefined || material(r))
        .count();
    all.truncate(MAX_DETAILS);
    Responded {
        more_changes: found.saturating_sub(all.len()),
        listed: all,
        strength: strongest,
    }
}

/// Whether a check found something in its responses: a series it made
/// undefined, which a check that ran is never passed with.
fn made_undefined(responses: &[Response]) -> bool {
    responses.iter().any(|r| r.went_undefined)
}

/// The family a behavior is of and the one it was of, each a series with the
/// shape the classifier read it as, when they differ: their modes' families
/// ([`BehaviorFamily::of`]) differ, and the series' turns at
/// [`MATERIAL_CHANGE`] of its range do too.
///
/// The modes say what changed, in the classifier's words, so a response's
/// mode and family never disagree. The turns confirm the change is more than
/// a movement on one side of a threshold of the classifier's in one run and
/// on the other in the other: a second rise of 9% of the range in one run
/// and 11% in the other names two modes, and is one series.
fn changed_family(
    now: (&Shape, &[f64]),
    was: (&Shape, &[f64]),
) -> Option<(BehaviorFamily, BehaviorFamily)> {
    // Goal seeking is one behavior whichever side of its goal a series
    // starts: a goal seeker whose goal moved past its start still seeks it.
    if now.0.mode.kind == ModeKind::GoalSeeking && was.0.mode.kind == ModeKind::GoalSeeking {
        return None;
    }
    let families = (
        BehaviorFamily::of(&now.0.mode, now.1)?,
        BehaviorFamily::of(&was.0.mode, was.1)?,
    );
    let turns_differ = |material: f64| {
        BehaviorFamily::of_turns(now.0, now.1, material)
            != BehaviorFamily::of_turns(was.0, was.1, material)
    };
    (families.0 != families.1 && turns_differ(MATERIAL_CHANGE)).then_some(families)
}

/// Whether a series moves materially: its range is [`MATERIAL_CHANGE`] of
/// its scale or more.
fn moves(series: &[f64]) -> bool {
    let (lo, hi) = series
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &v| {
            (lo.min(v), hi.max(v))
        });
    hi - lo >= MATERIAL_CHANGE * scale(series)
}

/// A change of family, when it is material: the series moved
/// [`MATERIAL_CHANGE`] of its scale somewhere in the run.
fn material_change(
    changed: Option<(BehaviorFamily, BehaviorFamily)>,
    largest_change: Option<f64>,
) -> Option<(BehaviorFamily, BehaviorFamily)> {
    changed.filter(|_| largest_change.is_some_and(|change| change >= MATERIAL_CHANGE))
}

/// Whether a response changed behavior materially ([`material_change`]): it
/// then says the family it is of and the one it was of.
fn material(response: &Response) -> bool {
    response.was_family.is_some()
}

/// Why a test that changes a constant by a factor does not run on `var`, a
/// date.
fn date_reason(var: &Variable) -> String {
    format!(
        "'{}' is used as a point in time, and a multiple of a date is no condition of the \
         system; move it with run_experiment",
        var.get_ident()
    )
}

/// Sensitivity: each target at half and at double, within its extremes
/// ([`Roles::half_and_double`]).
fn sensitivity(
    ws: &mut Workspace<'_>,
    model: &datamodel::Model,
    base: &Run,
    roles: &Roles,
    targets: &[&Variable],
    record: &[&Variable],
) -> Result<Vec<Check>, ToolError> {
    let dt = base.results.specs.dt;
    let mut planned: Vec<(Check, RunPlan)> = Vec::new();
    let mut checks = Vec::new();
    for &var in targets {
        let current = element_values(base, model, var);
        let (half, double) = roles.half_and_double(var, dt);
        let changes: [(Condition, &dyn Fn(f64) -> f64); 2] =
            [(Condition::Half, &half), (Condition::Double, &double)];
        for (condition, to) in changes {
            let check = Check::new(TestName::Sensitivity, Some(var), Some(condition));
            if roles.is_date(var) {
                checks.push(check.not_run(date_reason(var)));
                continue;
            }
            // A constant its half or double leaves where it is has no check
            // there: zero, a share at the whole, a time constant at its low
            // extreme.
            if settable(var).is_ok()
                && !current.is_empty()
                && current.iter().all(|&value| to(value) == value)
            {
                continue;
            }
            let mut check = check;
            // Per-variant semantics: what holds each condition back.
            check.result.held_at = current.first().and_then(|&value| match condition {
                Condition::Half if to(value) != value * 0.5 => Some(ExtremeRule::ShortTime),
                Condition::Double if to(value) != value * 2.0 => {
                    Some(if roles.share_whole(var).is_some() {
                        ExtremeRule::Whole
                    } else {
                        ExtremeRule::FastestRate
                    })
                }
                Condition::Half
                | Condition::Double
                | Condition::Low
                | Condition::High
                | Condition::OwnRun
                | Condition::FinerDt
                | Condition::Held
                | Condition::Step => None,
            });
            match change(base, var, None, to) {
                Err(reason) => checks.push(check.not_run(reason)),
                Ok((plan, value)) => {
                    check.result.value = value;
                    planned.push((check, plan));
                }
            }
        }
    }
    let plans: Vec<RunPlan> = planned.iter().map(|(_, plan)| plan.clone()).collect();
    let responded = runs::execute_values(ws, model, &plans, |plan, results| {
        let run = Run::new(String::new(), 0, 0, plan.clone(), results);
        responses(&run, base, model, record)
    })?;
    for ((mut check, _), responded) in planned.into_iter().zip(responded) {
        match responded {
            Err(reason) => checks.push(check.not_run(format!("the check's run fails: {reason}"))),
            Ok(found) => {
                if found.listed.iter().any(material) || made_undefined(&found.listed) {
                    check.result.outcome = Outcome::Flagged;
                }
                check.strength = found.strength;
                check.result.more_changes = found.more_changes;
                check.result.responses = found.listed;
                checks.push(check);
            }
        }
    }
    Ok(checks)
}

/// Loop knockouts: each target held at its initial value, each element of an
/// arrayed one at its own.
fn loop_knockout(
    runs: &mut RunStore,
    evidence: &mut Evidence,
    ws: &mut Workspace<'_>,
    resolved: &ResolvedModel<'_>,
    base: &Run,
    targets: &[&Variable],
    record: &[&Variable],
) -> Result<Vec<Check>, ToolError> {
    let model = resolved.model;
    let key = runs.key(ws);
    targets
        .iter()
        .map(|&var| {
            let check = Check::new(TestName::LoopKnockout, Some(var), Some(Condition::Held));
            let name = var.get_ident();
            match var {
                Variable::Stock(_) => {
                    return Ok(check.not_run(format!(
                        "'{name}' is a stock, which its flows change; hold its flows instead"
                    )));
                }
                Variable::Module(_) => {
                    return Ok(check.not_run(format!("'{name}' is a module; hold what it outputs")));
                }
                _ if settable(var).is_ok() => {
                    return Ok(check.not_run(format!(
                        "'{name}' is a constant, held already; hold a variable that reads it"
                    )));
                }
                _ => {}
            }
            let held = element_start_values(base, model, var);
            if held.is_empty() {
                return Ok(check.not_run(format!("'{name}' has no value in the run")));
            }
            if let Some((element, value)) = held.iter().find(|(_, v)| !v.is_finite()) {
                let at = if element.is_empty() {
                    name.to_string()
                } else {
                    format!("{name}[{element}]")
                };
                return Ok(check.not_run(format!("'{at}' starts at {value}, which cannot be held")));
            }
            let scalar = held.len() == 1 && held[0].0.is_empty();
            let replacement = if scalar {
                Replacement::Equation(format!("{}", held[0].1))
            } else {
                Replacement::Elements(
                    held.iter()
                        .map(|(element, value)| (element.clone(), format!("{value}")))
                        .collect(),
                )
            };
            let plan = RunPlan {
                equations: vec![EquationChange {
                    variable: crate::canonicalize(name).into_owned(),
                    replacement,
                }],
                ..RunPlan::default()
            };
            ws.yield_point()?;
            let results = match runs::execute(ws, model, &plan) {
                Ok(results) => results,
                Err(RunFailure::Stopped) => return Err(ToolError::interrupted()),
                Err(RunFailure::Failed(reason)) => {
                    return Ok(check.not_run(format!("the check's run fails: {reason}")));
                }
            };
            let run = std::sync::Arc::new(Run::new(String::new(), ws.revision, key, plan, results));
            let mut check = check;
            check.result.value = scalar.then(|| round(held[0].1));
            let found = responses(&run, base, model, record);
            check.result.more_changes = found.more_changes;
            check.result.responses = found.listed;
            check.result.outcome = if made_undefined(&check.result.responses) {
                Outcome::Flagged
            } else {
                Outcome::Observed
            };
            match analysis_of(runs, ws, model, resolved.source_model, &run) {
                Ok(analysis) => {
                    let (links, loops) = cut_of(evidence, &analysis, model);
                    check.result.cut_links = links;
                    check.result.loops = loops;
                }
                Err(err) if err.is_interrupted() => return Err(err),
                Err(err) => check.result.reason = Some(err.error),
            }
            Ok(check)
        })
        .collect()
}

/// Disturbances: each target stepped up by a tenth, a tenth of the way into
/// the run.
#[allow(clippy::too_many_arguments)]
fn disturbance(
    runs: &mut RunStore,
    evidence: &mut Evidence,
    ws: &mut Workspace<'_>,
    resolved: &ResolvedModel<'_>,
    base: &Run,
    roles: &Roles,
    targets: &[&Variable],
    record: &[&Variable],
) -> Result<Vec<Check>, ToolError> {
    let model = resolved.model;
    let key = runs.key(ws);
    let specs = &base.results.specs;
    let at = base.times()[base.row_at(specs.start + (specs.stop - specs.start) / 10.0)];
    targets
        .iter()
        .map(|&var| {
            let mut check = Check::new(TestName::Disturbance, Some(var), Some(Condition::Step));
            check.result.from_time = Some(round(at));
            if roles.is_date(var) {
                return Ok(check.not_run(date_reason(var)));
            }
            if is_zero_valued(base, model, var) {
                return Ok(check.not_run(format!(
                    "'{}' is zero, which a step of a tenth leaves as it is; step it with \
                     run_experiment",
                    var.get_ident()
                )));
            }
            let (plan, value) = match change(base, var, Some(at), |x| x * (1.0 + STEP_FRACTION)) {
                Ok(planned) => planned,
                Err(reason) => return Ok(check.not_run(reason)),
            };
            ws.yield_point()?;
            let results = match runs::execute(ws, model, &plan) {
                Ok(results) => results,
                Err(RunFailure::Stopped) => return Err(ToolError::interrupted()),
                Err(RunFailure::Failed(reason)) => {
                    return Ok(check.not_run(format!("the check's run fails: {reason}")));
                }
            };
            let run = std::sync::Arc::new(Run::new(String::new(), ws.revision, key, plan, results));
            check.result.value = value;
            let found = responses(&run, base, model, record);
            check.result.more_changes = found.more_changes;
            check.result.responses = found.listed;
            check.result.outcome = if made_undefined(&check.result.responses) {
                Outcome::Flagged
            } else {
                Outcome::Observed
            };
            match analysis_of(runs, ws, model, resolved.source_model, &run) {
                Ok(analysis) => {
                    check.result.loops = leaders_after(evidence, &analysis, at, MAX_DETAILS);
                }
                Err(err) if err.is_interrupted() => return Err(err),
                Err(err) => check.result.reason = Some(err.error),
            }
            Ok(check)
        })
        .collect()
}

#[cfg(test)]
#[path = "battery_tests.rs"]
mod tests;
