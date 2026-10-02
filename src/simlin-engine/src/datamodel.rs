// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{BTreeMap, HashMap};
use std::fmt::{Display, Formatter};
use std::iter::Iterator;

use crate::canonicalize;
use crate::common::{DimensionName, ElementName};
pub use crate::shared_vec::SharedVec;

/// Equality of the floats a datamodel type holds, bit for bit.
///
/// The datamodel's `==` answers "is this the same model", not "do these
/// numbers compare equal": it decides whether a salsa input is set again
/// (`db::sync`), whether an agent's record changed (`tools::changes`), and
/// whether a view element can share an allocation
/// (`SharedVec::share_identical`). By value, a NaN is unequal to itself, so a
/// model holding one would change on every look, and `0.0 == -0.0` although
/// the two save differently. So every float-bearing type compares its floats
/// through this, and is `Eq`.
trait BitEq {
    fn bit_eq(&self, other: &Self) -> bool;
}

impl BitEq for f64 {
    fn bit_eq(&self, other: &Self) -> bool {
        self.to_bits() == other.to_bits()
    }
}

impl<T: BitEq> BitEq for Vec<T> {
    fn bit_eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.iter().zip(other).all(|(a, b)| a.bit_eq(b))
    }
}

impl<T: BitEq> BitEq for Option<T> {
    fn bit_eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Some(a), Some(b)) => a.bit_eq(b),
            (None, None) => true,
            (Some(_), None) | (None, Some(_)) => false,
        }
    }
}

/// `a == b` for a field whose type is `Eq`: a float, which is not, fails to
/// compile here rather than being compared by value.
fn total_eq<T: Eq>(a: &T, b: &T) -> bool {
    a == b
}

/// Implements `PartialEq` and `Eq` for a struct that holds floats, field by
/// field: a field marked `: float` holds floats and is compared bit for bit
/// ([`BitEq`]), and every other field is compared with its own `==`, which
/// must be `Eq`. The struct is destructured without `..`, so a field added to
/// it fails to compile until it is listed here, and a float listed without
/// its mark fails to compile too.
macro_rules! bitwise_eq {
    ($type:ident { $($field:ident $(: $float:ident)?),* $(,)? }) => {
        impl PartialEq for $type {
            fn eq(&self, other: &Self) -> bool {
                let $type { $($field),* } = self;
                true $(&& bitwise_eq!(@compare $field, &other.$field $(, $float)?))*
            }
        }
        impl Eq for $type {}
    };
    (@compare $a:expr, $b:expr) => {
        $crate::datamodel::total_eq($a, $b)
    };
    (@compare $a:expr, $b:expr, float) => {
        $crate::datamodel::BitEq::bit_eq($a, $b)
    };
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Default, Eq, Clone)]
pub struct UnitMap {
    pub map: BTreeMap<String, i32>,
    pub ctx: Option<Vec<String>>,
}

impl PartialEq for UnitMap {
    fn eq(&self, other: &Self) -> bool {
        self.map == other.map
    }
}

impl UnitMap {
    pub fn new() -> UnitMap {
        Default::default()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn reciprocal(mut self) -> Self {
        for exp in self.map.values_mut() {
            // saturating: `-i32::MIN` overflows, and a saturated exponent
            // (see `exp` below) must stay merely absurd, never a panic.
            *exp = exp.saturating_neg();
        }
        self
    }

    /// Raise every unit to the power `exp`. `x^0` is dimensionless, so the
    /// map is cleared rather than left holding zero exponents (a
    /// non-normalized `{meter: 0}` compares unequal to the empty map under
    /// the raw BTreeMap equality `PartialEq` uses, while Display filters
    /// zeros -- yielding self-contradictory "dmnl doesn't match dmnl"
    /// diagnostics). The multiply saturates: exponents this large are
    /// nonsense units, but a model file is untrusted input and an overflow
    /// panic would abort a `panic=abort` host.
    pub fn exp(mut self, exp: i32) -> Self {
        if exp == 0 {
            self.map.clear();
            return self;
        }
        for unit in self.map.values_mut() {
            *unit = unit.saturating_mul(exp);
        }

        self
    }

    pub fn push_ctx(mut self, ctx: String) -> Self {
        let mut full_ctx = self.ctx.take().unwrap_or_default();
        full_ctx.push(ctx);
        self.ctx = Some(full_ctx);

        self
    }

    #[allow(dead_code)]
    pub fn pretty_print(&self) -> String {
        format!("{self}")
    }
}

impl std::ops::Div for UnitMap {
    type Output = Self;

    #[allow(clippy::suspicious_arithmetic_impl)]
    fn div(self, rhs: Self) -> Self::Output {
        self * rhs.reciprocal()
    }
}

impl std::ops::Mul for UnitMap {
    type Output = Self;

    fn mul(mut self, rhs: Self) -> Self::Output {
        let mut rhs = rhs;
        for (unit, n) in rhs.map.into_iter() {
            let new_value = match self.map.get(&unit) {
                None => n,
                // saturating: composing two `exp`-saturated maps (or one
                // with anything) must not overflow-panic on untrusted input.
                Some(m) => n.saturating_add(*m),
            };

            if new_value == 0 {
                self.map.remove(&unit);
            } else {
                self.map.insert(unit, new_value);
            }
        }

        if let Some(rctx) = rhs.ctx.take()
            && !rctx.is_empty()
        {
            let mut ctx = self.ctx.take().unwrap_or_default();
            ctx.extend(rctx);
            self.ctx = Some(ctx);
        }

        self
    }
}

impl Display for UnitMap {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let unit_names = {
            let mut unit_names = self
                .map
                .keys()
                .map(|unit| unit.as_str())
                .collect::<Vec<&str>>();
            unit_names.sort_unstable();
            unit_names
        };

        let mut written = false;
        let mut first = true;
        for (unit, exp) in unit_names
            .iter()
            .map(|unit| (unit, self.map[*unit]))
            .filter(|(_, exp)| *exp > 0)
        {
            if !first {
                write!(f, "*")?;
            }
            first = false;
            write!(f, "{unit}")?;
            // unsigned_abs: `i32::MIN.abs()` overflows, and a saturated
            // exponent must print rather than panic.
            if exp.unsigned_abs() > 1 {
                write!(f, "^{}", exp.unsigned_abs())?;
            }
            written = true;
        }

        for (unit, exp) in unit_names
            .iter()
            .map(|unit| (unit, self.map[*unit]))
            .filter(|(_, exp)| *exp < 0)
        {
            if !written {
                write!(f, "1")?;
                written = true;
            }
            write!(f, "/")?;
            write!(f, "{unit}")?;
            if exp.unsigned_abs() > 1 {
                write!(f, "^{}", exp.unsigned_abs())?;
            }
        }

        if !written {
            write!(f, "dmnl")?;
        }

        Ok(())
    }
}

impl FromIterator<(String, i32)> for UnitMap {
    fn from_iter<I: IntoIterator<Item = (String, i32)>>(iter: I) -> Self {
        UnitMap {
            map: iter.into_iter().collect(),
            ctx: None,
        }
    }
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum GraphicalFunctionKind {
    Continuous,
    Extrapolate,
    Discrete,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone)]
pub struct GraphicalFunctionScale {
    pub min: f64,
    pub max: f64,
}
bitwise_eq!(GraphicalFunctionScale {
    min: float,
    max: float,
});

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone)]
pub struct GraphicalFunction {
    pub kind: GraphicalFunctionKind,
    pub x_points: Option<Vec<f64>>,
    pub y_points: Vec<f64>,
    pub x_scale: GraphicalFunctionScale,
    pub y_scale: GraphicalFunctionScale,
}
bitwise_eq!(GraphicalFunction {
    kind,
    x_points: float,
    y_points: float,
    x_scale,
    y_scale,
});

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub enum Equation {
    Scalar(String),
    ApplyToAll(Vec<DimensionName>, String),
    Arrayed(
        Vec<DimensionName>,
        Vec<(
            ElementName,
            String,
            Option<String>,
            Option<GraphicalFunction>,
        )>,
        // Default equation for elements not explicitly listed (EXCEPT semantics).
        // When Some, this equation applies to all elements not in the Vec above.
        Option<String>,
        // True when this arrayed equation was derived from EXCEPT syntax during
        // MDL conversion. Drives `apply_default_to_missing` in the AST layer,
        // replacing the old text-comparison heuristic.
        bool,
    ),
}

impl Equation {
    /// The equation's source text concatenated into a single string:
    /// the scalar / Apply-to-All formula verbatim, or -- for the
    /// per-element (`Arrayed`) variant -- every element formula plus any
    /// EXCEPT default joined by newlines. Convenience for diagnostics and
    /// tests that want to inspect an equation as text without first
    /// matching on its variant.
    pub fn source_text(&self) -> String {
        match self {
            Equation::Scalar(s) | Equation::ApplyToAll(_, s) => s.clone(),
            Equation::Arrayed(_, elements, default, _) => {
                let mut parts: Vec<&str> =
                    elements.iter().map(|(_, eqn, _, _)| eqn.as_str()).collect();
                if let Some(default_eqn) = default {
                    parts.push(default_eqn.as_str());
                }
                parts.join("\n")
            }
        }
    }
}

/// The kind of external data function from Vensim's GET DIRECT family.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub enum DataSourceKind {
    Data,
    Constants,
    Lookups,
    Subscript,
}

/// Metadata for variables backed by external data files.
/// Stores the parsed arguments from GET DIRECT DATA/CONSTANTS/LOOKUPS/SUBSCRIPT
/// so the MDL writer can reconstruct the original function call.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub struct DataSource {
    /// The kind of data function (Data, Constants, Lookups, Subscript)
    pub kind: DataSourceKind,
    /// Path to the external data file
    pub file: String,
    /// Tab/sheet name (for Excel) or delimiter (for CSV)
    pub tab_or_delimiter: String,
    /// Row or column label for data lookup
    pub row_or_col: String,
    /// Cell label for data lookup
    pub cell: String,
}

