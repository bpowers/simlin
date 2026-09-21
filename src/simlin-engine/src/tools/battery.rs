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
//! - `units`: the engine's unit diagnostics, by id.
//! - `extreme_conditions`: each targeted constant at its low extreme and at
//!   its high one. The low extreme is zero, except for a time constant, whose
//!   low extreme is DT: a time constant below DT is an integration artifact,
//!   not a condition of the system, and zero divides by it. The high extreme
//!   is ten times the constant's value, except for a share or fraction, whose
//!   high extreme is the whole. A check fails when a value becomes NaN or
//!   infinite that is not so in the model's own run; a stock, or a flow the
//!   model marks non-negative, that goes negative when the model's run does
//!   not is flagged for judgment. Values that grow very large are not judged:
//!   growth is what ten times a growth rate should do.
//! - `integration_error`: the run at half the DT, and under RK4; a stock (any
//!   element of one) whose largest difference from the model's run exceeds 1%
//!   of its scale fails.
//! - `sensitivity`: each targeted constant at half and at double (a share at
//!   most the whole); a check is flagged when a recorded variable's behavior
//!   changes family materially (the series moves 5% of its scale somewhere, so
//!   a label flipping at the classifier's boundary is not a change of
//!   behavior), and the strongest responses are reported however they come
//!   out.
//! - `loop_knockout`: a targeted variable held at its initial value (each
//!   element at its own): the links and loops that cuts, and how the recorded
//!   variables respond.
//! - `disturbance`: a 10% step in a targeted constant a tenth of the way into
//!   the run: how the recorded variables respond, and the loops that lead
//!   after it. A model at rest hides its loops, and this is how the battery
//!   sees its structure.
//!
//! The targeted tests' default targets are the constants that feed the
//! model's flows, less its unit conversions: a constant whose value is one of
//! its units (`1e6 tons/Mton`, `one_year = 1 year`) changes the units a
//! quantity is counted in, not the quantity (`Roles`). A time constant is
//! recognized by its units where it has them, and by its role where it has
//! none (`time_constants`); each check at DT says which.
//!
//! A check that passed is counted and not listed, except sensitivity's
//! strongest responses; every other check is listed under an id (`T1`, ...)
//! keyed by its test, target and condition, so the same check keeps its id
//! when it is run again after an edit.

use std::collections::{HashMap, HashSet, VecDeque};

use serde::{Deserialize, Serialize};

#[cfg(feature = "schema")]
use schemars::JsonSchema;

use crate::ast::{BinaryOp, Expr0, IndexExpr0, Literal, UnaryOp};
use crate::builtins::UntypedBuiltinFn;
use crate::common::{Canonical, Ident};
use crate::datamodel::{self, Equation, UnitMap, Variable};
use crate::lexer::LexerType;
use crate::ltm::strip_subscript;
use crate::results::Results;

use super::behavior::{BehaviorMode, Direction, ModeKind, classify};
use super::evidence::Evidence;
use super::experiment::{settable, value_change};
use super::loops::{CutLink, analysis_of, cut_of, leaders_after};
use super::outline::IntegrationMethod;
use super::runs::{
    self, EquationChange, Replacement, Run, RunFailure, RunPlan, RunStore, SpecsChange, ValueChange,
};
use super::series::{element_series, element_series_upto, round};
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

/// A stock's largest difference, as a fraction of its scale, above which the
/// integration error test fails.
pub(crate) const INTEGRATION_TOLERANCE: f64 = 0.01;

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

/// The functions whose second argument is a time: a delay's, a smooth's or a
/// trend's.
const TIME_ARGUMENT_FUNCTIONS: [&str; 7] = [
    "smth1", "smth3", "delay1", "delay3", "delayn", "trend", "delay",
];

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

/// Number words a constant's name may state its value in.
const NUMBER_WORDS: [(&str, f64); 7] = [
    ("one", 1.0),
    ("ten", 10.0),
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
    /// A constant at zero.
    Zero,
    /// A time constant at its low extreme: DT, or three DTs for a
    /// third-order delay's or smooth's time, whose stages each take a third
    /// of it.
    Dt,
    /// A constant at ten times its value.
    TenTimes,
    /// A share or fraction at its high extreme, the whole: 1, or 100 for one
    /// in percent.
    Whole,
    /// A constant at half its value.
    Half,
    /// A constant at double its value (a share at most the whole).
    Double,
    /// The run at half its DT.
    HalfDt,
    /// The run under fourth-order Runge-Kutta.
    Rk4,
    /// A variable held at its initial value, each element of an arrayed one
    /// at its own.
    Held,
    /// A constant stepped up by a tenth, a tenth of the way into the run.
    Step,
}

