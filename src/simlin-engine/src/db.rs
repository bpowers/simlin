// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;

use crate::canonicalize;
use crate::datamodel;

// ── Database ───────────────────────────────────────────────────────────

#[salsa::db]
pub trait Db: salsa::Database {}

#[salsa::db]
#[derive(Default, Clone)]
pub struct SimlinDb {
    storage: salsa::Storage<Self>,
}

#[salsa::db]
impl salsa::Database for SimlinDb {}

#[salsa::db]
impl Db for SimlinDb {}

// ── Interned identifiers ───────────────────────────────────────────────

#[salsa::interned(debug)]
pub struct VariableId<'db> {
    #[returns(ref)]
    pub text: String,
}

#[salsa::interned(debug)]
pub struct ModelId<'db> {
    #[returns(ref)]
    pub text: String,
}

// ── Variable kind ──────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub enum SourceVariableKind {
    Stock,
    Flow,
    Aux,
    Module,
}

impl SourceVariableKind {
    fn from_datamodel_variable(var: &datamodel::Variable) -> Self {
        match var {
            datamodel::Variable::Stock(_) => SourceVariableKind::Stock,
            datamodel::Variable::Flow(_) => SourceVariableKind::Flow,
            datamodel::Variable::Aux(_) => SourceVariableKind::Aux,
            datamodel::Variable::Module(_) => SourceVariableKind::Module,
        }
    }
}

// ── Input types ────────────────────────────────────────────────────────

#[salsa::input]
pub struct SourceProject {
    #[returns(ref)]
    pub name: String,
    #[returns(ref)]
    pub sim_specs: SourceSimSpecs,
    #[returns(ref)]
    pub dimensions: Vec<SourceDimension>,
    #[returns(ref)]
    pub model_names: Vec<String>,
}

#[salsa::input]
pub struct SourceModel {
    #[returns(ref)]
    pub name: String,
    #[returns(ref)]
    pub variable_names: Vec<String>,
}

#[salsa::input]
pub struct SourceVariable {
    #[returns(ref)]
    pub equation: SourceEquation,
    pub kind: SourceVariableKind,
    #[returns(ref)]
    pub units: Option<String>,
    #[returns(ref)]
    pub gf: Option<SourceGraphicalFunction>,
    #[returns(ref)]
    pub inflows: Vec<String>,
    #[returns(ref)]
    pub outflows: Vec<String>,
    #[returns(ref)]
    pub module_refs: Vec<SourceModuleReference>,
    #[returns(ref)]
    pub model_name: String,
    pub non_negative: bool,
    pub can_be_module_input: bool,
}

// ── Mirror types for salsa compatibility ───────────────────────────────
//
// These types mirror the datamodel types but derive salsa::Update.
// This avoids modifying the datamodel module (which is used for
// serialization and has its own derive constraints).

#[derive(Clone, Debug, PartialEq, salsa::Update)]
pub struct SourceSimSpecs {
    pub start: f64,
    pub stop: f64,
    pub dt: SourceDt,
    pub save_step: Option<SourceDt>,
    pub sim_method: SourceSimMethod,
    pub time_units: Option<String>,
}