/// A conveyor stock: material rides a fixed-length belt and exits after the
/// transit time. See `docs/design/conveyors.md`. The fields hold XMILE
/// expression strings (evaluated by the engine); `None` means the tag was
/// absent and the documented default applies.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub struct Conveyor {
    /// `<len>`: transit time, in time units. Required.
    pub transit_time: String,
    /// `<capacity>`: max material on the belt (default INF).
    pub capacity: Option<String>,
    /// `<in_limit>`: max equation-driven inflow per time unit (default INF).
    pub inflow_limit: Option<String>,
    /// `<sample>`: when nonzero, re-latch the transit time (default: every DT).
    pub sample: Option<String>,
    /// `<arrest>`: when nonzero, freeze the belt and zero all flows (default: never).
    pub arrest: Option<String>,
    /// `discrete`: move whole units (batches) rather than a continuous stream.
    pub discrete: bool,
    /// `batch_integrity`: only whole upstream-queue batches may be taken.
    pub batch_integrity: bool,
    /// `one_at_a_time`: take only the front queue batch per DT (default true).
    pub one_at_a_time: bool,
    /// `exponential_leak`: exponential (vs linear) leakage for all leak flows.
    pub exponential_leak: bool,
    /// isee "Ignore losses from earlier leak zones" toggle (default false).
    pub ignore_earlier_zone_losses: bool,
}

/// Marks a flow as a conveyor leakage outflow. See `docs/design/conveyors.md` §3.3, §5.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub struct Leakage {
    /// The leak fraction expression (`<leak>` content or a leak flow's `<eqn>`);
    /// `None` for a bare `<leak/>` marker (leakage TBD, contributes zero).
    pub fraction: Option<String>,
    /// `<leak_integers/>`: leak only whole units.
    pub integers: bool,
    /// `leak_start`: fractional belt position where the leak zone starts (default 0).
    pub zone_start: Option<String>,
    /// `leak_end`: fractional belt position where the leak zone ends (default 1).
    pub zone_end: Option<String>,
}

/// isee `isee:spreadflow` inflow-placement method. See `docs/design/conveyors.md` §8.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub enum SpreadFlow {
    /// All inflow lands at the entry slat (the XMILE default).
    Beginning,
    /// Equal amount at every belt position.
    Even,
    /// Content-proportional across the current belt.
    Dest,
    /// Distribution named by `<isee:distrib_eq>`.
    Dist(String),
    /// Mirror an upstream leak flow's per-slat leakage.
    Source,
}

/// A queue stock: material waits in a FIFO of batches until something
/// downstream is ready to accept it. See `docs/design/queues.md`. A bare marker
/// with no options (XMILE §4.2: "Queues do not have any options"). It is kept a
/// struct rather than a bool -- mirroring `Conveyor` -- so a future vendor
/// attribute does not churn every construction site. `Debug` is `debug-derive`-
/// gated exactly like its `Compat` siblings (`Conveyor`/`Leakage`/`SpreadFlow`).
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub struct Queue {}

/// Per-variable metadata that is not part of the core equation: XMILE
/// stock/flow options (`non_negative`, `<conveyor>`, leak/spread markers),
/// Vensim `active_initial`, access/visibility, and data-source imports. These
/// ride alongside `Stock`/`Flow`/`Aux`/`Module` so adding one does not churn
/// every construction site. Conveyor/leak/spread live here for the same reason
/// `non_negative` does: they are all optional XMILE stock/flow options.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Default)]
pub struct Compat {
    pub active_initial: Option<String>,
    pub non_negative: bool,
    pub can_be_module_input: bool,
    pub visibility: Visibility,
    pub data_source: Option<DataSource>,
    /// Present on a stock iff it is a conveyor (`<conveyor>` block).
    pub conveyor: Option<Conveyor>,
    /// Present on a flow iff it is a conveyor leakage outflow.
    pub leakage: Option<Leakage>,
    /// Present on a conveyor inflow that selects a non-default placement.
    pub spreadflow: Option<SpreadFlow>,
    /// Present on a stock iff it is a queue (`<queue/>` marker). See docs/design/queues.md §10.1.
    pub queue: Option<Queue>,
    /// True on a queue outflow marked `<overflow/>`. See docs/design/queues.md §10.1.
    pub overflow: bool,
}

