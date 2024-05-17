// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};
use std::iter::Iterator;

#[cfg(feature = "wasm")]
use wasm_bindgen::prelude::*;

use crate::common::{DimensionName, ElementName};

#[derive(Debug, Default, Eq, Clone)]
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
        for (_id, exp) in self.map.iter_mut() {
            *exp *= -1;
        }
        self
    }

    pub fn exp(mut self, exp: i32) -> Self {
        for (_id, unit) in self.map.iter_mut() {
            *unit *= exp;
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
        format!("{}", self)
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
                Some(m) => n + *m,
            };

            if new_value == 0 {
                self.map.remove(&unit);
            } else {
                self.map.insert(unit, new_value);
            }
        }

        if let Some(rctx) = rhs.ctx.take() {
            if !rctx.is_empty() {
                let mut ctx = self.ctx.take().unwrap_or_default();
                ctx.extend(rctx);
                self.ctx = Some(ctx);
            }
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
            write!(f, "{}", unit)?;
            if exp.abs() > 1 {
                write!(f, "^{}", exp.abs())?;
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
            write!(f, "{}", unit)?;
            if exp.abs() > 1 {
                write!(f, "^{}", exp.abs())?;
            }
        }

        if !written {
            write!(f, "dmnl")?;
        }

        // if let Some(ctxs) = &self.ctx {
        //     write!(f, " (")?;
        //     for (i, ctx) in ctxs.iter().enumerate() {
        //         if i == 0 {
        //             write!(f, "{}", ctx)?;
        //         } else {
        //             write!(f, " ,{}", ctx)?;
        //         }
        //     }
        //     write!(f, ")")?;
        // }

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

#[cfg_attr(feature = "wasm", wasm_bindgen)]
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum GraphicalFunctionKind {
    Continuous,
    Extrapolate,
    Discrete,
}

#[cfg_attr(feature = "wasm", wasm_bindgen)]
#[derive(Clone, PartialEq, Debug)]
pub struct GraphicalFunctionScale {
    pub min: f64,
    pub max: f64,
}

#[derive(Clone, PartialEq, Debug)]
pub struct GraphicalFunction {
    pub kind: GraphicalFunctionKind,
    pub x_points: Option<Vec<f64>>,
    pub y_points: Vec<f64>,
    pub x_scale: GraphicalFunctionScale,
    pub y_scale: GraphicalFunctionScale,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Equation {
    Scalar(String, Option<String>),
    ApplyToAll(Vec<DimensionName>, String, Option<String>),
    Arrayed(
        Vec<DimensionName>,
        Vec<(ElementName, String, Option<String>)>,
    ),
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Visibility {
    Private,
    Public,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Stock {
    pub ident: String,
    pub equation: Equation,
    pub documentation: String,
    pub units: Option<String>,
    pub inflows: Vec<String>,
    pub outflows: Vec<String>,
    pub non_negative: bool,
    pub can_be_module_input: bool,
    pub visibility: Visibility,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Flow {
    pub ident: String,
    pub equation: Equation,
    pub documentation: String,
    pub units: Option<String>,
    pub gf: Option<GraphicalFunction>,
    pub non_negative: bool,
    pub can_be_module_input: bool,
    pub visibility: Visibility,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Aux {
    pub ident: String,
    pub equation: Equation,
    pub documentation: String,
    pub units: Option<String>,
    pub gf: Option<GraphicalFunction>,
    pub can_be_module_input: bool,
    pub visibility: Visibility,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ModuleReference {
    pub src: String,
    pub dst: String,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Module {
    pub ident: String,
    pub model_name: String,
    pub documentation: String,
    pub units: Option<String>,
    pub references: Vec<ModuleReference>,
    pub can_be_module_input: bool,
    pub visibility: Visibility,
}

#[derive(Clone, PartialEq, Debug)]
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
            Variable::Stock(stock) => stock.equation = Equation::Scalar(equation.to_string(), None),
            Variable::Flow(flow) => flow.equation = Equation::Scalar(equation.to_string(), None),
            Variable::Aux(aux) => aux.equation = Equation::Scalar(equation.to_string(), None),
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
            Variable::Stock(stock) => stock.visibility,
            Variable::Flow(flow) => flow.visibility,
            Variable::Aux(aux) => aux.visibility,
            Variable::Module(module) => module.visibility,
        }
    }

    pub fn can_be_module_input(&self) -> bool {
        match self {
            Variable::Stock(stock) => stock.can_be_module_input,
            Variable::Flow(flow) => flow.can_be_module_input,
            Variable::Aux(aux) => aux.can_be_module_input,
            Variable::Module(module) => module.can_be_module_input,
        }
    }
}

pub mod view_element {
    #[cfg(feature = "wasm")]
    use wasm_bindgen::prelude::*;

    #[cfg_attr(feature = "wasm", wasm_bindgen)]
    #[derive(Copy, Clone, PartialEq, Eq, Debug)]
    pub enum LabelSide {
        Top,
        Left,
        Center,
        Bottom,
        Right,
    }

    #[derive(Clone, PartialEq, Debug)]
    pub struct Aux {
        pub name: String,
        pub uid: i32,
        pub x: f64,
        pub y: f64,
        pub label_side: LabelSide,
    }

    #[derive(Clone, PartialEq, Debug)]
    pub struct Stock {
        pub name: String,
        pub uid: i32,
        pub x: f64,
        pub y: f64,
        pub label_side: LabelSide,
    }

    #[cfg_attr(feature = "wasm", wasm_bindgen)]
    #[derive(Clone, PartialEq, Debug)]
    pub struct FlowPoint {
        pub x: f64,
        pub y: f64,
        pub attached_to_uid: Option<i32>,
    }

    #[derive(Clone, PartialEq, Debug)]
    pub struct Flow {
        pub name: String,
        pub uid: i32,
        pub x: f64,
        pub y: f64,
        pub label_side: LabelSide,
        // pub segment_with_aux: i32,
        // pub aux_percentage_into_segment: f64,
        pub points: Vec<FlowPoint>,
    }

    #[derive(Clone, PartialEq, Debug)]
    pub enum LinkShape {
        Straight,
        Arc(f64), // angle in [0, 360)
        MultiPoint(Vec<FlowPoint>),
    }

    #[derive(Clone, PartialEq, Debug)]
    pub struct Link {
        pub uid: i32,
        pub from_uid: i32,
        pub to_uid: i32,
        pub shape: LinkShape,
    }

    #[derive(Clone, PartialEq, Debug)]
    pub struct Module {
        pub name: String,
        pub uid: i32,
        pub x: f64,
        pub y: f64,
        pub label_side: LabelSide,
    }

    #[cfg_attr(feature = "wasm", wasm_bindgen)]
    #[derive(Clone, PartialEq, Debug)]
    pub struct Alias {
        pub uid: i32,
        pub alias_of_uid: i32,
        pub x: f64,
        pub y: f64,
        pub label_side: LabelSide,
    }

    #[derive(Clone, PartialEq, Debug)]
    pub struct Cloud {
        pub uid: i32,
        pub flow_uid: i32,
        pub x: f64,
        pub y: f64,
    }
}

#[derive(Clone, PartialEq, Debug)]
pub enum ViewElement {
    Aux(view_element::Aux),
    Stock(view_element::Stock),
    Flow(view_element::Flow),
    Link(view_element::Link),
    Module(view_element::Module),
    Alias(view_element::Alias),
    Cloud(view_element::Cloud),
}

impl ViewElement {
    pub fn get_uid(&self) -> i32 {
        match self {
            ViewElement::Aux(var) => var.uid,
            ViewElement::Stock(var) => var.uid,
            ViewElement::Flow(var) => var.uid,
            ViewElement::Link(var) => var.uid,
            ViewElement::Module(var) => var.uid,
            ViewElement::Alias(var) => var.uid,
            ViewElement::Cloud(var) => var.uid,
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
        }
    }
}

#[derive(Clone, PartialEq, Debug, Default)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, PartialEq, Debug)]
pub struct StockFlow {
    pub elements: Vec<ViewElement>,
    pub view_box: Rect,
    pub zoom: f64,
}

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

#[derive(Clone, PartialEq, Debug)]
pub enum View {
    StockFlow(StockFlow),
}

#[derive(Clone, PartialEq, Debug)]
pub struct Model {
    pub name: String,
    pub variables: Vec<Variable>,
    pub views: Vec<View>,
}

impl Model {
    pub fn get_variable(&self, ident: &str) -> Option<&Variable> {
        self.variables.iter().find(|&var| var.get_ident() == ident)
    }

    pub fn get_variable_mut(&mut self, ident: &str) -> Option<&mut Variable> {
        self.variables
            .iter_mut()
            .find(|var| var.get_ident() == ident)
    }
}

#[cfg_attr(feature = "wasm", wasm_bindgen)]
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SimMethod {
    Euler,
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
#[derive(Clone, PartialEq, Debug)]
pub enum Dt {
    Dt(f64),
    Reciprocal(f64),
}

/// The default dt is 1, just like XMILE
impl Default for Dt {
    fn default() -> Self {
        Dt::Dt(1.0)
    }
}

#[derive(Clone, PartialEq, Debug, Default)]
pub struct SimSpecs {
    pub start: f64,
    pub stop: f64,
    pub dt: Dt,
    pub save_step: Option<Dt>,
    pub sim_method: SimMethod,
    pub time_units: Option<String>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Dimension {
    Indexed(String, u32),
    Named(String, Vec<String>),
}

impl Dimension {
    pub fn get_offset(&self, subscript: &str) -> Option<usize> {
        if let Dimension::Named(_, elements) = self {
            for (i, element) in elements.iter().enumerate() {
                if element == subscript {
                    return Some(i);
                }
            }
        }
        None
    }

    pub fn name(&self) -> &str {
        match self {
            Dimension::Indexed(name, _) => name,
            Dimension::Named(name, _) => name,
        }
    }

    pub fn len(&self) -> usize {
        match self {
            Dimension::Indexed(_, size) => *size as usize,
            Dimension::Named(_, elements) => elements.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Unit {
    pub name: String,
    pub equation: Option<String>,
    pub disabled: bool,
    pub aliases: Vec<String>,
}

#[cfg_attr(feature = "wasm", wasm_bindgen)]
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Extension {
    Unspecified,
    Xmile,
    Vensim,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Source {
    pub extension: Extension,
    pub content: String,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Project {
    pub name: String,
    pub sim_specs: SimSpecs,
    pub dimensions: Vec<Dimension>,
    pub units: Vec<Unit>,
    pub models: Vec<Model>,
    pub source: Option<Source>,
}

impl Project {
    pub fn get_model(&self, model_name: &str) -> Option<&Model> {
        self.models
            .iter()
            .find(|m| m.name == model_name || (model_name == "main" && m.name.is_empty()))
    }
    pub fn get_model_mut(&mut self, model_name: &str) -> Option<&mut Model> {
        self.models.iter_mut().find(|m| m.name == model_name)
    }
}