#[derive(Clone, Debug, PartialEq, salsa::Update)]
pub enum SourceDt {
    Dt(f64),
    Reciprocal(f64),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub enum SourceSimMethod {
    Euler,
    RungeKutta2,
    RungeKutta4,
}

#[derive(Clone, Debug, PartialEq, salsa::Update)]
pub struct SourceDimension {
    pub name: String,
    pub elements: SourceDimensionElements,
    pub maps_to: Option<String>,
}

#[derive(Clone, Debug, PartialEq, salsa::Update)]
pub enum SourceDimensionElements {
    Indexed(u32),
    Named(Vec<String>),
}

#[derive(Clone, Debug, PartialEq, salsa::Update)]
pub enum SourceEquation {
    Scalar(String),
    ApplyToAll(Vec<String>, String),
    Arrayed(Vec<String>, Vec<SourceArrayedEquationElement>),
}

#[derive(Clone, Debug, PartialEq, salsa::Update)]
pub struct SourceArrayedEquationElement {
    pub subscript: String,
    pub equation: String,
    pub gf_equation: Option<String>,
    pub gf: Option<SourceGraphicalFunction>,
}

#[derive(Clone, Debug, PartialEq, salsa::Update)]
pub struct SourceGraphicalFunction {
    pub kind: SourceGraphicalFunctionKind,
    pub x_points: Option<Vec<f64>>,
    pub y_points: Vec<f64>,
    pub x_scale: SourceGraphicalFunctionScale,
    pub y_scale: SourceGraphicalFunctionScale,
}

#[derive(Copy, Clone, Debug, PartialEq, salsa::Update)]
pub enum SourceGraphicalFunctionKind {
    Continuous,
    Extrapolate,
    Discrete,
}

#[derive(Clone, Debug, PartialEq, salsa::Update)]
pub struct SourceGraphicalFunctionScale {
    pub min: f64,
    pub max: f64,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct SourceModuleReference {
    pub src: String,
    pub dst: String,
}

// ── Conversion from datamodel types ────────────────────────────────────

impl From<&datamodel::SimSpecs> for SourceSimSpecs {
    fn from(specs: &datamodel::SimSpecs) -> Self {
        SourceSimSpecs {
            start: specs.start,
            stop: specs.stop,
            dt: SourceDt::from(&specs.dt),
            save_step: specs.save_step.as_ref().map(SourceDt::from),
            sim_method: SourceSimMethod::from(specs.sim_method),
            time_units: specs.time_units.clone(),
        }
    }
}

impl From<&datamodel::Dt> for SourceDt {
    fn from(dt: &datamodel::Dt) -> Self {
        match dt {
            datamodel::Dt::Dt(v) => SourceDt::Dt(*v),
            datamodel::Dt::Reciprocal(v) => SourceDt::Reciprocal(*v),
        }
    }
}

impl From<datamodel::SimMethod> for SourceSimMethod {
    fn from(method: datamodel::SimMethod) -> Self {
        match method {
            datamodel::SimMethod::Euler => SourceSimMethod::Euler,
            datamodel::SimMethod::RungeKutta2 => SourceSimMethod::RungeKutta2,
            datamodel::SimMethod::RungeKutta4 => SourceSimMethod::RungeKutta4,
        }
    }
}

impl From<&datamodel::Dimension> for SourceDimension {
    fn from(dim: &datamodel::Dimension) -> Self {
        SourceDimension {
            name: dim.name.clone(),
            elements: SourceDimensionElements::from(&dim.elements),
            maps_to: dim.maps_to.clone(),
        }
    }
}

impl From<&datamodel::DimensionElements> for SourceDimensionElements {
    fn from(elements: &datamodel::DimensionElements) -> Self {
        match elements {
            datamodel::DimensionElements::Indexed(size) => SourceDimensionElements::Indexed(*size),
            datamodel::DimensionElements::Named(names) => {
                SourceDimensionElements::Named(names.clone())
            }
        }
    }
}

impl From<&datamodel::Equation> for SourceEquation {
    fn from(eq: &datamodel::Equation) -> Self {
        match eq {
            datamodel::Equation::Scalar(s) => SourceEquation::Scalar(s.clone()),
            datamodel::Equation::ApplyToAll(dims, s) => {
                SourceEquation::ApplyToAll(dims.clone(), s.clone())
            }
            datamodel::Equation::Arrayed(dims, elements) => SourceEquation::Arrayed(
                dims.clone(),
                elements
                    .iter()
                    .map(|(subscript, eq, gf_eq, gf)| SourceArrayedEquationElement {
                        subscript: subscript.clone(),
                        equation: eq.clone(),
                        gf_equation: gf_eq.clone(),
                        gf: gf.as_ref().map(SourceGraphicalFunction::from),
                    })
                    .collect(),
            ),
        }
    }
}

impl From<&datamodel::GraphicalFunction> for SourceGraphicalFunction {
    fn from(gf: &datamodel::GraphicalFunction) -> Self {
        SourceGraphicalFunction {
            kind: SourceGraphicalFunctionKind::from(gf.kind),
            x_points: gf.x_points.clone(),
            y_points: gf.y_points.clone(),
            x_scale: SourceGraphicalFunctionScale::from(&gf.x_scale),
            y_scale: SourceGraphicalFunctionScale::from(&gf.y_scale),
        }
    }
}

impl From<datamodel::GraphicalFunctionKind> for SourceGraphicalFunctionKind {
    fn from(kind: datamodel::GraphicalFunctionKind) -> Self {
        match kind {
            datamodel::GraphicalFunctionKind::Continuous => SourceGraphicalFunctionKind::Continuous,
            datamodel::GraphicalFunctionKind::Extrapolate => {
                SourceGraphicalFunctionKind::Extrapolate
            }
            datamodel::GraphicalFunctionKind::Discrete => SourceGraphicalFunctionKind::Discrete,
        }
    }
}

impl From<&datamodel::GraphicalFunctionScale> for SourceGraphicalFunctionScale {
    fn from(scale: &datamodel::GraphicalFunctionScale) -> Self {
        SourceGraphicalFunctionScale {
            min: scale.min,
            max: scale.max,
        }
    }
}

impl From<&datamodel::ModuleReference> for SourceModuleReference {
    fn from(mr: &datamodel::ModuleReference) -> Self {
        SourceModuleReference {
            src: mr.src.clone(),
            dst: mr.dst.clone(),
        }
    }
}

// ── Sync result ────────────────────────────────────────────────────────

/// Result of syncing a datamodel::Project into the salsa database.
/// Maps names to their salsa input/interned IDs for subsequent lookups.
pub struct SyncResult<'db> {
    pub project: SourceProject,
    pub models: HashMap<String, SyncedModel<'db>>,
}

pub struct SyncedModel<'db> {
    pub id: ModelId<'db>,
    pub source: SourceModel,
    pub variables: HashMap<String, SyncedVariable<'db>>,
}