impl Compat {
    pub fn is_empty(&self) -> bool {
        self.active_initial.is_none()
            && !self.non_negative
            && !self.can_be_module_input
            && self.visibility == Visibility::Private
            && self.data_source.is_none()
            && self.conveyor.is_none()
            && self.leakage.is_none()
            && self.spreadflow.is_none()
            && self.queue.is_none()
            && !self.overflow
    }
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Copy, Clone, PartialEq, Eq, Default)]
pub enum Visibility {
    #[default]
    Private,
    Public,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum AiState {
    A, // No information
    B, // Human created. Will only occur when a modeler adds content using AI to an existing model. (Depending on the software implementation these may always be reported as F).
    C, // AI generated, not modified by human
    D, // Created by a person, edited by AI
    E, // Edited by a person, unknown creation (shouldn't occur)
    F, // Created and edited by a person not using AI.
    G, // Created by AI then edited by a person (though possibly edited again by AI).
    H, // Created and edited by a person and also edited by AI.
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub struct Stock {
    pub ident: String,
    pub equation: Equation,
    pub documentation: String,
    pub units: Option<String>,
    /// The inflows as the producer wrote them. A list can repeat a flow (a
    /// file with a repeated `<inflow>` is stored as written); every reader
    /// that decides what the model does reads the set through
    /// [`distinct_stock_flows`].
    pub inflows: Vec<String>,
    /// The outflows as the producer wrote them; see `inflows`.
    pub outflows: Vec<String>,
    pub ai_state: Option<AiState>,
    pub uid: Option<i32>,
    pub compat: Compat,
}

/// A stock's inflow or outflow list as the set the engine integrates, and the
/// flows the list repeats.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Default)]
pub struct DistinctFlows {
    /// Each flow's first occurrence, judged after canonicalization, in list
    /// order, spelled as written.
    pub flows: Vec<String>,
    /// The canonical name of every flow the list names more than once, each
    /// once, in the order their first repeat appears.
    pub repeated: Vec<String>,
}

/// The one statement of how the engine reads a stock's inflow or outflow list:
/// each flow counts once, at its first occurrence. The readers that decide
/// what the model does -- the salsa sync (`db::sync`), the special-stock
/// build before expansion (`queue_compile::build_compiled`), the layout's
/// stock-flow metadata and the MDL writer's `INTEG` -- read the set through
/// this, and the sync keeps `repeated` so a model whose list repeats a flow is
/// warned about (`ErrorCode::RepeatedStockFlow`), since taking the set changes
/// what a naive sum over the list would integrate.
///
/// This is the engine's rule, unverified against Stella or Vensim. XMILE 1.0
/// section 4.2 (`docs/reference/xmile-v1.0.html`, "Stocks") never addresses a
/// repeated tag; its whole statement on the list is "The set of inflows
/// and/or outflows is NOT REQUIRED. If there are multiple inflows, they appear
/// with multiple tags in inflow-priority order (if the order of inflow to the
/// stock is important)." Calling the list a set, with each flow holding one
/// priority, is what the rule leans on.
pub fn distinct_stock_flows(flows: &[String]) -> DistinctFlows {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut result = DistinctFlows::default();
    for flow in flows {
        let canonical = canonicalize(flow).into_owned();
        if seen.contains(&canonical) {
            if !result.repeated.contains(&canonical) {
                result.repeated.push(canonical);
            }
        } else {
            seen.insert(canonical);
            result.flows.push(flow.clone());
        }
    }
    result
}

impl Project {
    /// The project with every stock's inflow and outflow lists replaced by
    /// their sets ([`distinct_stock_flows`]), borrowed when no list repeats a
    /// flow. For a reader that walks the datamodel's lists wholesale, taken
    /// before any of it reads them.
    pub fn with_distinct_stock_flows(&self) -> std::borrow::Cow<'_, Project> {
        let repeats = |flows: &[String]| !distinct_stock_flows(flows).repeated.is_empty();
        let any_repeat = self.models.iter().any(|m| {
            m.variables.iter().any(|v| match v {
                Variable::Stock(s) => repeats(&s.inflows) || repeats(&s.outflows),
                _ => false,
            })
        });
        if !any_repeat {
            return std::borrow::Cow::Borrowed(self);
        }
        let mut project = self.clone();
        for model in &mut project.models {
            model.variables.edit_where(
                |v| matches!(v, Variable::Stock(s) if repeats(&s.inflows) || repeats(&s.outflows)),
                |v| {
                    if let Variable::Stock(s) = v {
                        s.inflows = distinct_stock_flows(&s.inflows).flows;
                        s.outflows = distinct_stock_flows(&s.outflows).flows;
                    }
                },
            );
        }
        std::borrow::Cow::Owned(project)
    }
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub struct Flow {
    pub ident: String,
    pub equation: Equation,
    pub documentation: String,
    pub units: Option<String>,
    pub gf: Option<GraphicalFunction>,
    pub ai_state: Option<AiState>,
    pub uid: Option<i32>,
    pub compat: Compat,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub struct Aux {
    pub ident: String,
    pub equation: Equation,
    pub documentation: String,
    pub units: Option<String>,
    pub gf: Option<GraphicalFunction>,
    pub ai_state: Option<AiState>,
    pub uid: Option<i32>,
    pub compat: Compat,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleReference {
    pub src: String,
    pub dst: String,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub struct Module {
    pub ident: String,
    pub model_name: String,
    pub documentation: String,
    pub units: Option<String>,
    pub references: Vec<ModuleReference>,
    pub ai_state: Option<AiState>,
    pub uid: Option<i32>,
    pub compat: Compat,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub enum Variable {
    Stock(Stock),
    Flow(Flow),
    Aux(Aux),
    Module(Module),
}

impl Variable {
    pub fn get_ident(&self) -> &str {
        match self {
            Variable::Stock(stock) => stock.ident.as_str(),
            Variable::Flow(flow) => flow.ident.as_str(),
            Variable::Aux(aux) => aux.ident.as_str(),
            Variable::Module(module) => module.ident.as_str(),
        }
    }

    pub fn get_equation(&self) -> Option<&Equation> {
        match self {
            Variable::Stock(stock) => Some(&stock.equation),
            Variable::Flow(flow) => Some(&flow.equation),
            Variable::Aux(aux) => Some(&aux.equation),
            Variable::Module(_module) => None,
        }
    }

    pub fn get_units(&self) -> Option<&String> {
        match self {
            Variable::Stock(stock) => stock.units.as_ref(),
            Variable::Flow(flow) => flow.units.as_ref(),
            Variable::Aux(aux) => aux.units.as_ref(),
            Variable::Module(module) => module.units.as_ref(),
        }
    }

    pub fn get_ai_state(&self) -> Option<AiState> {
        match self {
            Variable::Stock(stock) => stock.ai_state,
            Variable::Flow(flow) => flow.ai_state,
            Variable::Aux(aux) => aux.ai_state,
            Variable::Module(module) => module.ai_state,
        }
    }

    pub fn set_ident(&mut self, ident: String) {
        match self {
            Variable::Stock(stock) => stock.ident = ident,
            Variable::Flow(flow) => flow.ident = ident,
            Variable::Aux(aux) => aux.ident = ident,
            Variable::Module(module) => module.ident = ident,
        }
    }

    pub fn set_scalar_equation(&mut self, equation: &str) {
        match self {
            Variable::Stock(stock) => stock.equation = Equation::Scalar(equation.to_string()),
            Variable::Flow(flow) => flow.equation = Equation::Scalar(equation.to_string()),
            Variable::Aux(aux) => aux.equation = Equation::Scalar(equation.to_string()),
            Variable::Module(_module) => {}
        }
    }

    pub fn set_units(&mut self, units: &str) {
        let units = if units.is_empty() {
            None
        } else {
            Some(units.to_owned())
        };
        match self {
            Variable::Stock(stock) => stock.units = units,
            Variable::Flow(flow) => flow.units = units,
            Variable::Aux(aux) => aux.units = units,
            Variable::Module(module) => module.units = units,
        }
    }

    pub fn set_documentation(&mut self, doc: &str) {
        match self {
            Variable::Stock(stock) => doc.clone_into(&mut stock.documentation),
            Variable::Flow(flow) => doc.clone_into(&mut flow.documentation),
            Variable::Aux(aux) => doc.clone_into(&mut aux.documentation),
            Variable::Module(module) => doc.clone_into(&mut module.documentation),
        }
    }

    pub fn set_graphical_function(&mut self, gf: Option<GraphicalFunction>) {
        match self {
            Variable::Stock(_stock) => {}
            Variable::Flow(flow) => flow.gf = gf,
            Variable::Aux(aux) => aux.gf = gf,
            Variable::Module(_module) => {}
        }
    }

    pub fn get_visibility(&self) -> Visibility {
        match self {
            Variable::Stock(stock) => stock.compat.visibility,
            Variable::Flow(flow) => flow.compat.visibility,
            Variable::Aux(aux) => aux.compat.visibility,
            Variable::Module(module) => module.compat.visibility,
        }
    }

    pub fn can_be_module_input(&self) -> bool {
        match self {
            Variable::Stock(stock) => stock.compat.can_be_module_input,
            Variable::Flow(flow) => flow.compat.can_be_module_input,
            Variable::Aux(aux) => aux.compat.can_be_module_input,
            Variable::Module(module) => module.compat.can_be_module_input,
        }
    }

    pub fn set_can_be_module_input(&mut self, value: bool) {
        match self {
            Variable::Stock(stock) => stock.compat.can_be_module_input = value,
            Variable::Flow(flow) => flow.compat.can_be_module_input = value,
            Variable::Aux(aux) => aux.compat.can_be_module_input = value,
            Variable::Module(module) => module.compat.can_be_module_input = value,
        }
    }
}

/// What an expression text is to the variable that holds it.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum ExpressionRole {
    /// The equation itself: a scalar or apply-to-all text, one element's
    /// text, or an EXCEPT default.
    Equation,
    /// An initial value written apart from the equation: an element's own
    /// initial, or an `ACTIVE INITIAL`.
    Initial,
    /// A stock or flow option the engine evaluates: a conveyor's parameter, a
    /// leak's fraction or zone bound.
    Option,
}

/// Visits every expression text of an `Equation` reference, shared or
/// mutable: the one statement of where an equation's texts are.
macro_rules! visit_equation_texts {
    ($equation:expr, $out:ident $(, . $as_text:ident)?) => {
        match $equation {
            Equation::Scalar(text) | Equation::ApplyToAll(_, text) => {
                $out.push((ExpressionRole::Equation, text $(.$as_text())?));
            }
            Equation::Arrayed(_, elements, default, _) => {
                for (_, text, initial, _) in elements {
                    $out.push((ExpressionRole::Equation, text $(.$as_text())?));
                    if let Some(initial) = initial {
                        $out.push((ExpressionRole::Initial, initial $(.$as_text())?));
                    }
                }
                if let Some(default) = default {
                    $out.push((ExpressionRole::Equation, default $(.$as_text())?));
                }
            }
        }
    };
}

/// Visits every expression text of a `Variable` reference, shared or mutable:
/// the one statement of where a variable holds text the engine parses as an
/// expression. Every struct is destructured without `..`, so a field added to
/// a variable, to `Compat` or to a stock or flow option fails to compile until
/// it is listed here as an expression or as something else.
macro_rules! visit_variable_texts {
    ($variable:expr, $out:ident $(, . $as_text:ident)?) => {{
        let compat = match $variable {
            Variable::Stock(Stock {
                equation,
                compat,
                ident: _,
                documentation: _,
                units: _,
                inflows: _,
                outflows: _,
                ai_state: _,
                uid: _,
            }) => {
                visit_equation_texts!(equation, $out $(, . $as_text)?);
                compat
            }
            Variable::Flow(Flow {
                equation,
                compat,
                ident: _,
                documentation: _,
                units: _,
                gf: _,
                ai_state: _,
                uid: _,
            })
            | Variable::Aux(Aux {
                equation,
                compat,
                ident: _,
                documentation: _,
                units: _,
                gf: _,
                ai_state: _,
                uid: _,
            }) => {
                visit_equation_texts!(equation, $out $(, . $as_text)?);
                compat
            }
            Variable::Module(Module {
                compat,
                ident: _,
                model_name: _,
                documentation: _,
                units: _,
                references: _,
                ai_state: _,
                uid: _,
            }) => compat,
        };
        let Compat {
            active_initial,
            conveyor,
            leakage,
            // A spread flow's distribution is a variable's name or a list of
            // numbers, never an expression (`NameRole::Distribution`).
            spreadflow: _,
            non_negative: _,
            can_be_module_input: _,
            visibility: _,
            data_source: _,
            queue: _,
            overflow: _,
        } = compat;
        if let Some(text) = active_initial {
            $out.push((ExpressionRole::Initial, text $(.$as_text())?));
        }
        if let Some(Conveyor {
            transit_time,
            capacity,
            inflow_limit,
            sample,
            arrest,
            discrete: _,
            batch_integrity: _,
            one_at_a_time: _,
            exponential_leak: _,
            ignore_earlier_zone_losses: _,
        }) = conveyor
        {
            $out.push((ExpressionRole::Option, transit_time $(.$as_text())?));
            for text in [capacity, inflow_limit, sample, arrest].into_iter().flatten() {
                $out.push((ExpressionRole::Option, text $(.$as_text())?));
            }
        }
        if let Some(Leakage {
            fraction,
            zone_start,
            zone_end,
            integers: _,
        }) = leakage
        {
            for text in [fraction, zone_start, zone_end].into_iter().flatten() {
                $out.push((ExpressionRole::Option, text $(.$as_text())?));
            }
        }
    }};
}

/// What a name of a variable is to the place a model holds it in.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum NameRole {
    /// A variable's own name, as displayed.
    Ident,
    /// A flow in a stock's inflow list.
    Inflow,
    /// A flow in a stock's outflow list.
    Outflow,
    /// The end of a module reference the value is read from: a variable of
    /// the module's model, or one reached through an instance.
    ModuleSource,
    /// The end of a module reference the value is written to: a port of an
    /// instance.
    ModuleDestination,
    /// The variable whose graphical function a spread inflow distributes by
    /// (`SpreadFlow::Dist`). The text is that variable's name, or else a
    /// list of numbers (`conveyor_compile::resolve_dist_profile`).
    Distribution,
    /// A member of one of the model's groups.
    GroupMember,
    /// A formal parameter of a macro: a variable of the macro's body.
    MacroParameter,
    /// An output of a macro: a variable of the macro's body.
    MacroOutput,
    /// The label of a diagram element: the variable the element draws.
    ViewLabel,
}

/// Visits every name of a variable that a `Variable` reference holds outside
/// its expression texts, shared or mutable: the one statement of where those
/// are. Every struct is destructured without `..`, so a field added to a
/// variable or to `Compat` fails to compile until it is listed here as a
/// name or as something else.
macro_rules! visit_variable_names {
    ($variable:expr, $out:ident $(, . $as_text:ident)?) => {{
        let compat = match $variable {
            Variable::Stock(Stock {
                ident,
                inflows,
                outflows,
                compat,
                equation: _,
                documentation: _,
                units: _,
                ai_state: _,
                uid: _,
            }) => {
                $out.push((NameRole::Ident, ident $(.$as_text())?));
                for flow in inflows {
                    $out.push((NameRole::Inflow, flow $(.$as_text())?));
                }
                for flow in outflows {
                    $out.push((NameRole::Outflow, flow $(.$as_text())?));
                }
                compat
            }
            Variable::Flow(Flow {
                ident,
                compat,
                equation: _,
                documentation: _,
                units: _,
                gf: _,
                ai_state: _,
                uid: _,
            })
            | Variable::Aux(Aux {
                ident,
                compat,
                equation: _,
                documentation: _,
                units: _,
                gf: _,
                ai_state: _,
                uid: _,
            }) => {
                $out.push((NameRole::Ident, ident $(.$as_text())?));
                compat
            }
            Variable::Module(Module {
                ident,
                references,
                compat,
                // The name of a model, not of a variable.
                model_name: _,
                documentation: _,
                units: _,
                ai_state: _,
                uid: _,
            }) => {
                $out.push((NameRole::Ident, ident $(.$as_text())?));
                for ModuleReference { src, dst } in references {
                    $out.push((NameRole::ModuleSource, src $(.$as_text())?));
                    $out.push((NameRole::ModuleDestination, dst $(.$as_text())?));
                }
                compat
            }
        };
        let Compat {
            spreadflow,
            // Expression texts (`Variable::expression_texts`).
            active_initial: _,
            conveyor: _,
            leakage: _,
            non_negative: _,
            can_be_module_input: _,
            visibility: _,
            data_source: _,
            queue: _,
            overflow: _,
        } = compat;
        match spreadflow {
            Some(SpreadFlow::Dist(name)) => {
                $out.push((NameRole::Distribution, name $(.$as_text())?));
            }
            Some(
                SpreadFlow::Beginning | SpreadFlow::Even | SpreadFlow::Dest | SpreadFlow::Source,
            )
            | None => {}
        }
    }};
}

/// Visits the name of the variable a `ViewElement` reference draws, shared or
/// mutable. Every element is destructured without `..`, so a field added to
/// one fails to compile until it is listed here.
macro_rules! visit_view_element_names {
    ($element:expr, $out:ident $(, . $as_text:ident)?) => {
        match $element {
            ViewElement::Aux(view_element::Aux {
                name,
                uid: _,
                x: _,
                y: _,
                label_side: _,
                compat: _,
            })
            | ViewElement::Stock(view_element::Stock {
                name,
                uid: _,
                x: _,
                y: _,
                label_side: _,
                compat: _,
            }) => $out.push((NameRole::ViewLabel, name $(.$as_text())?)),
            ViewElement::Flow(view_element::Flow {
                name,
                uid: _,
                x: _,
                y: _,
                label_side: _,
                points: _,
                compat: _,
                label_compat: _,
            }) => $out.push((NameRole::ViewLabel, name $(.$as_text())?)),
            ViewElement::Module(view_element::Module {
                name,
                uid: _,
                x: _,
                y: _,
                label_side: _,
            }) => $out.push((NameRole::ViewLabel, name $(.$as_text())?)),
            // These name what they draw by uid.
            ViewElement::Link(view_element::Link {
                uid: _,
                from_uid: _,
                to_uid: _,
                shape: _,
                polarity: _,
            }) => {}
            ViewElement::Alias(view_element::Alias {
                uid: _,
                alias_of_uid: _,
                x: _,
                y: _,
                label_side: _,
                compat: _,
            }) => {}
            ViewElement::Cloud(view_element::Cloud {
                uid: _,
                flow_uid: _,
                x: _,
                y: _,
                compat: _,
            }) => {}
            // A group's name is its own: it draws no variable.
            ViewElement::Group(view_element::Group {
                name: _,
                uid: _,
                x: _,
                y: _,
                width: _,
                height: _,
                is_mdl_view_marker: _,
            }) => {}
        }
    };
}

/// `value` with each text `read` lists replaced by what `replace` returns for
/// it, or `None` when `replace` returns `None` for every one: the value is as
/// it was, and nothing was copied. `write` lists the same texts in the same
/// order, to write.
fn mapped<T: Clone, R>(
    value: &T,
    read: fn(&T) -> Vec<(R, &str)>,
    write: fn(&mut T) -> Vec<(R, &mut String)>,
    mut replace: impl FnMut(R, &str) -> Option<String>,
) -> Option<T> {
    let replacements: Vec<Option<String>> = read(value)
        .into_iter()
        .map(|(role, text)| replace(role, text))
        .collect();
    if replacements.iter().all(Option::is_none) {
        return None;
    }
    let mut next = value.clone();
    for ((_, text), replacement) in write(&mut next).into_iter().zip(replacements) {
        if let Some(replacement) = replacement {
            *text = replacement;
        }
    }
    Some(next)
}

impl Equation {
    /// Every expression text the equation holds, each with its role, in a
    /// fixed order: an element's text, then its initial, element by element,
    /// then the EXCEPT default.
    pub fn expression_texts(&self) -> Vec<(ExpressionRole, &str)> {
        let mut texts = Vec::new();
        visit_equation_texts!(self, texts, .as_str);
        texts
    }
}

impl Variable {
    /// Every text of the variable the engine parses as an expression, each
    /// with its role: the equation's texts (`Equation::expression_texts`),
    /// the `ACTIVE INITIAL`, and the conveyor and leak options. A pass that
    /// reads or rewrites what a variable references (a rename, the
    /// special-stock expansions' scans) takes the texts from here, so no
    /// expression is out of its reach. A name held outside an expression is
    /// one of [`Variable::names`].
    pub fn expression_texts(&self) -> Vec<(ExpressionRole, &str)> {
        let mut texts = Vec::new();
        visit_variable_texts!(self, texts, .as_str);
        texts
    }

    /// The texts of [`Variable::expression_texts`], in the same order, to
    /// write.
    pub fn expression_texts_mut(&mut self) -> Vec<(ExpressionRole, &mut String)> {
        let mut texts = Vec::new();
        visit_variable_texts!(self, texts);
        texts
    }

    /// The variable with each expression text replaced by what `replace`
    /// returns for it, or `None` when `replace` returns `None` for every
    /// text: the variable is as it was, and nothing was copied.
    pub fn map_expression_texts(
        &self,
        replace: impl FnMut(ExpressionRole, &str) -> Option<String>,
    ) -> Option<Variable> {
        mapped(
            self,
            Variable::expression_texts,
            Variable::expression_texts_mut,
            replace,
        )
    }

    /// Every name of a variable the variable holds outside its expression
    /// texts, each with its role: its own name, a stock's flows, a module's
    /// references, and a spread inflow's distribution.
    pub fn names(&self) -> Vec<(NameRole, &str)> {
        let mut names = Vec::new();
        visit_variable_names!(self, names, .as_str);
        names
    }

    /// The names of [`Variable::names`], in the same order, to write.
    pub fn names_mut(&mut self) -> Vec<(NameRole, &mut String)> {
        let mut names = Vec::new();
        visit_variable_names!(self, names);
        names
    }

    /// The variable with each name replaced by what `replace` returns for it,
    /// or `None` when `replace` returns `None` for every name: the variable
    /// is as it was, and nothing was copied.
    pub fn map_names(
        &self,
        replace: impl FnMut(NameRole, &str) -> Option<String>,
    ) -> Option<Variable> {
        mapped(self, Variable::names, Variable::names_mut, replace)
    }
}

pub mod view_element {
    #[cfg_attr(feature = "debug-derive", derive(Debug))]
    #[derive(Copy, Clone, PartialEq, Eq)]
    pub enum LabelSide {
        Top,
        Left,
        Center,
        Bottom,
        Right,
    }

    /// Vensim-specific layout metadata preserved during MDL roundtripping.
    /// Stores original element dimensions and type bits so the writer can
    /// reproduce the sketch section byte-for-byte.
    #[cfg_attr(feature = "debug-derive", derive(Debug))]
    #[derive(Clone)]
    pub struct ViewElementCompat {
        pub width: f64,
        pub height: f64,
        pub shape: i32,
        pub bits: u32,
        /// Optional raw sketch field stored between the UID and coordinates.
        /// Vensim uses this slot for attached valves and cloud comments.
        pub name_field: Option<String>,
        /// Raw fields following `bits` in the sketch record, preserved so the
        /// writer can roundtrip MDL-specific flags it does not interpret.
        pub tail: Option<String>,
    }
    bitwise_eq!(ViewElementCompat {
        width: float,
        height: float,
        shape,
        bits,
        name_field,
        tail,
    });

    #[cfg_attr(feature = "debug-derive", derive(Debug))]
    #[derive(Clone)]
    pub struct SketchSegmentCompat {
        /// Translation applied by MDL view composition before the datamodel sees
        /// the view. Serializing a split view subtracts this offset again.
        pub x_offset: f64,
        pub y_offset: f64,
    }
    bitwise_eq!(SketchSegmentCompat {
        x_offset: float,
        y_offset: float,
    });

    /// Connector fields the MDL writer roundtrips but does not derive from the
    /// datamodel `Link`: `field4` (whether the connector carries a meaningful
    /// control point -- Vensim re-routes it straight when 0), `field10`, and
    /// the control point the sketch gave the connector. `Link::shape` stays
    /// the connector's geometry: the writer writes `control_point` back only
    /// while it still reads as that shape between the endpoints it writes, and
    /// computes a point from the shape otherwise, so an edited or moved link
    /// gets a point of its own.
    ///
    /// The recorded point is what makes a save a fixed point. An arc's point
    /// is not a function of its angle -- Vensim puts it anywhere on the arc,
    /// and a point computed from the angle is rounded to whole units -- so
    /// writing only a computed point moves an untouched connector on the first
    /// save, and can move it again on the next. Writing the recorded point
    /// keeps an untouched connector's line as the file had it, and a point the
    /// writer computed is recorded when the file is read again, so the save
    /// after it writes that point unchanged.
    #[cfg_attr(feature = "debug-derive", derive(Debug))]
    #[derive(Clone, PartialEq, Eq)]
    pub struct LinkSketchCompat {
        pub uid: i32,
        pub field4: i32,
        pub field10: i32,
        /// The control point as the sketch was read, in the view's
        /// coordinates (after multi-view composition); None for the `(0, 0)`
        /// sentinel of a straight connector.
        pub control_point: Option<(i32, i32)>,
    }

    /// Per-view MDL sketch metadata the writer needs but the datamodel `View`
    /// doesn't otherwise carry: `segments` records the translation MDL view
    /// composition applied to each original named view (so a split export can
    /// subtract it again), and `links` carries the connector fields above.
    /// Flow pipe geometry is not stored -- the writer recomputes pipe connectors
    /// from the flow's points and stock edges.
    #[cfg_attr(feature = "debug-derive", derive(Debug))]
    #[derive(Clone, PartialEq, Eq, Default)]
    pub struct StockFlowSketchCompat {
        pub segments: Vec<SketchSegmentCompat>,
        pub links: Vec<LinkSketchCompat>,
    }

    #[cfg_attr(feature = "debug-derive", derive(Debug))]
    #[derive(Clone)]
    pub struct Aux {
        pub name: String,
        pub uid: i32,
        pub x: f64,
        pub y: f64,
        pub label_side: LabelSide,
        pub compat: Option<ViewElementCompat>,
    }
    bitwise_eq!(Aux {
        name,
        uid,
        x: float,
        y: float,
        label_side,
        compat,
    });

    #[cfg_attr(feature = "debug-derive", derive(Debug))]
    #[derive(Clone)]
    pub struct Stock {
        pub name: String,
        pub uid: i32,
        pub x: f64,
        pub y: f64,
        pub label_side: LabelSide,
        pub compat: Option<ViewElementCompat>,
    }
    bitwise_eq!(Stock {
        name,
        uid,
        x: float,
        y: float,
        label_side,
        compat,
    });

    #[cfg_attr(feature = "debug-derive", derive(Debug))]
    #[derive(Clone)]
    pub struct FlowPoint {
        pub x: f64,
        pub y: f64,
        pub attached_to_uid: Option<i32>,
    }
    bitwise_eq!(FlowPoint {
        x: float,
        y: float,
        attached_to_uid,
    });

    #[cfg_attr(feature = "debug-derive", derive(Debug))]
    #[derive(Clone)]
    pub struct Flow {
        pub name: String,
        pub uid: i32,
        pub x: f64,
        pub y: f64,
        pub label_side: LabelSide,
        // pub segment_with_aux: i32,
        // pub aux_percentage_into_segment: f64,
        pub points: Vec<FlowPoint>,
        pub compat: Option<ViewElementCompat>,
        pub label_compat: Option<ViewElementCompat>,
    }
    bitwise_eq!(Flow {
        name,
        uid,
        x: float,
        y: float,
        label_side,
        points,
        compat,
        label_compat,
    });

    #[cfg_attr(feature = "debug-derive", derive(Debug))]
    #[derive(Clone)]
    pub enum LinkShape {
        Straight,
        Arc(f64), // angle in [0, 360)
        MultiPoint(Vec<FlowPoint>),
    }

    #[cfg_attr(feature = "debug-derive", derive(Debug))]
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub enum LinkPolarity {
        Positive,
        Negative,
    }

    #[cfg_attr(feature = "debug-derive", derive(Debug))]
    #[derive(Clone, PartialEq, Eq)]
    pub struct Link {
        pub uid: i32,
        pub from_uid: i32,
        pub to_uid: i32,
        pub shape: LinkShape,
        pub polarity: Option<LinkPolarity>,
    }

    #[cfg_attr(feature = "debug-derive", derive(Debug))]
    #[derive(Clone)]
    pub struct Module {
        pub name: String,
        pub uid: i32,
        pub x: f64,
        pub y: f64,
        pub label_side: LabelSide,
    }
    bitwise_eq!(Module {
        name,
        uid,
        x: float,
        y: float,
        label_side,
    });

    #[cfg_attr(feature = "debug-derive", derive(Debug))]
    #[derive(Clone)]
    pub struct Alias {
        pub uid: i32,
        pub alias_of_uid: i32,
        pub x: f64,
        pub y: f64,
        pub label_side: LabelSide,
        pub compat: Option<ViewElementCompat>,
    }
    bitwise_eq!(Alias {
        uid,
        alias_of_uid,
        x: float,
        y: float,
        label_side,
        compat,
    });

    #[cfg_attr(feature = "debug-derive", derive(Debug))]
    #[derive(Clone)]
    pub struct Cloud {
        pub uid: i32,
        pub flow_uid: i32,
        pub x: f64,
        pub y: f64,
        pub compat: Option<ViewElementCompat>,
    }
    bitwise_eq!(Cloud {
        uid,
        flow_uid,
        x: float,
        y: float,
        compat,
    });

    /// Visual container for grouping related model elements.
    /// In XMILE these are called "groups" and in Vensim "sectors".
    /// Unlike other view elements, x/y represent the center of the group
    /// (matching the internal convention) rather than the XMILE top-left.
    /// Conversion to/from JSON handles the coordinate transformation.
    #[cfg_attr(feature = "debug-derive", derive(Debug))]
    #[derive(Clone)]
    pub struct Group {
        pub uid: i32,
        pub name: String,
        pub x: f64,
        pub y: f64,
        pub width: f64,
        pub height: f64,
        /// When true, this Group was synthesized during MDL multi-view merge
        /// to mark a view boundary.  The MDL writer splits on these markers
        /// to reconstruct the original per-view structure.  XMILE-sourced
        /// groups (organizational containers) leave this `false`.
        pub is_mdl_view_marker: bool,
    }
    bitwise_eq!(Group {
        uid,
        name,
        x: float,
        y: float,
        width: float,
        height: float,
        is_mdl_view_marker,
    });

    // A link's shape holds an angle, so it is compared by hand, bit for bit
    // (see `bitwise_eq!`). The match names every variant, so a new one fails
    // to compile until it is compared.
    impl PartialEq for LinkShape {
        fn eq(&self, other: &Self) -> bool {
            match (self, other) {
                (LinkShape::Straight, LinkShape::Straight) => true,
                (LinkShape::Arc(a), LinkShape::Arc(b)) => a.to_bits() == b.to_bits(),
                (LinkShape::MultiPoint(a), LinkShape::MultiPoint(b)) => a == b,
                (LinkShape::Straight | LinkShape::Arc(_) | LinkShape::MultiPoint(_), _) => false,
            }
        }
    }
    impl Eq for LinkShape {}
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub enum ViewElement {
    Aux(view_element::Aux),
    Stock(view_element::Stock),
    Flow(view_element::Flow),
    Link(view_element::Link),
    Module(view_element::Module),
    Alias(view_element::Alias),
    Cloud(view_element::Cloud),
    Group(view_element::Group),
}

impl ViewElement {
    /// The name of the variable the element draws, with its role; none for an
    /// element that draws no variable or names it by uid.
    pub fn names(&self) -> Vec<(NameRole, &str)> {
        let mut names = Vec::new();
        visit_view_element_names!(self, names, .as_str);
        names
    }

    /// The names of [`ViewElement::names`], to write.
    pub fn names_mut(&mut self) -> Vec<(NameRole, &mut String)> {
        let mut names = Vec::new();
        visit_view_element_names!(self, names);
        names
    }

    /// The element with its name replaced by what `replace` returns for it,
    /// or `None` when `replace` returns `None`: nothing was copied.
    pub fn map_names(
        &self,
        replace: impl FnMut(NameRole, &str) -> Option<String>,
    ) -> Option<ViewElement> {
        mapped(self, ViewElement::names, ViewElement::names_mut, replace)
    }

    pub fn get_uid(&self) -> i32 {
        match self {
            ViewElement::Aux(var) => var.uid,
            ViewElement::Stock(var) => var.uid,
            ViewElement::Flow(var) => var.uid,
            ViewElement::Link(var) => var.uid,
            ViewElement::Module(var) => var.uid,
            ViewElement::Alias(var) => var.uid,
            ViewElement::Cloud(var) => var.uid,
            ViewElement::Group(var) => var.uid,
        }
    }

    pub fn get_name(&self) -> Option<&str> {
        match self {
            ViewElement::Aux(var) => Some(var.name.as_str()),
            ViewElement::Stock(var) => Some(var.name.as_str()),
            ViewElement::Flow(var) => Some(var.name.as_str()),
            ViewElement::Link(_var) => None,
            ViewElement::Module(var) => Some(var.name.as_str()),
            ViewElement::Alias(_var) => None,
            ViewElement::Cloud(_var) => None,
            // Groups have names for display but are not model variables
            ViewElement::Group(_var) => None,
        }
    }
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Default)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}
bitwise_eq!(Rect {
    x: float,
    y: float,
    width: float,
    height: float,
});

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone)]
pub struct StockFlow {
    pub name: Option<String>,
    pub elements: SharedVec<ViewElement>,
    pub view_box: Rect,
    /// Zoom as a FACTOR: 1.0 = 100%, 2.0 = twice as big. This unit is shared
    /// by the protobuf, JSON, and TypeScript models. XMILE stores zoom as a
    /// PERCENTAGE (spec section 5.1: "100 is default, 200 is 2x bigger"), and
    /// the conversion lives only in `xmile::views` -- readers and writers of
    /// other formats hand the factor through unchanged.
    pub zoom: f64,
    /// When true, polarity labels on connectors should be displayed as
    /// letters (S/O) rather than symbols (+/-). Corresponds to xmutil's
    /// bLetterPolarity flag and XMILE's isee:use_lettered_polarity attribute.
    pub use_lettered_polarity: bool,
    /// Vensim default font string (e.g. "Arial|12||0-0-0|0-0-0|-1--1--1|-1--1--1|96,96")
    /// preserved during MDL roundtripping.
    pub font: Option<String>,
    /// Raw MDL sketch metadata that must survive parse normalization so the
    /// writer can invert the standard MDL import path when exporting again.
    pub sketch_compat: Option<view_element::StockFlowSketchCompat>,
}
bitwise_eq!(StockFlow {
    name,
    elements,
    view_box,
    zoom: float,
    use_lettered_polarity,
    font,
    sketch_compat,
});

impl StockFlow {
    pub fn get_variable_name(&self, uid: i32) -> Option<&str> {
        for element in self.elements.iter() {
            if element.get_uid() == uid {
                return element.get_name();
            }
        }

        None
    }
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub enum View {
    StockFlow(StockFlow),
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub struct LoopMetadata {
    pub uids: Vec<i32>,
    pub deleted: bool,
    pub name: String,
    pub description: String,
}

/// Marks a [`Model`] as a callable macro template rather than an ordinary
/// model, and records its calling convention. A macro definition is an
/// ordinary model whose `variables` are the macro body; this spec names which
/// body variables are the formal parameters and which are the outputs.
/// `Model.macro_spec` is `None` for every non-macro model.
//
// `MacroSpec` is carried directly on the `SourceModel` salsa input (mirroring
// `Compat`), so the per-project macro registry can be a salsa-tracked query
// keyed on the macro-marked models -- no mirror type and no separate
// model-level metadata input.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub struct MacroSpec {
    /// Formal parameter names, in positional calling order. Each names a body
    /// variable that a macro invocation binds an argument to.
    pub parameters: Vec<String>,
    /// The body variable whose value the call-site left-hand side receives.
    pub primary_output: String,
    /// Additional named outputs from Vensim's `:`-list multi-output call
    /// syntax, in declaration order. Empty for ordinary single-output macros.
    pub additional_outputs: Vec<String>,
}

/// Semantic/organizational group for categorizing model variables.
/// This is distinct from visual diagram groups (ViewElement::Group).
/// In Vensim, these are called "sectors".
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Default)]
pub struct ModelGroup {
    /// Group name (unique, normalized - dots become dashes)
    pub name: String,
    /// Optional documentation
    pub doc: Option<String>,
    /// Parent group name (xmutil calls this "owner")
    pub parent: Option<String>,
    /// Variable idents in this group (space_to_underbar format)
    pub members: Vec<String>,
    /// Whether this group can be run independently (XMILE run attribute)
    pub run_enabled: bool,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub struct Model {
    pub name: String,
    pub sim_specs: Option<SimSpecs>,
    pub variables: SharedVec<Variable>,
    pub views: Vec<View>,
    pub loop_metadata: Vec<LoopMetadata>,
    pub groups: Vec<ModelGroup>,
    /// `Some` if this model is a callable macro template. See [`MacroSpec`].
    pub macro_spec: Option<MacroSpec>,
}

impl Model {
    /// Replaces each name of a variable the model holds, outside expression
    /// texts, by what `replace` returns for it (`None` leaves it): the one
    /// statement of where a model holds the name of one of its variables. A
    /// pass that follows a variable through the model (a rename) decides what
    /// each role's name becomes here, so no place a name is held is out of
    /// its reach. Only a variable or a view element that holds a replaced
    /// name is copied.
    ///
    /// The model and everything it holds are destructured without `..`, so a
    /// field added to any of them fails to compile until it is listed as a
    /// name or as something else.
    pub fn map_variable_names(
        &mut self,
        mut replace: impl FnMut(NameRole, &str) -> Option<String>,
    ) {
        let Model {
            variables,
            views,
            loop_metadata,
            groups,
            macro_spec,
            name: _,
            sim_specs: _,
        } = self;
        variables.update(|variable| variable.map_names(&mut replace));
        for view in views {
            let View::StockFlow(StockFlow {
                elements,
                name: _,
                view_box: _,
                zoom: _,
                use_lettered_polarity: _,
                font: _,
                // Raw MDL sketch records, kept for the MDL writer.
                sketch_compat: _,
            }) = view;
            elements.update(|element| element.map_names(&mut replace));
        }
        // A loop names its variables by uid.
        for LoopMetadata {
            uids: _,
            deleted: _,
            name: _,
            description: _,
        } in loop_metadata.iter()
        {}
        for ModelGroup {
            members,
            name: _,
            doc: _,
            parent: _,
            run_enabled: _,
        } in groups
        {
            for member in members {
                if let Some(replacement) = replace(NameRole::GroupMember, member) {
                    *member = replacement;
                }
            }
        }
        if let Some(MacroSpec {
            parameters,
            primary_output,
            additional_outputs,
        }) = macro_spec
        {
            for parameter in parameters {
                if let Some(replacement) = replace(NameRole::MacroParameter, parameter) {
                    *parameter = replacement;
                }
            }
            for output in std::iter::once(primary_output).chain(additional_outputs) {
                if let Some(replacement) = replace(NameRole::MacroOutput, output) {
                    *output = replacement;
                }
            }
        }
    }