/// Why the battery took a constant for a time constant, and so tested it at
/// DT rather than zero.
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
    /// extreme conditions, sensitivity and disturbance take constants (the
    /// constants that feed the model's flows by default, less its unit
    /// conversions); a loop knockout holds variables at their initial
    /// values, and runs only on targets named here. At most 12.
    #[serde(default)]
    #[cfg_attr(feature = "schema", schemars(length(max = 12)))]
    pub targets: Vec<String>,
    /// The variables whose responses sensitivity, knockouts and disturbances
    /// report: the model's stocks when absent. At most 12.
    #[serde(default)]
    #[cfg_attr(feature = "schema", schemars(length(max = 12)))]
    pub record: Vec<String>,
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
    /// What the test left out and why: the unit conversions its defaults
    /// leave out, and series that are not a number in the model's own run.
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
    /// Why a check at DT took its constant for a time constant: if that is
    /// wrong, the check tested the wrong extreme.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_constant: Option<TimeConstantEvidence>,
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
    /// The stocks (or elements) that differed most from the model's run
    /// (integration error).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub differences: Vec<Difference>,
    /// How recorded variables responded, the largest changes and every
    /// material change of behavior (sensitivity, knockout, disturbance).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub responses: Vec<Response>,
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
    /// What the outcome rests on that the run cannot show: a non-negative
    /// marking this engine does not enforce.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
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
    /// Its largest difference from the model's run, as a fraction of its
    /// scale (the larger of its range and its largest magnitude).
    pub difference: f64,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct Response {
    pub variable: String,
    /// How far its final value moved from the model's run, as a fraction of
    /// its scale there: positive up, negative down. Left out when either run
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
    /// Whether the behavior changed family (still, rising, falling, one
    /// turn, oscillating), not only its label: exponential growth that
    /// reads as linear over a shorter horizon has not.
    #[serde(skip)]
    changed_family: bool,
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
    revision: u64,
    outcome: Outcome,
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
    let (key, record_names) = (record.key.clone(), record.record.clone());
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
    let roles = || {
        let graph = Graph::of(ws.db, resolved);
        let units = Units::of(ws, resolved);
        Roles::of(model, &graph, &units, &Parsed::of(model), &base)
    };
    // A check that stopped for other work answers with the stop's words;
    // the verifier stops at the next citation, while the work still waits.
    let stopped = |err: ToolError| err.error;
    let checks = match key.test {
        TestName::Units => vec![units_check(&mut session.evidence, ws, resolved)],
        TestName::ExtremeConditions => {
            let roles = roles();
            extreme_conditions(ws, model, &base, &roles, &targets)
                .map_err(stopped)?
                .checks
        }
        TestName::IntegrationError => integration_error(ws, model, &base).map_err(stopped)?,
        TestName::Sensitivity => {
            let roles = roles();
            sensitivity(ws, model, &base, &roles, &targets, &record).map_err(stopped)?
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
                time_constant: None,
                value: None,
                from_time: None,
                diagnostics: vec![],
                problems: vec![],
                differences: vec![],
                responses: vec![],
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

pub(crate) fn run_tests(
    session: &mut Session,
    ws: &mut Workspace<'_>,
    input: RunTestsInput,
) -> Result<RunTestsOutput, ToolError> {
    for (field, names) in [("targets", &input.targets), ("record", &input.record)] {
        if names.len() > MAX_TARGETS {
            return Err(ToolError::new(format!(
                "{field} names at most {MAX_TARGETS} variables (this call names {})",
                names.len()
            )));
        }
    }
    let resolved = resolve_model(ws.project, ws.db, &session.model_name)?;
    let model = resolved.model;
    let mut not_found = Vec::new();
    let mut resolve = |names: &[String]| -> Vec<&Variable> {
        names
            .iter()
            .filter_map(|name| match names::resolve(model, name) {
                Ok(var) => Some(var),
                Err(suggestions) => {
                    not_found.push(NotFound {
                        name: name.clone(),
                        suggestions,
                    });
                    None
                }
            })
            .collect()
    };
    let targets = resolve(&input.targets);
    let record = if input.record.is_empty() {
        model
            .variables
            .iter()
            .filter(|v| matches!(v, Variable::Stock(_)))
            .collect()
    } else {
        resolve(&input.record)
    };
    let tests: Vec<TestName> = TestName::ALL
        .into_iter()
        .filter(|t| input.tests.is_empty() || input.tests.contains(t))
        .collect();

    let graph = Graph::of(ws.db, &resolved);
    let units = Units::of(ws, &resolved);
    let parsed = Parsed::of(model);
    let current = match session.runs.current(ws, model) {
        Err(err) if err.is_interrupted() => return Err(err),
        current => current
            .map(|base| {
                let roles = Roles::of(model, &graph, &units, &parsed, &base);
                (base, roles)
            })
            .map_err(|err| err.error),
    };
    let mut summaries = Vec::new();
    let mut checks: Vec<Check> = Vec::new();
    for test in tests {
        let made: Result<Made, String> = match (test, &current) {
            (TestName::Units, _) => {
                Ok(vec![units_check(&mut session.evidence, ws, &resolved)].into())
            }
            (_, Err(err)) => Err(err.clone()),
            (TestName::ExtremeConditions, Ok((base, roles))) => {
                let (targets, note) = targets_or(&targets, || roles.defaults(model, &graph));
                let mut made = extreme_conditions(ws, model, base, roles, &targets)?;
                made.note = [note, made.note]
                    .into_iter()
                    .flatten()
                    .reduce(|a, b| format!("{a} {b}"));
                Ok(made)
            }
            (TestName::IntegrationError, Ok((base, _))) => {
                Ok(integration_error(ws, model, base)?.into())
            }
            (TestName::Sensitivity, Ok((base, roles))) => {
                let (targets, note) = targets_or(&targets, || roles.defaults(model, &graph));
                Ok(Made {
                    checks: sensitivity(ws, model, base, roles, &targets, &record)?,
                    note,
                })
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
            (TestName::Disturbance, Ok((base, roles))) => {
                let (targets, note) = targets_or(&targets, || {
                    let (defaults, note) = roles.defaults(model, &graph);
                    let defaults = defaults
                        .into_iter()
                        .filter(|var| !is_zero_valued(base, var))
                        .take(MAX_DEFAULT_DISTURBANCES)
                        .collect();
                    (defaults, note)
                });
                Ok(Made {
                    checks: disturbance(
                        &mut session.runs,
                        &mut session.evidence,
                        ws,
                        &resolved,
                        base,
                        &targets,
                        &record,
                    )?,
                    note,
                })
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

    let (listed, omitted) = listed(checks);
    let output = fitted(
        &mut session.evidence,
        RunTestsOutput {
            revision: ws.revision,
            tests: summaries,
            results: vec![],
            omitted,
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
        session.checks.checks.insert(
            result.id.clone(),
            CheckRecord {
                key: check.key.clone(),
                record: record_names.clone(),
                revision: ws.revision,
                outcome: result.outcome,
            },
        );
    }
    Ok(output)
}

/// `targets`, or `default()` and what it says it left out when the call
/// names none.
fn targets_or<'a>(
    targets: &[&'a Variable],
    default: impl FnOnce() -> (Vec<&'a Variable>, Option<String>),
) -> (Vec<&'a Variable>, Option<String>) {
    if targets.is_empty() {
        default()
    } else {
        (targets.to_vec(), None)
    }
}

/// The checks an answer lists, in order, and how many past its limit it
/// leaves out: every check that did not pass, failures first, then
/// sensitivity's strongest.
fn listed(checks: Vec<Check>) -> (Vec<Check>, usize) {
    let (mut worth, passed): (Vec<Check>, Vec<Check>) = checks
        .into_iter()
        .partition(|c| c.result.outcome != Outcome::Passed);
    // By outcome, then test; within a test, the strongest responses first,
    // and a stable sort keeps the rest in target order.
    let test_order = |test: TestName| TestName::ALL.iter().position(|&t| t == test);
    worth.sort_by(|a, b| {
        a.result
            .outcome
            .cmp(&b.result.outcome)
            .then(test_order(a.key.test).cmp(&test_order(b.key.test)))
            .then(b.strength.total_cmp(&a.strength))
    });
    let mut strongest: Vec<Check> = passed
        .into_iter()
        .filter(|c| c.key.test == TestName::Sensitivity && c.strength > 0.0)
        .collect();
    strongest.sort_by(|a, b| b.strength.total_cmp(&a.strength));
    strongest.truncate(MAX_STRONGEST);
    worth.extend(strongest);
    let omitted = worth.len().saturating_sub(MAX_RESULTS);
    worth.truncate(MAX_RESULTS);
    (worth, omitted)
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

/// A model's causal links between its variables, by canonical ident.
pub(crate) struct Graph {
    readers: HashMap<String, Vec<String>>,
}

impl Graph {
    fn of(db: &crate::db::SimlinDb, resolved: &ResolvedModel<'_>) -> Graph {
        let mut readers: HashMap<String, Vec<String>> = HashMap::new();
        for link in crate::analysis::model_links(
            db,
            resolved.source_model,
            resolved.source_project,
            None,
            false,
        ) {
            readers.entry(link.from).or_default().push(link.to);
        }
        Graph { readers }
    }

    /// Every variable `from` reaches through links, itself excluded.
    fn downstream(&self, from: &str) -> HashSet<String> {
        let mut seen: HashSet<String> = HashSet::new();
        let mut queue: VecDeque<&str> = VecDeque::from([from]);
        while let Some(node) = queue.pop_front() {
            for next in self.readers.get(node).into_iter().flatten() {
                if seen.insert(next.clone()) {
                    queue.push_back(next);
                }
            }
        }
        seen.remove(from);
        seen
    }

    /// The model's constants that feed its flows, those that reach the most
    /// stocks first (then by name).
    fn feeding_flows<'m>(&self, model: &'m datamodel::Model) -> Vec<&'m Variable> {
        let kinds: HashMap<String, &Variable> = model
            .variables
            .iter()
            .map(|v| (crate::canonicalize(v.get_ident()).into_owned(), v))
            .collect();
        let mut constants: Vec<(usize, &Variable)> = model
            .variables
            .iter()
            .filter(|v| settable(v).is_ok())
            .filter_map(|v| {
                let reached = self.downstream(&crate::canonicalize(v.get_ident()));
                let kind = |name: &String| kinds.get(name).copied();
                let feeds_a_flow = reached
                    .iter()
                    .any(|name| matches!(kind(name), Some(Variable::Flow(_))));
                let stocks = reached
                    .iter()
                    .filter(|name| matches!(kind(name), Some(Variable::Stock(_))))
                    .count();
                feeds_a_flow.then_some((stocks, v))
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
        if let Some(model_time) = super::changes::effective_specs(ws.project, resolved.model)
            .time_units
            .as_deref()
            .filter(|units| !units.trim().is_empty())
            .and_then(|units| crate::units::parse_units(&ctx, Some(units)).ok().flatten())
            .filter(|units| !units.is_empty())
        {
            time.push(model_time);
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
            seconds,
            warned,
        }
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
        Expr0::Const(text, _, _) => text.trim().parse().ok(),
        Expr0::Op1(UnaryOp::Negative, inner, _) => number(inner).map(|n| -n),
        Expr0::Op1(UnaryOp::Positive, inner, _) => number(inner),
        _ => None,
    }
}

/// A time constant: why the battery takes it for one, and how many DTs its
/// low extreme is.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq)]
struct TimeConstant {
    evidence: TimeConstantEvidence,
    stages: f64,
}

/// What the battery knows of the model's constants: which are time
/// constants, which convert units, and which are shares of a whole.
struct Roles {
    time_constants: HashMap<String, TimeConstant>,
    /// Constants whose value is one of their units.
    conversions: HashSet<String>,
    /// Shares and fractions, with their whole: 1, or 100 in percent.
    shares: HashMap<String, f64>,
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
        let complements = complements(parsed);
        for var in model.variables.iter().filter(|v| settable(v).is_ok()) {
            let canonical = crate::canonicalize(var.get_ident()).into_owned();
            let values = element_values(base, var);
            if values.is_empty() {
                continue;
            }
            let declared = units.of_variable(var);
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
            time_constants: time_constants(model, graph, units, parsed),
            conversions,
            shares,
        }
    }

    /// The constants the targeted tests change by default: those that feed
    /// the model's flows, less its unit conversions, and a note naming the
    /// conversions left out.
    fn defaults<'m>(
        &self,
        model: &'m datamodel::Model,
        graph: &Graph,
    ) -> (Vec<&'m Variable>, Option<String>) {
        let (conversions, targets): (Vec<&Variable>, Vec<&Variable>) =
            graph.feeding_flows(model).into_iter().partition(|var| {
                self.conversions
                    .contains(crate::canonicalize(var.get_ident()).as_ref())
            });
        let note = (!conversions.is_empty()).then(|| {
            let names: Vec<&str> = conversions
                .iter()
                .take(MAX_DETAILS)
                .map(|var| var.get_ident())
                .collect();
            let more = conversions.len().saturating_sub(MAX_DETAILS);
            format!(
                "Left out {} unit conversion{} ({}{}): a constant whose value is one of its \
                 units changes the units a quantity is counted in, not the quantity. Name one \
                 in targets to test it anyway.",
                conversions.len(),
                if conversions.len() == 1 { "" } else { "s" },
                names.join(", "),
                if more > 0 {
                    format!(" and {more} more")
                } else {
                    String::new()
                },
            )
        });
        (targets, note)
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
    let named = |unit: &str| {
        let singular = unit.strip_suffix('s').unwrap_or(unit);
        words
            .iter()
            .any(|word| *word == unit || word.strip_suffix('s').unwrap_or(word) == singular)
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

/// The constants some equation takes the complement of, `1 - c` (or `100 -
/// c`), with that whole: the rest of a share.
fn complements(parsed: &Parsed<'_>) -> HashMap<String, f64> {
    fn walk(expr: &Expr0, found: &mut HashMap<String, f64>) {
        match expr {
            Expr0::Const(..) => {}
            Expr0::Var(..) => {}
            Expr0::Op2(BinaryOp::Sub, l, r, _) => {
                if let (Expr0::Const(text, _, _), Expr0::Var(raw, _)) = (l.as_ref(), r.as_ref())
                    && let Ok(whole) = text.trim().parse::<f64>()
                    && (whole == 1.0 || whole == 100.0)
                {
                    found.insert(raw.canonicalize().as_str().to_string(), whole);
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
    let mut found = HashMap::new();
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
    complements: &HashMap<String, f64>,
) -> Option<f64> {
    let raw = var
        .get_units()
        .map(|units| crate::canonicalize(units.trim()).into_owned());
    let in_percent = matches!(raw.as_deref(), Some("percent" | "%" | "pct"));
    let whole = if in_percent { 100.0 } else { 1.0 };
    let dimensionless = declared.is_none_or(UnitMap::is_empty) || in_percent;
    let marked = matches!(raw.as_deref(), Some("fraction")) || in_percent;
    let named = canonical.split('_').any(|word| SHARE_WORDS.contains(&word));
    let complemented = complements.get(canonical) == Some(&whole);
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
) -> HashMap<String, TimeConstant> {
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
        if declared.as_ref().is_some_and(|u| units.is_time(u)) {
            found.insert(
                name.clone(),
                TimeConstant {
                    evidence: TimeConstantEvidence::TimeUnits,
                    stages: 1.0,
                },
            );
        }
    }
    for (_, exprs) in parsed.equations.values() {
        for expr in exprs {
            delay_times(expr, &mut found);
        }
    }

    // What depends on a stock: everything a stock reaches.
    let mut stock_dependent: HashSet<String> = HashSet::new();
    for var in &model.variables {
        if matches!(var, Variable::Stock(_)) {
            let stock = crate::canonicalize(var.get_ident()).into_owned();
            stock_dependent.extend(graph.downstream(&stock));
            stock_dependent.insert(stock);
        }
    }
    // A role counts for a constant without units, and never overrides a
    // delay's time. A constant declared dimensionless (`1`, `dmnl`) has no
    // units to go by either where the rate it divides carries a unit warning
    // (its own equation, or its flow's): a stock over it there cannot
    // balance, so the declaration is the modeler's slip, and the role
    // decides. Where the rate's units balance, the declaration is right.
    let mut by_role = |name: &str, evidence: TimeConstantEvidence, rate: [&str; 2]| {
        let undeclared = match constants.get(name) {
            Some(None) => true,
            Some(Some(declared)) => declared.is_empty() && rate.iter().any(|var| units.warned(var)),
            None => false,
        };
        if undeclared {
            found.entry(name.to_string()).or_insert(TimeConstant {
                evidence,
                stages: 1.0,
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
    found
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

/// Record that `name` is the time of a delay, whose low extreme is at least
/// `stages` DTs.
fn note_stages(found: &mut HashMap<String, TimeConstant>, name: &str, stages: f64) {
    let entry = found.entry(name.to_string()).or_insert(TimeConstant {
        evidence: TimeConstantEvidence::DelayTime,
        stages,
    });
    entry.evidence = TimeConstantEvidence::DelayTime;
    entry.stages = entry.stages.max(stages);
}

/// Add to `found` each variable `expr` gives a delay, smooth or trend as its
/// time, with the delay's order: its number of stages.
fn delay_times(expr: &Expr0, found: &mut HashMap<String, TimeConstant>) {
    match expr {
        Expr0::Const(..) | Expr0::Var(..) => {}
        Expr0::App(UntypedBuiltinFn(name, args), _) => {
            let name = name.to_lowercase();
            if TIME_ARGUMENT_FUNCTIONS.contains(&name.as_str())
                && let Some(Expr0::Var(raw, _)) = args.get(1)
            {
                let stages = match name.as_str() {
                    "smth3" | "delay3" => 3.0,
                    "delayn" => match args.get(2) {
                        Some(Expr0::Const(text, _, _)) => text.trim().parse().unwrap_or(1.0),
                        _ => 1.0,
                    },
                    _ => 1.0,
                };
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
fn is_zero_valued(base: &Run, var: &Variable) -> bool {
    element_values(base, var).iter().all(|&v| v == 0.0)
}

/// The values `var`'s elements start `base` with, each by its subscript
/// (empty for a scalar), in results order.
fn element_start_values(base: &Run, var: &Variable) -> Vec<(String, f64)> {
    let canonical = crate::canonicalize(var.get_ident()).into_owned();
    let Some(first) = base.results.iter().next() else {
        return vec![];
    };
    let prefix = format!("{canonical}[");
    let mut values: Vec<(usize, String, f64)> = base
        .results
        .offsets
        .iter()
        .filter_map(|(key, &offset)| {
            let key = key.as_str();
            if key == canonical {
                Some((offset, String::new(), first[offset]))
            } else {
                let subscript = key.strip_prefix(&prefix)?.strip_suffix(']')?;
                Some((offset, subscript.to_string(), first[offset]))
            }
        })
        .collect();
    values.sort_by_key(|(offset, _, _)| *offset);
    values
        .into_iter()
        .map(|(_, element, value)| (element, value))
        .collect()
}

/// The values `var`'s elements start `base` with.
fn element_values(base: &Run, var: &Variable) -> Vec<f64> {
    element_start_values(base, var)
        .into_iter()
        .map(|(_, value)| value)
        .collect()
}

/// Extreme conditions: each target at its low extreme (zero, or DT for a
/// time constant) and at its high one (ten times its value, or the whole for
/// a share).
fn extreme_conditions(
    ws: &mut Workspace<'_>,
    model: &datamodel::Model,
    base: &Run,
    roles: &Roles,
    targets: &[&Variable],
) -> Result<Made, ToolError> {
    let watched = Watched::of(base, model);
    let dt = base.results.specs.dt;
    let mut planned: Vec<(Check, RunPlan)> = Vec::new();
    let mut checks = Vec::new();
    for &var in targets {
        let time_constant = roles.time_constant(var);
        let low: (Condition, Box<dyn Fn(f64) -> f64>) = match time_constant {
            Some(tc) => (Condition::Dt, Box::new(move |_| dt * tc.stages)),
            None => (Condition::Zero, Box::new(|_| 0.0)),
        };
        let whole = roles.share_whole(var);
        let high: (Condition, Box<dyn Fn(f64) -> f64>) = match whole {
            Some(whole) => (Condition::Whole, Box::new(move |_| whole)),
            None => (Condition::TenTimes, Box::new(|x| x * 10.0)),
        };
        let zero = is_zero_valued(base, var);
        let at_whole =
            whole.is_some_and(|whole| element_values(base, var).iter().all(|&v| v == whole));
        for (condition, to) in [low, high] {
            // A zero constant is at zero already, and ten times zero is zero;
            // a share at the whole is at its high extreme already.
            if (zero && condition != Condition::Dt) || (at_whole && condition == Condition::Whole) {
                continue;
            }
            let mut check = Check::new(TestName::ExtremeConditions, Some(var), Some(condition));
            if condition == Condition::Dt {
                check.result.time_constant = time_constant.map(|tc| tc.evidence);
            }
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
    let problems =
        runs::execute_values(ws, model, &plans, |_, results| watched.problems(&results))?;
    for ((mut check, _), problems) in planned.into_iter().zip(problems) {
        match problems {
            Err(reason) => {
                check.result.outcome = Outcome::Failed;
                check.result.reason = Some(format!("the run fails: {reason}"));
            }
            Ok(problems) => {
                check.result.outcome = if problems.iter().any(|p| p.kind == ProblemKind::NonFinite)
                {
                    Outcome::Failed
                } else if problems.is_empty() {
                    Outcome::Passed
                } else {
                    Outcome::Flagged
                };
                let marked: Vec<&str> = problems
                    .iter()
                    .filter(|p| p.non_negative)
                    .map(|p| p.variable.as_str())
                    .collect();
                if !marked.is_empty() {
                    check.result.note = Some(format!(
                        "{} {} marked non-negative, which this engine does not enforce: a tool \
                         that enforces the marking would hold {} at zero",
                        marked.join(", "),
                        if marked.len() == 1 { "is" } else { "are" },
                        if marked.len() == 1 { "it" } else { "them" },
                    ));
                }
                check.result.problems = problems.into_iter().take(MAX_DETAILS).collect();
            }
        }
        checks.push(check);
    }
    Ok(Made {
        checks,
        note: watched.undefined_note(),
    })
}

/// The model's series an extreme conditions check watches, labeled once for
/// every check: each variable's (or element's) results key, its label, and
/// whether and how going negative counts. A series that is not a number
/// somewhere in the model's own run is not watched: a check cannot make it
/// so.
struct Watched {
    series: Vec<WatchedSeries>,
    /// The series not a number in the model's run, by label, with when each
    /// first is.
    undefined: Vec<(String, f64)>,
}

struct WatchedSeries {
    key: Ident<Canonical>,
    label: String,
    /// Checked for going negative: a stock, or a flow the model marks
    /// non-negative.
    signed: bool,
    non_negative: bool,
    /// Whether it goes negative in the model's run already.
    negative_before: bool,
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
        let mut series = Vec::new();
        let mut undefined = Vec::new();
        for (key, offset) in keys {
            let ident = strip_subscript(key.as_str());
            let Some(var) = by_ident.get(ident) else {
                continue;
            };
            let label = format!("{}{}", var.get_ident(), &key.as_str()[ident.len()..]);
            let values = base.series(offset);
            if let Some(row) = values.iter().position(|v| !v.is_finite()) {
                undefined.push((label, round(times[row])));
                continue;
            }
            let (signed, non_negative) = match var {
                Variable::Stock(stock) => (true, stock.compat.non_negative),
                Variable::Flow(flow) => (flow.compat.non_negative, flow.compat.non_negative),
                _ => (false, false),
            };
            series.push(WatchedSeries {
                key: key.clone(),
                label,
                signed,
                non_negative,
                negative_before: goes_negative(&values).is_some(),
            });
        }
        Watched { series, undefined }
    }

    /// What went wrong in `results` that did not in the model's run: values
    /// that became NaN or infinite, and stocks and non-negative flows that
    /// went negative; non-finite values first, then by time.
    fn problems(&self, results: &Results) -> Vec<Problem> {
        let rows: Vec<&[f64]> = results.iter().take(runs::saved_rows(results)).collect();
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
            } else if watched.signed
                && !watched.negative_before
                && let Some(row) = goes_negative(&values)
            {
                problems.push(Problem {
                    kind: ProblemKind::GoesNegative,
                    variable: watched.label.clone(),
                    time: time(row),
                    value: Some(round(values.iter().copied().fold(f64::INFINITY, f64::min))),
                    non_negative: watched.non_negative,
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

    /// What the test says of the series it does not judge.
    fn undefined_note(&self) -> Option<String> {
        let (first, at) = self.undefined.first()?;
        let n = self.undefined.len();
        Some(format!(
            "{n} series {} not a number in the model's own run ({first} from {at}{}), so no \
             check judges {}.",
            if n == 1 { "is" } else { "are" },
            if n > 1 {
                format!(", and {} more", n - 1)
            } else {
                String::new()
            },
            if n == 1 { "it" } else { "them" },
        ))
    }
}

/// The first row at which `values` is below zero by more than rounding: a
/// billionth of its largest magnitude, or of one.
pub(crate) fn goes_negative(values: &[f64]) -> Option<usize> {
    let largest = values
        .iter()
        .filter(|v| v.is_finite())
        .fold(1.0_f64, |m, v| m.max(v.abs()));
    values.iter().position(|&v| v < -1e-9 * largest)
}

/// Integration error: the run at half the DT, and under RK4 unless it runs
/// under RK4 already, compared at every element of every stock.
fn integration_error(
    ws: &mut Workspace<'_>,
    model: &datamodel::Model,
    base: &Run,
) -> Result<Vec<Check>, ToolError> {
    let specs = &base.results.specs;
    // Half the DT, saved at the model's own times: the rows compare one for
    // one, and the run holds no more than the model's does.
    let mut variants = vec![(
        Condition::HalfDt,
        SpecsChange {
            dt: Some(specs.dt / 2.0),
            save_step: Some(specs.save_step.max(specs.dt)),
            ..SpecsChange::default()
        },
    )];
    if specs.method != crate::results::Method::RungeKutta4 {
        variants.push((
            Condition::Rk4,
            SpecsChange {
                method: Some(IntegrationMethod::Rk4),
                ..SpecsChange::default()
            },
        ));
    }
    let stocks: Vec<&Variable> = model
        .variables
        .iter()
        .filter(|v| matches!(v, Variable::Stock(_)))
        .collect();
    let base_times = base.times();
    variants
        .into_iter()
        .map(|(condition, specs)| {
            let mut check = Check::new(TestName::IntegrationError, None, Some(condition));
            let plan = RunPlan {
                specs,
                ..base.plan.clone()
            };
            ws.yield_point()?;
            let results = match runs::execute(ws, model, &plan) {
                Ok(results) => results,
                Err(RunFailure::Stopped) => return Err(ToolError::interrupted()),
                Err(RunFailure::Failed(reason)) => {
                    return Ok(check.not_run(format!("the run fails: {reason}")));
                }
            };
            let run = Run::new(String::new(), 0, 0, plan, results);
            let quarter = base.results.specs.dt / 4.0;
            let mut differences: Vec<Difference> = Vec::new();
            for var in &stocks {
                let every = usize::MAX;
                let (theirs, _) = element_series_upto(&run, model, var.get_ident(), None, every);
                let (ours, _) = element_series_upto(base, model, var.get_ident(), None, every);
                for ((label, b), (_, a)) in theirs.into_iter().zip(ours) {
                    let scale = scale(&a);
                    let largest = base_times
                        .iter()
                        .enumerate()
                        .map(|(i, &t)| (a[i] - b[run.row_at(t + quarter)]).abs() / scale)
                        .fold(0.0_f64, f64::max);
                    differences.push(Difference {
                        variable: label,
                        difference: round(largest),
                    });
                }
            }
            differences.sort_by(|a, b| b.difference.total_cmp(&a.difference));
            if differences
                .first()
                .is_some_and(|d| d.difference > INTEGRATION_TOLERANCE)
            {
                check.result.outcome = Outcome::Failed;
            }
            check.result.differences = differences.into_iter().take(MAX_DETAILS).collect();
            Ok(check)
        })
        .collect()
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

/// A behavior's family: what a change of behavior means, as the label within
/// it does not. Linear, exponential, goal seeking and S-shaped growth are one
/// family, rising, since which of them a series reads as depends on the
/// horizon as much as on the structure.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Family {
    Still,
    /// Growth or decline, and which: a series that rose and now falls has
    /// changed family.
    Monotone(Direction),
    OneTurn(Direction),
    Oscillating,
    /// Undefined, or a mode the classifier could not name.
    Unnamed,
}

impl Family {
    fn of(mode: &BehaviorMode) -> Family {
        match mode.kind {
            ModeKind::AtRest => Family::Still,
            ModeKind::Linear
            | ModeKind::Exponential
            | ModeKind::GoalSeeking
            | ModeKind::SShaped => mode.direction.map_or(Family::Unnamed, Family::Monotone),
            ModeKind::Overshoot | ModeKind::RiseAndFall | ModeKind::FallAndRise => {
                mode.direction.map_or(Family::Unnamed, Family::OneTurn)
            }
            ModeKind::Oscillation => Family::Oscillating,
            ModeKind::Undefined | ModeKind::Other => Family::Unnamed,
        }
    }
}

/// How each recorded variable of `run` responded, against `base`: the series
/// the check made undefined, then the material changes of behavior, then the
/// largest changes; and the strength of the strongest, infinite for a series
/// made undefined, so a check that did that is never passed over.
///
/// A change of mode is reported when the classifier's label changes; it is
/// material when the behavior changes family between named families, and
/// the series moved at least [`MATERIAL_CHANGE`] of its scale somewhere in
/// the run. A series not a number somewhere in either run has no change to
/// report, and its numbers are left out.
fn responses(
    run: &Run,
    base: &Run,
    model: &datamodel::Model,
    record: &[&Variable],
) -> (Vec<Response>, f64) {
    let (times, base_times) = (run.times(), base.times());
    let mut all: Vec<Response> = Vec::new();
    for var in record {
        let (theirs, _) = element_series(run, model, var.get_ident());
        let (ours, _) = element_series(base, model, var.get_ident());
        for ((label, values), (_, base_values)) in theirs.into_iter().zip(ours) {
            let (Some(&last), Some(&base_last)) = (values.last(), base_values.last()) else {
                continue;
            };
            let defined = |series: &[f64]| series.iter().all(|v| v.is_finite());
            let (defined, base_defined) = (defined(&values), defined(&base_values));
            let (change, largest_change) = if defined && base_defined {
                let scale = scale(&base_values);
                let largest = values
                    .iter()
                    .zip(&base_values)
                    .map(|(a, b)| (a - b).abs() / scale)
                    .fold(0.0_f64, f64::max);
                let finite = |x: f64| x.is_finite().then_some(x);
                (
                    finite(round((last - base_last) / scale)),
                    finite(round(largest)),
                )
            } else {
                (None, None)
            };
            let mode = classify(&times, &values);
            let was = classify(&base_times, &base_values);
            all.push(Response {
                variable: label,
                change,
                largest_change,
                mode: mode.kind,
                was: (mode.kind != was.kind).then_some(was.kind),
                changed_family: changed_family(&mode, &was),
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
    all.truncate(found.max(MAX_DETAILS));
    (all, strongest)
}

/// Whether a check found something in its responses: a series it made
/// undefined, which a check that ran is never passed with.
fn made_undefined(responses: &[Response]) -> bool {
    responses.iter().any(|r| r.went_undefined)
}

/// Whether a behavior is of another family than it `was`, both named.
fn changed_family(mode: &BehaviorMode, was: &BehaviorMode) -> bool {
    // Goal seeking is one behavior whichever side of its goal a series
    // starts: a goal seeker whose goal moved past its start still seeks it.
    if mode.kind == ModeKind::GoalSeeking && was.kind == ModeKind::GoalSeeking {
        return false;
    }
    let (family, was) = (Family::of(mode), Family::of(was));
    family != was && family != Family::Unnamed && was != Family::Unnamed
}

/// Whether a response changed behavior materially: from one family to
/// another, both named, with the series moving [`MATERIAL_CHANGE`] of its
/// scale somewhere.
fn material(response: &Response) -> bool {
    response.changed_family
        && response
            .largest_change
            .is_some_and(|change| change >= MATERIAL_CHANGE)
}

/// Sensitivity: each target at half and at double (a share at most its
/// whole).
fn sensitivity(
    ws: &mut Workspace<'_>,
    model: &datamodel::Model,
    base: &Run,
    roles: &Roles,
    targets: &[&Variable],
    record: &[&Variable],
) -> Result<Vec<Check>, ToolError> {
    let mut planned: Vec<(Check, RunPlan)> = Vec::new();
    let mut checks = Vec::new();
    for &var in targets {
        if is_zero_valued(base, var) {
            continue;
        }
        let whole = roles.share_whole(var).unwrap_or(f64::INFINITY);
        let at_whole = element_values(base, var).iter().all(|&v| v >= whole);
        for (condition, factor) in [(Condition::Half, 0.5), (Condition::Double, 2.0)] {
            if condition == Condition::Double && at_whole {
                continue;
            }
            let mut check = Check::new(TestName::Sensitivity, Some(var), Some(condition));
            match change(base, var, None, |x| (x * factor).min(whole)) {
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
            Err(reason) => checks.push(check.not_run(format!("the run fails: {reason}"))),
            Ok((responses, strength)) => {
                if responses.iter().any(material) || made_undefined(&responses) {
                    check.result.outcome = Outcome::Flagged;
                }
                check.result.responses = responses;
                check.strength = strength;
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
            let held = element_start_values(base, var);
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
                    return Ok(check.not_run(format!("the run fails: {reason}")));
                }
            };
            let run = std::sync::Arc::new(Run::new(String::new(), ws.revision, key, plan, results));
            let mut check = check;
            check.result.value = scalar.then(|| round(held[0].1));
            check.result.responses = responses(&run, base, model, record).0;
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
fn disturbance(
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
    let specs = &base.results.specs;
    let at = base.times()[base.row_at(specs.start + (specs.stop - specs.start) / 10.0)];
    targets
        .iter()
        .map(|&var| {
            let mut check = Check::new(TestName::Disturbance, Some(var), Some(Condition::Step));
            check.result.from_time = Some(round(at));
            if is_zero_valued(base, var) {
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
                    return Ok(check.not_run(format!("the run fails: {reason}")));
                }
            };
            let run = std::sync::Arc::new(Run::new(String::new(), ws.revision, key, plan, results));
            check.result.value = value;
            check.result.responses = responses(&run, base, model, record).0;
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