pub struct SyncedVariable<'db> {
    pub id: VariableId<'db>,
    pub source: SourceVariable,
}

// ── Sync function ──────────────────────────────────────────────────────

/// Populate salsa inputs from a `datamodel::Project`.
///
/// Creates `SourceProject`, `SourceModel`, and `SourceVariable` inputs in
/// the database, along with interned `ModelId` and `VariableId` identifiers.
/// For Phase 2, these inputs are populated but not yet read by the
/// compilation pipeline.
pub fn sync_from_datamodel<'db>(
    db: &'db SimlinDb,
    project: &datamodel::Project,
) -> SyncResult<'db> {
    let model_names: Vec<String> = project.models.iter().map(|m| m.name.clone()).collect();

    let source_project = SourceProject::new(
        db,
        project.name.clone(),
        SourceSimSpecs::from(&project.sim_specs),
        project
            .dimensions
            .iter()
            .map(SourceDimension::from)
            .collect(),
        model_names,
    );

    let mut models = HashMap::new();

    for dm_model in &project.models {
        let canonical_model_name = canonicalize(&dm_model.name).into_owned();
        let model_id = ModelId::new(db, canonical_model_name.clone());

        let variable_names: Vec<String> = dm_model
            .variables
            .iter()
            .map(|v| v.get_ident().to_string())
            .collect();

        let source_model = SourceModel::new(db, dm_model.name.clone(), variable_names);

        let mut variables = HashMap::new();

        for dm_var in &dm_model.variables {
            let canonical_var_name = canonicalize(dm_var.get_ident()).into_owned();
            let var_id = VariableId::new(db, canonical_var_name.clone());

            let source_var = source_variable_from_datamodel(db, dm_var, &dm_model.name);

            variables.insert(
                canonical_var_name,
                SyncedVariable {
                    id: var_id,
                    source: source_var,
                },
            );
        }

        models.insert(
            canonical_model_name,
            SyncedModel {
                id: model_id,
                source: source_model,
                variables,
            },
        );
    }

    SyncResult {
        project: source_project,
        models,
    }
}