    /// Build a macro-marked [`Model`] from an already-built body variable
    /// list, synthesizing any missing formal-parameter port variables and
    /// attaching the [`MacroSpec`].
    ///
    /// This is the shared port-synthesis + `MacroSpec`-construction step used
    /// by *both* the MDL converter (`mdl/convert/`) and the XMILE reader
    /// (`xmile/`); only the way `body_variables` is produced differs between
    /// the two formats.
    ///
    /// Contract:
    /// - `macro_name`, `parameters`, and `additional_outputs` are already
    ///   canonicalized to the engine ident form used by the body equations
    ///   (`quoted_space_to_underbar` for the MDL path) so they are
    ///   byte-identical to how the body references them.
    /// - For each formal parameter: if a variable with that ident already
    ///   exists in `body_variables` (e.g. the MDL path prepended a
    ///   `<param> = 0` equation so the pipeline could assign the right
    ///   stock/flow/aux kind), its `can_be_module_input` flag is set to
    ///   `true` in place (the kind is preserved). Otherwise a placeholder
    ///   port `Variable` is synthesized: a [`Variable::Flow`] if the
    ///   parameter name appears in any body stock's `inflows`/`outflows`,
    ///   else a [`Variable::Aux`], with a `"0"` placeholder equation and
    ///   `can_be_module_input == true`. The flag is *required*: it is what
    ///   marks the port as a module-input slot whose own equation is only a
    ///   fallback -- an empty one is not an `EmptyEquation` error, and the
    ///   unfilled-equation diagnostic does not report it (`db::diagnostic`)
    ///   -- the same flag an ordinary XMILE submodel sets via
    ///   `access="input"` (a macro model is registered as an ordinary,
    ///   non-`stdlib⁚`-prefixed sub-model).
    /// - `additional_outputs` are computed by the body and so already have
    ///   body equations; they are *not* synthesized as ports.
    /// - `primary_output` is set to `macro_name` without validating that a
    ///   body equation defines it (a missing primary-output equation
    ///   surfaces later, in the compiler).
    pub(crate) fn new_macro(
        macro_name: &str,
        parameters: &[String],
        additional_outputs: &[String],
        mut body_variables: Vec<Variable>,
    ) -> Model {
        // A parameter that the body uses as a stock inflow/outflow must be a
        // Flow port (mirroring stdlib⁚delay1's `input`); otherwise it is an
        // Aux port (mirroring stdlib⁚smth1's `input`).
        let stock_flow_names: std::collections::HashSet<String> = body_variables
            .iter()
            .filter_map(|v| match v {
                Variable::Stock(s) => Some(s),
                _ => None,
            })
            .flat_map(|s| s.inflows.iter().chain(s.outflows.iter()))
            .map(|n| canonicalize(n).to_string())
            .collect();

        for param in parameters {
            let param_canonical = canonicalize(param);
            if let Some(existing) = body_variables
                .iter_mut()
                .find(|v| canonicalize(v.get_ident()) == param_canonical)
            {
                // The pipeline already built this port (with the correct
                // stock/flow/aux kind from a prepended placeholder equation).
                // It only lacks the module-input flag.
                existing.set_can_be_module_input(true);
                continue;
            }

            let compat = Compat {
                can_be_module_input: true,
                ..Compat::default()
            };
            let is_flow = stock_flow_names.contains(param_canonical.as_ref());
            let port = if is_flow {
                Variable::Flow(Flow {
                    ident: param.clone(),
                    equation: Equation::Scalar("0".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat,
                })
            } else {
                Variable::Aux(Aux {
                    ident: param.clone(),
                    equation: Equation::Scalar("0".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat,
                })
            };
            body_variables.push(port);
        }

        Model {
            name: macro_name.to_string(),
            sim_specs: None,
            variables: body_variables.into(),
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: Some(MacroSpec {
                parameters: parameters.to_vec(),
                primary_output: macro_name.to_string(),
                additional_outputs: additional_outputs.to_vec(),
            }),
        }
    }

    pub fn get_variable(&self, ident: &str) -> Option<&Variable> {
        let ident = canonicalize(ident);
        self.variables
            .iter()
            .find(|&var| canonicalize(var.get_ident()) == ident)
    }

    pub fn get_variable_mut(&mut self, ident: &str) -> Option<&mut Variable> {
        let ident = canonicalize(ident);
        self.variables
            .find_mut(|var| canonicalize(var.get_ident()) == ident)
    }

    /// The position of the variable named `ident` in `variables`.
    pub fn variable_index(&self, ident: &str) -> Option<usize> {
        let ident = canonicalize(ident);
        self.variables
            .iter()
            .position(|var| canonicalize(var.get_ident()) == ident)
    }
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum SimMethod {
    Euler,
    RungeKutta2,
    RungeKutta4,
}

/// The default SimMethod is Euler
impl Default for SimMethod {
    fn default() -> Self {
        Self::Euler
    }
}

/// Dt is a UI thing: it can be nice to specify exact
/// fractions that don't display neatly in the UI, like 1/3
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone)]
pub enum Dt {
    Dt(f64),
    Reciprocal(f64),
}

// A dt holds a float, so it is compared by hand, bit for bit (see
// `bitwise_eq!`). The match names every variant, so a new one fails to
// compile until it is compared.
impl PartialEq for Dt {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Dt::Dt(a), Dt::Dt(b)) | (Dt::Reciprocal(a), Dt::Reciprocal(b)) => {
                a.to_bits() == b.to_bits()
            }
            (Dt::Dt(_) | Dt::Reciprocal(_), _) => false,
        }
    }
}
impl Eq for Dt {}

/// The default dt is 1, just like XMILE
impl Default for Dt {
    fn default() -> Self {
        Dt::Dt(1.0)
    }
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Default)]
pub struct SimSpecs {
    pub start: f64,
    pub stop: f64,
    pub dt: Dt,
    pub save_step: Option<Dt>,
    pub sim_method: SimMethod,
    pub time_units: Option<String>,
}
bitwise_eq!(SimSpecs {
    start: float,
    stop: float,
    dt,
    save_step,
    sim_method,
    time_units,
});

/// The elements of a dimension: either indexed (numeric) or named.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub enum DimensionElements {
    /// Indexed dimension with size (e.g., `Periods(5)` has indices 1-5)
    Indexed(u32),
    /// Named dimension with explicit element names
    Named(Vec<String>),
}

/// Element-level correspondence between two subscript families.
///
/// A positional mapping (empty `element_map`) means elements correspond by index:
/// source[0] <-> target[0], source[1] <-> target[1], etc.
///
/// An element-level mapping (non-empty `element_map`) provides explicit
/// source -> target element name pairs for arbitrary correspondence.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub struct DimensionMapping {
    /// Target dimension name
    pub target: String,
    /// Element-level correspondence. When empty, positional mapping is assumed.
    /// When present, maps source elements to target elements by name.
    pub element_map: Vec<(String, String)>,
}