fn source_variable_from_datamodel(
    db: &SimlinDb,
    var: &datamodel::Variable,
    model_name: &str,
) -> SourceVariable {
    let kind = SourceVariableKind::from_datamodel_variable(var);

    let equation = var
        .get_equation()
        .map(SourceEquation::from)
        .unwrap_or_else(|| SourceEquation::Scalar(String::new()));

    let units = var.get_units().cloned();

    let gf = match var {
        datamodel::Variable::Flow(f) => f.gf.as_ref().map(SourceGraphicalFunction::from),
        datamodel::Variable::Aux(a) => a.gf.as_ref().map(SourceGraphicalFunction::from),
        _ => None,
    };

    let inflows = match var {
        datamodel::Variable::Stock(s) => s.inflows.clone(),
        _ => Vec::new(),
    };

    let outflows = match var {
        datamodel::Variable::Stock(s) => s.outflows.clone(),
        _ => Vec::new(),
    };

    let module_refs = match var {
        datamodel::Variable::Module(m) => m
            .references
            .iter()
            .map(SourceModuleReference::from)
            .collect(),
        _ => Vec::new(),
    };

    let non_negative = match var {
        datamodel::Variable::Stock(s) => s.non_negative,
        datamodel::Variable::Flow(f) => f.non_negative,
        _ => false,
    };

    let can_be_module_input = var.can_be_module_input();

    SourceVariable::new(
        db,
        equation,
        kind,
        units,
        gf,
        inflows,
        outflows,
        module_refs,
        model_name.to_string(),
        non_negative,
        can_be_module_input,
    )
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datamodel;

    fn simple_project() -> datamodel::Project {
        datamodel::Project {
            name: "test".to_string(),
            sim_specs: datamodel::SimSpecs {
                start: 0.0,
                stop: 10.0,
                dt: datamodel::Dt::Dt(1.0),
                save_step: None,
                sim_method: datamodel::SimMethod::Euler,
                time_units: Some("months".to_string()),
            },
            dimensions: vec![],
            units: vec![],
            models: vec![datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![datamodel::Variable::Aux(datamodel::Aux {
                    ident: "population".to_string(),
                    equation: datamodel::Equation::Scalar("100".to_string()),
                    documentation: String::new(),
                    units: Some("people".to_string()),
                    gf: None,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                })],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
            }],
            source: None,
            ai_information: None,
        }
    }

    #[test]
    fn test_create_db() {
        let _db = SimlinDb::default();
    }

    #[test]
    fn test_intern_variable_id_same_name() {
        let db = SimlinDb::default();
        let id1 = VariableId::new(&db, "population".to_string());
        let id2 = VariableId::new(&db, "population".to_string());
        assert_eq!(id1, id2);
    }

    #[test]
    fn test_intern_variable_id_different_names() {
        let db = SimlinDb::default();
        let id1 = VariableId::new(&db, "population".to_string());
        let id2 = VariableId::new(&db, "birth_rate".to_string());
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_intern_model_id_same_name() {
        let db = SimlinDb::default();
        let id1 = ModelId::new(&db, "main".to_string());
        let id2 = ModelId::new(&db, "main".to_string());
        assert_eq!(id1, id2);
    }

    #[test]
    fn test_intern_model_id_different_names() {
        let db = SimlinDb::default();
        let id1 = ModelId::new(&db, "main".to_string());
        let id2 = ModelId::new(&db, "submodel".to_string());
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_intern_variable_id_text_roundtrip() {
        let db = SimlinDb::default();
        let id = VariableId::new(&db, "birth_rate".to_string());
        assert_eq!(id.text(&db), "birth_rate");
    }

    #[test]
    fn test_intern_model_id_text_roundtrip() {
        let db = SimlinDb::default();
        let id = ModelId::new(&db, "main".to_string());
        assert_eq!(id.text(&db), "main");
    }

    #[test]
    fn test_sync_simple_project() {
        let db = SimlinDb::default();
        let project = simple_project();
        let result = sync_from_datamodel(&db, &project);

        assert_eq!(result.project.name(&db), "test");
        assert_eq!(result.project.model_names(&db).len(), 1);
        assert_eq!(result.project.model_names(&db)[0], "main");

        let sim_specs = result.project.sim_specs(&db);
        assert_eq!(sim_specs.start, 0.0);
        assert_eq!(sim_specs.stop, 10.0);
        assert_eq!(sim_specs.time_units, Some("months".to_string()));

        assert!(result.models.contains_key("main"));
        let main_model = &result.models["main"];
        assert_eq!(main_model.source.name(&db), "main");
        assert_eq!(main_model.source.variable_names(&db).len(), 1);
        assert_eq!(main_model.source.variable_names(&db)[0], "population");

        let pop_var = &main_model.variables["population"];
        assert_eq!(pop_var.id.text(&db), "population");
        assert_eq!(pop_var.source.kind(&db), SourceVariableKind::Aux);
        assert_eq!(pop_var.source.units(&db), &Some("people".to_string()));
        assert_eq!(
            pop_var.source.equation(&db),
            &SourceEquation::Scalar("100".to_string())
        );
        assert!(!pop_var.source.non_negative(&db));
        assert!(!pop_var.source.can_be_module_input(&db));
    }

    #[test]
    fn test_sync_multi_model() {
        let db = SimlinDb::default();
        let project = datamodel::Project {
            name: "multi".to_string(),
            sim_specs: datamodel::SimSpecs::default(),
            dimensions: vec![],
            units: vec![],
            models: vec![
                datamodel::Model {
                    name: "main".to_string(),
                    sim_specs: None,
                    variables: vec![datamodel::Variable::Aux(datamodel::Aux {
                        ident: "x".to_string(),
                        equation: datamodel::Equation::Scalar("1".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        can_be_module_input: false,
                        visibility: datamodel::Visibility::Private,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    })],
                    views: vec![],
                    loop_metadata: vec![],
                    groups: vec![],
                },
                datamodel::Model {
                    name: "submodel".to_string(),
                    sim_specs: None,
                    variables: vec![datamodel::Variable::Aux(datamodel::Aux {
                        ident: "y".to_string(),
                        equation: datamodel::Equation::Scalar("2".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        can_be_module_input: false,
                        visibility: datamodel::Visibility::Private,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    })],
                    views: vec![],
                    loop_metadata: vec![],
                    groups: vec![],
                },
            ],
            source: None,
            ai_information: None,
        };

        let result = sync_from_datamodel(&db, &project);
        assert_eq!(result.models.len(), 2);
        assert!(result.models.contains_key("main"));
        assert!(result.models.contains_key("submodel"));

        // Different model names get different IDs
        let main_id = result.models["main"].id;
        let sub_id = result.models["submodel"].id;
        assert_ne!(main_id, sub_id);
    }

    #[test]
    fn test_sync_all_variable_kinds() {
        let db = SimlinDb::default();
        let project = datamodel::Project {
            name: "kinds".to_string(),
            sim_specs: datamodel::SimSpecs::default(),
            dimensions: vec![],
            units: vec![],
            models: vec![datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![
                    datamodel::Variable::Stock(datamodel::Stock {
                        ident: "stock_var".to_string(),
                        equation: datamodel::Equation::Scalar("100".to_string()),
                        documentation: String::new(),
                        units: None,
                        inflows: vec!["flow_in".to_string()],
                        outflows: vec!["flow_out".to_string()],
                        non_negative: true,
                        can_be_module_input: false,
                        visibility: datamodel::Visibility::Private,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Flow(datamodel::Flow {
                        ident: "flow_var".to_string(),
                        equation: datamodel::Equation::Scalar("10".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        non_negative: true,
                        can_be_module_input: false,
                        visibility: datamodel::Visibility::Private,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "aux_var".to_string(),
                        equation: datamodel::Equation::Scalar("5".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        can_be_module_input: false,
                        visibility: datamodel::Visibility::Private,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Module(datamodel::Module {
                        ident: "mod_var".to_string(),
                        model_name: "submodel".to_string(),
                        documentation: String::new(),
                        units: None,
                        references: vec![datamodel::ModuleReference {
                            src: "x".to_string(),
                            dst: "y".to_string(),
                        }],
                        can_be_module_input: false,
                        visibility: datamodel::Visibility::Private,
                        ai_state: None,
                        uid: None,
                    }),
                ],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
            }],
            source: None,
            ai_information: None,
        };

        let result = sync_from_datamodel(&db, &project);
        let main = &result.models["main"];

        let stock = &main.variables["stock_var"];
        assert_eq!(stock.source.kind(&db), SourceVariableKind::Stock);
        assert_eq!(stock.source.inflows(&db), &vec!["flow_in".to_string()]);
        assert_eq!(stock.source.outflows(&db), &vec!["flow_out".to_string()]);
        assert!(stock.source.non_negative(&db));

        let flow = &main.variables["flow_var"];
        assert_eq!(flow.source.kind(&db), SourceVariableKind::Flow);
        assert!(flow.source.non_negative(&db));

        let aux = &main.variables["aux_var"];
        assert_eq!(aux.source.kind(&db), SourceVariableKind::Aux);

        let module = &main.variables["mod_var"];
        assert_eq!(module.source.kind(&db), SourceVariableKind::Module);
        assert_eq!(module.source.module_refs(&db).len(), 1);
        assert_eq!(module.source.module_refs(&db)[0].src, "x");
        assert_eq!(module.source.module_refs(&db)[0].dst, "y");
    }

    #[test]
    fn test_sync_variable_with_gf() {
        let db = SimlinDb::default();
        let project = datamodel::Project {
            name: "gf_test".to_string(),
            sim_specs: datamodel::SimSpecs::default(),
            dimensions: vec![],
            units: vec![],
            models: vec![datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![datamodel::Variable::Aux(datamodel::Aux {
                    ident: "lookup_var".to_string(),
                    equation: datamodel::Equation::Scalar("lookup_var(time)".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: Some(datamodel::GraphicalFunction {
                        kind: datamodel::GraphicalFunctionKind::Continuous,
                        x_points: Some(vec![0.0, 1.0, 2.0]),
                        y_points: vec![0.0, 5.0, 10.0],
                        x_scale: datamodel::GraphicalFunctionScale { min: 0.0, max: 2.0 },
                        y_scale: datamodel::GraphicalFunctionScale {
                            min: 0.0,
                            max: 10.0,
                        },
                    }),
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                })],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
            }],
            source: None,
            ai_information: None,
        };

        let result = sync_from_datamodel(&db, &project);
        let var = &result.models["main"].variables["lookup_var"];
        let gf = var.source.gf(&db);
        assert!(gf.is_some());
        let gf = gf.as_ref().unwrap();
        assert_eq!(gf.kind, SourceGraphicalFunctionKind::Continuous);
        assert_eq!(gf.x_points, Some(vec![0.0, 1.0, 2.0]));
        assert_eq!(gf.y_points, vec![0.0, 5.0, 10.0]);
        assert_eq!(gf.x_scale.min, 0.0);
        assert_eq!(gf.x_scale.max, 2.0);
    }

    #[test]
    fn test_sync_dimensions() {
        let db = SimlinDb::default();
        let project = datamodel::Project {
            name: "dim_test".to_string(),
            sim_specs: datamodel::SimSpecs::default(),
            dimensions: vec![
                datamodel::Dimension::named(
                    "Region".to_string(),
                    vec!["North".to_string(), "South".to_string()],
                ),
                datamodel::Dimension::indexed("Periods".to_string(), 5),
            ],
            units: vec![],
            models: vec![datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
            }],
            source: None,
            ai_information: None,
        };

        let result = sync_from_datamodel(&db, &project);
        let dims = result.project.dimensions(&db);
        assert_eq!(dims.len(), 2);

        assert_eq!(dims[0].name, "Region");
        assert_eq!(
            dims[0].elements,
            SourceDimensionElements::Named(vec!["North".to_string(), "South".to_string()])
        );

        assert_eq!(dims[1].name, "Periods");
        assert_eq!(dims[1].elements, SourceDimensionElements::Indexed(5));
    }

    #[test]
    fn test_sync_module_refs() {
        let db = SimlinDb::default();
        let project = datamodel::Project {
            name: "mod_test".to_string(),
            sim_specs: datamodel::SimSpecs::default(),
            dimensions: vec![],
            units: vec![],
            models: vec![datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![datamodel::Variable::Module(datamodel::Module {
                    ident: "my_module".to_string(),
                    model_name: "sub".to_string(),
                    documentation: String::new(),
                    units: None,
                    references: vec![
                        datamodel::ModuleReference {
                            src: "input_a".to_string(),
                            dst: "a".to_string(),
                        },
                        datamodel::ModuleReference {
                            src: "input_b".to_string(),
                            dst: "b".to_string(),
                        },
                    ],
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
                    ai_state: None,
                    uid: None,
                })],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
            }],
            source: None,
            ai_information: None,
        };

        let result = sync_from_datamodel(&db, &project);
        let module = &result.models["main"].variables["my_module"];
        assert_eq!(module.source.kind(&db), SourceVariableKind::Module);
        let refs = module.source.module_refs(&db);
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].src, "input_a");
        assert_eq!(refs[0].dst, "a");
        assert_eq!(refs[1].src, "input_b");
        assert_eq!(refs[1].dst, "b");
    }

    #[test]
    fn test_sync_resync_updates() {
        let db = SimlinDb::default();
        let mut project = simple_project();
        let result1 = sync_from_datamodel(&db, &project);

        let pop1 = &result1.models["main"].variables["population"];
        assert_eq!(
            pop1.source.equation(&db),
            &SourceEquation::Scalar("100".to_string())
        );

        // Modify the equation and re-sync
        project.models[0].variables[0].set_scalar_equation("200");
        let result2 = sync_from_datamodel(&db, &project);

        let pop2 = &result2.models["main"].variables["population"];
        assert_eq!(
            pop2.source.equation(&db),
            &SourceEquation::Scalar("200".to_string())
        );

        // Interned IDs for the same canonical name should be the same
        assert_eq!(pop1.id, pop2.id);
    }

    #[test]
    fn test_sync_empty_model_name_canonicalized() {
        let db = SimlinDb::default();
        let id1 = ModelId::new(&db, "".to_string());
        let id2 = ModelId::new(&db, "main".to_string());
        // Empty and "main" are different canonical strings
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_sync_sim_specs_dt_reciprocal() {
        let db = SimlinDb::default();
        let mut project = simple_project();
        project.sim_specs.dt = datamodel::Dt::Reciprocal(4.0);
        project.sim_specs.save_step = Some(datamodel::Dt::Dt(0.5));
        project.sim_specs.sim_method = datamodel::SimMethod::RungeKutta4;

        let result = sync_from_datamodel(&db, &project);
        let specs = result.project.sim_specs(&db);
        assert_eq!(specs.dt, SourceDt::Reciprocal(4.0));
        assert_eq!(specs.save_step, Some(SourceDt::Dt(0.5)));
        assert_eq!(specs.sim_method, SourceSimMethod::RungeKutta4);
    }
}