/// A dimension definition, optionally mapping to another dimension.
///
/// Vensim allows specifying dimension mappings like `DimA: A1, A2, A3 -> DimB`
/// which means elements of DimA correspond positionally to elements of DimB.
/// This is used when a variable indexed by DimB references variables indexed by DimA.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub struct Dimension {
    pub name: String,
    pub elements: DimensionElements,
    /// Dimension mappings. Supports both simple positional mappings (single target,
    /// empty element_map) and element-level correspondence mappings.
    ///
    /// **Important**: Dimension mappings are only supported for named dimensions
    /// (those with explicit element names). Indexed dimensions (numeric subscripts)
    /// cannot have mappings - the engine will return None from `get_maps_to` for
    /// indexed dimensions. Additionally, both dimensions in a mapping must have
    /// the same number of elements for valid positional correspondence.
    pub mappings: Vec<DimensionMapping>,
    /// For indexed subdimensions, the name of the parent dimension this
    /// subdimension belongs to. For example, if `SubIndex(3)` is a
    /// subdimension of `FullIndex(5)`, then `parent = Some("FullIndex")`.
    /// This enables `compute_subdimension_relation` to resolve Indexed/Indexed
    /// pairs that share a parent-child relationship.
    pub parent: Option<String>,
}

impl Dimension {
    /// Create a new indexed dimension
    pub fn indexed(name: String, size: u32) -> Self {
        Dimension {
            name,
            elements: DimensionElements::Indexed(size),
            mappings: vec![],
            parent: None,
        }
    }

    /// Create a new named dimension
    pub fn named(name: String, elements: Vec<String>) -> Self {
        Dimension {
            name,
            elements: DimensionElements::Named(elements),
            mappings: vec![],
            parent: None,
        }
    }

    /// Convenience accessor for the simple single-target positional mapping case.
    /// Returns the target dimension name if exactly one positional mapping exists
    /// (i.e., one mapping with an empty element_map).
    pub fn maps_to(&self) -> Option<&str> {
        if self.mappings.len() == 1 && self.mappings[0].element_map.is_empty() {
            Some(&self.mappings[0].target)
        } else {
            None
        }
    }

    /// Set a simple positional mapping to a target dimension.
    pub fn set_maps_to(&mut self, target: String) {
        self.mappings = vec![DimensionMapping {
            target,
            element_map: vec![],
        }];
    }

    pub fn get_offset(&self, subscript: &str) -> Option<usize> {
        match &self.elements {
            DimensionElements::Named(elements) => {
                for (i, element) in elements.iter().enumerate() {
                    if element == subscript {
                        return Some(i);
                    }
                }
                None
            }
            DimensionElements::Indexed(size) => {
                // Parse as number for indexed dimensions, returning None if
                // the subscript is not a valid integer or is out of bounds.
                // This aligns with the safe Option-returning pattern used in
                // dimensions.rs and allows callers to handle invalid subscripts
                // gracefully rather than panicking.
                subscript.parse::<u32>().ok().and_then(|n| {
                    if n >= 1 && n <= *size {
                        Some((n - 1) as usize)
                    } else {
                        None
                    }
                })
            }
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn len(&self) -> usize {
        match &self.elements {
            DimensionElements::Indexed(size) => *size as usize,
            DimensionElements::Named(elements) => elements.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns true if this is an indexed dimension (numeric indices)
    pub fn is_indexed(&self) -> bool {
        matches!(self.elements, DimensionElements::Indexed(_))
    }
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub struct Unit {
    pub name: String,
    pub equation: Option<String>,
    pub disabled: bool,
    pub aliases: Vec<String>,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub enum Extension {
    Unspecified,
    Xmile,
    Vensim,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub struct Source {
    pub extension: Extension,
    pub content: String,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub struct Project {
    pub name: String,
    pub sim_specs: SimSpecs,
    pub dimensions: Vec<Dimension>,
    pub units: Vec<Unit>,
    pub models: Vec<Model>,
    pub source: Option<Source>,
    pub ai_information: Option<AiInformation>,
}

/// Unicode TWO DOT PUNCTUATION used as a separator in stdlib model names.
pub(crate) const STDLIB_PREFIX: &str = "stdlib\u{205A}";

impl Project {
    /// Ensures the project's `models` vec contains definitions for every
    /// stdlib model referenced by a module variable, and removes stdlib
    /// definitions that are no longer referenced. Without this, clients
    /// that build their view from the serialized datamodel (e.g. the
    /// TypeScript diagram editor) cannot display or navigate into stdlib
    /// modules.
    ///
    /// Idempotent. Preserves any user model that shadows a stdlib name.
    pub fn ensure_referenced_stdlib_models(&mut self) {
        // Collect the set of stdlib model names referenced by any module
        // variable across all models.
        let mut referenced: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for model in &self.models {
            for var in &model.variables {
                if let Variable::Module(m) = var
                    && !m.model_name.is_empty()
                    && m.model_name.starts_with(STDLIB_PREFIX)
                {
                    referenced.insert(m.model_name.clone());
                }
            }
        }

        // Prune stdlib models that are no longer referenced by any module.
        // Only removes models whose name matches both the stdlib prefix and
        // one of the 9 known stdlib names from MODEL_NAMES. A user model
        // that shadows a stdlib name (same prefix+name) is also pruned when
        // unreferenced, which is correct: the stdlib prefix uses a special
        // Unicode character that normal model creation never produces, so
        // the only way a shadow model exists is through prior enrichment or
        // an unusual import. When referenced, it is preserved (the add step
        // below respects existing models).
        self.models.retain(|model| {
            if let Some(short) = model.name.strip_prefix(STDLIB_PREFIX)
                && crate::stdlib::MODEL_NAMES.contains(&short)
            {
                return referenced.contains(&model.name);
            }
            true
        });

        // Add missing stdlib models from the embedded definitions.
        // Collect into a Vec first to avoid borrowing self.models while
        // pushing to it.
        let existing: std::collections::HashSet<String> =
            self.models.iter().map(|m| m.name.clone()).collect();
        let to_add: Vec<Model> = referenced
            .into_iter()
            .filter(|name| !existing.contains(name.as_str()))
            .filter_map(|full_name| {
                full_name
                    .strip_prefix(STDLIB_PREFIX)
                    .and_then(crate::stdlib::get)
            })
            .collect();
        self.models.extend(to_add);
    }
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub struct AiInformation {
    pub status: AiStatus,
    pub testing: Option<AiTesting>,
    pub log: Option<String>,
    // TODO: settings
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub struct AiStatus {
    pub key_url: String,
    pub algorithm: String,
    pub signature: String,
    pub tags: HashMap<String, String>,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub struct AiTesting {
    pub signed_message_body: String,
}

impl Project {
    /// The position in `models` of the model named `model_name`: the name as
    /// stored, with `main` also naming a model stored under the empty name.
    pub fn model_index(&self, model_name: &str) -> Option<usize> {
        self.models
            .iter()
            .position(|m| m.name == model_name || (model_name == "main" && m.name.is_empty()))
    }
    pub fn get_model(&self, model_name: &str) -> Option<&Model> {
        self.model_index(model_name).map(|i| &self.models[i])
    }
    pub fn get_model_mut(&mut self, model_name: &str) -> Option<&mut Model> {
        self.model_index(model_name).map(|i| &mut self.models[i])
    }

    /// The position in `models` of the model a host runs when it names none:
    /// the model whose name is `main` ([`canonical_model_name`], so `Main`
    /// and an unnamed model are it too), the last of several as the db files
    /// the later of two models of one name (a project it refuses to compile,
    /// `db::diagnostic::project_duplicate_models`); else the first model that
    /// is neither a macro nor a stdlib model. `None` only for a project with
    /// no such model.
    pub fn default_model_index(&self) -> Option<usize> {
        self.models
            .iter()
            .rposition(|m| canonical_model_name(&m.name) == "main")
            .or_else(|| {
                self.models
                    .iter()
                    .position(|m| m.macro_spec.is_none() && !m.name.starts_with(STDLIB_PREFIX))
            })
    }
    /// The model at [`Project::default_model_index`].
    pub fn default_model(&self) -> Option<&Model> {
        self.default_model_index().map(|i| &self.models[i])
    }
    /// The model at [`Project::default_model_index`], to edit.
    pub fn default_model_mut(&mut self) -> Option<&mut Model> {
        self.default_model_index().map(|i| &mut self.models[i])
    }
}

/// The name a model is known by, which two models of a project must not
/// share: its canonical name (model names are case-, whitespace- and
/// underscore-insensitive), with the empty name read as `main`, as
/// [`Project::model_index`] reads a patch addressed to `main`. Empty is
/// decided on the canonical form, so a blank name is the unnamed model's too:
/// the db files both under the one empty key.
pub fn canonical_model_name(name: &str) -> std::borrow::Cow<'_, str> {
    let canonical = crate::common::canonicalize(name);
    if canonical.is_empty() {
        return std::borrow::Cow::Borrowed("main");
    }
    canonical
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_project_with_module(model_name: &str) -> Project {
        Project {
            name: "test".to_string(),
            sim_specs: SimSpecs::default(),
            dimensions: vec![],
            units: vec![],
            models: vec![Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![Variable::Module(Module {
                    ident: "my_module".to_string(),
                    model_name: model_name.to_string(),
                    documentation: String::new(),
                    units: None,
                    references: vec![],
                    ai_state: None,
                    uid: None,
                    compat: Compat::default(),
                })]
                .into(),
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
                macro_spec: None,
            }],
            source: None,
            ai_information: None,
        }
    }

    #[test]
    fn ensure_stdlib_adds_referenced_model() {
        let mut project = minimal_project_with_module("stdlib\u{205A}systems_rate");
        assert_eq!(project.models.len(), 1);

        project.ensure_referenced_stdlib_models();

        assert_eq!(project.models.len(), 2);
        let stdlib_model = project.get_model("stdlib\u{205A}systems_rate");
        assert!(
            stdlib_model.is_some(),
            "stdlib model should be present after enrichment"
        );
    }

    #[test]
    fn ensure_stdlib_idempotent() {
        let mut project = minimal_project_with_module("stdlib\u{205A}systems_rate");
        project.ensure_referenced_stdlib_models();
        assert_eq!(project.models.len(), 2);

        // Calling again should not duplicate
        project.ensure_referenced_stdlib_models();
        assert_eq!(project.models.len(), 2);
    }

    #[test]
    fn ensure_stdlib_skips_user_models() {
        let mut project = minimal_project_with_module("my_custom_model");
        project.ensure_referenced_stdlib_models();

        // Should not add anything for non-stdlib references
        assert_eq!(project.models.len(), 1);
    }

    #[test]
    fn ensure_stdlib_skips_empty_model_name() {
        let mut project = minimal_project_with_module("");
        project.ensure_referenced_stdlib_models();
        assert_eq!(project.models.len(), 1);
    }

    #[test]
    fn ensure_stdlib_handles_multiple_references_to_same_model() {
        let mut project = Project {
            name: "test".to_string(),
            sim_specs: SimSpecs::default(),
            dimensions: vec![],
            units: vec![],
            models: vec![Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![
                    Variable::Module(Module {
                        ident: "mod_a".to_string(),
                        model_name: "stdlib\u{205A}systems_rate".to_string(),
                        documentation: String::new(),
                        units: None,
                        references: vec![],
                        ai_state: None,
                        uid: None,
                        compat: Compat::default(),
                    }),
                    Variable::Module(Module {
                        ident: "mod_b".to_string(),
                        model_name: "stdlib\u{205A}systems_rate".to_string(),
                        documentation: String::new(),
                        units: None,
                        references: vec![],
                        ai_state: None,
                        uid: None,
                        compat: Compat::default(),
                    }),
                ]
                .into(),
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
                macro_spec: None,
            }],
            source: None,
            ai_information: None,
        };

        project.ensure_referenced_stdlib_models();
        // Two modules referencing the same stdlib model should only add it once
        assert_eq!(project.models.len(), 2);
    }

    #[test]
    fn ensure_stdlib_adds_multiple_different_models() {
        let mut project = Project {
            name: "test".to_string(),
            sim_specs: SimSpecs::default(),
            dimensions: vec![],
            units: vec![],
            models: vec![Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![
                    Variable::Module(Module {
                        ident: "rate_mod".to_string(),
                        model_name: "stdlib\u{205A}systems_rate".to_string(),
                        documentation: String::new(),
                        units: None,
                        references: vec![],
                        ai_state: None,
                        uid: None,
                        compat: Compat::default(),
                    }),
                    Variable::Module(Module {
                        ident: "leak_mod".to_string(),
                        model_name: "stdlib\u{205A}systems_leak".to_string(),
                        documentation: String::new(),
                        units: None,
                        references: vec![],
                        ai_state: None,
                        uid: None,
                        compat: Compat::default(),
                    }),
                ]
                .into(),
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
                macro_spec: None,
            }],
            source: None,
            ai_information: None,
        };

        project.ensure_referenced_stdlib_models();
        assert_eq!(project.models.len(), 3);
        assert!(project.get_model("stdlib\u{205A}systems_rate").is_some());
        assert!(project.get_model("stdlib\u{205A}systems_leak").is_some());
    }

    #[test]
    fn ensure_stdlib_preserves_user_shadow_model() {
        // A project may already contain a model whose name matches a stdlib
        // name (e.g. imported from an older format). The enrichment must not
        // overwrite it -- the user's model definition takes precedence.
        let custom_var = Variable::Aux(Aux {
            ident: "my_custom_var".to_string(),
            equation: Equation::Scalar("42".to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: Compat::default(),
        });
        let mut project = Project {
            name: "test".to_string(),
            sim_specs: SimSpecs::default(),
            dimensions: vec![],
            units: vec![],
            models: vec![
                Model {
                    name: "main".to_string(),
                    sim_specs: None,
                    variables: vec![Variable::Module(Module {
                        ident: "rate_mod".to_string(),
                        model_name: "stdlib\u{205A}systems_rate".to_string(),
                        documentation: String::new(),
                        units: None,
                        references: vec![],
                        ai_state: None,
                        uid: None,
                        compat: Compat::default(),
                    })]
                    .into(),
                    views: vec![],
                    loop_metadata: vec![],
                    groups: vec![],
                    macro_spec: None,
                },
                // User-created model that shadows the stdlib name
                Model {
                    name: "stdlib\u{205A}systems_rate".to_string(),
                    sim_specs: None,
                    variables: vec![custom_var].into(),
                    views: vec![],
                    loop_metadata: vec![],
                    groups: vec![],
                    macro_spec: None,
                },
            ],
            source: None,
            ai_information: None,
        };

        project.ensure_referenced_stdlib_models();

        // Should NOT add a duplicate -- the user's model already exists
        assert_eq!(project.models.len(), 2);
        // The user's custom variable should still be there (not replaced
        // by the canonical stdlib definition)
        let model = project.get_model("stdlib\u{205A}systems_rate").unwrap();
        assert!(
            model
                .variables
                .iter()
                .any(|v| v.get_ident() == "my_custom_var"),
            "user's custom variable should be preserved"
        );
    }

    #[test]
    fn ensure_stdlib_prunes_unreferenced_models() {
        // Start with a project that has a stdlib model in its models vec
        // but no module variable referencing it.
        let stdlib_model = crate::stdlib::get("systems_rate").unwrap();
        let mut project = Project {
            name: "test".to_string(),
            sim_specs: SimSpecs::default(),
            dimensions: vec![],
            units: vec![],
            models: vec![
                Model {
                    name: "main".to_string(),
                    sim_specs: None,
                    variables: vec![].into(),
                    views: vec![],
                    loop_metadata: vec![],
                    groups: vec![],
                    macro_spec: None,
                },
                stdlib_model,
            ],
            source: None,
            ai_information: None,
        };

        assert_eq!(project.models.len(), 2);
        project.ensure_referenced_stdlib_models();
        // The stdlib model should be removed since nothing references it
        assert_eq!(project.models.len(), 1);
        assert!(project.get_model("stdlib\u{205A}systems_rate").is_none());
    }

    #[test]
    fn ensure_stdlib_keeps_referenced_prunes_unreferenced() {
        let rate_model = crate::stdlib::get("systems_rate").unwrap();
        let leak_model = crate::stdlib::get("systems_leak").unwrap();
        let mut project = Project {
            name: "test".to_string(),
            sim_specs: SimSpecs::default(),
            dimensions: vec![],
            units: vec![],
            models: vec![
                Model {
                    name: "main".to_string(),
                    sim_specs: None,
                    // Only references systems_rate, not systems_leak
                    variables: vec![Variable::Module(Module {
                        ident: "rate_mod".to_string(),
                        model_name: "stdlib\u{205A}systems_rate".to_string(),
                        documentation: String::new(),
                        units: None,
                        references: vec![],
                        ai_state: None,
                        uid: None,
                        compat: Compat::default(),
                    })]
                    .into(),
                    views: vec![],
                    loop_metadata: vec![],
                    groups: vec![],
                    macro_spec: None,
                },
                rate_model,
                leak_model,
            ],
            source: None,
            ai_information: None,
        };

        assert_eq!(project.models.len(), 3);
        project.ensure_referenced_stdlib_models();
        // systems_rate stays (referenced), systems_leak is pruned
        assert_eq!(project.models.len(), 2);
        assert!(project.get_model("stdlib\u{205A}systems_rate").is_some());
        assert!(project.get_model("stdlib\u{205A}systems_leak").is_none());
    }

    #[test]
    fn equation_source_text_scalar_and_apply_to_all_round_trip_verbatim() {
        assert_eq!(Equation::Scalar("a + b".to_string()).source_text(), "a + b");
        assert_eq!(
            Equation::ApplyToAll(vec!["Region".to_string()], "pop * 0.02".to_string())
                .source_text(),
            "pop * 0.02"
        );
    }

    #[test]
    fn equation_source_text_arrayed_joins_elements_and_except_default() {
        let elements = vec![
            ("NYC".to_string(), "pop[NYC] * 0.03".to_string(), None, None),
            (
                "Boston".to_string(),
                "pop[Boston] * 0.02".to_string(),
                None,
                None,
            ),
        ];

        // Without an EXCEPT default: only the per-element formulas.
        let no_default =
            Equation::Arrayed(vec!["Region".to_string()], elements.clone(), None, false);
        assert_eq!(
            no_default.source_text(),
            "pop[NYC] * 0.03\npop[Boston] * 0.02"
        );

        // With an EXCEPT default: the default formula is appended after the
        // explicitly-listed elements. This is the branch the prior tests
        // never exercised.
        let with_default = Equation::Arrayed(
            vec!["Region".to_string()],
            elements,
            Some("pop * 0.01".to_string()),
            true,
        );
        assert_eq!(
            with_default.source_text(),
            "pop[NYC] * 0.03\npop[Boston] * 0.02\npop * 0.01"
        );
    }
}

#[cfg(test)]
#[path = "datamodel_equality_tests.rs"]
mod equality_tests;
