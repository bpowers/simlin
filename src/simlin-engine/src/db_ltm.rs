// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! LTM (Loops That Matter) equation parsing and compilation.
//!
//! This module contains the per-equation compilation pipeline for LTM
//! synthetic variables: link scores, loop scores, and relative loop
//! scores. It handles intrinsic PREVIOUS helper rewrites plus other
//! implicit variables produced during parsing, and produces symbolic
//! bytecodes for assembly by `assemble_module`.

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::canonicalize;
use crate::common::{Canonical, Ident};
use crate::datamodel;
use crate::ltm::strip_subscript;

use super::{
    Db, LtmLinkId, LtmSyntheticVar, ModelDepGraphResult, ParsedVariableResult, RefShape,
    SourceModel, SourceProject, SourceVariableKind, VarFragmentResult, build_module_inputs,
    build_stub_variable, build_submodel_metadata, canonical_module_input_set, compute_layout,
    link_score_equation_text, model_implicit_var_info, model_module_ident_context,
    model_module_map, parse_source_variable_with_module_context, project_datamodel_dims,
    project_units_context, variable_dimensions, variable_size,
};

pub(super) fn ltm_module_idents(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> HashSet<Ident<Canonical>> {
    let source_vars = model.variables(db);
    let mut module_idents: HashSet<Ident<Canonical>> = source_vars
        .iter()
        .filter_map(|(name, source_var)| {
            if source_var.kind(db) == SourceVariableKind::Module {
                Some(Ident::new(name))
            } else {
                None
            }
        })
        .collect();

    for (name, meta) in model_implicit_var_info(db, model, project).iter() {
        if meta.is_module {
            module_idents.insert(Ident::new(name));
        }
    }

    module_idents
}

/// Parse an LTM synthetic variable's equation.
///
/// Creates a transient `datamodel::Variable::Aux` carrying `equation`
/// verbatim, runs it through `parse_var` (which invokes `BuiltinVisitor`
/// and `instantiate_implicit_modules`), and returns the parsed variable
/// plus any implicit helper/module variables generated while parsing.
///
/// `equation` already carries its own dimensionality: `Equation::Scalar`
/// for scalar LTM vars, `Equation::ApplyToAll` for A2A vars (so the
/// compiler expands the formula across all dimension elements), and
/// `Equation::Arrayed` for per-element link-score equations -- the
/// `datamodel::Equation` -> `Ast` conversion in `variable.rs` produces
/// the matching `Ast` variant for each.
pub(super) fn parse_ltm_equation(
    var_name: &str,
    equation: &datamodel::Equation,
    dims: &[datamodel::Dimension],
    units_ctx: &crate::units::Context,
    module_idents: Option<&HashSet<Ident<Canonical>>>,
) -> ParsedVariableResult {
    let dm_var = datamodel::Variable::Aux(datamodel::Aux {
        ident: canonicalize(var_name).into_owned(),
        equation: equation.clone(),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    });

    let mut implicit_vars = Vec::new();
    let variable = crate::variable::parse_var_with_module_context(
        dims,
        &dm_var,
        &mut implicit_vars,
        units_ctx,
        |mi| Ok(Some(mi.clone())),
        module_idents,
    );

    ParsedVariableResult {
        variable,
        implicit_vars,
    }
}

pub(super) fn parse_ltm_equation_for_model_with_ids(
    db: &dyn Db,
    var_name: &str,
    equation: &datamodel::Equation,
    project: SourceProject,
    module_idents: &HashSet<Ident<Canonical>>,
) -> ParsedVariableResult {
    let dims = project_datamodel_dims(db, project);
    let units_ctx = project_units_context(db, project);
    parse_ltm_equation(var_name, equation, dims, units_ctx, Some(module_idents))
}

pub(super) fn parse_ltm_var_with_ids(
    db: &dyn Db,
    ltm_var: &super::LtmSyntheticVar,
    project: SourceProject,
    module_idents: &HashSet<Ident<Canonical>>,
) -> ParsedVariableResult {
    parse_ltm_equation_for_model_with_ids(
        db,
        &ltm_var.name,
        &ltm_var.equation,
        project,
        module_idents,
    )
}

/// Parse *and lower* an LTM equation to a `Variable<ModuleInput, Expr2>`
/// (the type `classify_reducer` and the rest of the type-checked AST machinery
/// expect). Mirrors the parse-then-lower boilerplate `compile_ltm_equation_fragment`
/// does; scoped with an empty model set and the project's datamodel dimensions.
/// Returns `None` if the equation fails to parse.
pub(super) fn reconstruct_ltm_var_lowered(
    db: &dyn Db,
    var_name: &str,
    equation: &datamodel::Equation,
    model: SourceModel,
    project: SourceProject,
) -> Option<crate::variable::Variable> {
    let dims = project_datamodel_dims(db, project);
    let dim_context = crate::dimensions::DimensionsContext::from(dims.as_slice());
    let module_idents = ltm_module_idents(db, model, project);
    let parsed =
        parse_ltm_equation_for_model_with_ids(db, var_name, equation, project, &module_idents);
    if parsed
        .variable
        .equation_errors()
        .is_some_and(|e| !e.is_empty())
    {
        return None;
    }
    let models = HashMap::new();
    let scope = crate::model::ScopeStage0 {
        models: &models,
        dimensions: &dim_context,
        model_name: "",
    };
    Some(crate::model::lower_variable(&scope, &parsed.variable))
}

/// Metadata about implicit variables generated by LTM equation parsing.
///
/// LTM equations may synthesize helper auxes for intrinsic PREVIOUS/INIT
/// routing and may also expand stdlib module calls such as SMOOTH/DELAY.
/// This structure collects those implicit variables across all LTM
/// equations in a model so that `compute_layout` can allocate slots and
/// `assemble_module` can compile them.
#[derive(Clone, Debug, PartialEq, Eq, salsa::Update)]
pub struct LtmImplicitVarMeta {
    /// Canonical name of the LTM variable that created this implicit var
    pub ltm_parent_name: String,
    /// Index into the parent's implicit_vars list
    pub index_in_parent: usize,
    /// Whether this implicit var is a stock
    pub is_stock: bool,
    /// Whether this implicit var is a module
    pub is_module: bool,
    /// Sub-model name if is_module is true
    pub model_name: Option<String>,
    /// Size in slots (for scalar vars: 1; for modules: sub-model n_slots)
    pub size: usize,
}

/// Cached implicit variable info for all LTM synthetic variables.
///
/// Parses each LTM equation to discover implicit helper/module variables,
/// caching the results. Both `compute_layout` and `assemble_module` read
/// this to allocate slots and compile fragments for those implicit vars
/// within LTM equations.
#[salsa::tracked(returns(ref))]
pub fn model_ltm_implicit_var_info(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> HashMap<String, LtmImplicitVarMeta> {
    if !project.ltm_enabled(db) {
        return HashMap::new();
    }

    let ltm_vars = model_ltm_variables(db, model, project);

    let dims = project_datamodel_dims(db, project);
    let units_ctx = project_units_context(db, project);
    let module_idents = ltm_module_idents(db, model, project);

    let mut result = HashMap::new();

    for ltm_var in &ltm_vars.vars {
        let parsed = parse_ltm_equation(
            &ltm_var.name,
            &ltm_var.equation,
            dims,
            units_ctx,
            Some(&module_idents),
        );

        let project_models = project.models(db);

        for (idx, implicit_dm_var) in parsed.implicit_vars.iter().enumerate() {
            let im_name = canonicalize(implicit_dm_var.get_ident()).into_owned();
            let is_module = matches!(implicit_dm_var, datamodel::Variable::Module(_));
            let is_stock = matches!(implicit_dm_var, datamodel::Variable::Stock(_));
            let model_name = if let datamodel::Variable::Module(m) = implicit_dm_var {
                Some(m.model_name.clone())
            } else {
                None
            };
            let size = if is_module {
                model_name
                    .as_deref()
                    .and_then(|mn| {
                        let sub_canonical = canonicalize(mn);
                        project_models
                            .get(sub_canonical.as_ref())
                            .map(|sm| compute_layout(db, *sm, project, false).n_slots)
                    })
                    .unwrap_or(1)
            } else {
                1
            };

            result.insert(
                im_name,
                LtmImplicitVarMeta {
                    ltm_parent_name: ltm_var.name.clone(),
                    index_in_parent: idx,
                    is_stock,
                    is_module,
                    model_name,
                    size,
                },
            );
        }
    }

    result
}

/// Compile a single LTM synthetic variable's equation to symbolic
/// bytecodes.
///
/// This is the per-link compilation granularity that enables incremental
/// recomputation: when a variable's equation changes, salsa only
/// recompiles fragments for affected links. Equation edits that don't
/// change the dependency set return their cached fragment (AC1.2).
///
/// LTM equations are pure scalar aux equations that may reference:
/// - Model variables (stocks, flows, auxes) from the parent model
/// - Other LTM variables (loop scores referencing link scores)
/// - Implicit helper/module variables created during parsing
/// - Implicit time/dt/initial_time/final_time variables
///
/// Parsed LTM equations may synthesize helper auxes for PREVIOUS/INIT
/// and may also expand stdlib module calls, so those implicit vars need
/// to be handled the same way as in `compile_var_fragment`.
#[salsa::tracked(returns(ref))]
pub fn compile_ltm_var_fragment(
    db: &dyn Db,
    link_id: LtmLinkId<'_>,
    model: SourceModel,
    project: SourceProject,
) -> Option<VarFragmentResult> {
    let lsv = link_score_equation_text(db, link_id, model, project).as_ref()?;

    compile_ltm_equation_fragment(db, &lsv.name, &lsv.equation, model, project)
}

/// Compute the per-shape link score equation text for a single causal link.
///
/// Sibling of [`super::link_score_equation_text`]. Where the legacy
/// function keys only on `(from, to)` and emits one variable per causal
/// link, this shape-aware variant emits one variable per
/// `(from, to, shape)` tuple. `model_ltm_variables` calls this once per
/// unique shape in the target's AST so per-shape link scores can be
/// ceteris-paribus scored against their actual reference site.
///
/// Module-involved links delegate to the same module formulas as the
/// legacy function (composite reference / black-box delta-ratio). Their
/// equations are independent of `shape`, but the variable name still
/// carries the suffix so the emission loop can keep one entry per
/// (from, to, shape) tuple in the `Vec<LtmSyntheticVar>`.
///
/// `lsv.dimensions` is left empty here; the caller (the link emission
/// loop) sets dimensions per the link-score-dimensions policy after
/// receiving the value.
///
/// Salsa-tracked so a per-shape link score is recomputed only when the
/// involved variables (and their shape-classifying dimensions) change.
/// Lives in `db_ltm.rs` rather than `db.rs` so the latter stays under
/// the project's per-file line cap.
#[salsa::tracked(returns(ref))]
pub fn link_score_equation_text_shaped<'db>(
    db: &'db dyn Db,
    link_id: LtmLinkId<'db>,
    shape: RefShape,
    model: SourceModel,
    project: SourceProject,
) -> Option<crate::db::LtmSyntheticVar> {
    use crate::common::{Canonical, Ident};
    use crate::db::LtmSyntheticVar;
    use crate::db::{
        black_box_delta_ratio_equation, find_model_output_ports_for_module, model_causal_edges,
    };

    let from_name = link_id.link_from(db);
    let to_name = link_id.link_to(db);
    let from_ident = Ident::<Canonical>::new(from_name);
    let to_ident = Ident::<Canonical>::new(to_name);

    let from_var = super::reconstruct_single_variable(db, model, project, from_name);
    let to_var = super::reconstruct_single_variable(db, model, project, to_name)?;

    let var_name = crate::ltm_augment::link_score_var_name(from_name, to_name, &shape);

    let from_is_module = from_var.as_ref().is_some_and(|v| v.is_module());
    let to_is_module = to_var.is_module();

    // Module-involved links: shape doesn't change the equation (modules
    // are scalar nodes in the causal graph; their composite-reference and
    // black-box delta-ratio formulas don't reach into the AST). Reuse the
    // legacy formulas, but key the synthetic variable by the shape-driven
    // name so the emission loop's per-shape map works.
    if from_is_module || to_is_module {
        let is_discovery = project.ltm_discovery_mode(db);
        let equation = if !from_is_module && to_is_module {
            if let crate::variable::Variable::Module { inputs, .. } = &to_var {
                if let Some(input) = inputs.iter().find(|i| i.src == from_ident) {
                    if is_discovery {
                        let edges = model_causal_edges(db, model, project);
                        let output_ports = find_model_output_ports_for_module(edges, to_name);
                        let output_ref = output_ports
                            .first()
                            .map(|port| format!("{}\u{00B7}{}", to_ident.as_str(), port))
                            .unwrap_or_else(|| format!("{}\u{00B7}output", to_ident.as_str()));
                        black_box_delta_ratio_equation(from_ident.as_str(), &output_ref)
                    } else {
                        format!(
                            "\"{module}\u{00B7}$\u{205A}ltm\u{205A}composite\u{205A}{port}\"",
                            module = to_ident.as_str(),
                            port = input.dst.as_str(),
                        )
                    }
                } else {
                    black_box_delta_ratio_equation(from_ident.as_str(), to_ident.as_str())
                }
            } else {
                black_box_delta_ratio_equation(from_ident.as_str(), to_ident.as_str())
            }
        } else if from_is_module && !to_is_module {
            let module_output_ref: Option<String> = to_var
                .ast()
                .map(|ast| crate::variable::identifier_set(ast, &[], None))
                .and_then(|deps| {
                    let prefix = format!("{}\u{00B7}", from_ident.as_str());
                    deps.into_iter()
                        .find(|d| d.as_str().starts_with(&prefix))
                        .map(|d| d.to_string())
                });
            if let Some(output_ref) = module_output_ref {
                black_box_delta_ratio_equation(&output_ref, to_ident.as_str())
            } else {
                black_box_delta_ratio_equation(from_ident.as_str(), to_ident.as_str())
            }
        } else {
            black_box_delta_ratio_equation(from_ident.as_str(), to_ident.as_str())
        };

        return Some(LtmSyntheticVar {
            name: var_name,
            equation: datamodel::Equation::Scalar(equation),
            dimensions: vec![],
            compile_directly: false,
        });
    }

    // Standard ceteris-paribus formula for non-module links.
    //
    // Build the source's per-dimension element lists so the per-shape
    // partial-equation builder can validate literal-index names like
    // `[NYC]` against the source's actual dimensions. For scalar sources
    // this is empty, which is the right input for Bare-shape calls (no
    // subscripts to classify).
    let source_dim_elements: Vec<Vec<String>> =
        if let Some(from_sv) = model.variables(db).get(from_name) {
            variable_dimensions(db, *from_sv, project)
                .iter()
                .map(crate::ltm_augment::dimension_element_names)
                .collect()
        } else {
            // Implicit variables (SMOOTH/DELAY expansions) aren't in
            // source_vars and are scalar by construction.
            Vec::new()
        };

    let mut all_vars = HashMap::new();
    if let Some(ref fv) = from_var {
        all_vars.insert(from_ident.clone(), fv.clone());
    }
    all_vars.insert(to_ident.clone(), to_var.clone());
    // The project's `DimensionsContext` is threaded into the GH #511
    // iterated-dimension recognition for the mapped-dimension case
    // (`x[State]` over a source declared with `Region`, `State` maps to
    // `Region`); the datamodel dim list is salsa-tracked, so this fn is
    // recomputed when a dimension's mappings change.
    let dm_dims = project_datamodel_dims(db, project);
    let dim_ctx = crate::dimensions::DimensionsContext::from(dm_dims.as_slice());
    // The generator returns the equation already tagged with the target's
    // dimensionality (`Scalar`, `ApplyToAll`, or `Arrayed`). `dimensions`
    // and `compile_directly` are left at defaults here; the emission loop
    // in `model_ltm_variables` (`emit_per_shape_link_scores`) overwrites
    // `dimensions`, the equation's dimension names, and `compile_directly`
    // (set when `shape` is not `Bare`) with the per-shape policy result.
    let equation = crate::ltm_augment::generate_link_score_equation_for_link(
        &from_ident,
        &to_ident,
        &shape,
        &source_dim_elements,
        &to_var,
        &all_vars,
        Some(&dim_ctx),
    );

    Some(LtmSyntheticVar {
        name: var_name,
        equation,
        dimensions: vec![],
        compile_directly: false,
    })
}

/// The dimension names an LTM `Equation` carries (datamodel casing),
/// or `&[]` for a scalar one. These are the names whose product gives
/// the variable's layout slot count, and the same names `compute_layout`
/// reads from `LtmSyntheticVar::dimensions` -- the two are kept in sync
/// at every `LtmSyntheticVar` construction site.
fn ltm_equation_dimensions(equation: &datamodel::Equation) -> &[String] {
    match equation {
        datamodel::Equation::Scalar(_) => &[],
        datamodel::Equation::ApplyToAll(dims, _) | datamodel::Equation::Arrayed(dims, _, _, _) => {
            dims
        }
    }
}

/// Build the `Equation` variant an `LtmSyntheticVar` should carry given
/// its synthetic equation *text* and dimension list: empty `dimensions`
/// ⇒ `Equation::Scalar`, non-empty ⇒ `Equation::ApplyToAll` over exactly
/// those names. (`Equation::Arrayed` link-score equations are built
/// directly by the augmentation layer, not via this helper.)
fn ltm_synthetic_equation(text: String, dimensions: &[String]) -> datamodel::Equation {
    if dimensions.is_empty() {
        datamodel::Equation::Scalar(text)
    } else {
        datamodel::Equation::ApplyToAll(dimensions.to_vec(), text)
    }
}

/// Reduce an LTM equation to a scalar one, keeping the equation text.
/// Used by the legacy `(from, to)`-keyed link-score path
/// (`link_score_equation_text`), which always emits a scalar variable
/// regardless of the target's dimensionality, and by
/// [`retarget_ltm_equation_dims`] to collapse a degenerate zero-dimension
/// `Arrayed` (the empty-`dims` case) -- a per-element link score with no
/// dimension to index is meaningless, so it falls back to scalar. For an
/// `Equation::Arrayed`, the first per-element slot's text is used (the
/// legacy path predates per-element link scores and is only ever compiled
/// for scalar targets in practice).
pub(super) fn scalarize_ltm_equation(equation: datamodel::Equation) -> datamodel::Equation {
    match equation {
        datamodel::Equation::Scalar(_) => equation,
        datamodel::Equation::ApplyToAll(_, text) => datamodel::Equation::Scalar(text),
        datamodel::Equation::Arrayed(_, elements, default, _) => {
            let text = elements
                .into_iter()
                .next()
                .map(|(_, eqn, _, _)| eqn)
                .or(default)
                .unwrap_or_else(|| "0".to_string());
            datamodel::Equation::Scalar(text)
        }
    }
}

/// Re-tag a link-score `Equation` so its dimension names match `dims`
/// (the link-score-dimensions policy result the emission loop assigned to
/// `LtmSyntheticVar::dimensions`). Empty `dims` collapses the equation to
/// `Scalar` (via [`scalarize_ltm_equation`] for an `Arrayed` input, since
/// a zero-dimension `Arrayed` is degenerate); non-empty `dims` widens a
/// scalar to `ApplyToAll` or re-targets the dimension names of an existing
/// `ApplyToAll`/`Arrayed`, preserving the equation text / per-element
/// formulas verbatim.
fn retarget_ltm_equation_dims(
    equation: datamodel::Equation,
    dims: &[String],
) -> datamodel::Equation {
    use datamodel::Equation::{ApplyToAll, Arrayed, Scalar};
    match equation {
        Scalar(text) | ApplyToAll(_, text) => {
            if dims.is_empty() {
                Scalar(text)
            } else {
                ApplyToAll(dims.to_vec(), text)
            }
        }
        Arrayed(orig_dims, elements, default, apply_default) => {
            if dims.is_empty() {
                // The link-score-dimensions policy assigned no dimensions:
                // the (rare) arrayed-target edge whose source dimensions
                // are incompatible with the target's (so the per-shape A2A
                // path was skipped and the cross-dimensional path declined
                // it for having a non-scalar target). A zero-dimension
                // `Arrayed` is degenerate -- its per-element partials are
                // meaningless without a target dimension to index -- so
                // collapse to a scalar link score, matching the contract
                // in this function's doc comment and the pre-existing
                // behavior where such edges produced a scalar link score.
                scalarize_ltm_equation(Arrayed(orig_dims, elements, default, apply_default))
            } else {
                Arrayed(dims.to_vec(), elements, default, apply_default)
            }
        }
    }
}

/// Compile an arbitrary LTM `Equation` to symbolic bytecodes.
///
/// Shared implementation used by `compile_ltm_var_fragment` (link scores)
/// and the loop/relative score compilation in `assemble_module`. Builds
/// a mini-context that includes both model variables and implicit vars
/// synthesized while parsing the LTM equation.
///
/// The variant of `equation` determines the variable's slot count: a
/// `Scalar` equation gets 1 slot; an `ApplyToAll`/`Arrayed` equation
/// gets `product(dim_lengths)` slots and is compiled with the A2A /
/// per-element expansion the compiler applies to those variants.
pub(super) fn compile_ltm_equation_fragment(
    db: &dyn Db,
    var_name: &str,
    equation: &datamodel::Equation,
    model: SourceModel,
    project: SourceProject,
) -> Option<VarFragmentResult> {
    use crate::compiler::symbolic::{
        CompiledVarFragment, PerVarBytecodes, ReverseOffsetMap, VariableLayout,
    };

    let dims = project_datamodel_dims(db, project);
    let dim_context = crate::dimensions::DimensionsContext::from(dims.as_slice());
    let converted_dims: Vec<crate::dimensions::Dimension> = dims
        .iter()
        .map(crate::dimensions::Dimension::from)
        .collect();

    let units_ctx = project_units_context(db, project);
    let module_idents = ltm_module_idents(db, model, project);

    let var_dimensions = ltm_equation_dimensions(equation);

    let parsed = parse_ltm_equation(var_name, equation, dims, units_ctx, Some(&module_idents));

    // Check for parse errors
    if parsed
        .variable
        .equation_errors()
        .is_some_and(|e| !e.is_empty())
    {
        return None;
    }

    // Lower the variable. Scalar LTM vars produce a plain Var;
    // A2A LTM vars produce a Var with dimension views that the
    // compiler's expand_a2a_with_hoisting handles automatically.
    let models = HashMap::new();
    let scope = crate::model::ScopeStage0 {
        models: &models,
        dimensions: &dim_context,
        model_name: "",
    };
    let lowered = crate::model::lower_variable(&scope, &parsed.variable);

    let model_name_ident = Ident::new(model.name(db));
    let var_name_canonical = canonicalize(var_name).into_owned();
    let var_ident_canonical: Ident<Canonical> = Ident::new(&var_name_canonical);

    // Arena for sub-model stub variables allocated by build_submodel_metadata
    let arena = bumpalo::Bump::new();

    let mut mini_metadata: HashMap<Ident<Canonical>, crate::compiler::VariableMetadata<'_>> =
        HashMap::new();

    // Mini-layout starts after the 4 implicit time vars (time, dt, initial_time, final_time)
    let mut mini_offset = crate::vm::IMPLICIT_VAR_COUNT;

    // Add implicit time/dt/initial_time/final_time variables
    {
        use std::sync::LazyLock;
        static IMPLICIT_TIME: LazyLock<crate::variable::Variable> =
            LazyLock::new(|| crate::variable::Variable::Var {
                ident: Ident::new("time"),
                ast: None,
                init_ast: None,
                eqn: None,
                units: None,
                tables: vec![],
                non_negative: false,
                is_flow: false,
                is_table_only: false,
                errors: vec![],
                unit_errors: vec![],
            });
        static IMPLICIT_DT: LazyLock<crate::variable::Variable> =
            LazyLock::new(|| crate::variable::Variable::Var {
                ident: Ident::new("dt"),
                ast: None,
                init_ast: None,
                eqn: None,
                units: None,
                tables: vec![],
                non_negative: false,
                is_flow: false,
                is_table_only: false,
                errors: vec![],
                unit_errors: vec![],
            });
        static IMPLICIT_INITIAL_TIME: LazyLock<crate::variable::Variable> =
            LazyLock::new(|| crate::variable::Variable::Var {
                ident: Ident::new("initial_time"),
                ast: None,
                init_ast: None,
                eqn: None,
                units: None,
                tables: vec![],
                non_negative: false,
                is_flow: false,
                is_table_only: false,
                errors: vec![],
                unit_errors: vec![],
            });
        static IMPLICIT_FINAL_TIME: LazyLock<crate::variable::Variable> =
            LazyLock::new(|| crate::variable::Variable::Var {
                ident: Ident::new("final_time"),
                ast: None,
                init_ast: None,
                eqn: None,
                units: None,
                tables: vec![],
                non_negative: false,
                is_flow: false,
                is_table_only: false,
                errors: vec![],
                unit_errors: vec![],
            });
        mini_metadata.insert(
            Ident::new("time"),
            crate::compiler::VariableMetadata {
                offset: 0,
                size: 1,
                var: &IMPLICIT_TIME,
            },
        );
        mini_metadata.insert(
            Ident::new("dt"),
            crate::compiler::VariableMetadata {
                offset: 1,
                size: 1,
                var: &IMPLICIT_DT,
            },
        );
        mini_metadata.insert(
            Ident::new("initial_time"),
            crate::compiler::VariableMetadata {
                offset: 2,
                size: 1,
                var: &IMPLICIT_INITIAL_TIME,
            },
        );
        mini_metadata.insert(
            Ident::new("final_time"),
            crate::compiler::VariableMetadata {
                offset: 3,
                size: 1,
                var: &IMPLICIT_FINAL_TIME,
            },
        );
    }

    // Compute the LTM variable's size from the equation's dimension names.
    // Scalar vars get size 1; A2A/Arrayed vars get product(dim_lengths).
    let var_size: usize = if var_dimensions.is_empty() {
        1
    } else {
        var_dimensions
            .iter()
            .map(|dim_name| {
                let canonical = crate::common::CanonicalDimensionName::from_raw(dim_name);
                dim_context.get(&canonical).map(|d| d.len()).unwrap_or(1)
            })
            .product()
    };

    // Add self (the LTM var itself)
    mini_metadata.insert(
        var_ident_canonical.clone(),
        crate::compiler::VariableMetadata {
            offset: mini_offset,
            size: var_size,
            var: &lowered,
        },
    );
    mini_offset += var_size;

    // Collect dependency variable names from the lowered AST
    let dep_idents = if let Some(ast) = lowered.ast() {
        crate::variable::identifier_set(ast, &[], None)
    } else {
        HashSet::new()
    };

    let source_vars = model.variables(db);
    let project_models = project.models(db);
    let implicit_info = model_implicit_var_info(db, model, project);
    let ltm_implicit_info = model_ltm_implicit_var_info(db, model, project);

    let mut dep_variables: Vec<(Ident<Canonical>, crate::variable::Variable, usize)> = Vec::new();
    let mut implicit_module_vars: Vec<(Ident<Canonical>, crate::variable::Variable, usize)> =
        Vec::new();
    let mut implicit_module_refs: HashMap<Ident<Canonical>, crate::vm::ModuleKey> = HashMap::new();
    let mut implicit_submodels: Vec<(String, SourceModel)> = Vec::new();

    // Process dependencies from the parsed AST
    for dep_ident_str in &dep_idents {
        let dep_str = dep_ident_str.as_str();
        let effective = dep_str.strip_prefix('\u{00B7}').unwrap_or(dep_str);

        if effective == var_name_canonical
            || matches!(effective, "time" | "dt" | "initial_time" | "final_time")
        {
            continue;
        }

        // Handle module output references (contains middle dot)
        if let Some(dot_pos) = effective.find('\u{00B7}') {
            let module_var_name = &effective[..dot_pos];
            let module_ident: Ident<Canonical> = Ident::new(module_var_name);

            if mini_metadata.contains_key(&module_ident)
                || implicit_module_vars
                    .iter()
                    .any(|(id, _, _)| id == &module_ident)
            {
                continue;
            }

            // Check if this is an explicit model variable (module type)
            if let Some(mod_source_var) = source_vars.get(module_var_name) {
                if mod_source_var.kind(db) == SourceVariableKind::Module {
                    let mod_model_name = mod_source_var.model_name(db);
                    let sub_canonical = canonicalize(mod_model_name);
                    let sub_size = project_models
                        .get(sub_canonical.as_ref())
                        .map(|sm| compute_layout(db, *sm, project, false).n_slots)
                        .unwrap_or(1);

                    let mod_input_prefix = format!("{module_var_name}\u{00B7}");
                    let module_inputs = build_module_inputs(
                        model.name(db),
                        &mod_input_prefix,
                        mod_source_var
                            .module_refs(db)
                            .iter()
                            .map(|mr| (canonicalize(&mr.src), canonicalize(&mr.dst))),
                    );

                    let mod_var = crate::variable::Variable::Module {
                        ident: module_ident.clone(),
                        model_name: Ident::new(mod_model_name),
                        units: None,
                        inputs: module_inputs.clone(),
                        errors: vec![],
                        unit_errors: vec![],
                    };
                    dep_variables.push((module_ident.clone(), mod_var, sub_size));

                    let input_set: BTreeSet<Ident<Canonical>> =
                        module_inputs.iter().map(|mi| mi.dst.clone()).collect();
                    implicit_module_refs
                        .insert(module_ident, (Ident::new(mod_model_name), input_set));

                    if let Some(sub_model) = project_models.get(sub_canonical.as_ref()) {
                        implicit_submodels.push((mod_model_name.to_string(), *sub_model));
                    }
                }
                continue;
            }

            // Check if this is an implicit var from the LTM equation's own
            // parse-time helper/module synthesis.
            let mut found_in_parsed = false;
            for implicit_dm_var in &parsed.implicit_vars {
                if let datamodel::Variable::Module(dm_module) = implicit_dm_var
                    && canonicalize(&dm_module.ident) == module_var_name
                {
                    let sub_canonical = canonicalize(&dm_module.model_name);
                    let sub_size = project_models
                        .get(sub_canonical.as_ref())
                        .map(|sm| compute_layout(db, *sm, project, false).n_slots)
                        .unwrap_or(1);

                    let input_prefix = format!("{module_var_name}\u{00B7}");
                    let module_inputs = build_module_inputs(
                        model.name(db),
                        &input_prefix,
                        dm_module
                            .references
                            .iter()
                            .map(|mr| (canonicalize(&mr.src), canonicalize(&mr.dst))),
                    );

                    let im_var = crate::variable::Variable::Module {
                        ident: module_ident.clone(),
                        model_name: Ident::new(&dm_module.model_name),
                        units: None,
                        inputs: module_inputs.clone(),
                        errors: vec![],
                        unit_errors: vec![],
                    };
                    implicit_module_vars.push((module_ident.clone(), im_var, sub_size));

                    let input_set: BTreeSet<Ident<Canonical>> =
                        module_inputs.iter().map(|mi| mi.dst.clone()).collect();
                    implicit_module_refs.insert(
                        module_ident.clone(),
                        (Ident::new(&dm_module.model_name), input_set),
                    );

                    if let Some(sub_model) = project_models.get(sub_canonical.as_ref()) {
                        implicit_submodels.push((dm_module.model_name.clone(), *sub_model));
                    }

                    found_in_parsed = true;
                    break;
                }
            }

            if !found_in_parsed {
                // Check the model's own implicit vars (SMOOTH, DELAY, etc.)
                if let Some(im_meta) = implicit_info.get(module_var_name)
                    && im_meta.is_module
                    && let Some(im_model_name) = im_meta.model_name.as_deref()
                {
                    let sub_canonical = canonicalize(im_model_name);
                    let sub_size = project_models
                        .get(sub_canonical.as_ref())
                        .map(|sm| compute_layout(db, *sm, project, false).n_slots)
                        .unwrap_or(1);

                    let module_ctx = model_module_ident_context(db, model, vec![]);
                    let parent_parsed = parse_source_variable_with_module_context(
                        db,
                        im_meta.parent_source_var,
                        project,
                        module_ctx,
                    );
                    let input_prefix = format!("{module_var_name}\u{00B7}");
                    let module_inputs = parent_parsed
                        .implicit_vars
                        .iter()
                        .find_map(|iv| match iv {
                            datamodel::Variable::Module(dm_module)
                                if canonicalize(dm_module.ident.as_str()) == module_var_name =>
                            {
                                Some(build_module_inputs(
                                    model.name(db),
                                    &input_prefix,
                                    dm_module
                                        .references
                                        .iter()
                                        .map(|mr| (canonicalize(&mr.src), canonicalize(&mr.dst))),
                                ))
                            }
                            _ => None,
                        })
                        .unwrap_or_default();

                    let mod_var = crate::variable::Variable::Module {
                        ident: module_ident.clone(),
                        model_name: Ident::new(im_model_name),
                        units: None,
                        inputs: module_inputs.clone(),
                        errors: vec![],
                        unit_errors: vec![],
                    };
                    dep_variables.push((module_ident.clone(), mod_var, sub_size));

                    let input_set: BTreeSet<Ident<Canonical>> =
                        module_inputs.iter().map(|mi| mi.dst.clone()).collect();
                    implicit_module_refs
                        .insert(module_ident, (Ident::new(im_model_name), input_set));

                    if let Some(sub_model) = project_models.get(sub_canonical.as_ref()) {
                        implicit_submodels.push((im_model_name.to_string(), *sub_model));
                    }
                }
                // Also check LTM implicit vars from other LTM equations
                else if let Some(ltm_im_meta) = ltm_implicit_info.get(module_var_name)
                    && ltm_im_meta.is_module
                    && let Some(im_model_name) = ltm_im_meta.model_name.as_deref()
                {
                    let sub_canonical = canonicalize(im_model_name);
                    let sub_size = project_models
                        .get(sub_canonical.as_ref())
                        .map(|sm| compute_layout(db, *sm, project, false).n_slots)
                        .unwrap_or(1);

                    // Parse the parent LTM equation to get implicit var references
                    let parent_ltm_vars = model_ltm_variables(db, model, project);
                    let parent_ltm = parent_ltm_vars
                        .vars
                        .iter()
                        .find(|v| v.name == ltm_im_meta.ltm_parent_name);
                    let module_inputs = if let Some(parent_lsv) = parent_ltm {
                        let parent_parsed = parse_ltm_equation(
                            &parent_lsv.name,
                            &parent_lsv.equation,
                            dims,
                            units_ctx,
                            Some(&module_idents),
                        );
                        let input_prefix = format!("{module_var_name}\u{00B7}");
                        parent_parsed
                            .implicit_vars
                            .iter()
                            .find_map(|iv| match iv {
                                datamodel::Variable::Module(dm_module)
                                    if canonicalize(dm_module.ident.as_str())
                                        == module_var_name =>
                                {
                                    Some(build_module_inputs(
                                        model.name(db),
                                        &input_prefix,
                                        dm_module.references.iter().map(|mr| {
                                            (canonicalize(&mr.src), canonicalize(&mr.dst))
                                        }),
                                    ))
                                }
                                _ => None,
                            })
                            .unwrap_or_default()
                    } else {
                        Vec::new()
                    };

                    let mod_var = crate::variable::Variable::Module {
                        ident: module_ident.clone(),
                        model_name: Ident::new(im_model_name),
                        units: None,
                        inputs: module_inputs.clone(),
                        errors: vec![],
                        unit_errors: vec![],
                    };
                    dep_variables.push((module_ident.clone(), mod_var, sub_size));

                    let input_set: BTreeSet<Ident<Canonical>> =
                        module_inputs.iter().map(|mi| mi.dst.clone()).collect();
                    implicit_module_refs
                        .insert(module_ident, (Ident::new(im_model_name), input_set));

                    if let Some(sub_model) = project_models.get(sub_canonical.as_ref()) {
                        implicit_submodels.push((im_model_name.to_string(), *sub_model));
                    }
                }
            }
            continue;
        }

        let dep_ident = Ident::new(effective);
        if mini_metadata.contains_key(&dep_ident) {
            continue;
        }

        // Look up in explicit model variables
        if let Some(dep_source_var) = source_vars.get(effective) {
            let dep_dims = variable_dimensions(db, *dep_source_var, project);
            let dep_size = variable_size(db, *dep_source_var, project);
            let dep_var = build_stub_variable(db, dep_source_var, &dep_ident, dep_dims);
            dep_variables.push((dep_ident, dep_var, dep_size));
        } else if let Some(im_meta) = implicit_info.get(effective) {
            // Dep is an implicit var from the model (SMOOTH/DELAY expansion).
            // Module-type implicits need their full size and Module variant so
            // the compiler can resolve submodel offset lookups. Scalar implicits
            // (helper auxes) use a plain Var stub.
            if im_meta.is_module
                && let Some(ref mn) = im_meta.model_name
            {
                let sub_size = {
                    let sub_canonical = canonicalize(mn);
                    project_models
                        .get(sub_canonical.as_ref())
                        .map(|sm| compute_layout(db, *sm, project, false).n_slots)
                        .unwrap_or(1)
                };
                dep_variables.push((
                    dep_ident.clone(),
                    crate::variable::Variable::Module {
                        ident: dep_ident.clone(),
                        model_name: Ident::new(mn),
                        units: None,
                        inputs: vec![],
                        errors: vec![],
                        unit_errors: vec![],
                    },
                    sub_size,
                ));
                implicit_module_refs.insert(dep_ident, (Ident::new(mn), BTreeSet::new()));
                let sub_canonical = canonicalize(mn);
                if let Some(sub_model) = project_models.get(sub_canonical.as_ref()) {
                    implicit_submodels.push((mn.to_string(), *sub_model));
                }
            } else {
                dep_variables.push((
                    dep_ident.clone(),
                    crate::variable::Variable::Var {
                        ident: dep_ident,
                        ast: None,
                        init_ast: None,
                        eqn: None,
                        units: None,
                        tables: vec![],
                        non_negative: false,
                        is_flow: false,
                        is_table_only: false,
                        errors: vec![],
                        unit_errors: vec![],
                    },
                    im_meta.size,
                ));
            }
        }
        // Dep could also be another LTM var (e.g., loop score refs link
        // scores; composite refs paths).  These cases need dimension-
        // aware stubs: an A2A loop_score that references an A2A
        // link_score must see that dep as A2A so the compiler emits
        // per-element fetches; otherwise references collapse to slot 0
        // and every output slot reads the same numerator (tech-debt #34).
        //
        // model_ltm_variables is salsa-cached, so this lookup is cheap
        // and safe to call from within compile_ltm_equation_fragment --
        // the same pattern is used by the implicit-module branch above.
        else {
            let ltm_dep = model_ltm_variables(db, model, project)
                .vars
                .iter()
                .find(|v| v.name == effective)
                .cloned();
            let (dep_size, dep_ast) = match ltm_dep {
                Some(lsv) if !lsv.dimensions.is_empty() => {
                    let canonical_dims: Vec<crate::dimensions::Dimension> = lsv
                        .dimensions
                        .iter()
                        .filter_map(|name| {
                            let canonical = crate::common::CanonicalDimensionName::from_raw(name);
                            dim_context.get(&canonical).cloned()
                        })
                        .collect();
                    let size: usize = canonical_dims.iter().map(|d| d.len()).product();
                    let ast = if canonical_dims.is_empty() {
                        None
                    } else {
                        Some(crate::ast::Ast::ApplyToAll(
                            canonical_dims,
                            crate::ast::Expr2::Const(
                                "0".to_string(),
                                0.0,
                                crate::ast::Loc::default(),
                            ),
                        ))
                    };
                    (size.max(1), ast)
                }
                _ => (1, None),
            };
            dep_variables.push((
                dep_ident.clone(),
                crate::variable::Variable::Var {
                    ident: dep_ident,
                    ast: dep_ast,
                    init_ast: None,
                    eqn: None,
                    units: None,
                    tables: vec![],
                    non_negative: false,
                    is_flow: false,
                    is_table_only: false,
                    errors: vec![],
                    unit_errors: vec![],
                },
                dep_size,
            ));
        }
    }

    // Add dep metadata
    for (dep_ident, dep_var, dep_size) in &dep_variables {
        if !mini_metadata.contains_key(dep_ident) {
            mini_metadata.insert(
                dep_ident.clone(),
                crate::compiler::VariableMetadata {
                    offset: mini_offset,
                    size: *dep_size,
                    var: dep_var,
                },
            );
            mini_offset += dep_size;
        }
    }

    // Add implicit vars synthesized while parsing the LTM equation
    for (im_ident, im_var, im_size) in &implicit_module_vars {
        if !mini_metadata.contains_key(im_ident) {
            mini_metadata.insert(
                im_ident.clone(),
                crate::compiler::VariableMetadata {
                    offset: mini_offset,
                    size: *im_size,
                    var: im_var,
                },
            );
            mini_offset += im_size;
        }
    }

    // Build the all_metadata map
    let mut all_metadata: HashMap<
        Ident<Canonical>,
        HashMap<Ident<Canonical>, crate::compiler::VariableMetadata<'_>>,
    > = HashMap::new();
    all_metadata.insert(model_name_ident.clone(), mini_metadata);

    // Populate sub-model metadata for implicit module sub-models
    for (_sub_name, sub_model) in &implicit_submodels {
        build_submodel_metadata(&arena, db, *sub_model, project, &mut all_metadata);
    }

    let mini_layout =
        crate::compiler::symbolic::layout_from_metadata(&all_metadata, &model_name_ident)
            .unwrap_or_else(|_| VariableLayout::new(HashMap::new(), 0));
    let rmap = ReverseOffsetMap::from_layout(&mini_layout);

    // LTM vars don't have graphical functions or lookup tables
    let tables: HashMap<Ident<Canonical>, Vec<crate::compiler::Table>> = HashMap::new();

    let inputs = BTreeSet::new();
    let mut module_models = model_module_map(db, model, project).clone();

    // Merge LTM implicit module references from LTM equation parsing into the
    // module_models map so the compiler context can resolve module_var_name ->
    // sub_model_name lookups.
    if !implicit_module_refs.is_empty() {
        let current_model_modules = module_models.entry(model_name_ident.clone()).or_default();
        for (var_ident, (sub_model_name, _input_set)) in &implicit_module_refs {
            current_model_modules.insert(var_ident.clone(), sub_model_name.clone());
        }
    }

    let core = crate::compiler::ContextCore {
        dimensions: &converted_dims,
        dimensions_ctx: &dim_context,
        model_name: &model_name_ident,
        metadata: &all_metadata,
        module_models: &module_models,
        inputs: &inputs,
    };

    let build_var = |is_initial: bool| {
        crate::compiler::Var::new(
            &crate::compiler::Context::new(core, &var_ident_canonical, is_initial),
            &lowered,
        )
    };

    let compile_phase = |exprs: &[crate::compiler::Expr]| -> Option<PerVarBytecodes> {
        if exprs.is_empty() {
            return None;
        }

        let module_inputs_set: HashSet<Ident<Canonical>> = inputs.iter().cloned().collect();
        let module = crate::compiler::Module {
            ident: model_name_ident.clone(),
            inputs: module_inputs_set,
            n_slots: mini_offset,
            n_temps: 0,
            temp_sizes: vec![],
            runlist_initials: vec![],
            runlist_initials_by_var: vec![],
            runlist_flows: exprs.to_vec(),
            runlist_stocks: vec![],
            offsets: all_metadata
                .iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        v.iter()
                            .map(|(vk, vm)| (vk.clone(), (vm.offset, vm.size)))
                            .collect(),
                    )
                })
                .collect(),
            runlist_order: vec![var_ident_canonical.clone()],
            tables: tables.clone(),
            dimensions: converted_dims.clone(),
            dimensions_ctx: dim_context.clone(),
            module_refs: implicit_module_refs.clone(),
        };

        let mut temp_sizes_map: HashMap<u32, usize> = HashMap::new();
        for expr in exprs {
            crate::compiler::extract_temp_sizes_pub(expr, &mut temp_sizes_map);
        }
        let n_temps = temp_sizes_map.len();
        let mut temp_sizes: Vec<usize> = vec![0; n_temps];
        for (id, size) in &temp_sizes_map {
            if (*id as usize) < temp_sizes.len() {
                temp_sizes[*id as usize] = *size;
            }
        }

        let module = crate::compiler::Module {
            n_temps,
            temp_sizes: temp_sizes.clone(),
            ..module
        };

        match module.compile() {
            Ok(compiled) => {
                let sym_bc =
                    crate::compiler::symbolic::symbolize_bytecode(&compiled.compiled_flows, &rmap)
                        .ok()?;

                let ctx = &*compiled.context;
                let sym_views: Vec<_> = ctx
                    .static_views
                    .iter()
                    .map(|sv| crate::compiler::symbolic::symbolize_static_view(sv, &rmap))
                    .collect::<Result<Vec<_>, _>>()
                    .ok()?;
                let sym_mods: Vec<_> = ctx
                    .modules
                    .iter()
                    .map(|md| crate::compiler::symbolic::symbolize_module_decl(md, &rmap))
                    .collect::<Result<Vec<_>, _>>()
                    .ok()?;

                let temp_sizes_vec: Vec<(u32, usize)> =
                    temp_sizes_map.iter().map(|(&k, &v)| (k, v)).collect();

                let dim_lists: Vec<Vec<u16>> = ctx
                    .dim_lists
                    .iter()
                    .map(|(n, arr)| arr[..(*n as usize)].to_vec())
                    .collect();

                Some(PerVarBytecodes {
                    symbolic: sym_bc,
                    graphical_functions: ctx.graphical_functions.clone(),
                    module_decls: sym_mods,
                    static_views: sym_views,
                    temp_sizes: temp_sizes_vec,
                    dim_lists,
                })
            }
            Err(_) => None,
        }
    };

    // LTM vars are always flow-phase only (scalar auxes, not stocks)
    let flow_bytecodes = match build_var(false) {
        Ok(var_result) => compile_phase(&var_result.ast),
        Err(_) => None,
    };

    Some(VarFragmentResult {
        fragment: CompiledVarFragment {
            ident: var_name_canonical,
            initial_bytecodes: None,
            flow_bytecodes,
            stock_bytecodes: None,
        },
    })
}

/// Select-and-compile a single LTM synthetic variable's flow-phase
/// fragment, exactly as `assemble_module`'s LTM pass does.
///
/// Most synthetic equations are compiled verbatim from `ltm_var.equation`
/// (`compile_direct`); the one exception is the standard scalar Bare
/// `from→to` link score, which routes through the salsa-cached
/// `(from, to)`-keyed `compile_ltm_var_fragment` so an equation edit that
/// does not change the dependency set reuses the cached fragment.
///
/// Returns `None` -- or a `VarFragmentResult` whose `flow_bytecodes` is
/// `None` -- when the synthetic equation fails to parse or compile.
/// `assemble_module` silently drops such failures (the variable keeps its
/// layout slot but no bytecode writes it, so it reads a constant 0);
/// [`model_ltm_fragment_diagnostics`] calls this to detect those failures
/// and surface them as `Warning`s instead of letting them masquerade as
/// a correct zero score.
pub(super) fn compile_ltm_synthetic_fragment(
    db: &dyn Db,
    ltm_var: &LtmSyntheticVar,
    model: SourceModel,
    project: SourceProject,
) -> Option<VarFragmentResult> {
    // Compile this LTM var's already-prepared equation verbatim.
    // Used for everything except the standard scalar Bare `from→to`
    // link score, which goes through the salsa-cached
    // `compile_ltm_var_fragment` path below: that path re-derives the
    // equation from `link_score_equation_text` (always scalar, Bare;
    // per-shape dimensions, element subscripts and reducer
    // substitutions are applied later in `model_ltm_variables`), so
    // for anything that carries those it would produce the wrong (or
    // a degenerate) fragment.
    let compile_direct =
        || compile_ltm_equation_fragment(db, &ltm_var.name, &ltm_var.equation, model, project);
    const LINK_SCORE_PREFIX: &str = "$\u{205A}ltm\u{205A}link_score\u{205A}";
    if ltm_var.name.starts_with(LINK_SCORE_PREFIX) {
        if ltm_var.dimensions.is_empty() {
            // Scalar link score. Sub-cases:
            // (a) Standard scalar Bare score (from→to): use salsa-cached fragment.
            // (b) Cross-dimensional per-source-element score (from[elem]→to,
            //     try_cross_dimensional_link_scores) or per-target-element
            //     score (from→to[elem], try_scalar_to_arrayed_link_scores):
            //     compile directly. The equation is unique per element and the
            //     (from, to)-keyed salsa path can't round-trip the bracketed
            //     name back to a user variable (it'd drop the fragment and stub
            //     the var to zero).
            // (d) Aggregate-node link score (from = $⁚ltm⁚agg⁚n, or to =
            //     $⁚ltm⁚agg⁚n): compile directly. The (from, to)-keyed salsa
            //     path would `reconstruct_single_variable` the synthetic agg
            //     name, get `None`, and emit a degenerate ceteris-paribus
            //     equation against the *target's* original (reducer-bearing)
            //     equation -- which the agg name appears nowhere in -- so the
            //     numerator collapses to zero. `model_ltm_variables` already
            //     produced the correct reducer-substituted equation in
            //     `ltm_var.equation`; use it verbatim.
            // (e) Non-Bare-shaped scalar score (`Wildcard`/`DynamicIndex`
            //     reference into a scalar target, e.g. `total = arr[idx]`):
            //     `emit_per_shape_link_scores` set `compile_directly` because
            //     the salsa path re-derives with `RefShape::Bare`, wrapping
            //     the subscript in `PREVIOUS()` and zeroing the numerator.
            let suffix = &ltm_var.name[LINK_SCORE_PREFIX.len()..];
            let arrow_pos = suffix.find('\u{2192}');
            let from_to: Option<(&str, &str)> =
                arrow_pos.map(|arrow| (&suffix[..arrow], &suffix[arrow + '\u{2192}'.len_utf8()..]));
            // Any `[` -- on either side of the arrow -- marks an
            // element-pinned equation that ltm_var.equation already
            // carries verbatim; the (from, to)-keyed salsa path can't
            // round-trip the bracketed name back to a user variable.
            let has_element_subscript = suffix.contains('[');
            let touches_synthetic_agg = from_to.is_some_and(|(from_name, to_name)| {
                crate::ltm_agg::is_synthetic_agg_name(from_name)
                    || crate::ltm_agg::is_synthetic_agg_name(to_name)
            });

            if has_element_subscript || touches_synthetic_agg || ltm_var.compile_directly {
                compile_direct()
            } else if let Some((from_name, to_name)) = from_to {
                let link_id = LtmLinkId::new(db, from_name.to_string(), to_name.to_string());
                compile_ltm_var_fragment(db, link_id, model, project)
                    .as_ref()
                    .cloned()
            } else {
                compile_direct()
            }
        } else {
            // A2A link score: the equation is the dimension-tagged
            // ApplyToAll/Arrayed variant, not the scalar one the
            // salsa-cached path would re-derive.
            compile_direct()
        }
    } else {
        // Loop scores and relative loop scores.
        compile_direct()
    }
}

/// Salsa-tracked diagnostic pass that compiles every LTM synthetic
/// variable the way `assemble_module` does and emits a `Warning` for
/// each one whose fragment fails to compile.
///
/// Why this exists: `assemble_module` silently drops a synthetic
/// fragment that fails to compile -- the variable keeps its layout slot
/// but no bytecode ever writes it, so it reads a constant 0. That silent
/// stubbing masks correctness bugs in the LTM augmentation layer (an
/// arrayed flow-to-stock link score that compiled to 0 and produced
/// plausible-but-wrong loop scores went unnoticed precisely because of
/// this). Surfacing the failure makes a degraded LTM analysis *visible*
/// instead of silently wrong.
///
/// Severity is `Warning`, not `Error`: LTM is opt-in, the rest of the
/// model still simulates, and a hard error would break compilation of
/// every `ltm_enabled` model that hits a single bad fragment. This
/// mirrors the auto-flip-to-discovery warning in `model_ltm_variables`.
///
/// `model_all_diagnostics` drives this when `ltm_enabled`, so the
/// warning reaches `collect_all_diagnostics` exactly when the auto-flip
/// warning does. (GH #466 tracks the separate plumbing gap: the
/// diagnostic-collection FFI paths leave `ltm_enabled` false by default,
/// so neither this warning nor the auto-flip warning reaches
/// `simlin_project_get_errors` today.)
///
/// Only the layout-independent compile failure is reported here. A
/// fragment that compiles but whose variable references do not resolve
/// in the model's layout is the documented sub-model dedup case
/// (`assemble_module`'s `fragment_vars_in_layout` drop), where the root
/// model emits an equivalent fragment under qualified names -- that drop
/// is intentionally left silent.
#[salsa::tracked]
pub fn model_ltm_fragment_diagnostics(db: &dyn Db, model: SourceModel, project: SourceProject) {
    use salsa::Accumulator;

    use super::{CompilationDiagnostic, Diagnostic, DiagnosticError, DiagnosticSeverity};

    let ltm_vars = model_ltm_variables(db, model, project);
    for ltm_var in &ltm_vars.vars {
        let fragment = compile_ltm_synthetic_fragment(db, ltm_var, model, project);
        // A fragment is usable only if it compiled *and* produced
        // flow-phase bytecodes. `compile_ltm_equation_fragment` returns
        // `Some(_)` with `flow_bytecodes: None` when the synthetic
        // equation parses but fails to lower or compile.
        let compiled_ok = fragment
            .as_ref()
            .is_some_and(|r| r.fragment.flow_bytecodes.is_some());
        if compiled_ok {
            continue;
        }
        let msg = format!(
            "LTM synthetic variable '{}' failed to compile; it keeps a \
             layout slot but no bytecode, so it evaluates to a constant 0. \
             Any loop or link score derived from it is silently degraded. \
             This usually means the LTM augmentation layer emitted an \
             equation the compiler rejected.",
            ltm_var.name,
        );
        CompilationDiagnostic(Diagnostic {
            model: model.name(db).clone(),
            variable: Some(ltm_var.name.clone()),
            error: DiagnosticError::Assembly(msg),
            severity: DiagnosticSeverity::Warning,
        })
        .accumulate(db);
    }
}

/// Compile a single implicit variable from an
/// LTM equation to symbolic bytecodes.
///
/// This is analogous to `compile_implicit_var_fragment` but for implicit
/// variables generated by LTM equation parsing rather than by
/// SourceVariable parsing. The parent is an LTM equation (not a
/// SourceVariable), so we reconstruct the parse result from the LTM
/// equation text.
#[allow(clippy::too_many_arguments)]
pub(super) fn compile_ltm_implicit_var_fragment(
    db: &dyn Db,
    parsed: &ParsedVariableResult,
    idx: usize,
    meta: &LtmImplicitVarMeta,
    model: SourceModel,
    project: SourceProject,
    _dep_graph: &ModelDepGraphResult,
    module_input_names: &[String],
) -> Option<VarFragmentResult> {
    use crate::compiler::symbolic::{
        CompiledVarFragment, PerVarBytecodes, ReverseOffsetMap, VariableLayout,
    };

    let implicit_dm_var = parsed.implicit_vars.get(idx)?;
    let implicit_name = canonicalize(implicit_dm_var.get_ident()).into_owned();

    let dims = project_datamodel_dims(db, project);
    let dim_context = crate::dimensions::DimensionsContext::from(dims.as_slice());
    let converted_dims: Vec<crate::dimensions::Dimension> = dims
        .iter()
        .map(crate::dimensions::Dimension::from)
        .collect();

    let units_ctx = project_units_context(db, project);

    let mut dummy_implicits = Vec::new();
    let parsed_implicit = crate::variable::parse_var(
        dims,
        implicit_dm_var,
        &mut dummy_implicits,
        units_ctx,
        |mi| Ok(Some(mi.clone())),
    );

    if parsed_implicit
        .equation_errors()
        .is_some_and(|e| !e.is_empty())
    {
        return None;
    }

    // Module-type implicit vars need direct Module construction
    let lowered = if meta.is_module {
        if let datamodel::Variable::Module(dm_module) = implicit_dm_var {
            let module_inputs: Vec<crate::variable::ModuleInput> = dm_module
                .references
                .iter()
                .filter_map(|mr| {
                    let ident_prefix = format!("{}\u{00B7}", canonicalize(&implicit_name));
                    let src = canonicalize(&mr.src);
                    let dst = canonicalize(&mr.dst);
                    if src.starts_with(&ident_prefix) {
                        return None;
                    }
                    let dst_stripped = dst.strip_prefix(&ident_prefix)?;
                    let src_str = if model.name(db) == "main" && src.starts_with('\u{00B7}') {
                        &src['\u{00B7}'.len_utf8()..]
                    } else {
                        &src
                    };
                    Some(crate::variable::ModuleInput {
                        src: Ident::new(src_str),
                        dst: Ident::new(dst_stripped),
                    })
                })
                .collect();
            crate::variable::Variable::Module {
                ident: Ident::new(&implicit_name),
                model_name: Ident::new(&dm_module.model_name),
                units: None,
                inputs: module_inputs,
                errors: vec![],
                unit_errors: vec![],
            }
        } else {
            return None;
        }
    } else {
        let models = HashMap::new();
        let scope = crate::model::ScopeStage0 {
            models: &models,
            dimensions: &dim_context,
            model_name: "",
        };
        crate::model::lower_variable(&scope, &parsed_implicit)
    };

    let model_name_ident = Ident::new(model.name(db));
    let var_ident_canonical: Ident<Canonical> = Ident::new(&implicit_name);

    // Arena for sub-model stub variables
    let arena = bumpalo::Bump::new();

    let mut mini_metadata: HashMap<Ident<Canonical>, crate::compiler::VariableMetadata<'_>> =
        HashMap::new();
    // LTM implicit vars are in the root model context
    let mut mini_offset = crate::vm::IMPLICIT_VAR_COUNT;

    // Add implicit time/dt/initial_time/final_time
    {
        use std::sync::LazyLock;
        static IMPLICIT_TIME: LazyLock<crate::variable::Variable> =
            LazyLock::new(|| crate::variable::Variable::Var {
                ident: Ident::new("time"),
                ast: None,
                init_ast: None,
                eqn: None,
                units: None,
                tables: vec![],
                non_negative: false,
                is_flow: false,
                is_table_only: false,
                errors: vec![],
                unit_errors: vec![],
            });
        static IMPLICIT_DT: LazyLock<crate::variable::Variable> =
            LazyLock::new(|| crate::variable::Variable::Var {
                ident: Ident::new("dt"),
                ast: None,
                init_ast: None,
                eqn: None,
                units: None,
                tables: vec![],
                non_negative: false,
                is_flow: false,
                is_table_only: false,
                errors: vec![],
                unit_errors: vec![],
            });
        static IMPLICIT_INITIAL_TIME: LazyLock<crate::variable::Variable> =
            LazyLock::new(|| crate::variable::Variable::Var {
                ident: Ident::new("initial_time"),
                ast: None,
                init_ast: None,
                eqn: None,
                units: None,
                tables: vec![],
                non_negative: false,
                is_flow: false,
                is_table_only: false,
                errors: vec![],
                unit_errors: vec![],
            });
        static IMPLICIT_FINAL_TIME: LazyLock<crate::variable::Variable> =
            LazyLock::new(|| crate::variable::Variable::Var {
                ident: Ident::new("final_time"),
                ast: None,
                init_ast: None,
                eqn: None,
                units: None,
                tables: vec![],
                non_negative: false,
                is_flow: false,
                is_table_only: false,
                errors: vec![],
                unit_errors: vec![],
            });
        mini_metadata.insert(
            Ident::new("time"),
            crate::compiler::VariableMetadata {
                offset: 0,
                size: 1,
                var: &IMPLICIT_TIME,
            },
        );
        mini_metadata.insert(
            Ident::new("dt"),
            crate::compiler::VariableMetadata {
                offset: 1,
                size: 1,
                var: &IMPLICIT_DT,
            },
        );
        mini_metadata.insert(
            Ident::new("initial_time"),
            crate::compiler::VariableMetadata {
                offset: 2,
                size: 1,
                var: &IMPLICIT_INITIAL_TIME,
            },
        );
        mini_metadata.insert(
            Ident::new("final_time"),
            crate::compiler::VariableMetadata {
                offset: 3,
                size: 1,
                var: &IMPLICIT_FINAL_TIME,
            },
        );
    }

    let project_models = project.models(db);
    let self_size = meta.size;

    mini_metadata.insert(
        var_ident_canonical.clone(),
        crate::compiler::VariableMetadata {
            offset: mini_offset,
            size: self_size,
            var: &lowered,
        },
    );
    mini_offset += self_size;

    // Collect dependencies from the implicit var itself
    let source_vars = model.variables(db);
    let mut dep_variables: Vec<(Ident<Canonical>, crate::variable::Variable, usize)> = Vec::new();
    let mut module_refs: HashMap<Ident<Canonical>, crate::vm::ModuleKey> = HashMap::new();

    // For module-type implicit vars, build module_refs from the dm_module references
    if meta.is_module
        && let datamodel::Variable::Module(dm_module) = implicit_dm_var
    {
        let input_prefix = format!("{implicit_name}\u{00B7}");
        let input_set: BTreeSet<Ident<Canonical>> = dm_module
            .references
            .iter()
            .filter_map(|mr| {
                let dst_canonical = canonicalize(&mr.dst);
                let bare = dst_canonical.strip_prefix(&input_prefix)?;
                Some(Ident::new(bare))
            })
            .collect();
        module_refs.insert(
            var_ident_canonical.clone(),
            (Ident::new(&dm_module.model_name), input_set),
        );

        // Add dependency stubs for module input sources
        let ltm_implicit_all = model_ltm_implicit_var_info(db, model, project);
        for mr in &dm_module.references {
            let src = canonicalize(&mr.src);
            let effective = src.strip_prefix('\u{00B7}').unwrap_or(&src);
            if effective == implicit_name.as_str()
                || matches!(effective, "time" | "dt" | "initial_time" | "final_time")
            {
                continue;
            }
            // For submodel references like `module_var·output`, extract the
            // module variable name and add it as a module-type dependency so
            // the compiler context can resolve the submodel offset lookup.
            if let Some(dot_pos) = effective.find('\u{00B7}') {
                let module_var_name = &effective[..dot_pos];
                let dep_ident = Ident::new(module_var_name);
                if mini_metadata.contains_key(&dep_ident)
                    || dep_variables.iter().any(|(id, _, _)| id == &dep_ident)
                {
                    continue;
                }
                // Look up the referenced module in LTM implicit vars
                if let Some(ref_meta) = ltm_implicit_all.get(module_var_name)
                    && ref_meta.is_module
                    && let Some(ref mn) = ref_meta.model_name
                {
                    dep_variables.push((
                        dep_ident.clone(),
                        crate::variable::Variable::Module {
                            ident: dep_ident,
                            model_name: Ident::new(mn),
                            units: None,
                            inputs: vec![],
                            errors: vec![],
                            unit_errors: vec![],
                        },
                        ref_meta.size,
                    ));
                }
                continue;
            }
            let dep_ident = Ident::new(effective);
            if mini_metadata.contains_key(&dep_ident)
                || dep_variables.iter().any(|(id, _, _)| id == &dep_ident)
            {
                continue;
            }
            if let Some(dep_sv) = source_vars.get(effective) {
                let dep_dims = variable_dimensions(db, *dep_sv, project);
                let dep_size = variable_size(db, *dep_sv, project);
                let dep_var = build_stub_variable(db, dep_sv, &dep_ident, dep_dims);
                dep_variables.push((dep_ident, dep_var, dep_size));
            } else {
                // Could be another LTM var or implicit var -- scalar stub
                dep_variables.push((
                    dep_ident.clone(),
                    crate::variable::Variable::Var {
                        ident: dep_ident,
                        ast: None,
                        init_ast: None,
                        eqn: None,
                        units: None,
                        tables: vec![],
                        non_negative: false,
                        is_flow: false,
                        is_table_only: false,
                        errors: vec![],
                        unit_errors: vec![],
                    },
                    1,
                ));
            }
        }
    } else {
        // Non-module implicit vars (e.g., temp args from PREVIOUS rewrite)
        // may reference module variables from the parent model. Collect
        // those dependencies so the compilation context can resolve them.
        let dep_idents = if let Some(ast) = lowered.ast() {
            crate::variable::identifier_set(ast, &[], None)
        } else {
            HashSet::new()
        };

        let implicit_info = model_implicit_var_info(db, model, project);

        for dep_ident_str in &dep_idents {
            let dep_str = dep_ident_str.as_str();
            let effective = dep_str.strip_prefix('\u{00B7}').unwrap_or(dep_str);

            if effective == implicit_name.as_str()
                || matches!(effective, "time" | "dt" | "initial_time" | "final_time")
            {
                continue;
            }

            // Handle dotted module·port references (e.g., module·output).
            // Extract the module variable name and add it as a dependency.
            if let Some(dot_pos) = effective.find('\u{00B7}') {
                let module_var_name = &effective[..dot_pos];
                let dep_ident = Ident::new(module_var_name);
                if mini_metadata.contains_key(&dep_ident)
                    || dep_variables.iter().any(|(id, _, _)| id == &dep_ident)
                {
                    continue;
                }
                // Check model's implicit module vars (SMOOTH/DELAY instances)
                if let Some(im_meta) = implicit_info.get(module_var_name)
                    && im_meta.is_module
                    && let Some(ref mn) = im_meta.model_name
                {
                    dep_variables.push((
                        dep_ident.clone(),
                        crate::variable::Variable::Module {
                            ident: dep_ident,
                            model_name: Ident::new(mn),
                            units: None,
                            inputs: vec![],
                            errors: vec![],
                            unit_errors: vec![],
                        },
                        im_meta.size,
                    ));
                    module_refs.insert(
                        Ident::new(module_var_name),
                        (Ident::new(mn), BTreeSet::new()),
                    );
                } else if let Some(dep_sv) = source_vars.get(module_var_name)
                    && dep_sv.kind(db) == SourceVariableKind::Module
                {
                    let mod_model_name = dep_sv.model_name(db);
                    let sub_canonical = canonicalize(mod_model_name);
                    let sub_size = project_models
                        .get(sub_canonical.as_ref())
                        .map(|sm| compute_layout(db, *sm, project, false).n_slots)
                        .unwrap_or(1);
                    dep_variables.push((
                        dep_ident.clone(),
                        crate::variable::Variable::Module {
                            ident: dep_ident,
                            model_name: Ident::new(mod_model_name),
                            units: None,
                            inputs: vec![],
                            errors: vec![],
                            unit_errors: vec![],
                        },
                        sub_size,
                    ));
                    module_refs.insert(
                        Ident::new(module_var_name),
                        (Ident::new(mod_model_name), BTreeSet::new()),
                    );
                }
                continue;
            }

            let dep_ident = Ident::new(effective);
            if mini_metadata.contains_key(&dep_ident)
                || dep_variables.iter().any(|(id, _, _)| id == &dep_ident)
            {
                continue;
            }

            if let Some(dep_sv) = source_vars.get(effective) {
                let dep_dims = variable_dimensions(db, *dep_sv, project);
                let dep_size = variable_size(db, *dep_sv, project);
                let dep_var = build_stub_variable(db, dep_sv, &dep_ident, dep_dims);
                dep_variables.push((dep_ident, dep_var, dep_size));
            } else {
                dep_variables.push((
                    dep_ident.clone(),
                    crate::variable::Variable::Var {
                        ident: dep_ident,
                        ast: None,
                        init_ast: None,
                        eqn: None,
                        units: None,
                        tables: vec![],
                        non_negative: false,
                        is_flow: false,
                        is_table_only: false,
                        errors: vec![],
                        unit_errors: vec![],
                    },
                    1,
                ));
            }
        }
    }

    for (dep_ident, dep_var, dep_size) in &dep_variables {
        if !mini_metadata.contains_key(dep_ident) {
            mini_metadata.insert(
                dep_ident.clone(),
                crate::compiler::VariableMetadata {
                    offset: mini_offset,
                    size: *dep_size,
                    var: dep_var,
                },
            );
            mini_offset += dep_size;
        }
    }

    let mut all_metadata: HashMap<
        Ident<Canonical>,
        HashMap<Ident<Canonical>, crate::compiler::VariableMetadata<'_>>,
    > = HashMap::new();
    all_metadata.insert(model_name_ident.clone(), mini_metadata);

    // Build sub-model metadata for module-type implicit vars and any
    // module dependencies discovered during dependency collection.
    if meta.is_module
        && let Some(model_name) = &meta.model_name
    {
        let sub_canonical = canonicalize(model_name);
        if let Some(sub_model) = project_models.get(sub_canonical.as_ref()) {
            build_submodel_metadata(&arena, db, *sub_model, project, &mut all_metadata);
        }
    }
    for (sub_model_name, _input_set) in module_refs.values() {
        let sub_canonical = canonicalize(sub_model_name.as_str());
        if let Some(sub_model) = project_models.get(sub_canonical.as_ref())
            && !all_metadata.contains_key(sub_model_name)
        {
            build_submodel_metadata(&arena, db, *sub_model, project, &mut all_metadata);
        }
    }

    let mini_layout =
        crate::compiler::symbolic::layout_from_metadata(&all_metadata, &model_name_ident)
            .unwrap_or_else(|_| VariableLayout::new(HashMap::new(), 0));
    let rmap = ReverseOffsetMap::from_layout(&mini_layout);

    let tables: HashMap<Ident<Canonical>, Vec<crate::compiler::Table>> = HashMap::new();
    let inputs = canonical_module_input_set(module_input_names);
    let mut module_models = model_module_map(db, model, project).clone();

    // Merge ALL LTM implicit module references into the module_models map.
    // A module's inputs may reference outputs from OTHER LTM implicit modules
    // (e.g., PREVIOUS instance #6 reading from PREVIOUS instance #5's output),
    // so we must include all LTM implicit modules, not just the current one.
    {
        let ltm_implicit = model_ltm_implicit_var_info(db, model, project);
        let current_model_modules = module_models.entry(model_name_ident.clone()).or_default();
        // Add the current variable's own module refs
        for (var_ident, (sub_model_name, _input_set)) in &module_refs {
            current_model_modules.insert(var_ident.clone(), sub_model_name.clone());
        }
        // Add all other LTM implicit module refs so cross-references resolve
        for (name, im_meta) in ltm_implicit.iter() {
            if im_meta.is_module
                && let Some(mn) = &im_meta.model_name
            {
                let im_ident: Ident<Canonical> = Ident::new(name);
                current_model_modules.insert(im_ident, Ident::new(mn.as_str()));
            }
        }
    }

    let core = crate::compiler::ContextCore {
        dimensions: &converted_dims,
        dimensions_ctx: &dim_context,
        model_name: &model_name_ident,
        metadata: &all_metadata,
        module_models: &module_models,
        inputs: &inputs,
    };

    let build_var = |is_initial: bool| {
        crate::compiler::Var::new(
            &crate::compiler::Context::new(core, &var_ident_canonical, is_initial),
            &lowered,
        )
    };

    let compile_phase = |exprs: &[crate::compiler::Expr]| -> Option<PerVarBytecodes> {
        if exprs.is_empty() {
            return None;
        }

        let module_inputs_set: HashSet<Ident<Canonical>> = inputs.iter().cloned().collect();
        let module = crate::compiler::Module {
            ident: model_name_ident.clone(),
            inputs: module_inputs_set,
            n_slots: mini_offset,
            n_temps: 0,
            temp_sizes: vec![],
            runlist_initials: vec![],
            runlist_initials_by_var: vec![],
            runlist_flows: exprs.to_vec(),
            runlist_stocks: vec![],
            offsets: all_metadata
                .iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        v.iter()
                            .map(|(vk, vm)| (vk.clone(), (vm.offset, vm.size)))
                            .collect(),
                    )
                })
                .collect(),
            runlist_order: vec![var_ident_canonical.clone()],
            tables: tables.clone(),
            dimensions: converted_dims.clone(),
            dimensions_ctx: dim_context.clone(),
            module_refs: module_refs.clone(),
        };

        let mut temp_sizes_map: HashMap<u32, usize> = HashMap::new();
        for expr in exprs {
            crate::compiler::extract_temp_sizes_pub(expr, &mut temp_sizes_map);
        }
        let n_temps = temp_sizes_map.len();
        let mut temp_sizes: Vec<usize> = vec![0; n_temps];
        for (id, size) in &temp_sizes_map {
            if (*id as usize) < temp_sizes.len() {
                temp_sizes[*id as usize] = *size;
            }
        }

        let module = crate::compiler::Module {
            n_temps,
            temp_sizes: temp_sizes.clone(),
            ..module
        };

        match module.compile() {
            Ok(compiled) => {
                let sym_bc =
                    crate::compiler::symbolic::symbolize_bytecode(&compiled.compiled_flows, &rmap)
                        .ok()?;

                let ctx = &*compiled.context;
                let sym_views: Vec<_> = ctx
                    .static_views
                    .iter()
                    .map(|sv| crate::compiler::symbolic::symbolize_static_view(sv, &rmap))
                    .collect::<Result<Vec<_>, _>>()
                    .ok()?;
                let sym_mods: Vec<_> = ctx
                    .modules
                    .iter()
                    .map(|md| crate::compiler::symbolic::symbolize_module_decl(md, &rmap))
                    .collect::<Result<Vec<_>, _>>()
                    .ok()?;

                let temp_sizes_vec: Vec<(u32, usize)> =
                    temp_sizes_map.iter().map(|(&k, &v)| (k, v)).collect();

                let dim_lists: Vec<Vec<u16>> = ctx
                    .dim_lists
                    .iter()
                    .map(|(n, arr)| arr[..(*n as usize)].to_vec())
                    .collect();

                Some(PerVarBytecodes {
                    symbolic: sym_bc,
                    graphical_functions: ctx.graphical_functions.clone(),
                    module_decls: sym_mods,
                    static_views: sym_views,
                    temp_sizes: temp_sizes_vec,
                    dim_lists,
                })
            }
            Err(_) => None,
        }
    };

    // LTM implicit vars participate in whichever phases their lowered
    // form needs. The dep_graph won't have them in its runlists since
    // they are not part of the original model.
    // We compile all available phases.
    let initial_bytecodes = match build_var(true) {
        Ok(var_result) => compile_phase(&var_result.ast),
        Err(_) => None,
    };

    let flow_bytecodes = if !meta.is_stock {
        match build_var(false) {
            Ok(var_result) => compile_phase(&var_result.ast),
            Err(_) => None,
        }
    } else {
        None
    };

    let stock_bytecodes = if meta.is_stock || meta.is_module {
        match build_var(false) {
            Ok(var_result) => compile_phase(&var_result.ast),
            Err(_) => None,
        }
    } else {
        None
    };

    Some(VarFragmentResult {
        fragment: CompiledVarFragment {
            ident: implicit_name,
            initial_bytecodes,
            flow_bytecodes,
            stock_bytecodes,
        },
    })
}

/// Find the output ports for a model by scanning other models' variable
/// dependencies for module·var references that target this model.
///
/// When variable X depends on `module_var·internal_var` and `module_var`
/// maps to this model (via `dynamic_modules`), then `internal_var` is
/// an output port. The result is passed to `enumerate_pathways_to_outputs`
/// so that composite scores are generated for the correct output ports.
fn find_model_output_ports(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> Vec<Ident<Canonical>> {
    let model_name = model.name(db);
    let project_models = project.models(db);
    let middot = '\u{00B7}';
    let mut output_ports: HashSet<Ident<Canonical>> = HashSet::new();

    for (_, other_model) in project_models.iter() {
        if other_model == &model {
            continue;
        }
        let other_edges = super::model_causal_edges(db, *other_model, project);

        // Build a set of module variable names that reference this model
        let module_var_names: HashSet<&String> = other_edges
            .dynamic_modules
            .iter()
            .filter(|(_var_name, mn)| mn.as_str() == model_name.as_str())
            .map(|(var_name, _mn)| var_name)
            .collect();

        if module_var_names.is_empty() {
            continue;
        }

        // Scan dependencies for module·internal_var references
        let other_vars = other_model.variables(db);
        let module_ctx = super::model_module_ident_context(db, *other_model, vec![]);
        for (_, source_var) in other_vars.iter() {
            let deps = super::variable_direct_dependencies(db, *source_var, project);
            for dep in &deps.dt_deps {
                if let Some(dot_pos) = dep.find(middot) {
                    let module_part = &dep[..dot_pos];
                    let internal_var = &dep[dot_pos + middot.len_utf8()..];
                    if module_var_names.contains(&module_part.to_string()) {
                        output_ports.insert(Ident::new(internal_var));
                    }
                }
            }

            // Also check implicit variable deps (SMOOTH/DELAY expansion
            // creates helper auxes whose deps may reference module outputs)
            let parsed = super::parse_source_variable_with_module_context(
                db,
                *source_var,
                project,
                module_ctx,
            );
            for implicit_dm_var in &parsed.implicit_vars {
                if let datamodel::Variable::Module(_) = implicit_dm_var {
                    continue;
                }
                let deps = super::variable_direct_dependencies(db, *source_var, project);
                for iv_dep in &deps.implicit_vars {
                    for dep in &iv_dep.dt_deps {
                        if let Some(dot_pos) = dep.find(middot) {
                            let module_part = &dep[..dot_pos];
                            let internal_var = &dep[dot_pos + middot.len_utf8()..];
                            if module_var_names.contains(&module_part.to_string()) {
                                output_ports.insert(Ident::new(internal_var));
                            }
                        }
                    }
                }
            }
        }
    }

    output_ports.into_iter().collect()
}

/// Whether the edge `from -> to` is a *partial reduce*: `from` is arrayed,
/// `to` is arrayed with strictly fewer dimensions, and every `to` dimension
/// is one of `from`'s (matched by name). That is exactly the shape
/// `try_cross_dimensional_link_scores` emits per-`(reduced-elem,
/// result-elem)` scalar link scores for (`matrix[D1,D2] -> agg[D1]`). Same
/// dimensions, broadcast, mismatched dims, and module-involved edges are
/// not partial reduces. (Whether `to`'s equation actually applies a
/// reducing builtin to `from` is not checked here -- the loop-link builder
/// only needs to know to keep both element subscripts; the equation-text
/// path classifies the reducer.)
///
/// This shape-only check is *not* superseded by the aggregate-node reroute:
/// that reroute (`enumerate_agg_nodes` + `model_element_causal_edges`) only
/// covers *scalar synthetic* aggs hoisted out of a larger expression. A
/// whole-RHS arrayed-result reducer (`agg[D1] = SUM(matrix[D1,*])`) is a
/// *variable-backed* agg whose edges still come from the normal reference
/// walker, so the loop-link builder still needs this predicate to know to
/// keep both element subscripts on the `matrix[d1,d2] -> agg[d1]` link.
fn is_partial_reduce_edge(
    db: &dyn Db,
    source_vars: &HashMap<String, super::SourceVariable>,
    from: &str,
    to: &str,
    project: SourceProject,
) -> bool {
    let from_sv = match source_vars.get(from) {
        Some(sv) if sv.kind(db) != SourceVariableKind::Module => sv,
        _ => return false,
    };
    let to_sv = match source_vars.get(to) {
        Some(sv) if sv.kind(db) != SourceVariableKind::Module => sv,
        _ => return false,
    };
    let from_dims = variable_dimensions(db, *from_sv, project);
    let to_dims = variable_dimensions(db, *to_sv, project);
    if from_dims.is_empty() || to_dims.is_empty() || to_dims.len() >= from_dims.len() {
        return false;
    }
    let from_names: Vec<&str> = from_dims.iter().map(|d| d.name()).collect();
    to_dims.iter().all(|td| from_names.contains(&td.name()))
}

/// Compute the cartesian product of element name lists as comma-joined
/// subscript strings.
///
/// For a single dimension `[["nyc", "boston"]]`, returns `["nyc", "boston"]`.
/// For two dimensions `[["nyc", "boston"], ["adult", "child"]]`, returns
/// `["nyc,adult", "nyc,child", "boston,adult", "boston,child"]`.
fn cartesian_subscripts(dim_element_lists: &[Vec<String>]) -> Vec<String> {
    if dim_element_lists.is_empty() {
        return vec![];
    }
    let mut result: Vec<String> = dim_element_lists[0].clone();
    for dim_elements in &dim_element_lists[1..] {
        let mut expanded = Vec::with_capacity(result.len() * dim_elements.len());
        for existing in &result {
            for elem in dim_elements {
                expanded.push(format!("{existing},{elem}"));
            }
        }
        result = expanded;
    }
    result
}

/// One source row a hoisted reducer reads, paired with the agg result slot it
/// feeds and the co-reduced rows (the rows mapping to the *same* slot -- the
/// `from`-slice the reducer combines for that slot, used as the
/// `all_elements` argument for the per-element link-score equation builders).
struct ReadSliceRow {
    /// The source element subscript (comma-joined element names).
    row: String,
    /// The agg result slot's subscript (the `Iterated` axes' elements,
    /// comma-joined; empty when the agg is scalar).
    slot: String,
    /// All source rows mapping to `slot`, in row-major order.
    coreduced: Vec<String>,
}

/// Enumerate the source rows a hoisted reducer reads, given the agg's
/// `read_slice` (one [`crate::ltm_agg::AxisRead`] per `from`'s axis -- which
/// holds because `from` is one of `agg`'s `source_vars`) and `from`'s
/// dimension element lists. A `Pinned` axis is fixed to its single element; an
/// `Iterated` or `Reduced` axis ranges over every element of that axis. The
/// agg result slot for a row is its `Iterated` coordinates in order.
///
/// `None` when `read_slice` doesn't have one entry per `from` axis (it always
/// should for a hoisted agg whose `source_vars` contains `from`): the caller
/// then falls back to the conservative "every source element, scalar agg" form.
fn read_slice_rows(
    read_slice: &[crate::ltm_agg::AxisRead],
    from_dim_element_lists: &[Vec<String>],
) -> Option<Vec<ReadSliceRow>> {
    use crate::ltm_agg::AxisRead;
    if read_slice.len() != from_dim_element_lists.len() {
        return None;
    }
    // Per axis: the element list to iterate, plus whether the axis contributes
    // a coordinate to the result slot.
    let per_axis: Vec<(Vec<String>, bool)> = read_slice
        .iter()
        .zip(from_dim_element_lists)
        .map(|(a, elems)| match a {
            AxisRead::Pinned(e) => (vec![e.clone()], false),
            AxisRead::Iterated(_) => (elems.clone(), true),
            AxisRead::Reduced => (elems.clone(), false),
        })
        .collect();
    // Cartesian product, tracking each row's full element tuple and its slot
    // coordinates.
    let mut rows: Vec<(Vec<String>, Vec<String>)> = vec![(Vec::new(), Vec::new())];
    for (elems, contributes_to_slot) in &per_axis {
        let mut next: Vec<(Vec<String>, Vec<String>)> =
            Vec::with_capacity(rows.len() * elems.len());
        for (row, slot) in &rows {
            for e in elems {
                let mut new_row = row.clone();
                new_row.push(e.clone());
                let mut new_slot = slot.clone();
                if *contributes_to_slot {
                    new_slot.push(e.clone());
                }
                next.push((new_row, new_slot));
            }
        }
        rows = next;
    }
    // Group rows by slot to build each row's `coreduced` set.
    let mut by_slot: HashMap<String, Vec<String>> = HashMap::new();
    for (row, slot) in &rows {
        by_slot
            .entry(slot.join(","))
            .or_default()
            .push(row.join(","));
    }
    Some(
        rows.into_iter()
            .map(|(row, slot)| {
                let slot = slot.join(",");
                let row = row.join(",");
                let coreduced = by_slot[&slot].clone();
                ReadSliceRow {
                    row,
                    slot,
                    coreduced,
                }
            })
            .collect(),
    )
}

/// Build the element-level `Loop::stocks` list for a cycle.
///
/// For a scalar cycle (`dimensions` empty), the stocks are the
/// variable-level stock idents unchanged -- a genuinely scalar variable's
/// name *is* its (degenerate, subscript-free) element-level node name.
///
/// For an A2A cycle (`dimensions` non-empty) the stocks cover the loop's
/// *entire* dimension element space: one `"{var}[{elem-tuple}]"` per element
/// of the dimension space (in the runtime's row-major slot order, via
/// [`crate::ltm::loop_dimension_element_tuples`]), for each variable in
/// `var_stocks`.  This is the granularity the `Loop` docstring's invariant
/// requires so `CyclePartitions::partition_for_loop` can resolve a partition
/// *per slot* against `model_element_cycle_partitions`'s element-keyed
/// `stock_partition` map.  If `dm_dims` doesn't cover the cycle's declared
/// dimensions (a mid-edit inconsistency), the variable-level stocks are
/// returned unchanged -- `partition_for_loop` then falls back to whatever
/// suffixes are present (none, here), bucketing the loop into the `None`
/// group, the same degradation the pre-element-level code exhibited.
fn build_a2a_loop_stocks(
    var_stocks: &[Ident<Canonical>],
    dimensions: &[String],
    dm_dims: &[crate::datamodel::Dimension],
) -> Vec<Ident<Canonical>> {
    if dimensions.is_empty() {
        return var_stocks.to_vec();
    }
    let tuples = crate::ltm::loop_dimension_element_tuples(dimensions, dm_dims);
    if tuples.is_empty() {
        return var_stocks.to_vec();
    }
    let mut stocks = Vec::with_capacity(tuples.len() * var_stocks.len());
    for tuple in &tuples {
        for s in var_stocks {
            stocks.push(Ident::new(&format!("{}[{}]", s.as_str(), tuple)));
        }
    }
    stocks
}

/// Build `Loop` structs from the tiered loop-enumeration result.
///
/// The fast path (`tiered.fast_path`) carries variable-level cycles
/// already classified as PureScalar or PureSameElementA2A; each one
/// emits a single `Loop` directly. The slow path
/// (`tiered.slow_path`) carries element-level circuits over the
/// cross-element subgraph; those flow through the same per-circuit
/// grouping logic the legacy `build_element_level_loops` uses.
///
/// The merged Loop list is passed to `assign_loop_ids` once so loop
/// IDs (`r1, b1, ...`) are assigned over the unified set, matching
/// the legacy ordering: `assign_loop_ids` sorts by content-derived
/// key (sorted distinct var names) before numbering, so the final IDs
/// are stable regardless of which path produced each Loop.
///
/// `agg_loop_budget` and the returned `Vec<String>` thread the cross-element-
/// through-aggregate loop-count budget / truncated-aggregate-node names
/// through to `build_element_level_loops` -> `recover_cross_agg_loops` (only
/// the slow path can carry agg nodes); the caller surfaces the flag (and the
/// names) on `LtmVariablesResult::agg_recovery_truncated` and the truncation
/// `Warning`. The returned vector is empty iff nothing was clipped.
pub(crate) fn build_loops_from_tiered(
    tiered: &super::TieredCircuitsResult,
    var_graph: &crate::ltm::CausalGraph,
    source_vars: &HashMap<String, super::SourceVariable>,
    db: &dyn Db,
    project: SourceProject,
    dm_dims: &[crate::datamodel::Dimension],
    agg_loop_budget: usize,
) -> (Vec<crate::ltm::Loop>, Vec<String>) {
    use crate::common::{Canonical, Ident};
    use crate::ltm::{Loop, assign_loop_ids};

    let mut all_loops: Vec<Loop> = Vec::new();

    // Fast-path: each cycle materializes directly into one Loop. The
    // FastPathCircuit's `dimensions` field carries the shared
    // arrayed dimensions (empty for PureScalar). The links / stocks /
    // polarity are derived from the variable-level cycle exactly as
    // the legacy pure-dimension branch did.
    for fp in &tiered.fast_path {
        if fp.variables.is_empty() {
            continue;
        }
        let var_level_nodes: Vec<Ident<Canonical>> = fp
            .variables
            .iter()
            .map(|s| Ident::new(s.as_str()))
            .collect();
        let links = var_graph.circuit_to_links(&var_level_nodes);
        let var_stocks = var_graph.find_stocks_in_loop(&var_level_nodes);
        let polarity = var_graph.calculate_polarity(&links);

        // Map the canonical dimension names to original datamodel
        // names so equation parsing on the loop-score variable
        // resolves the dimension by string match. Mirrors the legacy
        // pure-dimension branch's mapping logic.
        //
        // Fallback: if a canonical name in `fp.dimensions` is missing
        // from `dm_dims`, fall back to the canonical name itself.
        // This matches `build_element_level_loops` (the slow-path
        // consumer below) so incremental or partially-invalid model
        // states -- where the tiered enumerator's cached dim closure
        // can outrun a still-being-edited datamodel dim list -- surface
        // as a downstream analysis warning rather than a hard panic
        // that takes down the whole LTM pipeline. We assert in debug
        // builds so the mismatch stays observable when the model
        // really is internally consistent.
        let dimensions: Vec<String> = fp
            .dimensions
            .iter()
            .map(|canonical| {
                let resolved = dm_dims
                    .iter()
                    .find(|dm| crate::common::canonicalize(dm.name()).as_ref() == canonical)
                    .map(|dm| dm.name().to_string());
                debug_assert!(
                    resolved.is_some(),
                    "fast-path A2A cycle references dimension {canonical:?} that is not in \
                     the project's datamodel dimensions {known:?}; falling back to canonical \
                     name. This usually means the source project's dim list and the parsed \
                     variable dims got out of sync mid-edit.",
                    known = dm_dims.iter().map(|d| d.name()).collect::<Vec<_>>(),
                );
                resolved.unwrap_or_else(|| canonical.to_string())
            })
            .collect();

        // For a PureSameElementA2A cycle the stocks are element-level over
        // the dimension space (per the `Loop` docstring's invariant); for a
        // PureScalar cycle (`dimensions` empty) they're the variable-level
        // names unchanged.
        let stocks = build_a2a_loop_stocks(&var_stocks, &dimensions, dm_dims);

        all_loops.push(Loop {
            id: String::new(),
            links,
            stocks,
            polarity,
            dimensions,
        });
    }

    // Slow-path: feed the element-level circuit list through the
    // existing per-circuit grouping logic. This emits cross-element
    // and mixed scalar loops the same way the legacy code did. The
    // helper does its own `assign_loop_ids`; we strip the IDs
    // afterward because we re-run id assignment over the merged set
    // to keep numbering consistent.
    let mut truncated_aggs: Vec<String> = Vec::new();
    if !tiered.slow_path.is_empty() {
        let (mut slow_path_loops, t) = build_element_level_loops(
            &tiered.slow_path,
            var_graph,
            source_vars,
            db,
            project,
            dm_dims,
            agg_loop_budget,
        );
        truncated_aggs = t;
        for l in &mut slow_path_loops {
            l.id.clear();
        }
        all_loops.extend(slow_path_loops);
    }

    assign_loop_ids(&mut all_loops);
    (all_loops, truncated_aggs)
}

/// Build element-subscripted `Link`s for one element-level circuit.
///
/// For circuit nodes `[n_0, ..., n_{k-1}]` (each `n_i` either `var` or
/// `var[e_i]`), each link `n_i -> n_{i+1}` keeps the element subscript on
/// the side(s) the loop-score equation needs to pin a per-element link
/// score:
///
///   - `from = n_i` (subscript kept) when `n_i` is subscripted. A
///     per-source-element FixedIndex / cross-dimensional link score is
///     named `{from}[{e_i}]->{to}` (the bracketed `from` form
///     `try_cross_dimensional_link_scores` and FixedIndex emission
///     produce); `generate_loop_score_equation`'s resolver also falls
///     back to the variable-level `from` form when the bracketed name
///     wasn't emitted (e.g. a structural flow->stock A2A link score is
///     `{strip(from)}->{to}`).
///   - `to = n_{i+1}` (subscript kept) when `n_{i+1}` is subscripted AND
///     its variable is dimensioned (so the link score is A2A and the loop
///     visits one element of it -- `generate_loop_score_equation` then
///     emits `"...→{to}"[e_{i+1}]`). A synthetic aggregate node
///     (`$⁚ltm⁚agg⁚{n}[<slot>]`) is treated like a dimensioned variable
///     here even though it isn't in `source_vars`: when the agg is arrayed
///     its slot rides in the link-score names `emit_source_to_agg_link_scores`
///     / `emit_agg_to_target_link_scores` emit (`{src}[<row>]→{agg}[<slot>]`,
///     `{agg}[<slot>]→{tgt}[<elem>]`), so `loop_link_score_ref` needs the
///     `[<slot>]` on the agg endpoint to resolve those names. Otherwise
///     `to = strip(n_{i+1})` (the link score is scalar / cross-dimensional,
///     referenced without a subscript).
///
/// `var_links` carries the variable-level links for the same circuit
/// (from `circuit_to_links` on the stripped node sequence); link `i`'s
/// polarity is taken from `var_links[i]` (the variable-level static
/// polarity for that hop), defaulting to `Unknown` if the lengths ever
/// disagree (they shouldn't).
fn build_element_subscripted_links(
    circuit: &[&str],
    var_links: &[crate::ltm::Link],
    source_vars: &HashMap<String, super::SourceVariable>,
    db: &dyn Db,
    project: SourceProject,
) -> Vec<crate::ltm::Link> {
    let mut links = Vec::with_capacity(circuit.len());
    for i in 0..circuit.len() {
        let from_raw = circuit[i];
        let to_raw = circuit[(i + 1) % circuit.len()];
        let polarity = if i < var_links.len() {
            var_links[i].polarity
        } else {
            crate::ltm::LinkPolarity::Unknown
        };
        let link_from = if from_raw.contains('[') {
            from_raw
        } else {
            strip_subscript(from_raw)
        };
        let to_var_level = strip_subscript(to_raw);
        let to_is_arrayed = crate::ltm_agg::is_synthetic_agg_name(to_var_level)
            || source_vars
                .get(to_var_level)
                .map(|sv| {
                    sv.kind(db) != SourceVariableKind::Module
                        && !variable_dimensions(db, *sv, project).is_empty()
                })
                .unwrap_or(false);
        let link_to = if to_raw.contains('[') && to_is_arrayed {
            to_raw
        } else {
            to_var_level
        };
        links.push(crate::ltm::Link {
            from: Ident::new(link_from),
            to: Ident::new(link_to),
            polarity,
        });
    }
    links
}

/// Build `Loop` structs from element-level circuits, grouping
/// pure-dimension circuits into shared A2A loops, scoring cross-element
/// circuits along their element-level path, and keeping mixed circuits as
/// individual scalar loops.
///
/// Pure-dimension: all circuits in a group have the same variable-level
/// node sequence (e.g., `[population, births]` for both `[population[nyc],
/// births[nyc]]` and `[population[boston], births[boston]]`). These share
/// one loop ID and produce an A2A loop score with dimensions.
///
/// Cross-element: a circuit that visits different elements at different
/// points (a per-element-equation hop reading the *other* element, or a
/// wildcard reducer). Each circuit becomes its own scalar Loop whose
/// `Link`s carry element subscripts so the loop-score equation references
/// the per-element link scores along the element path.
///
/// Mixed: any circuit containing a scalar node or where the group has
/// circuits with different variable-level structures. Each gets its own
/// scalar loop with a unique element-specific ID suffix.
///
/// `agg_loop_budget` caps how many non-elementary cross-element-through-
/// aggregate loops `recover_cross_agg_loops` materializes (across all
/// aggregate nodes); the returned `Vec<String>` names the aggregate nodes
/// whose enumeration that budget (or the per-aggregate petal cap) clipped
/// (sorted, deduped -- empty iff nothing was clipped). Callers pass
/// `cross_agg_loop_budget()` (or, in tests, a small value) and thread the
/// names up to `LtmVariablesResult::agg_recovery_truncated` and the
/// truncation `Warning`.
///
/// Visibility is `pub(crate)` so unit tests in
/// `db_ltm_unified_tests.rs` can drive this function directly to
/// inspect the element-subscripted `Link.from` / `Link.to` strings the
/// loop builder produces (e.g. `"population[nyc]"`) -- there is no
/// separate per-link shape field, and these per-link strings aren't
/// observable through the `LtmVariablesResult.vars` surface (which only
/// exposes the rendered equation strings).
pub(crate) fn build_element_level_loops(
    element_circuits: &super::LoopCircuitsResult,
    var_graph: &crate::ltm::CausalGraph,
    source_vars: &HashMap<String, super::SourceVariable>,
    db: &dyn Db,
    project: SourceProject,
    dm_dims: &[crate::datamodel::Dimension],
    agg_loop_budget: usize,
) -> (Vec<crate::ltm::Loop>, Vec<String>) {
    use crate::common::{Canonical, Ident};
    use crate::ltm::{Loop, assign_loop_ids};

    // Materialize each circuit as a small `Vec<&str>` once so downstream
    // grouping, name stripping, and node-wise comparisons don't pay the
    // indexed-lookup cost repeatedly.  The backing storage stays in
    // `element_circuits.names`, so these slices are all borrows into the
    // existing name table rather than per-call allocations.
    let circuit_strs: Vec<Vec<&str>> = (0..element_circuits.len())
        .map(|i| element_circuits.circuit_names(i).collect())
        .collect();

    // Group element-level circuits by their variable-level node sequence.
    // The key is the joined stripped names; the value collects indices
    // into `circuit_strs` that share that variable-level structure.
    let mut groups: HashMap<String, Vec<usize>> = HashMap::new();
    for (ci, circuit) in circuit_strs.iter().enumerate() {
        let var_level_key: String = circuit
            .iter()
            .map(|n| strip_subscript(n))
            .collect::<Vec<_>>()
            .join("\x00");
        groups.entry(var_level_key).or_default().push(ci);
    }

    // Sort groups deterministically by their key.
    let mut sorted_groups: Vec<(String, Vec<usize>)> = groups.into_iter().collect();
    sorted_groups.sort_by(|a, b| a.0.cmp(&b.0));

    let mut all_loops: Vec<Loop> = Vec::new();

    for (_group_key, group_indices) in &sorted_groups {
        let circuits_in_group: Vec<&[&str]> = group_indices
            .iter()
            .map(|&ci| circuit_strs[ci].as_slice())
            .collect();
        // Determine if this is a pure-dimension group.
        //
        // A group is pure-dimension when:
        // 1. Every node in every circuit has a subscript (no scalar nodes)
        // 2. The stripped variable-level sequence has NO repeated variables
        //    (repeated variables indicate cross-element circuits, e.g.,
        //    pop[nyc]->share[boston]->...->pop[boston]->share[nyc], where
        //    "population" appears twice in the stripped sequence)
        // 3. The group has more than one circuit (multiple elements share
        //    the same structure), OR has exactly one circuit with subscripted
        //    nodes (single-element dimension is still A2A)
        //
        // When a model has no arrayed variables, circuits won't have
        // subscripts and each group has exactly one circuit -- they are
        // scalar loops.
        let representative: &[&str] = circuits_in_group[0];
        let all_subscripted = representative.iter().all(|n| n.contains('['));

        // A circuit that traverses a synthetic aggregate node
        // (`$⁚ltm⁚agg⁚{n}`) must never be collapsed into a single A2A loop:
        // the per-row source link scores (`{src}[<row>]→{agg}[<slot>]`) and
        // the agg→target link scores (`{agg}[<slot>]→{tgt}[<elem>]`) are
        // scalar variables keyed by literal element names, so there is no
        // dimensioned `{src}→{agg}` link-score variable an A2A loop-score
        // equation could reference. Route these to the element-subscripted
        // per-circuit path (one scalar `Loop` per element combination), the
        // same path cross-element loops take -- `build_element_subscripted_links`
        // keeps the agg's `[<slot>]` so `loop_link_score_ref` can resolve the
        // agg-half names. (Even though the agg circuit's leading subscript
        // elements may all agree -- e.g. `[matrix[a,x], agg[a], growth[a,x]]`
        // -- which would otherwise classify it as a same-element A2A circuit.)
        let representative_has_synthetic_agg = representative
            .iter()
            .any(|n| crate::ltm_agg::is_synthetic_agg_name(strip_subscript(n)));

        // Detect cross-element circuits that should NOT be collapsed
        // into A2A loops. Two patterns indicate cross-element:
        //
        // 1. Repeated variable names: the stripped sequence has a variable
        //    appearing more than once (e.g., pop[nyc]->share[boston]->
        //    pop[boston]->share[nyc] has pop and share each twice).
        //
        // 2. Mixed subscripts: nodes in a circuit have different element
        //    subscripts at shared dimensions. A genuine A2A circuit visits
        //    each variable at the SAME element in shared dimensions
        //    (pop[nyc]->births[nyc]->pop[nyc], all nyc). Cross-element
        //    circuits visit different elements (pop[nyc]->mp[boston]->...).
        //
        //    Partial-collapse loops are NOT cross-element: source[a,x]->
        //    target[a] has subscripts "a,x" and "a" which differ in length
        //    but share the same element "a" on the shared dimension. We
        //    compare only the first element (leading shared dimension).
        let is_cross_element = if all_subscripted {
            // Check 1: repeated variable names
            let stripped: Vec<&str> = representative.iter().map(|n| strip_subscript(n)).collect();
            let mut seen = std::collections::HashSet::new();
            let has_repeated = stripped.iter().any(|v| !seen.insert(*v));
            if has_repeated {
                true
            } else {
                // Check 2: compare the leading (first) subscript element
                // across all nodes. Nodes with partial-collapse dimensions
                // have fewer subscript components (e.g., "a" vs "a,x"), but
                // the leading element is shared. If leading elements differ,
                // it's a genuine cross-element circuit.
                circuits_in_group.iter().any(|circuit| {
                    let leading_elements: Vec<&str> = circuit
                        .iter()
                        .filter_map(|n| {
                            let start = n.find('[')?;
                            let end = n.rfind(']')?;
                            let subscript = &n[start + 1..end];
                            // Take the first comma-separated component
                            Some(subscript.split(',').next().unwrap_or(subscript))
                        })
                        .collect();
                    // If leading elements differ, it's cross-element
                    leading_elements.windows(2).any(|w| w[0] != w[1])
                })
            }
        } else {
            false
        };

        if all_subscripted
            && !is_cross_element
            && !representative_has_synthetic_agg
            && !representative.is_empty()
        {
            // Pure-dimension group: produce a single A2A loop.
            //
            // Use the variable-level graph for polarity analysis and stock
            // detection (the element-level graph has empty variables).
            let var_level_nodes: Vec<Ident<Canonical>> = representative
                .iter()
                .map(|n| Ident::new(strip_subscript(n)))
                .collect();
            // A2A loop: every link is a same-element diagonal access. The
            // loop-score equation references the canonical
            // `{from}->{to}` link score (the Bare-shape name) via the
            // variable-level link names that `circuit_to_links` produces.
            let links = var_graph.circuit_to_links(&var_level_nodes);
            let var_stocks = var_graph.find_stocks_in_loop(&var_level_nodes);
            let polarity = var_graph.calculate_polarity(&links);

            // Determine the shared dimension(s) from the subscripts.
            // Look at the first subscripted node to find which dimensions
            // it carries, then map canonical dim names to original
            // datamodel names for equation parsing.
            let first_var_name = strip_subscript(representative[0]);
            let dimensions = source_vars
                .get(first_var_name)
                .map(|sv| {
                    variable_dimensions(db, *sv, project)
                        .iter()
                        .map(|d| {
                            let canonical = d.name();
                            dm_dims
                                .iter()
                                .find(|dm| {
                                    crate::common::canonicalize(dm.name()).as_ref() == canonical
                                })
                                .map(|dm| dm.name().to_string())
                                .unwrap_or_else(|| canonical.to_string())
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            // Stocks must be element-level over the dimension space so
            // `partition_for_loop` can resolve a partition per slot (the
            // `Loop` docstring's invariant -- the same rule the cross-element
            // and mixed branches below already follow).
            let stocks = build_a2a_loop_stocks(&var_stocks, &dimensions, dm_dims);

            all_loops.push(Loop {
                id: String::new(),
                links,
                stocks,
                polarity,
                dimensions,
            });
        } else if is_cross_element || representative_has_synthetic_agg {
            // Cross-element circuits: a circuit that genuinely visits
            // different elements at different points -- e.g.
            //   population[nyc] -> migration_pressure[boston] ->
            //   migration_in[nyc] -> population[nyc]
            // (a per-element-equation hop that reads the *other* element),
            // or the wildcard-reducer pattern
            //   pop[nyc] -> total[boston] -> update[boston] ->
            //   pop[boston] -> total[nyc] -> update[nyc] -> pop[nyc].
            //
            // Also: an elementary circuit through a synthetic aggregate node
            // visited once (`matrix[a,x] -> $⁚ltm⁚agg⁚0[a] -> growth[a,x] ->
            // matrix[a,x]` -- the "self-aggregate" feedback of a sliced
            // reducer) lands here even when its leading subscript elements all
            // agree; the loop score still has to reference the per-element
            // agg-half link scores, which only exist as literal-element scalar
            // variables.
            //
            // Each circuit becomes its own scalar Loop (the loop-score
            // *variable* is scalar: a cross-element loop visits fixed
            // elements, it is not parameterized by a free dimension) whose
            // `Link`s carry the element subscripts so the loop-score
            // equation references the per-element link scores along the
            // element-level path: `"$⁚ltm⁚link_score⁚{from}→{to}"[e]` for
            // an A2A (dimensioned) link score visited at element `e`. See
            // `ltm_augment::generate_loop_score_equation` for how the
            // subscript and the link-score-name resolution interact.
            //
            // (Pre-#503-fix this branch instead found the "shortest unique
            // cycle" in the *stripped* node sequence and emitted a single
            // scalar Loop referencing the *diagonal* A2A link scores via
            // `circuit_to_links` -- which scored a cross-element loop as if
            // its hops were same-element diagonal sensitivities. The
            // diagonal collapse and the unique-cycle stripping are gone.)
            for circuit in &circuits_in_group {
                let element_nodes: &[&str] = circuit;
                let var_level_nodes: Vec<Ident<Canonical>> = element_nodes
                    .iter()
                    .map(|n| Ident::new(strip_subscript(n)))
                    .collect();
                let var_links = var_graph.circuit_to_links(&var_level_nodes);

                let links = build_element_subscripted_links(
                    element_nodes,
                    &var_links,
                    source_vars,
                    db,
                    project,
                );

                // Stocks must be element-level so `partition_for_loop`
                // can resolve them in `model_element_cycle_partitions::
                // stock_partition` (which is keyed element-level). The
                // `Loop` docstring's stocks-granularity invariant says any
                // loop with `dimensions.is_empty()` MUST carry element-
                // level stock names. Collect every element-level stock node
                // in the circuit (a 6-node cross-element loop can traverse
                // the same stock variable at multiple elements).
                let stocks: Vec<Ident<Canonical>> = element_nodes
                    .iter()
                    .filter(|n| var_graph.stocks.contains(&Ident::new(strip_subscript(n))))
                    .map(|n| Ident::new(n))
                    .collect();

                let polarity = var_graph.calculate_polarity(&var_links);

                all_loops.push(Loop {
                    id: String::new(),
                    links,
                    stocks,
                    polarity,
                    dimensions: vec![], // scalar: cross-element loops visit fixed elements
                });
            }
        } else {
            // Mixed or scalar group: each circuit becomes its own scalar loop.
            for circuit in circuits_in_group {
                // For mixed loops, we can still attempt polarity via the
                // variable-level graph. Strip subscripts and analyze.
                let var_level_nodes: Vec<Ident<Canonical>> = circuit
                    .iter()
                    .map(|n| Ident::new(strip_subscript(n)))
                    .collect();
                let var_links = var_graph.circuit_to_links(&var_level_nodes);
                let polarity = var_graph.calculate_polarity(&var_links);

                // Build links with names that match what the link-score
                // emission system produces, so loop-score equations
                // reference existing variables.
                //
                // Link-score emission generates names in three forms:
                //
                //   1. Cross-dimensional (arrayed-from, scalar-to):
                //      `try_cross_dimensional_link_scores` emits
                //      "$⁚ltm⁚link_score⁚{from}[{elem}]→{to}" per source
                //      element. Element-level circuit nodes encode this as
                //      "{from}[{elem}]" for the source and "{to}" for the
                //      (scalar) target. We keep the bracketed `from` and
                //      bare `to` so `resolve_link_score_name_for_loop`
                //      matches that name.
                //
                //   2. Scalar-source -> arrayed-target:
                //      `try_scalar_to_arrayed_link_scores` emits
                //      "$⁚ltm⁚link_score⁚{from}→{to}[{elem}]" per target
                //      element. Element-level circuit nodes encode this as
                //      "{from}" for the (scalar) source and "{to}[{elem}]"
                //      for the target -- so we KEEP the `to` subscript
                //      whenever the target is arrayed (Phase 2's "keep
                //      `to[e]` when the link score is dimensioned" rule,
                //      extended to the per-target-element scalar var case).
                //      `generate_loop_score_equation` then references the
                //      per-element scalar variable directly.
                //
                //   3. Same-element A2A: `emit_per_shape_link_scores`
                //      emits "$⁚ltm⁚link_score⁚{from}→{to}" with
                //      `dimensions = [target_dims]`. We keep `to[e]` (case
                //      2's rule, since the target is arrayed) and strip the
                //      `from` subscript; `generate_loop_score_equation`
                //      then subscripts the dimensioned link score at the
                //      visited element. Scalar->scalar edges keep neither.
                let element_nodes: Vec<&str> = circuit.iter().map(|n| n.as_ref()).collect();

                let mut links = Vec::with_capacity(element_nodes.len());
                for (i, _) in element_nodes.iter().enumerate() {
                    let from_raw = element_nodes[i];
                    let to_raw = element_nodes[(i + 1) % element_nodes.len()];
                    // Use the polarity from the corresponding var-level link
                    let var_link_polarity = if i < var_links.len() {
                        var_links[i].polarity
                    } else {
                        crate::ltm::LinkPolarity::Unknown
                    };
                    let from_subscripted = from_raw.contains('[');
                    let to_subscripted = to_raw.contains('[');
                    let from_var_level = strip_subscript(from_raw);
                    let to_var_level = strip_subscript(to_raw);
                    let to_is_arrayed = source_vars
                        .get(to_var_level)
                        .map(|sv| {
                            sv.kind(db) != SourceVariableKind::Module
                                && !variable_dimensions(db, *sv, project).is_empty()
                        })
                        .unwrap_or(false);
                    let (link_from, link_to) = if from_subscripted && !to_subscripted {
                        // Cross-dimensional (full reduce, arrayed-from /
                        // scalar-to): keep element-level from, bare to.
                        (from_raw, to_raw)
                    } else if from_subscripted
                        && to_subscripted
                        && is_partial_reduce_edge(
                            db,
                            source_vars,
                            from_var_level,
                            to_var_level,
                            project,
                        )
                    {
                        // Partial reduce (`matrix[d1,d2] → row_sum[d1]`): the
                        // link score is the per-`(reduced-elem, result-elem)`
                        // scalar var `$⁚ltm⁚link_score⁚{from}[d1,d2]→{to}[d1]`
                        // from `try_cross_dimensional_link_scores`, so keep
                        // BOTH subscripts -- the source carries the collapsed
                        // axis too.
                        (from_raw, to_raw)
                    } else if to_subscripted && to_is_arrayed {
                        // Scalar->arrayed or same-element A2A: keep `to[e]`,
                        // strip any `from` subscript.
                        (strip_subscript(from_raw), to_raw)
                    } else {
                        // Scalar->scalar (or an arrayed target reduced to a
                        // scalar node that lost its subscript): variable-level.
                        (strip_subscript(from_raw), to_var_level)
                    };
                    // `Link.from` keeps any bracket it carries; the
                    // downstream resolver maps a bracketed `from` to a
                    // FixedIndex/cross-dimensional name and an unbracketed
                    // one to the Bare / per-target-element form.
                    links.push(crate::ltm::Link {
                        from: Ident::new(link_from),
                        to: Ident::new(link_to),
                        polarity: var_link_polarity,
                    });
                }

                // Find stocks among element-level nodes. We check the
                // variable-level stock set by stripping subscripts from
                // the circuit's element-level names, but preserve the
                // element-level form in the result so partition_for_loop
                // (which uses element-level keys from
                // model_element_cycle_partitions) can resolve them.
                let stocks: Vec<Ident<Canonical>> = element_nodes
                    .iter()
                    .filter(|n| {
                        let var_name = strip_subscript(n);
                        var_graph.stocks.contains(&Ident::new(var_name))
                    })
                    .map(|n| Ident::new(n))
                    .collect();

                // Each arrayed circuit node that is a stock must appear in
                // `stocks` with its subscript intact. Stripping the subscript
                // here would break partition_for_loop: model_element_cycle_
                // partitions keys stock_partition on element-level names
                // (e.g. "pop[nyc]"), not variable-level names (e.g. "pop").
                debug_assert!(
                    element_nodes
                        .iter()
                        .filter(|n| n.contains('[') && {
                            let var_name = strip_subscript(n);
                            var_graph.stocks.contains(&Ident::new(var_name))
                        })
                        .all(|n| stocks.iter().any(|s| s.as_str() == *n)),
                    "mixed/scalar branch: arrayed stock node lost its subscript; \
                     element_nodes={element_nodes:?} stocks={stocks:?}"
                );

                all_loops.push(Loop {
                    id: String::new(),
                    links,
                    stocks,
                    polarity,
                    dimensions: vec![],
                });
            }
        }
    }

    // Recover cross-element loops that traverse a synthetic aggregate node
    // more than once (Phase 5, GH #515). Johnson enumerates only *elementary*
    // circuits, so a loop like `pop[nyc] → agg → share[boston] → ... →
    // pop[boston] → agg → share[nyc] → ...` -- which visits `agg` twice -- is
    // never emitted. But each agg-touching elementary circuit contributes one
    // "petal" `agg → ... → agg`; stitching `k ≥ 2` petals of the same agg
    // whose internal nodes are pairwise disjoint, in some cyclic order,
    // reconstructs exactly those non-elementary loops -- bounded by
    // `agg_loop_budget` (when it clips, the clipped aggs' names are returned
    // and the caller accumulates a `Warning` naming them).
    let (recovered, truncated_aggs) = recover_cross_agg_loops(
        &circuit_strs,
        var_graph,
        source_vars,
        db,
        project,
        agg_loop_budget,
    );
    all_loops.extend(recovered);

    assign_loop_ids(&mut all_loops);
    (all_loops, truncated_aggs)
}

/// Hard cap on the number of non-elementary cross-element-through-aggregate
/// loops `recover_cross_agg_loops` materializes for a model (summed across
/// all synthetic aggregate nodes), Phase 5 / GH #515.
///
/// With `k` disjoint petals through one agg the recoverable loop count is
/// `Σ_{m=2}^{k} C(k,m)·orderings(m)` where `orderings(m)` is 1 for m=2 and
/// `(m-1)!/2` for m≥3 -- it grows super-exponentially, so a budget is
/// mandatory. When recovery hits this budget it stops, sets the truncation
/// flag, and the caller emits a `Warning`; the deterministic petal priority
/// (fewest internal nodes first, then a stable joined-name tiebreaker)
/// makes *which* loops survive truncation reproducible. The prior implicit
/// ceiling was `2^8 - 8 - 1 = 247` per agg (the old `MAX_AGG_PETALS = 8`
/// hard drop); 256 keeps roughly that order of magnitude as a model-wide
/// total. Because the budget is modest, recovery never reaches a subset
/// large enough for `cyclic_orderings(m)` to blow up in practice.
pub(crate) const MAX_CROSS_AGG_LOOPS: usize = 256;

/// Soft per-aggregate cap on the petals considered when stitching them into
/// loops. After sorting an agg's petals by the deterministic priority, only
/// the smallest `MAX_AGG_PETALS` are considered (the rest are dropped and
/// the truncation flag is set). This bounds the `2^k` subset enumeration to
/// `2^MAX_AGG_PETALS` regardless of how dense the agg's element-graph
/// neighborhood is. A model with more than this many *disjoint* petals
/// through one agg is at or near the `MAX_LTM_SCC_NODES` auto-flip
/// threshold anyway (each petal contributes ≥2 distinct nodes plus the
/// shared agg to one SCC), so this is a conservative belt-and-suspenders;
/// 8 matches the pre-#515 hard cap.
const MAX_AGG_PETALS: usize = 8;

#[cfg(test)]
thread_local! {
    /// Test-only override of [`MAX_CROSS_AGG_LOOPS`], scoped by an active
    /// [`AggLoopBudgetGuard`]. Lets a test trip the loop budget with a tiny
    /// fixture instead of building one large enough to trip the production
    /// constant (per docs/dev/rust.md#test-time-budgets, the PR #461
    /// cautionary tale).
    static AGG_LOOP_BUDGET_OVERRIDE: std::cell::Cell<Option<usize>> =
        const { std::cell::Cell::new(None) };
}

/// The cross-element-through-aggregate loop-count budget for the current
/// `model_ltm_variables` invocation.  Returns [`MAX_CROSS_AGG_LOOPS`] in
/// production builds; in `#[cfg(test)]` builds an active
/// [`AggLoopBudgetGuard`] override takes precedence.
fn cross_agg_loop_budget() -> usize {
    #[cfg(test)]
    {
        if let Some(b) = AGG_LOOP_BUDGET_OVERRIDE.with(|c| c.get()) {
            return b;
        }
    }
    MAX_CROSS_AGG_LOOPS
}

/// RAII guard (test-only) that overrides [`cross_agg_loop_budget`] for the
/// current thread for the guard's lifetime, restoring the previous value on
/// drop -- so a panicking test does not leak the override to the next test
/// reusing the thread.
///
/// Because `model_ltm_variables` is salsa-memoized, the guard must outlive
/// every `model_ltm_variables` call in the test whose budget it controls
/// (a later call on the same `db` would otherwise return the memoized
/// tiny-budget result regardless of the override state).
#[cfg(test)]
pub(crate) struct AggLoopBudgetGuard {
    prev: Option<usize>,
}

#[cfg(test)]
impl AggLoopBudgetGuard {
    pub(crate) fn new(budget: usize) -> Self {
        let prev = AGG_LOOP_BUDGET_OVERRIDE.with(|c| c.replace(Some(budget)));
        Self { prev }
    }
}

#[cfg(test)]
impl Drop for AggLoopBudgetGuard {
    fn drop(&mut self) {
        AGG_LOOP_BUDGET_OVERRIDE.with(|c| c.set(self.prev));
    }
}

/// Distinct orderings of `[0, 1, ..., n-1]` modulo rotation (index `0`
/// pinned first to kill rotations) and modulo mirror reversal (the loop
/// score is a commutative product over the edge multiset, so a directed
/// cycle and its reverse share a score; the design enumerates only one of
/// each mirror pair). Count: `1` for `n ∈ {0, 1, 2}`, `(n-1)!/2` for
/// `n ≥ 3` (`(n-1)!` rotation classes, halved by the mirror involution --
/// which has no fixed point for `n ≥ 3` since a length-`(n-1)` permutation
/// of distinct indices is never a palindrome; for `n = 2` reversing the
/// 2-cycle gives the same sequence, so there is nothing to quotient).
///
/// "Mirror" here is the petal-sequence reversed with the first petal still
/// pinned -- `[0, p1, .., p_{n-1}] ↦ [0, p_{n-1}, .., p1]` -- so the rule
/// is: enumerate the permutations of `[1, .., n-1]` via Heap's algorithm
/// (deterministic order, which `assign_loop_ids`' stable sort relies on for
/// stable distinct loop ids), and keep a permutation `tail` iff it is
/// lexicographically `<= reverse(tail)` (equivalently: track the emitted
/// canonicals in a set -- the lex test is the cheaper involution).
///
/// Pure: depends only on `n`. Only called from `recover_cross_agg_loops`
/// with `n` = a disjoint petal-subset size (≥ 2, ≤ `MAX_AGG_PETALS`), and
/// the loop budget stops recovery long before it reaches a subset large
/// enough for `(n-1)!/2` to be a concern.
fn cyclic_orderings(n: usize) -> Vec<Vec<usize>> {
    if n <= 1 {
        return vec![(0..n).collect()];
    }
    // Generate every permutation of the tail `[1, .., n-1]` via Heap's
    // algorithm, in its canonical order.
    let mut tail: Vec<usize> = (1..n).collect();
    let mut perms: Vec<Vec<usize>> = Vec::new();
    heaps_permutations(tail.len(), &mut tail, &mut perms);

    let mut orderings: Vec<Vec<usize>> = Vec::with_capacity(perms.len().div_ceil(2));
    for perm in perms {
        // Skip a permutation whose mirror (the tail reversed, with 0 still
        // pinned) sorts strictly before it -- we keep one of each mirror pair.
        let rev: Vec<usize> = perm.iter().rev().copied().collect();
        if perm > rev {
            continue;
        }
        let mut ordering = Vec::with_capacity(n);
        ordering.push(0);
        ordering.extend(perm);
        orderings.push(ordering);
    }
    orderings
}

/// Recursive Heap's algorithm: append every permutation of `a[0..k]` (with
/// `a[k..]` held fixed) to `out`, in Heap's canonical order. Called with
/// `k == a.len()`.
fn heaps_permutations(k: usize, a: &mut [usize], out: &mut Vec<Vec<usize>>) {
    if k <= 1 {
        out.push(a.to_vec());
        return;
    }
    for i in 0..k {
        heaps_permutations(k - 1, a, out);
        if k.is_multiple_of(2) {
            a.swap(i, k - 1);
        } else {
            a.swap(0, k - 1);
        }
    }
}

/// One agg petal: the element-level node sequence rotated to start at the
/// aggregate node (`[A, x_1, ..., x_m]`), plus its internal node set
/// `{x_1..x_m}`. `A` is the *element-level* agg node -- for an arrayed
/// synthetic agg that means the subscripted `$⁚ltm⁚agg⁚{n}[<elem>]` (so two
/// petals through `agg[a]` vs `agg[b]` go through *different* nodes and are
/// correctly never combined; `is_synthetic_agg_name` recognizes both the
/// bare and the subscripted form via its prefix check).
struct AggPetal<'a> {
    /// `[agg, x_1, ..., x_m]` -- the agg followed by the m internal nodes.
    nodes: Vec<&'a str>,
    internal: std::collections::HashSet<&'a str>,
}

/// Reconstruct the cross-element loops that traverse a synthetic aggregate
/// node more than once, from the element-level circuit list, under a
/// loop-count budget.
///
/// For each synthetic agg `A`, an elementary circuit that visits `A` exactly
/// once is its "petal": rotated to start at `A`, the node sequence
/// `[A, x_1, ..., x_m]` (the `x_i` are the petal's *internal* nodes -- the
/// rest of the cycle). Two petals are disjoint when their internal node sets
/// don't overlap. For a pairwise-disjoint subset of `m ≥ 2` petals of `A`,
/// the recovered loop's element-level node sequence concatenates the petals'
/// `nodes` (each of which starts with `A`) in some order:
/// `[A, p1_x..., A, p2_x..., ...]` -- `build_element_subscripted_links`
/// builds `seq[i] → seq[(i+1) % n]`, so this is exactly the cyclic sequence
/// `... → A → p1_x... → A → p2_x... → ... → A` (the last internal node wraps
/// to the first petal's `A`). Links / polarities come from
/// `build_element_subscripted_links`; the loop polarity is the product of
/// the link polarities (Unknown anywhere → Undetermined; the synthetic-agg
/// hops are Unknown here and patched later by `recover_agg_hop_polarities`).
///
/// Enumeration is bounded: an agg's petals are sorted by a deterministic
/// priority (fewest internal nodes first -- smaller petals combine into more
/// loops -- then a stable joined-`nodes` tiebreaker), the smallest
/// `MAX_AGG_PETALS` are kept (clipping flags that agg as truncated), the
/// disjoint subsets are walked smallest-cardinality-first (so under
/// truncation the few-petal -- likely most interesting -- loops survive), and
/// a running count is checked against `agg_loop_budget` (a global budget
/// across all aggs; once hit, recovery stops). The deterministic agg/petal
/// order is what makes the *truncated* loop set reproducible rather than
/// HashMap-iteration-order dependent.
///
/// Returns the recovered loops and the (sorted, deduped) names of the
/// aggregate nodes whose enumeration was clipped -- empty iff nothing was
/// clipped. An agg `A` is reported as truncated when either (a) the soft
/// per-agg petal cap dropped ≥ 1 of its petals, or (b) the global loop-count
/// budget fired at any point while `A` was being enumerated *or before `A`
/// would have been reached* (the per-agg loop walks aggs in sorted order, so
/// when the budget fires at agg `X` every agg sorted after `X` -- that has
/// ≥ 2 petals and so could have contributed loops -- is also un-enumerated
/// and thus reported). This is conservative in the same direction as the
/// petal-cap flag (see below): an agg whose disjoint-petal subsets would all
/// have collapsed to zero loops anyway is still flagged if the budget never
/// reached it -- over-reporting incompleteness is the safe direction, and
/// distinguishing it would require running the very enumeration the budget
/// exists to skip.
fn recover_cross_agg_loops(
    circuit_strs: &[Vec<&str>],
    var_graph: &crate::ltm::CausalGraph,
    source_vars: &HashMap<String, super::SourceVariable>,
    db: &dyn Db,
    project: SourceProject,
    agg_loop_budget: usize,
) -> (Vec<crate::ltm::Loop>, Vec<String>) {
    use crate::common::{Canonical, Ident};
    use crate::ltm::{LinkPolarity, Loop, LoopPolarity};

    // agg name -> its (deduped) petals.
    let mut petals_by_agg: HashMap<&str, Vec<AggPetal>> = HashMap::new();
    for circuit in circuit_strs {
        // Synthetic agg nodes in this circuit (bare or subscripted).
        let aggs: Vec<&str> = circuit
            .iter()
            .copied()
            .filter(|n| crate::ltm_agg::is_synthetic_agg_name(n))
            .collect();
        // Only build petals from a circuit that touches exactly one agg, once
        // (Johnson emits simple cycles, so "one agg in the node list" already
        // means "visited once"; a circuit through two distinct agg nodes --
        // `agg[a]` and `agg[b]` -- is a complete loop on its own, not a petal).
        if aggs.len() != 1 {
            continue;
        }
        let agg = aggs[0];
        let Some(pos) = circuit.iter().position(|n| *n == agg) else {
            continue;
        };
        let n = circuit.len();
        // Rotate so the agg is first; the rest is the petal's internal nodes.
        let nodes: Vec<&str> = (0..n).map(|j| circuit[(pos + j) % n]).collect();
        let internal: std::collections::HashSet<&str> = nodes[1..].iter().copied().collect();
        let entry = petals_by_agg.entry(agg).or_default();
        // Dedup on the internal set (Johnson can emit rotations of the same
        // simple cycle in some graphs; the internal set is rotation-invariant).
        if entry.iter().any(|p| p.internal == internal) {
            continue;
        }
        entry.push(AggPetal { nodes, internal });
    }

    let mut recovered: Vec<Loop> = Vec::new();
    // Names of aggs whose enumeration was clipped (by the soft petal cap or by
    // the global budget firing during/before them). A BTreeSet so the result
    // is deterministic-sorted and deduped regardless of insertion order.
    let mut truncated_aggs: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut emitted: usize = 0;
    // Deterministic agg iteration order so the budget clips reproducibly.
    let mut aggs: Vec<&str> = petals_by_agg.keys().copied().collect();
    aggs.sort();
    // If the global budget fires mid-enumeration, every agg sorted after the
    // one we were on is un-enumerated; this records that position so they can
    // be folded into `truncated_aggs` afterward.
    let mut budget_clip_idx: Option<usize> = None;
    'outer: for (agg_idx, agg) in aggs.iter().enumerate() {
        let mut petals: Vec<&AggPetal> = petals_by_agg[*agg].iter().collect();
        if petals.len() < 2 {
            continue;
        }
        // Deterministic priority: fewest internal nodes first (smaller petals
        // combine into more loops), then a stable tiebreaker (the petal's
        // node sequence joined). After dedup-on-internal-set the petals have
        // distinct `internal` sets and hence distinct `nodes` sequences, so
        // this key is a total order.
        petals.sort_by_cached_key(|p| (p.internal.len(), p.nodes.join("\u{0}")));
        if petals.len() > MAX_AGG_PETALS {
            // Drop the larger petals; the recovered set for this agg is now
            // incomplete, so report it. Conservatively flags truncation
            // whenever the soft petal cap drops >= 1 petal, even if those
            // petals would have contributed no disjoint-pair loop (every
            // dropped petal's internal nodes overlap a kept petal) --
            // over-reporting incompleteness is the safe direction, and
            // checking would mean running the disjoint-subset enumeration the
            // cap exists to bound.
            petals.truncate(MAX_AGG_PETALS);
            truncated_aggs.insert((*agg).to_string());
        }
        let k = petals.len();

        // Walk the 2^k subset masks smallest-cardinality-first (so the few-
        // petal loops -- which survive a tight budget -- come out first), and
        // within each cardinality in increasing mask order (which, given the
        // priority sort above, visits the smallest-internal petals first).
        // `k ≤ MAX_AGG_PETALS` keeps `1u32 << k` small.
        let mut masks: Vec<u32> = (0u32..(1u32 << k)).collect();
        masks.sort_by_key(|&m| (m.count_ones(), m));
        for mask in masks {
            if mask.count_ones() < 2 {
                continue;
            }
            let chosen: Vec<usize> = (0..k).filter(|&i| (mask >> i) & 1 == 1).collect();
            // Pairwise-disjoint internal node sets.
            let mut union: std::collections::HashSet<&str> = std::collections::HashSet::new();
            if chosen
                .iter()
                .any(|&i| !petals[i].internal.iter().all(|n| union.insert(n)))
            {
                continue;
            }
            // Each distinct cyclic ordering of the chosen petals is its own
            // directed cycle (a different edge *sequence*; per #308 that is a
            // distinct loop) -- but all of them share the same edge *multiset*
            // (every petal always contributes its same `A → first_internal →
            // ... → last_internal → A` edges, since the sequence is cyclic),
            // so they share a `loop_score`. For m = 2 there is exactly one
            // ordering, equal to the pre-#515 per-subset output.
            let m = chosen.len();
            for ord in cyclic_orderings(m) {
                let seq: Vec<&str> = ord
                    .iter()
                    .flat_map(|&j| petals[chosen[j]].nodes.iter().copied())
                    .collect();
                let var_level_nodes: Vec<Ident<Canonical>> =
                    seq.iter().map(|n| Ident::new(strip_subscript(n))).collect();
                let var_links = var_graph.circuit_to_links(&var_level_nodes);
                let links =
                    build_element_subscripted_links(&seq, &var_links, source_vars, db, project);
                let stocks: Vec<Ident<Canonical>> = seq
                    .iter()
                    .filter(|n| var_graph.stocks.contains(&Ident::new(strip_subscript(n))))
                    .map(|n| Ident::new(n))
                    .collect();
                let polarity = if links.iter().any(|l| l.polarity == LinkPolarity::Unknown) {
                    LoopPolarity::Undetermined
                } else {
                    let neg = links
                        .iter()
                        .filter(|l| l.polarity == LinkPolarity::Negative)
                        .count();
                    if neg % 2 == 0 {
                        LoopPolarity::Reinforcing
                    } else {
                        LoopPolarity::Balancing
                    }
                };
                recovered.push(Loop {
                    id: String::new(),
                    links,
                    stocks,
                    polarity,
                    // Cross-element loops visit fixed elements -- scalar loop score.
                    dimensions: vec![],
                });
                emitted += 1;
                if emitted >= agg_loop_budget {
                    // This agg's enumeration was clipped; the ones sorted
                    // after it never ran (handled below).
                    truncated_aggs.insert((*agg).to_string());
                    budget_clip_idx = Some(agg_idx);
                    break 'outer;
                }
            }
        }
    }

    // The global budget clipped mid-enumeration: every agg sorted after the
    // clip point is un-enumerated. Report those that had >= 2 petals (and so
    // could have contributed loops); an agg with < 2 petals would have been
    // skipped regardless of budget, so it is not "truncated".
    if let Some(clip_idx) = budget_clip_idx {
        for agg in &aggs[clip_idx + 1..] {
            if petals_by_agg[*agg].len() >= 2 {
                truncated_aggs.insert((*agg).to_string());
            }
        }
    }

    (recovered, truncated_aggs.into_iter().collect())
}

/// Recover the polarity of synthetic-aggregate-node hops in `loops` (GH #516).
///
/// The loop builders derive every link's polarity from the *variable-level*
/// causal graph, but synthetic `$⁚ltm⁚agg⁚{n}` nodes exist only in the
/// *element* graph -- so `analyze_link_polarity` finds no reference to the
/// agg's name in either endpoint's equation and a hop into or out of an agg
/// comes back `Unknown`, forcing every agg-traversing loop to `Undetermined`.
/// For the common (monotone) reducers the polarity is derivable, so patch it
/// here:
///
/// - `source[d] → agg`: `SUM`/`MEAN`/`MIN`/`MAX` are monotone non-decreasing
///   in each source element (raising any one element raises-or-holds the
///   result), so the hop is `Positive`. `STDDEV`/`RANK` are not monotone --
///   left `Unknown`.
/// - `agg → consumer`: the polarity of `consumer`'s equation with respect to
///   the reducer subexpression, computed by substituting the reducer with the
///   agg name and running ordinary static polarity analysis
///   (`CausalGraph::agg_consumer_polarity`).
///
/// Any loop whose links change is re-classified via `calculate_polarity`.
/// If anything was patched, loop IDs are re-assigned (the `r`/`b`/`u` prefix
/// is polarity-derived).
fn recover_agg_hop_polarities(
    loops: &mut [crate::ltm::Loop],
    var_graph: &crate::ltm::CausalGraph,
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) {
    use crate::common::{Canonical, Ident};
    use crate::ltm::LinkPolarity;

    let aggs = crate::ltm_agg::enumerate_agg_nodes(db, model, project);
    // Canonicalize each synthetic agg's name once so it compares directly
    // against the (canonical) `Link.from` / `Link.to` idents.
    let synthetic: Vec<(Ident<Canonical>, &crate::ltm_agg::AggNode)> = aggs
        .aggs
        .iter()
        .filter(|a| a.is_synthetic)
        .map(|a| (Ident::new(a.name.as_str()), a))
        .collect();
    if synthetic.is_empty() {
        return;
    }

    let mut any_patched = false;
    for lp in loops.iter_mut() {
        let mut patched = false;
        for link in lp.links.iter_mut() {
            if link.polarity != LinkPolarity::Unknown {
                continue;
            }
            // `source[d] → agg`: the agg is this link's target.
            if let Some((_, agg)) = synthetic.iter().find(|(name, _)| name == &link.to) {
                if crate::ltm_agg::agg_reducer_is_monotone(&agg.equation_text) {
                    link.polarity = LinkPolarity::Positive;
                    patched = true;
                }
                continue;
            }
            // `agg → consumer`: the agg is this link's source.
            if let Some((agg_ident, agg)) = synthetic.iter().find(|(name, _)| name == &link.from) {
                let consumer = Ident::new(strip_subscript(link.to.as_str()));
                let p = var_graph.agg_consumer_polarity(&consumer, &agg.equation_text, agg_ident);
                if p != LinkPolarity::Unknown {
                    link.polarity = p;
                    patched = true;
                }
            }
        }
        if patched {
            lp.polarity = var_graph.calculate_polarity(&lp.links);
            any_patched = true;
        }
    }

    if any_patched {
        crate::ltm::assign_loop_ids(loops);
    }
}

/// Unified LTM variable generation for any model (root or sub-model).
///
/// Auto-detects sub-model behavior by checking for input ports with causal
/// pathways to output. Sub-models and discovery mode generate link scores
/// for ALL edges; exhaustive mode on root models generates link scores only
/// for edges in detected loops, plus loop/relative loop scores.
///
/// Pathway and composite scores are generated for models with input ports.
/// Module-containing loops are no longer filtered out because
/// `link_score_equation_text` now handles module links via composite refs.
#[salsa::tracked(returns(ref))]
pub fn model_ltm_variables(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> super::LtmVariablesResult {
    use crate::common::Ident;
    use crate::ltm::{CyclePartitions, Loop};
    use salsa::Accumulator;
    use std::collections::HashSet;

    use super::{
        CompilationDiagnostic, Diagnostic, DiagnosticError, DiagnosticSeverity, LtmLinkId,
        LtmSyntheticVar, LtmVariablesResult, causal_graph_from_edges, causal_graph_with_modules,
        generate_max_abs_chain_str, model_causal_edges, model_element_cycle_partitions,
        model_loop_circuits_tiered, module_input_pathways_from_edges,
    };

    let edges_result = model_causal_edges(db, model, project);
    if edges_result.stocks.is_empty() {
        return LtmVariablesResult {
            vars: vec![],
            loop_partitions: HashMap::new(),
            agg_recovery_truncated: false,
        };
    }

    // When the user explicitly requested discovery mode, honor it
    // directly. Otherwise auto-flip if either:
    //   1. The variable-level SCC exceeds `MAX_LTM_SCC_NODES`, or
    //   2. The slow-path (cross-element / mixed) element-level
    //      subgraph's SCC exceeds `MAX_LTM_SCC_NODES` after the tiered
    //      enumerator classifies cycles.
    //
    // Gating on the *variable-level* SCC (instead of the full
    // element-graph SCC) is the structural change that motivates this
    // PR: pure-A2A models with tens of variables over hundreds of
    // elements have a huge element-graph SCC but a small variable
    // SCC, and the new tiered enumerator produces no element-level
    // circuits for them. Today's gate fires anyway on the inflated
    // element-graph SCC, dropping these models into discovery mode
    // unnecessarily.
    //
    // The variable-level SCC check (Tarjan, O(V+E)) runs first so
    // models like WRLD3 (var SCC = 166) auto-flip without paying for
    // variable-level Johnson. Only models that pass that check pay
    // for the tiered enumerator, which is itself bounded: variable
    // Johnson sees at most `MAX_LTM_SCC_NODES` nodes, and slow-path
    // Johnson is skipped internally when the slow-path subgraph SCC
    // exceeds the threshold.
    //
    // Cliff A (~17 GB in legacy `build_element_level_loops` on
    // WRLD3-scale models) was tied to enumerating the full element
    // graph; the tiered path skips that entirely on pure-scalar /
    // pure-A2A models. Cliff B (rel_loop_score equation text) was
    // retired earlier when post-simulation normalization moved out of
    // the VM. See `docs/design-plans/2026-04-18-ltm-cap-lift-diagnosis.md`
    // and `docs/design-plans/2026-05-06-ltm-482-variable-level-loop-enumeration.md`.
    let is_discovery_user = project.ltm_discovery_mode(db);
    let var_scc_size = if is_discovery_user {
        0
    } else {
        let edges = model_causal_edges(db, model, project);
        causal_graph_from_edges(edges).largest_scc_size()
    };
    let var_auto_flipped = !is_discovery_user && var_scc_size > crate::ltm::MAX_LTM_SCC_NODES;

    if var_auto_flipped {
        let msg = format!(
            "LTM analysis auto-switched from exhaustive to discovery mode: \
             the variable-level causal graph's largest SCC has {} nodes, \
             exceeding MAX_LTM_SCC_NODES = {}.  Variable-level Johnson at \
             this scale would enumerate millions of circuits (see \
             docs/design-plans/2026-04-18-ltm-cap-lift-diagnosis.md and \
             docs/design-plans/2026-05-06-ltm-482-variable-level-loop-enumeration.md). \
             Per-loop scores are ranked post-simulation via the \
             strongest-path search; see \
             docs/design/ltm--loops-that-matter.md for the two-tier \
             strategy.",
            var_scc_size,
            crate::ltm::MAX_LTM_SCC_NODES,
        );
        CompilationDiagnostic(Diagnostic {
            model: model.name(db).clone(),
            variable: None,
            error: DiagnosticError::Assembly(msg),
            severity: DiagnosticSeverity::Warning,
        })
        .accumulate(db);
    }
    let mut is_discovery = is_discovery_user || var_auto_flipped;

    // Determine output ports for this model. Stdlib models always use
    // the "output" convention. For user-defined models, output ports are
    // determined by which internal variables are referenced from parent
    // models via module·var syntax.
    let model_name_str = model.name(db);
    let output_ports = if model_name_str.starts_with("stdlib\u{205A}") {
        vec![Ident::new("output")]
    } else {
        find_model_output_ports(db, model, project)
    };
    let pathways = if output_ports.is_empty() {
        HashMap::new()
    } else {
        module_input_pathways_from_edges(edges_result, &output_ports)
    };
    let has_input_ports = !pathways.is_empty();

    let mut vars = Vec::new();

    // Fetch source variables and dimension metadata early -- needed by
    // both loop pre-computation (for dimension lookups) and link score
    // classification.
    let source_vars = model.variables(db);
    let dm_dims = project_datamodel_dims(db, project);

    // Phase 5: emit the synthetic aggregate-node auxiliaries
    // (`$⁚ltm⁚agg⁚{n}`). Each is a plain computed aux whose equation is one
    // maximal inlined reducer subexpression; the simulation evaluates it
    // each timestep (so `PREVIOUS(agg)` is available) and the two
    // link-score halves (`source[d] → agg`, `agg → target`) reference it.
    // A whole-RHS reducer (`total_population = SUM(population[*])`) mints
    // *no* synthetic -- the variable already is the aggregate node. The agg
    // vars are pushed before any link-score var so they sort first (the LTM
    // flow fragments are appended to the runlist in `vars` order, and the
    // `agg → target` link score reads the agg's current-step value -- so the
    // agg fragment must execute first in the same timestep). The category
    // function in the final `vars.sort_by` keeps them ahead of link scores.
    let agg_nodes = crate::ltm_agg::enumerate_agg_nodes(db, model, project);
    for agg in &agg_nodes.aggs {
        if !agg.is_synthetic {
            continue;
        }
        // The equation text is the canonical reducer subexpression. A
        // whole-extent or pinned-slice reducer (`SUM(pop[*])`,
        // `SUM(pop[NYC,*])`) has a scalar result; a partial-reduce slice over
        // an iterated dimension (`SUM(matrix[D1,*])` inside an A2A-over-`D1`
        // body) has `result_dims = [D1]` -- in an A2A-over-`D1` body
        // `matrix[D1,*]` is exactly "the `D1`-th row, all of axis 2", so this
        // evaluates correctly as the `Equation::ApplyToAll` body.
        let equation = if agg.result_dims.is_empty() {
            datamodel::Equation::Scalar(agg.equation_text.clone())
        } else {
            datamodel::Equation::ApplyToAll(agg.result_dims.clone(), agg.equation_text.clone())
        };
        vars.push(LtmSyntheticVar {
            name: agg.name.clone(),
            equation,
            dimensions: agg.result_dims.clone(),
            compile_directly: false,
        });
    }

    // Pre-compute loops for exhaustive mode using tiered loop
    // enumeration: variable-level Johnson runs first, and only the
    // cross-element / mixed slice descends into element-level Johnson
    // on the slow-path subgraph. Pure-A2A and pure-scalar cycles emit
    // a single Loop directly (one per variable-level cycle, with
    // `dimensions` set on the A2A case).
    //
    // See `docs/design-plans/2026-05-06-ltm-482-variable-level-loop-enumeration.md`
    // for the architecture; the legacy element-graph Johnson path is
    // still available via `model_element_loop_circuits` for callers
    // outside this function.
    //
    // We also need the variable-level graph for polarity analysis
    // (`build_loops_from_tiered` calls `var_graph.calculate_polarity`
    // and `var_graph.find_stocks_in_loop`); the element-level graph
    // produced by `model_element_causal_edges` carries no variable
    // data.
    //
    // `agg_recovery_truncated` rides on the result so a model author
    // knows the cross-agg loop list is incomplete even if the `Warning`
    // never reaches them (#466); set inside the loop-building branch below.
    let mut agg_recovery_truncated = false;
    let loops: Option<Vec<Loop>> = if !is_discovery {
        let tiered = model_loop_circuits_tiered(db, model, project);
        // Late-stage auto-flip: the variable-level SCC was small enough
        // to clear the early gate, but the cross-element / mixed
        // subgraph (the slow path) blew past the threshold. The tiered
        // enumerator already skipped Johnson on the slow path in that
        // case (`slow_path` is empty); we just need to flip the
        // is_discovery flag so link-score generation, etc. follow the
        // discovery path.
        if tiered.slow_path_largest_scc > crate::ltm::MAX_LTM_SCC_NODES {
            let msg = format!(
                "LTM analysis auto-switched from exhaustive to discovery mode: \
                 the cross-element / mixed slow-path subgraph's largest SCC has {} nodes, \
                 exceeding MAX_LTM_SCC_NODES = {}.  Per-loop scores are ranked \
                 post-simulation via the strongest-path search; see \
                 docs/design/ltm--loops-that-matter.md for the two-tier strategy.",
                tiered.slow_path_largest_scc,
                crate::ltm::MAX_LTM_SCC_NODES,
            );
            CompilationDiagnostic(Diagnostic {
                model: model.name(db).clone(),
                variable: None,
                error: DiagnosticError::Assembly(msg),
                severity: DiagnosticSeverity::Warning,
            })
            .accumulate(db);
            is_discovery = true;
            None
        } else if tiered.fast_path.is_empty() && tiered.slow_path.is_empty() {
            if !has_input_ports {
                return LtmVariablesResult {
                    vars: vec![],
                    loop_partitions: HashMap::new(),
                    agg_recovery_truncated: false,
                };
            }
            None
        } else {
            // Bind the budget once: `build_loops_from_tiered` enforces it and
            // the `Warning` text below reports it, and a `#[cfg(test)]`
            // override could in principle change between two reads.
            let cross_agg_budget = cross_agg_loop_budget();
            let var_graph = causal_graph_with_modules(db, model, project);
            let (mut detected, truncated_aggs) = build_loops_from_tiered(
                tiered,
                &var_graph,
                source_vars,
                db,
                project,
                dm_dims,
                cross_agg_budget,
            );
            // Surface a truncated cross-element-through-aggregate loop
            // recovery the same way the auto-flip-to-discovery gate surfaces
            // its mode change: a `Warning` (the human channel) plus the
            // robust `agg_recovery_truncated` flag on the result. The loop
            // list is incomplete -- the budget clipped which disjoint-petal
            // combinations were materialized. `truncated_aggs` names exactly
            // the aggregate nodes whose enumeration was clipped (sorted), so
            // the message points the author at the affected reducers rather
            // than every synthetic agg in the model.
            if !truncated_aggs.is_empty() {
                let msg = format!(
                    "LTM cross-element-through-aggregate loop recovery was truncated: a \
                     reducer in a feedback loop produces more disjoint-petal combinations \
                     than the loop budget ({}); the recovered loop list is incomplete. \
                     Affected aggregate node(s): {}.",
                    cross_agg_budget,
                    truncated_aggs.join(", "),
                );
                CompilationDiagnostic(Diagnostic {
                    model: model.name(db).clone(),
                    variable: None,
                    error: DiagnosticError::Assembly(msg),
                    severity: DiagnosticSeverity::Warning,
                })
                .accumulate(db);
            }
            agg_recovery_truncated = !truncated_aggs.is_empty();
            // GH #516: hops into/out of synthetic `$⁚ltm⁚agg⁚{n}` nodes come
            // back Unknown-polarity from the loop builders (the variable-level
            // graph has no agg node); recover the derivable cases here so
            // agg-traversing loops aren't all forced to Undetermined.
            recover_agg_hop_polarities(&mut detected, &var_graph, db, model, project);
            Some(detected)
        }
    } else {
        None
    };

    let mut loop_partitions: HashMap<String, Vec<Option<usize>>> = HashMap::new();

    // Part 1: Link scores.
    // Sub-models and discovery mode need scores for ALL edges (pathways
    // reference arbitrary edges). Exhaustive root models only need
    // scores for edges that participate in loops.
    //
    // For each link score, classify the edge to determine whether the
    // score should be arrayed (A2A). When the target variable has
    // dimensions and either the source is scalar or shares the same
    // dimensions, the link score inherits the target's dimensions so
    // that per-element scores are computed via the A2A expansion.

    /// Determine the dimensions a link score should carry.
    ///
    /// Returns the target's dimension names when the edge is
    /// same-dimension A2A or scalar-to-arrayed. Returns empty for
    /// scalar edges, module-involved links (modules are scalar nodes),
    /// and arrayed-to-scalar edges (cross-dimensional; handled by
    /// `try_cross_dimensional_link_scores` which generates N separate
    /// scalar variables).
    ///
    /// The returned names use the original datamodel casing (e.g.,
    /// "Region" not "region") because `parse_ltm_equation` feeds them
    /// into `Equation::ApplyToAll`, which `get_dimensions` resolves by
    /// exact string match against the project's datamodel dimensions.
    fn link_score_dimensions(
        db: &dyn Db,
        source_vars: &HashMap<String, super::SourceVariable>,
        from: &str,
        to: &str,
        project: SourceProject,
        dm_dims: &[crate::datamodel::Dimension],
    ) -> Vec<String> {
        let to_sv = match source_vars.get(to) {
            Some(sv) => sv,
            // Implicit variables (SMOOTH/DELAY expansions) may not be
            // in source_vars; treat as scalar.
            None => return vec![],
        };
        // Module variables are scalar nodes in the causal graph.
        if to_sv.kind(db) == SourceVariableKind::Module {
            return vec![];
        }
        let to_dims = variable_dimensions(db, *to_sv, project);
        if to_dims.is_empty() {
            return vec![];
        }

        let from_dims = source_vars
            .get(from)
            .filter(|sv| sv.kind(db) != SourceVariableKind::Module)
            .map(|sv| variable_dimensions(db, *sv, project).clone())
            .unwrap_or_default();

        // Scalar source -> arrayed target: NOT handled here. The main
        // link-score loop routes these to `try_scalar_to_arrayed_link_scores`
        // (one scalar link score per target element) before
        // `emit_per_shape_link_scores` is reached. Returning empty here is
        // the safe fallback if that routing is ever bypassed (e.g. the
        // target failed to lower): a scalar Bare link score
        // (`{from}→{to}`, no dims) parses to the useless-but-harmless edge
        // `(from, to)`, whereas a Bare-A2A var would make
        // `expand_a2a_link_offsets` invent a phantom `from[elem]` node that
        // breaks loops through `from` in the search graph.
        if from_dims.is_empty() {
            return vec![];
        }

        // Same-dimension A2A: both have identical dimension(s).
        // Partial-collapse: source has more dimensions than target, but all
        //   target dimensions are present in the source (e.g., source[D1,D2]
        //   -> target[D1]). The link score gets the target's (shared) dims.
        //
        // NOTE: When from_dims == to_dims and the dependency is CrossElement
        // (e.g., `share[R] = population[R] / SUM(population[*])`), this
        // creates only N diagonal link scores (one per element). Off-diagonal
        // link scores (e.g., population[boston] -> share[nyc]) are not
        // generated; the wildcard reducer already aggregates the
        // cross-element contribution into the diagonal A2A link score, and
        // build_element_level_loops uses those diagonal values when
        // emitting scalar Loops for the surviving cross-element circuits.
        //
        // Check whether this edge should use the target's dimensions for
        // the link score. This covers:
        // - Same-dimension A2A: from_dims == to_dims
        // - Partial-collapse: to_dims ⊆ from_dims (e.g., [D1,D2]→[D1])
        // - Broadcast: from_dims ⊆ to_dims (e.g., [D1]→[D1,D2])
        //
        // In all these cases, the link score inherits the target's
        // dimensions so per-element values are computed via A2A expansion.
        let dims_compatible = from_dims == *to_dims
            || to_dims
                .iter()
                .all(|td| from_dims.iter().any(|fd| fd.name() == td.name()))
            || from_dims
                .iter()
                .all(|fd| to_dims.iter().any(|td| td.name() == fd.name()));

        if dims_compatible {
            // Map canonical dimension names back to their original
            // datamodel names for correct equation parsing.
            to_dims
                .iter()
                .map(|d| {
                    let canonical = d.name();
                    dm_dims
                        .iter()
                        .find(|dm| crate::common::canonicalize(dm.name()).as_ref() == canonical)
                        .map(|dm| dm.name().to_string())
                        .unwrap_or_else(|| canonical.to_string())
                })
                .collect()
        } else {
            // Cross-dimensional (arrayed-to-scalar, or mismatched
            // dimensions). These edges are handled by
            // try_cross_dimensional_link_scores which generates N
            // separate scalar variables instead of one arrayed variable.
            // Return empty here so the normal A2A path is skipped.
            vec![]
        }
    }

    /// Generate per-element link score variables for a reducer edge, or
    /// return `None` if the edge is not a reduce.
    ///
    /// Two shapes are handled:
    ///   * **Full reduce** -- an arrayed source feeds a *scalar* target
    ///     through an array-reducing builtin (`total = SUM(pop[*])`). Each
    ///     source element gets its own scalar link score
    ///     `$⁚ltm⁚link_score⁚pop[e]→total` measuring how much varying that
    ///     single element affects the scalar target while holding all other
    ///     elements at `PREVIOUS`.
    ///   * **Partial reduce** -- an arrayed source feeds an *arrayed-result*
    ///     reducer whose dims are a strict subset of the source's dims
    ///     (`agg[D1] = SUM(matrix[D1,*])` collapses only `D2`). For each
    ///     `(d1, d2)` pair the relevant target is only `agg[d1]`, so the
    ///     link score is the per-`(d1, d2)` scalar variable
    ///     `$⁚ltm⁚link_score⁚matrix[d1,d2]→agg[d1]` (both axes ride in the
    ///     source subscript; only the surviving axis in the target
    ///     subscript). The ceteris-paribus partial holds the rest of the
    ///     `matrix[d1,*]` slice at `PREVIOUS`. All emitted vars are scalar
    ///     (`dimensions = vec![]`), consistent with the full-reduce naming
    ///     -- `parse_link_offsets` keeps element-level-on-both-sides names
    ///     as a single passthrough edge, so no parser change is needed.
    ///
    /// Returns `None` for scalar-to-scalar, same-dimension A2A, broadcast
    /// (`from_dims ⊆ to_dims`), mismatched dimensions, module-involved
    /// edges, and any edge where the reducer cannot be classified. Returns
    /// `Some(vec![])` for SIZE edges (constant reducer, no scores).
    fn try_cross_dimensional_link_scores(
        db: &dyn Db,
        source_vars: &HashMap<String, super::SourceVariable>,
        from: &str,
        to: &str,
        model: SourceModel,
        project: SourceProject,
    ) -> Option<Vec<LtmSyntheticVar>> {
        // Only applies when the source is arrayed.
        let from_sv = source_vars.get(from)?;
        if from_sv.kind(db) == SourceVariableKind::Module {
            return None;
        }
        let from_dims = variable_dimensions(db, *from_sv, project);
        if from_dims.is_empty() {
            return None;
        }

        let to_sv = source_vars.get(to)?;
        if to_sv.kind(db) == SourceVariableKind::Module {
            return None;
        }
        let to_dims = variable_dimensions(db, *to_sv, project);

        // Determine whether this edge is a full reduce (scalar target) or a
        // partial reduce (arrayed result over a strict subset of the
        // source's axes). The "result axis" names are the target's dims for
        // a partial reduce (empty for a full reduce); the implied reduced
        // axes are `from_dims` minus the result axes -- we never need the
        // reduced-axis names explicitly because the co-reduced source slice
        // is derived directly from the source element tuples.
        let result_axis_names: Vec<String> = if to_dims.is_empty() {
            vec![]
        } else {
            // Partial reduce requires every target dim to be a source dim
            // and strictly fewer target dims than source dims (so at least
            // one axis collapses). Same-dim A2A, broadcast, and mismatched
            // dims all fall through to `None` (handled by other paths).
            let from_names: Vec<&str> = from_dims.iter().map(|d| d.name()).collect();
            let to_names: Vec<&str> = to_dims.iter().map(|d| d.name()).collect();
            if to_names.len() >= from_names.len()
                || !to_names.iter().all(|tn| from_names.contains(tn))
            {
                return None;
            }
            to_names.iter().map(|s| s.to_string()).collect()
        };

        // The source is a reducer argument. Classify the reducing function
        // in the target's equation.
        let to_var = super::reconstruct_single_variable(db, model, project, to)?;
        let (reducer_kind, reducer_name, is_bare) =
            crate::ltm_augment::classify_reducer(&to_var, from)?;

        if reducer_kind == crate::ltm_augment::ReducerKind::Constant {
            // SIZE is constant; link score is always 0. Skip entirely.
            return Some(vec![]);
        }

        // Compute the cartesian product of all source dimensions to get
        // per-element subscripts. For a single dimension, this is just the
        // element names. For multi-dimensional sources (e.g., x[Region,Age]),
        // this produces tuples like "nyc,adult", "nyc,child", etc.
        let dim_element_lists: Vec<Vec<String>> = from_dims
            .iter()
            .map(crate::ltm_augment::dimension_element_names)
            .collect();
        let source_elements = cartesian_subscripts(&dim_element_lists);

        if result_axis_names.is_empty() {
            // Full reduce: one scalar link score per source element.
            let mut cross_vars = Vec::with_capacity(source_elements.len());
            for element in &source_elements {
                let var_name = format!(
                    "$\u{205A}ltm\u{205A}link_score\u{205A}{}[{}]\u{2192}{}",
                    from, element, to
                );
                let equation = crate::ltm_augment::generate_element_to_scalar_equation(
                    from,
                    to,
                    element,
                    &source_elements,
                    &reducer_kind,
                    reducer_name,
                    is_bare,
                );
                cross_vars.push(LtmSyntheticVar {
                    name: var_name,
                    equation: datamodel::Equation::Scalar(equation),
                    dimensions: vec![], // scalar -- one variable per element
                    // bracketed name -> routed direct by `assemble_module`'s
                    // element-subscript check; the flag is irrelevant here.
                    compile_directly: false,
                });
            }
            return Some(cross_vars);
        }

        // Partial reduce: project each source element tuple onto the
        // surviving axes (in target-dim order) to get the result element,
        // then group source elements by result element so each group is the
        // `matrix[d1,*]` slice the reducer combines for that row. The MEAN
        // divisor and the nonlinear expansion both operate over that slice
        // only (other rows are irrelevant to `agg[d1]`).
        let from_pos: HashMap<&str, usize> = from_dims
            .iter()
            .enumerate()
            .map(|(i, d)| (d.name(), i))
            .collect();
        // For each surviving target dim, the index into a split source
        // element tuple where that dim's element name lives. Built from the
        // membership check above, so every name resolves.
        let result_positions: Vec<usize> = result_axis_names
            .iter()
            .map(|n| from_pos[n.as_str()])
            .collect();
        let project_to_result = |source_elem: &str| -> String {
            let parts: Vec<&str> = source_elem.split(',').collect();
            result_positions
                .iter()
                .map(|&p| parts[p])
                .collect::<Vec<_>>()
                .join(",")
        };

        // result_element -> the source element tuples that share it, in
        // row-major source order (deterministic).
        let mut slices: HashMap<String, Vec<String>> = HashMap::new();
        for se in &source_elements {
            slices
                .entry(project_to_result(se))
                .or_default()
                .push(se.clone());
        }

        let mut cross_vars = Vec::with_capacity(source_elements.len());
        for source_elem in &source_elements {
            let result_elem = project_to_result(source_elem);
            let coreduced = &slices[&result_elem];
            let var_name = format!(
                "$\u{205A}ltm\u{205A}link_score\u{205A}{}[{}]\u{2192}{}[{}]",
                from, source_elem, to, result_elem
            );
            let equation = crate::ltm_augment::generate_element_to_reduced_equation(
                from,
                to,
                source_elem,
                &result_elem,
                coreduced,
                &reducer_kind,
                reducer_name,
                is_bare,
            );
            cross_vars.push(LtmSyntheticVar {
                name: var_name,
                equation: datamodel::Equation::Scalar(equation),
                dimensions: vec![], // scalar -- one variable per (reduced-elem, result-elem)
                // bracketed name -> routed direct by `assemble_module`.
                compile_directly: false,
            });
        }
        Some(cross_vars)
    }

    /// Generate per-target-element link score variables for a
    /// scalar-source -> arrayed-target edge, or return `None` if the edge
    /// is not of that shape.
    ///
    /// The mirror of [`try_cross_dimensional_link_scores`]: where that one
    /// fires for (arrayed source, scalar target) reducers and emits one
    /// scalar `LtmSyntheticVar` per *source* element named
    /// `$⁚ltm⁚link_score⁚{from}[{elem}]→{to}`, this one fires for (scalar
    /// source, arrayed target) edges and emits one scalar `LtmSyntheticVar`
    /// per *target* element named `$⁚ltm⁚link_score⁚{from}→{to}[{elem}]`,
    /// `dimensions: vec![]`.
    ///
    /// Why not a single Bare-A2A var with `dimensions = [target_dims]`:
    /// that form is undiscoverable. `parse_link_offsets`'s
    /// `expand_a2a_link_offsets` subscripts *both* `from` and `to` over
    /// `target_dims`, inventing a `from[elem]` node -- but `from` is scalar,
    /// so the invented node doesn't match the unsubscripted `from` node
    /// that other edges (e.g. an arrayed->scalar reducer feeding `from`)
    /// produce, and a loop through `from` is unreachable in the search
    /// graph. The per-target-element scalar name parses via the `[`-in-`to`
    /// single-passthrough branch to the edge `(from, to[elem])` with no
    /// parser change, and `generate_loop_score_equation` references the
    /// per-element scalar variable directly.
    ///
    /// The per-element equation is the partial of `to[elem]`'s equation
    /// w.r.t. `from` live (everything else PREVIOUS), wrapped in the
    /// standard link-score guard form with the target reference pinned to
    /// `elem`. For an `Equation::ApplyToAll` target the body is the same
    /// for every element (with the element pinned on the `to` side and on
    /// the target's arrayed deps); for an `Equation::Arrayed` target it is
    /// that element's own slot expression (which already carries explicit
    /// subscripts). See [`crate::ltm_augment::generate_scalar_to_element_equation`].
    ///
    /// Returns `None` for scalar-to-scalar, A2A (same-dimension),
    /// arrayed-to-scalar, module-involved, and any edge where the target
    /// has no usable AST (so per-element equations can't be derived) -- in
    /// those cases the caller falls back to its existing emission path.
    fn try_scalar_to_arrayed_link_scores(
        db: &dyn Db,
        source_vars: &HashMap<String, super::SourceVariable>,
        from: &str,
        to: &str,
        model: SourceModel,
        project: SourceProject,
    ) -> Option<Vec<LtmSyntheticVar>> {
        // Source must be a scalar, non-module variable.
        let from_sv = source_vars.get(from)?;
        if from_sv.kind(db) == SourceVariableKind::Module {
            return None;
        }
        if !variable_dimensions(db, *from_sv, project).is_empty() {
            return None;
        }

        // Target must be an arrayed, non-module variable.
        let to_sv = source_vars.get(to)?;
        if to_sv.kind(db) == SourceVariableKind::Module {
            return None;
        }
        let to_dims = variable_dimensions(db, *to_sv, project).clone();
        if to_dims.is_empty() {
            return None;
        }

        let to_var = super::reconstruct_single_variable(db, model, project, to)?;
        // Without a lowered AST we can't derive per-element equations.
        // Decline and let the caller's existing path handle the (degenerate)
        // failed-to-lower target.
        let ast = to_var.ast()?;

        // The per-element equation text and dependency-set source differ
        // by AST variant:
        //   - ApplyToAll: one shared body; deps from the whole AST.
        //   - Arrayed:    per-element slot text (or the default slot);
        //                 deps from that slot's expression.
        // In both cases dependency classification is given the target's
        // AST dimensions so explicit element-name subscripts (e.g. `[NYC]`)
        // are recognized as dimension references, not variables.
        use crate::ast::Ast;
        let target_ast_dims: &[crate::dimensions::Dimension] = match ast {
            Ast::Scalar(_) => &[],
            Ast::ApplyToAll(dims, _) | Ast::Arrayed(dims, _, _, _) => dims,
        };

        // Which target deps must be pinned to the element in the per-element
        // scalar equation: the arrayed deps that share a dimension with the
        // target. (Scalar deps stay bare; the target self-reference is
        // pinned implicitly via the subscripted `to[elem]` in the guard
        // form built by `generate_scalar_to_element_equation`.)
        let deps_to_subscript = |deps: &HashSet<Ident<Canonical>>| -> HashSet<Ident<Canonical>> {
            deps.iter()
                .filter(|d| {
                    source_vars
                        .get(d.as_str())
                        .filter(|sv| sv.kind(db) != SourceVariableKind::Module)
                        .map(|sv| {
                            let dd = variable_dimensions(db, *sv, project);
                            !dd.is_empty()
                                && dd
                                    .iter()
                                    .any(|x| to_dims.iter().any(|td| td.name() == x.name()))
                        })
                        .unwrap_or(false)
                })
                .cloned()
                .collect()
        };

        let dim_element_lists: Vec<Vec<String>> = to_dims
            .iter()
            .map(crate::ltm_augment::dimension_element_names)
            .collect();
        let elements = cartesian_subscripts(&dim_element_lists);

        // Build one `LtmSyntheticVar` for `element` from its equation text and
        // that text's dependency set. The element name is the only part of the
        // generated equation/name that varies between elements.
        let build_var = |element: &str,
                         elem_text: &str,
                         elem_deps: &HashSet<Ident<Canonical>>,
                         deps_to_sub: &HashSet<Ident<Canonical>>|
         -> LtmSyntheticVar {
            let equation = if elem_text.is_empty() {
                "0".to_string()
            } else {
                crate::ltm_augment::generate_scalar_to_element_equation(
                    from,
                    to,
                    element,
                    elem_text,
                    elem_deps,
                    deps_to_sub,
                    // A true scalar source: the bare `quote_ident(from)`
                    // denominator is correct.
                    None,
                )
            };
            LtmSyntheticVar {
                name: format!(
                    "$\u{205A}ltm\u{205A}link_score\u{205A}{}\u{2192}{}[{}]",
                    from, to, element
                ),
                equation: datamodel::Equation::Scalar(equation),
                dimensions: vec![], // scalar -- one variable per target element
                // bracketed name -> routed direct by `assemble_module`.
                compile_directly: false,
            }
        };

        let mut cross_vars = Vec::with_capacity(elements.len());
        match ast {
            // ApplyToAll: one shared body for every element, so its text, its
            // dependency set, and the subset to element-pin are all
            // element-invariant -- compute them once, outside the loop.
            Ast::ApplyToAll(_, expr) => {
                let elem_text = crate::patch::expr2_to_string(expr);
                let elem_deps = crate::variable::identifier_set(ast, target_ast_dims, None);
                let deps_to_sub = deps_to_subscript(&elem_deps);
                for element in &elements {
                    cross_vars.push(build_var(element, &elem_text, &elem_deps, &deps_to_sub));
                }
            }
            // Arrayed: each element has its own slot expression (or the default
            // slot), so the body and its dependency set genuinely differ per
            // element and must be recomputed inside the loop.
            Ast::Arrayed(_, per_elem, default_expr, _) => {
                for element in &elements {
                    let canonical_elem = crate::common::CanonicalElementName::from_raw(element);
                    let slot = per_elem.get(&canonical_elem).or(default_expr.as_ref());
                    let (elem_text, elem_deps): (String, HashSet<Ident<Canonical>>) = match slot {
                        Some(expr) => (
                            crate::patch::expr2_to_string(expr),
                            crate::variable::identifier_set(
                                &Ast::Scalar(expr.clone()),
                                target_ast_dims,
                                None,
                            ),
                        ),
                        // No slot and no default: the target has a hole at
                        // this element. A zero equation is the right
                        // link-score value (no sensitivity), matching the
                        // historical placeholder behaviour for un-derivable
                        // partials.
                        None => (String::new(), HashSet::new()),
                    };
                    let deps_to_sub = deps_to_subscript(&elem_deps);
                    cross_vars.push(build_var(element, &elem_text, &elem_deps, &deps_to_sub));
                }
            }
            Ast::Scalar(_) => unreachable!("target is arrayed"),
        }
        Some(cross_vars)
    }

    /// Accumulate the AC3.4 `Warning` for a disjoint-dim arrayed -> arrayed
    /// edge that is not statically scoreable (the target's per-element
    /// equations reference the source via a dynamic index, so which target
    /// slots depend on which source elements can't be decided at compile
    /// time). The edge gets *no* link-score variable -- a missing link score
    /// is graceful (loop/path scores referencing it get the zero-contribution
    /// stub-dep fallback) and far less misleading than the scalarized stand-in
    /// the pre-#510 path produced.
    fn emit_unscoreable_disjoint_edge_warning(
        db: &dyn Db,
        model: SourceModel,
        from: &str,
        to: &str,
    ) {
        use salsa::Accumulator;
        let msg = format!(
            "LTM link score for edge {from} -> {to} could not be computed: {to} is a \
             per-element-equation arrayed variable whose equations reference {from} via a \
             dynamic index, which is not statically scoreable; this edge will have no \
             link-score variable"
        );
        CompilationDiagnostic(Diagnostic {
            model: model.name(db).clone(),
            variable: None,
            error: DiagnosticError::Assembly(msg),
            severity: DiagnosticSeverity::Warning,
        })
        .accumulate(db);
    }

    /// Emit per-distinct-source-element link scores for a disjoint-dim
    /// arrayed -> arrayed edge whose target is a per-element-equation
    /// (`Ast::Arrayed`) variable (GH #510), or return `None` if the edge is
    /// not of that shape.
    ///
    /// The case: `from` is arrayed, `to` is arrayed with an `Ast::Arrayed`
    /// AST, `from`'s dims and `to`'s dims share *no* dimension name (so the
    /// same-dim A2A / partial-collapse / broadcast / partial-reduce paths all
    /// declined), and `to`'s per-element equations reference `from` only via
    /// literal element subscripts (`source[m]`) of a dimension disjoint from
    /// `to`'s dims. For each *distinct* referenced source element `m` we emit
    /// one `LtmSyntheticVar` named `$⁚ltm⁚link_score⁚{from}[{m}]→{to}`, an
    /// `Equation::Arrayed` over `to`'s dims whose partial holds `from[m]` live
    /// in the slots whose equation references `from[m]` and is the trivial-zero
    /// guard form (`from[m]` frozen at `PREVIOUS`) elsewhere -- exactly what
    /// `build_arrayed_link_score_equation` produces for a `FixedIndex` source
    /// into an `Ast::Arrayed` target (reached here via the salsa-cached
    /// `link_score_equation_text_shaped`).
    ///
    /// Returns:
    ///  - `Some(vec)` with one var per distinct referenced source element when
    ///    every reference is a literal element subscript;
    ///  - `Some(vec![])` when the target references `from` via a non-literal
    ///    index (a `DynamicIndex` site -- or, defensively, a `Wildcard`/`Bare`
    ///    site that can't be a literal-element reference into a disjoint-dim
    ///    target): a `Warning` is accumulated and no link score is emitted, and
    ///    the caller must *not* fall through to `emit_per_shape_link_scores`;
    ///  - `None` when the edge isn't an arrayed -> disjoint-dim-`Ast::Arrayed`
    ///    edge at all -- the caller's existing emission path handles it.
    ///
    /// We reuse the Phase-1 reference-site IR (`model_ltm_reference_sites`)
    /// for `(from, to)` rather than re-walking the target's AST: each site's
    /// `shape` is `FixedIndex(elems)` for `from[m]`, and `target_element`
    /// records which `to` slot it sits in (unused here -- the per-slot partial
    /// re-derives that from the slot equation).
    fn try_disjoint_dim_arrayed_link_scores(
        db: &dyn Db,
        source_vars: &HashMap<String, super::SourceVariable>,
        from: &str,
        to: &str,
        model: SourceModel,
        project: SourceProject,
    ) -> Option<Vec<LtmSyntheticVar>> {
        // Both ends must be arrayed, non-module variables.
        let from_sv = source_vars.get(from)?;
        if from_sv.kind(db) == SourceVariableKind::Module {
            return None;
        }
        let from_dims = variable_dimensions(db, *from_sv, project);
        if from_dims.is_empty() {
            return None;
        }
        let to_sv = source_vars.get(to)?;
        if to_sv.kind(db) == SourceVariableKind::Module {
            return None;
        }
        let to_dims = variable_dimensions(db, *to_sv, project);
        if to_dims.is_empty() {
            return None;
        }
        // Disjoint: no shared dimension name. (Same-dim A2A, partial-collapse,
        // broadcast, and partial-reduce edges share at least one dim and are
        // handled by `link_score_dimensions` / `try_cross_dimensional_link_scores`.)
        let shares_a_dim = from_dims
            .iter()
            .any(|fd| to_dims.iter().any(|td| td.name() == fd.name()));
        if shares_a_dim {
            return None;
        }
        // The target must be a per-element-equation (`Ast::Arrayed`) variable.
        // An `Ast::ApplyToAll` target referencing a disjoint-dim source would
        // be a dimension error (a D3-shaped value can't broadcast onto D1xD2);
        // a scalar source into an arrayed target is `try_scalar_to_arrayed`'s job.
        let to_var = super::reconstruct_single_variable(db, model, project, to)?;
        if !matches!(to_var.ast(), Some(crate::ast::Ast::Arrayed(..))) {
            return None;
        }
        // Consult the reference-site IR. A `FixedIndex(elems)` site is a
        // `from[m]` reference; anything else (a dynamic index, or -- defensively
        // -- a `Wildcard`/`Bare`, which can't be a valid literal-element
        // reference into a disjoint-dim target) makes the edge unscoreable.
        let ir = crate::db_ltm_ir::model_ltm_reference_sites(db, model, project);
        let sites = ir.sites.get(&(from.to_string(), to.to_string()))?;
        if sites.is_empty() {
            return None;
        }
        // Distinct referenced source elements, in first-occurrence order.
        let mut elem_keys: Vec<String> = Vec::new();
        for site in sites {
            match &site.shape {
                RefShape::FixedIndex(es) => {
                    // The comma-joined form is the `[m]` (or `[m,k]`) the var
                    // name carries and `shape_aware_source_ref` renders.
                    let key = es.join(",");
                    if !elem_keys.contains(&key) {
                        elem_keys.push(key);
                    }
                }
                RefShape::Bare | RefShape::Wildcard | RefShape::DynamicIndex => {
                    emit_unscoreable_disjoint_edge_warning(db, model, from, to);
                    return Some(vec![]);
                }
            }
        }

        // One link-score variable per distinct referenced source element. The
        // equation comes from the salsa-cached shaped path, which routes the
        // `Ast::Arrayed` target through `build_arrayed_link_score_equation`
        // (an `Equation::Arrayed` over `to`'s dims, already tagged with `to`'s
        // datamodel dim names -- so `dimensions` mirrors them and the
        // `retarget` is a no-op). The `[m]` in the name routes the var
        // directly in `assemble_module`'s bracket check, so `compile_directly`
        // is irrelevant but set `false` for consistency with the other
        // bracketed cross-dimensional vars.
        let mut vars = Vec::with_capacity(elem_keys.len());
        for key in &elem_keys {
            let shape = RefShape::FixedIndex(key.split(',').map(|s| s.to_string()).collect());
            let link_id = LtmLinkId::new(db, from.to_string(), to.to_string());
            if let Some(mut lsv) =
                link_score_equation_text_shaped(db, link_id, shape.clone(), model, project).clone()
            {
                // `lsv.name` is already `link_score_var_name(from, to, &shape)`
                // from the shaped path -- no need to re-derive it here.
                let dims = ltm_equation_dimensions(&lsv.equation).to_vec();
                lsv.dimensions = dims.clone();
                lsv.equation = retarget_ltm_equation_dims(lsv.equation, &dims);
                lsv.compile_directly = false;
                vars.push(lsv);
            }
        }
        Some(vars)
    }

    /// Emit per-shape link scores for a single (from, to) edge.
    ///
    /// The emission is shape-driven (Phase 3): one `LtmSyntheticVar` per
    /// `(from, to, shape)` tuple, named by `link_score_var_name`. The shape
    /// set comes from `model_ltm_reference_sites` (the distinct `shape`
    /// fields of `(from, to)`'s classified sites); module links and edges
    /// with no AST reference have no IR entry and fall back to a single Bare
    /// emission so the legacy behavior is preserved at structural boundaries.
    ///
    /// `fallback_shape` is the shape to use when the IR has no entry for the
    /// edge (e.g. a module edge, or an implicit synthesized reference that
    /// doesn't appear in the target's AST). Callers pass `RefShape::Bare` to
    /// preserve the legacy single-shape behavior.
    ///
    /// `skip_reducer_shapes` is set when the caller has already handled the
    /// `from` reference's reducer occurrences by routing them through an
    /// aggregate node (Phase 5) -- i.e. when the routed-agg set for `(from,
    /// to)` is non-empty. Only the `Wildcard` shape is suppressed in that
    /// case: a `Wildcard` reference to `from` in `to` is the hoisted
    /// reducer's argument (`SUM(pop[*])`, classified `ThroughAgg` in the
    /// IR), already scored by the `source → agg` / `agg → target` halves,
    /// so re-scoring it here would double-count. Every *other* shape is kept
    /// -- including `DynamicIndex` (a direct `pop[idx]` alongside a hoisted
    /// `SUM(pop[*])`, classified `Direct`) **and `Bare`** (a bare arrayed
    /// reducer arg like `SUM(pop)`, classified `ThroughAgg` for the element
    /// graph but still given its own `Bare`-named link score here -- this is
    /// exactly the pre-IR behavior, where `enumerate_shapes` returned every
    /// shape of `from` in `to` and `skip_reducer_shapes` dropped only
    /// `Wildcard`). Equivalently: feed every distinct site `shape` to the
    /// per-shape pass, removing `Wildcard` iff the routed-agg set is
    /// non-empty -- which is what the element-graph routing and the
    /// link-score routing "agree" on (the same `ClassifiedSite::routing`
    /// data), differing only in that the link scorer additionally keeps
    /// non-`Wildcard` shapes from `ThroughAgg` sites.
    ///
    /// `Wildcard`/`DynamicIndex` shapes that reach this function share the
    /// canonical `link_score_var_name` form with `Bare`, so we dedup by the
    /// resulting name and keep the first occurrence -- the AST walk records
    /// `Bare` before any subscripted reference, so the canonical-Bare link
    /// score wins the slot when both a bare and a subscripted reference exist.
    #[allow(clippy::too_many_arguments)] // helper threads through emission context
    fn emit_per_shape_link_scores(
        db: &dyn Db,
        source_vars: &HashMap<String, super::SourceVariable>,
        from: &str,
        to: &str,
        fallback_shape: RefShape,
        model: SourceModel,
        project: SourceProject,
        dm_dims: &[crate::datamodel::Dimension],
        skip_reducer_shapes: bool,
        vars: &mut Vec<LtmSyntheticVar>,
    ) {
        let ir = crate::db_ltm_ir::model_ltm_reference_sites(db, model, project);
        // The distinct `shape` fields of `(from, to)`'s classified sites,
        // in AST-walk order of first occurrence (equivalent to the per-edge
        // shape set the AST walker produced before the IR).
        let mut shapes: Vec<RefShape> = ir
            .sites
            .get(&(from.to_string(), to.to_string()))
            .map(|sites| {
                let mut v: Vec<RefShape> = Vec::new();
                for s in sites {
                    if !v.contains(&s.shape) {
                        v.push(s.shape.clone());
                    }
                }
                v
            })
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| vec![fallback_shape]);
        if skip_reducer_shapes {
            shapes.retain(|s| !matches!(s, RefShape::Wildcard));
        }
        // Distinct `RefShape`s can map to the same synthetic name (a
        // conservative-slice / direct-dynamic-index `Wildcard`/`DynamicIndex`
        // ref shares the canonical Bare name now that the per-shape suffix is
        // retired); keep only the first shape that produces each name.
        let mut emitted_names: HashSet<String> = HashSet::new();
        shapes.retain(|shape| {
            emitted_names.insert(crate::ltm_augment::link_score_var_name(from, to, shape))
        });

        let target_dims = link_score_dimensions(db, source_vars, from, to, project, dm_dims);

        for shape in shapes {
            let link_id = LtmLinkId::new(db, from.to_string(), to.to_string());
            if let Some(mut lsv) =
                link_score_equation_text_shaped(db, link_id, shape.clone(), model, project).clone()
            {
                // Set the canonical name and dimensions per Phase 3 Task 4/5.
                lsv.name = crate::ltm_augment::link_score_var_name(from, to, &shape);
                // Every shape takes the target's dimensions: for FixedIndex
                // each per-element link score is scalar when the target is
                // scalar and arrayed when the target is arrayed; Bare (and
                // the rare conservative-slice Wildcard/DynamicIndex) inherit
                // the target's dims via the same compatibility rule.
                // link_score_dimensions already implements this for every
                // case, so one assignment suffices.
                lsv.dimensions = target_dims.clone();
                // Keep the equation's dimensionality in lockstep with the
                // `dimensions` field that layout sizing keys off of. The
                // shaped fn returns a `Scalar`/`ApplyToAll` equation tagged
                // with the *target's own* dimension names, which equal
                // `target_dims` for every compatible-dimension edge; for the
                // (rare) incompatible-dimension arrayed-target edge,
                // link_score_dimensions returns empty, so we collapse the
                // equation to scalar here -- matching the pre-existing
                // behavior where such edges produced a scalar link score.
                lsv.equation = retarget_ltm_equation_dims(lsv.equation, &target_dims);
                // A non-`Bare` shape carries a partial that the (from, to)-
                // keyed salsa compilation path (`link_score_equation_text`,
                // always `RefShape::Bare`) cannot reproduce: a
                // `Wildcard`/`DynamicIndex` reference into a scalar target
                // would have its whole subscript wrapped in `PREVIOUS()` and
                // the ceteris-paribus numerator zeroed. Force `assemble_module`
                // to compile this var's equation verbatim. (For `Bare`, the
                // salsa-cached path is correct and keeps incrementality;
                // `FixedIndex` carries an element subscript on the name and is
                // already routed directly by the bracket check, but flagging
                // it too is harmless.)
                lsv.compile_directly = !matches!(shape, RefShape::Bare);
                vars.push(lsv);
            }
        }
    }

    /// Emit the `source[<read row>] → agg` link-score half: one scalar
    /// `$⁚ltm⁚link_score⁚{from}[<row>]→{agg}` (when the agg is scalar) or
    /// `$⁚ltm⁚link_score⁚{from}[<row>]→{agg}[<slot>]` (when the agg is arrayed
    /// over its `result_dims` -- the partial-reduce shape that mirrors
    /// `try_cross_dimensional_link_scores`'s `{from}[{d1,d2}]→{to}[{d1}]`) per
    /// source *read row* -- not every source element. For a whole-extent
    /// reducer that's all of `from`'s elements; for a sliced reducer
    /// (`SUM(pop[NYC,*])`, `read_slice = [Pinned(nyc), Reduced]`) it's only
    /// the rows the slice reads (here, the `pop[nyc,*]` slice), so the unread
    /// rows get *no* link score rather than a nonzero garbage one. Each row's
    /// `coreduced` set (the rows mapping to the same agg slot -- the
    /// `from`-slice combined for that slot) is the `all_elements` for the
    /// MEAN divisor / nonlinear expansion. The agg's *own* equation is exactly
    /// the reducer, so the "bare" algebraic shortcut applies.
    fn emit_source_to_agg_link_scores(
        db: &dyn Db,
        source_vars: &HashMap<String, super::SourceVariable>,
        from: &str,
        agg: &crate::ltm_agg::AggNode,
        model: SourceModel,
        project: SourceProject,
        vars: &mut Vec<LtmSyntheticVar>,
    ) {
        let Some(from_sv) = source_vars.get(from) else {
            return;
        };
        if from_sv.kind(db) == SourceVariableKind::Module {
            return;
        }
        let from_dims = variable_dimensions(db, *from_sv, project);
        if from_dims.is_empty() {
            return;
        }
        // Reconstruct a transient (parsed + lowered) `Variable` from the
        // agg's equation text so `classify_reducer` can read the reducer
        // kind/name. An arrayed agg (`result_dims` non-empty -- a sliced
        // reducer like `SUM(matrix[D1,*])` over an A2A-`D1` body) must be
        // reconstructed as an `ApplyToAll` over `result_dims`; treating
        // `matrix[d1,*]` as a scalar equation is a type error, so the lowered
        // reconstruction would fail and no per-read-row source link scores
        // would be emitted (silently zeroing the synthetic agg's loop score).
        // Mirrors the agg-aux emission above.
        let agg_eqn = if agg.result_dims.is_empty() {
            datamodel::Equation::Scalar(agg.equation_text.clone())
        } else {
            datamodel::Equation::ApplyToAll(agg.result_dims.clone(), agg.equation_text.clone())
        };
        let Some(agg_var) = reconstruct_ltm_var_lowered(db, &agg.name, &agg_eqn, model, project)
        else {
            return;
        };
        let Some((reducer_kind, reducer_name, _is_bare)) =
            crate::ltm_augment::classify_reducer(&agg_var, from)
        else {
            return;
        };
        if reducer_kind == crate::ltm_augment::ReducerKind::Constant {
            return;
        }
        let dim_element_lists: Vec<Vec<String>> = from_dims
            .iter()
            .map(crate::ltm_augment::dimension_element_names)
            .collect();
        // The read rows: only the slice the reducer reads. If `read_slice`
        // doesn't line up with `from`'s axes (shouldn't happen for a hoisted
        // agg whose `source_vars` contains `from`), fall back to all elements
        // / scalar agg.
        let rows: Vec<ReadSliceRow> = read_slice_rows(&agg.read_slice, &dim_element_lists)
            .unwrap_or_else(|| {
                let all = cartesian_subscripts(&dim_element_lists);
                all.iter()
                    .map(|r| ReadSliceRow {
                        row: r.clone(),
                        slot: String::new(),
                        coreduced: all.clone(),
                    })
                    .collect()
            });
        for ReadSliceRow {
            row,
            slot,
            coreduced,
        } in &rows
        {
            let (var_name, equation) = if slot.is_empty() {
                (
                    format!(
                        "$\u{205A}ltm\u{205A}link_score\u{205A}{}[{}]\u{2192}{}",
                        from, row, agg.name
                    ),
                    crate::ltm_augment::generate_element_to_scalar_equation(
                        from,
                        &agg.name,
                        row,
                        coreduced,
                        &reducer_kind,
                        reducer_name,
                        /* is_bare = */ true,
                    ),
                )
            } else {
                (
                    format!(
                        "$\u{205A}ltm\u{205A}link_score\u{205A}{}[{}]\u{2192}{}[{}]",
                        from, row, agg.name, slot
                    ),
                    crate::ltm_augment::generate_element_to_reduced_equation(
                        from,
                        &agg.name,
                        row,
                        slot,
                        coreduced,
                        &reducer_kind,
                        reducer_name,
                        /* is_bare = */ true,
                    ),
                )
            };
            vars.push(LtmSyntheticVar {
                name: var_name,
                equation: datamodel::Equation::Scalar(equation),
                dimensions: vec![],
                // bracketed name (+ subscripted synthetic agg) -> routed direct.
                compile_directly: false,
            });
        }
    }

    /// Emit the `agg → to` link-score half: the partial of `to`'s equation
    /// w.r.t. `agg` held live, with every hoisted reducer subexpression in
    /// `to` first substituted by its agg name (so `agg` appears live where
    /// `SUM(...)` was, and any other hoisted reducers end up as
    /// `PREVIOUS(agg_j)`). For an arrayed `to` this is one scalar
    /// `$⁚ltm⁚link_score⁚{agg}→{to}[{e}]` per target element (mirroring the
    /// scalar→arrayed convention from `try_scalar_to_arrayed_link_scores`);
    /// for a scalar `to` it is a single `$⁚ltm⁚link_score⁚{agg}→{to}`. When the
    /// agg is itself arrayed over its `result_dims` (a partial-reduce
    /// sub-expression like `x[D1] = ... + SUM(matrix[D1,*])`), the agg side of
    /// the name carries the target element's projection onto `result_dims`
    /// (`{agg}[{d1}]→{to}[{e}]`), and the agg reference in the per-slot
    /// equation is element-pinned to the same slot.
    #[allow(clippy::too_many_arguments)]
    fn emit_agg_to_target_link_scores(
        db: &dyn Db,
        source_vars: &HashMap<String, super::SourceVariable>,
        agg_nodes: &crate::ltm_agg::AggNodesResult,
        agg: &crate::ltm_agg::AggNode,
        to: &str,
        model: SourceModel,
        project: SourceProject,
        vars: &mut Vec<LtmSyntheticVar>,
    ) {
        let Some(to_var) = super::reconstruct_single_variable(db, model, project, to) else {
            return;
        };
        let Some(ast) = to_var.ast() else { return };

        // Map of canonical reducer text -> agg name for every synthetic agg
        // occurring in `to`'s equation.
        let reducer_subst: HashMap<String, String> = agg_nodes
            .aggs_in_var(to)
            .filter(|a| a.is_synthetic)
            .map(|a| (a.equation_text.clone(), a.name.clone()))
            .collect();

        let agg_canonical = Ident::<Canonical>::new(&agg.name);
        let agg_is_arrayed = !agg.result_dims.is_empty();

        // The set of arrayed deps that share `to`'s dimensions (need to be
        // element-pinned in the per-target-element scalar equation); scalar
        // deps and the agg names are not. Computed over the original target's
        // dims and dep set, extended with the agg name (harmless if it never
        // appears).
        let to_dims = source_vars
            .get(to)
            .map(|sv| variable_dimensions(db, *sv, project).clone())
            .unwrap_or_default();
        use crate::ast::Ast;
        let target_ast_dims: &[crate::dimensions::Dimension] = match ast {
            Ast::Scalar(_) => &[],
            Ast::ApplyToAll(dims, _) | Ast::Arrayed(dims, _, _, _) => dims,
        };
        let base_deps = crate::variable::identifier_set(ast, target_ast_dims, None);
        let mut all_deps = base_deps.clone();
        all_deps.insert(agg_canonical.clone());
        // When `to` hoists 2+ reducers (e.g. `x = SUM(a[*]) / SUM(b[*])`), the
        // substituted equation text references *every* agg name, not just the
        // one this link starts from. The other aggs must be in `all_deps` so
        // they get PREVIOUS-wrapped (ceteris paribus) -- otherwise they are
        // left live and the agg→target link score collapses to ±1.
        for other_agg in reducer_subst.values() {
            all_deps.insert(Ident::<Canonical>::new(other_agg));
        }
        let mut deps_to_subscript: HashSet<Ident<Canonical>> = all_deps
            .iter()
            .filter(|d| {
                source_vars
                    .get(d.as_str())
                    .filter(|sv| sv.kind(db) != SourceVariableKind::Module)
                    .map(|sv| {
                        let dd = variable_dimensions(db, *sv, project);
                        !dd.is_empty()
                            && dd
                                .iter()
                                .any(|x| to_dims.iter().any(|td| td.name() == x.name()))
                    })
                    .unwrap_or(false)
            })
            .cloned()
            .collect();
        // An arrayed agg (`result_dims` non-empty) is element-pinned in the
        // per-target-element equation just like an arrayed dep that shares
        // `to`'s dims: `$⁚ltm⁚agg⁚0` → `$⁚ltm⁚agg⁚0[<element>]`. This is
        // exact when `result_dims` equals `to`'s dimensions (the diagonal
        // `agg[d1] → to[d1]` case -- a partial-reduce sub-expression in an
        // A2A target over exactly that dim, e.g. `x[D1] = ... + SUM(matrix[D1,*])`);
        // for the rarer broadcast case (`agg[D1]` into `to[D1,D2]`) the
        // element tuple over-subscripts the agg, which is a known imprecision.
        if agg_is_arrayed {
            deps_to_subscript.insert(agg_canonical.clone());
        }

        // Positions of the agg's `result_dims` within `to`'s dimensions, so a
        // target element tuple can be projected onto the agg-slot subscript
        // for the link-score name's agg side.
        let result_dim_positions: Vec<usize> = agg
            .result_dims
            .iter()
            .filter_map(|rd| {
                let canon = crate::common::canonicalize(rd);
                to_dims.iter().position(|td| td.name() == canon.as_ref())
            })
            .collect();
        // The target element projected onto the agg's `result_dims` (the
        // agg-slot subscript), or `None` when the agg is scalar / the
        // projection is empty.
        let agg_slot_for_target = |element: &str| -> Option<String> {
            if !agg_is_arrayed {
                return None;
            }
            let parts: Vec<&str> = element.split(',').collect();
            let slot: Vec<&str> = result_dim_positions
                .iter()
                .filter_map(|&p| parts.get(p).copied())
                .collect();
            (!slot.is_empty()).then(|| slot.join(","))
        };
        // The agg side of the link-score name for a given target element: the
        // bare agg name when scalar, `agg[<slot>]` when arrayed (the target
        // element projected onto `result_dims`).
        let agg_name_for_target = |element: &str| -> String {
            match agg_slot_for_target(element) {
                None => agg.name.clone(),
                Some(slot) => format!("{}[{}]", agg.name, slot),
            }
        };
        // The agg side of the `agg → target` link-score *equation*'s `Δsource`
        // denominator for a given target element: the quoted agg name with the
        // same `[<slot>]` subscript the link-score name carries (so the
        // denominator indexes the agg slot the loop traverses). The
        // bare-agg-name form `generate_scalar_to_element_equation` would build
        // by default does not compile when the agg is arrayed (a multi-slot
        // var referenced bare in a scalar equation).
        let agg_q = crate::ltm_augment::quote_ident(&agg.name);
        let agg_source_ref_for_target = |element: &str| -> String {
            match agg_slot_for_target(element) {
                None => agg_q.clone(),
                Some(slot) => format!("{agg_q}[{slot}]"),
            }
        };

        // Helper: substitute the reducers in a slot expr's canonical text and
        // build the agg→target link-score equation for one target element (or
        // the scalar case when `element` is `None`).
        let slot_text = |expr: &crate::ast::Expr2| -> String {
            crate::ltm_augment::substitute_reducers_in_equation(
                &crate::patch::expr2_to_string(expr),
                &reducer_subst,
            )
        };

        match ast {
            Ast::Scalar(expr) => {
                // A scalar `to` cannot reference a reducer in a way that makes
                // the agg arrayed (a scalar target has no iterated dims), so
                // the agg is always scalar here.
                debug_assert!(!agg_is_arrayed, "a scalar target implies a scalar agg");
                let substituted = slot_text(expr);
                let equation = crate::ltm_augment::generate_agg_to_scalar_target_equation(
                    &agg.name,
                    to,
                    &substituted,
                    &all_deps,
                );
                vars.push(LtmSyntheticVar {
                    name: format!(
                        "$\u{205A}ltm\u{205A}link_score\u{205A}{}\u{2192}{}",
                        agg.name, to
                    ),
                    equation: datamodel::Equation::Scalar(equation),
                    dimensions: vec![],
                    // synthetic agg on the `from` side -> routed direct already.
                    compile_directly: false,
                });
            }
            Ast::ApplyToAll(_, expr) => {
                // One shared body; emit one per-target-element scalar var.
                let substituted = slot_text(expr);
                if to_dims.is_empty() {
                    return;
                }
                let dim_element_lists: Vec<Vec<String>> = to_dims
                    .iter()
                    .map(crate::ltm_augment::dimension_element_names)
                    .collect();
                for element in &cartesian_subscripts(&dim_element_lists) {
                    // The partial is built around the *bare* agg name (which is
                    // what `substituted` holds); `deps_to_subscript` then
                    // element-pins it to `agg[<element>]` when the agg is
                    // arrayed, matching `agg_name_for_target` (exact when
                    // `result_dims == to`'s dims). The `Δsource` denominator
                    // is element-pinned the same way via the explicit override.
                    let equation = crate::ltm_augment::generate_scalar_to_element_equation(
                        &agg.name,
                        to,
                        element,
                        &substituted,
                        &all_deps,
                        &deps_to_subscript,
                        Some(&agg_source_ref_for_target(element)),
                    );
                    vars.push(LtmSyntheticVar {
                        name: format!(
                            "$\u{205A}ltm\u{205A}link_score\u{205A}{}\u{2192}{}[{}]",
                            agg_name_for_target(element),
                            to,
                            element
                        ),
                        equation: datamodel::Equation::Scalar(equation),
                        dimensions: vec![],
                        // synthetic agg on `from` + bracketed `to` -> routed direct.
                        compile_directly: false,
                    });
                }
            }
            Ast::Arrayed(_, per_elem, default_expr, _) => {
                if to_dims.is_empty() {
                    return;
                }
                let dim_element_lists: Vec<Vec<String>> = to_dims
                    .iter()
                    .map(crate::ltm_augment::dimension_element_names)
                    .collect();
                for element in &cartesian_subscripts(&dim_element_lists) {
                    let canonical_elem = crate::common::CanonicalElementName::from_raw(element);
                    // Thread the slot expression through directly rather than
                    // relying on the invariant that `substituted.is_empty()`
                    // iff there is no slot expression.
                    let equation = match per_elem.get(&canonical_elem).or(default_expr.as_ref()) {
                        None => "0".to_string(),
                        Some(slot_expr) => {
                            let substituted = slot_text(slot_expr);
                            // Re-derive per-slot deps (the union over all slots
                            // would over-freeze refs absent from this slot),
                            // then extend with this agg's name and every other
                            // agg referenced in the (substituted) slot text so
                            // they are all PREVIOUS-wrapped (ceteris paribus).
                            let mut slot_deps = crate::variable::classify_dependencies(
                                &Ast::Scalar(slot_expr.clone()),
                                target_ast_dims,
                                None,
                            )
                            .all;
                            slot_deps.insert(agg_canonical.clone());
                            for other_agg in reducer_subst.values() {
                                slot_deps.insert(Ident::<Canonical>::new(other_agg));
                            }
                            crate::ltm_augment::generate_scalar_to_element_equation(
                                &agg.name,
                                to,
                                element,
                                &substituted,
                                &slot_deps,
                                &deps_to_subscript,
                                Some(&agg_source_ref_for_target(element)),
                            )
                        }
                    };
                    vars.push(LtmSyntheticVar {
                        name: format!(
                            "$\u{205A}ltm\u{205A}link_score\u{205A}{}\u{2192}{}[{}]",
                            agg_name_for_target(element),
                            to,
                            element
                        ),
                        equation: datamodel::Equation::Scalar(equation),
                        dimensions: vec![],
                        // synthetic agg on `from` + bracketed `to` -> routed direct.
                        compile_directly: false,
                    });
                }
            }
        }
    }

    /// Emit all link scores for a single variable-level causal edge
    /// `(from, to)`: a synthetic-agg reroute (Phase 5) if `to` hoists a
    /// reducer reading `from`, else a cross-dimensional (arrayed→scalar)
    /// reducer split, else a scalar→arrayed split, else the per-shape
    /// emission. The agg reroute still emits the non-reducer (Bare /
    /// FixedIndex) shapes of `from` in `to` via `emit_per_shape_link_scores`
    /// with the reducer shapes suppressed.
    ///
    /// `skip_agg_halves` is set by the exhaustive loop-link caller: when a
    /// loop traverses the *direct* `from → to` reference (e.g. the `pop[r]`
    /// numerator in `share[r] = pop[r] / SUM(pop[*])`) the routed agg's two
    /// halves (`from → agg`, `agg → to`) are emitted -- if at all -- by the
    /// `agg_by_name` branches of that caller when the loop also traverses the
    /// reducer path, so re-emitting them here would push duplicate
    /// `LtmSyntheticVar`s into the `Vec`. The discovery / sub-model caller
    /// passes `false` since it iterates causal edges (not loop links) and the
    /// `from → agg`/`agg → to` edges aren't separately visited there.
    #[allow(clippy::too_many_arguments)]
    fn emit_link_scores_for_edge(
        db: &dyn Db,
        source_vars: &HashMap<String, super::SourceVariable>,
        agg_nodes: &crate::ltm_agg::AggNodesResult,
        from: &str,
        to: &str,
        model: SourceModel,
        project: SourceProject,
        dm_dims: &[crate::datamodel::Dimension],
        skip_agg_halves: bool,
        vars: &mut Vec<LtmSyntheticVar>,
    ) {
        // The set of synthetic aggs `(from, to)` routes through, read off
        // the reference-site IR (the unique `ThroughAgg` `AggRef`s of this
        // edge's classified sites, in first-occurrence order). This is the
        // single place the old per-edge `routed_aggs` filter
        // (`aggs_in_var(to).filter(is_synthetic && reads from)`) used to be
        // restated -- it now lives only in the IR builder; here we just
        // project the result, resolving each `AggRef` to its `AggNode` for
        // the half-emitters.
        let ir = crate::db_ltm_ir::model_ltm_reference_sites(db, model, project);
        let routed_aggs: Vec<&crate::ltm_agg::AggNode> = {
            let mut idxs: Vec<usize> = Vec::new();
            if let Some(sites) = ir.sites.get(&(from.to_string(), to.to_string())) {
                for s in sites {
                    if let crate::db_ltm_ir::SiteRouting::ThroughAgg { agg } = &s.routing
                        && !idxs.contains(&agg.0)
                    {
                        idxs.push(agg.0);
                    }
                }
            }
            idxs.iter().map(|&i| &agg_nodes.aggs[i]).collect()
        };
        if !routed_aggs.is_empty() {
            if !skip_agg_halves {
                for agg in &routed_aggs {
                    emit_source_to_agg_link_scores(
                        db,
                        source_vars,
                        from,
                        agg,
                        model,
                        project,
                        vars,
                    );
                    emit_agg_to_target_link_scores(
                        db,
                        source_vars,
                        agg_nodes,
                        agg,
                        to,
                        model,
                        project,
                        vars,
                    );
                }
            }
            // The Bare numerator / FixedIndex references of `from` in `to`
            // still get their own (non-reducer) link scores. (And a bare
            // arrayed reducer arg like `SUM(pop)` keeps its `Bare`-named
            // link score too -- `emit_per_shape_link_scores` drops only the
            // `Wildcard` reducer-arg shape; see its doc.)
            emit_per_shape_link_scores(
                db,
                source_vars,
                from,
                to,
                RefShape::Bare,
                model,
                project,
                dm_dims,
                /* skip_reducer_shapes = */ true,
                vars,
            );
            return;
        }
        // Cross-dimensional (arrayed-to-scalar) edges -- includes the
        // *variable-backed* reducer aggs like `total = SUM(pop[*])`.
        if let Some(cross_vars) =
            try_cross_dimensional_link_scores(db, source_vars, from, to, model, project)
        {
            vars.extend(cross_vars);
            return;
        }
        // Scalar-source -> arrayed-target edges (one scalar link score per
        // target element).
        if let Some(cross_vars) =
            try_scalar_to_arrayed_link_scores(db, source_vars, from, to, model, project)
        {
            vars.extend(cross_vars);
            return;
        }
        // Disjoint-dim arrayed -> arrayed edges with a per-element-equation
        // (`Ast::Arrayed`) target (GH #510): one link score per distinct
        // referenced source element, each `Equation::Arrayed` over `to`'s
        // dims. `Some(vec![])` means the edge is genuinely unscoreable (a
        // dynamic-index source): a `Warning` was accumulated and no link
        // score is emitted -- crucially, we *don't* fall through to
        // `emit_per_shape_link_scores`, which would build a scalarized stand-in.
        if let Some(disjoint_vars) =
            try_disjoint_dim_arrayed_link_scores(db, source_vars, from, to, model, project)
        {
            vars.extend(disjoint_vars);
            return;
        }
        emit_per_shape_link_scores(
            db,
            source_vars,
            from,
            to,
            RefShape::Bare,
            model,
            project,
            dm_dims,
            /* skip_reducer_shapes = */ false,
            vars,
        );
    }

    if has_input_ports || is_discovery {
        for (from, tos) in &edges_result.edges {
            for to in tos {
                emit_link_scores_for_edge(
                    db,
                    source_vars,
                    agg_nodes,
                    from,
                    to,
                    model,
                    project,
                    dm_dims,
                    /* skip_agg_halves = */ false,
                    &mut vars,
                );
            }
        }
    } else if let Some(ref detected_loops) = loops {
        // Helper: look up the `AggNode` for a synthetic-agg node name.
        let agg_by_name = |name: &str| -> Option<&crate::ltm_agg::AggNode> {
            if crate::ltm_agg::is_synthetic_agg_name(name) {
                agg_nodes.aggs.iter().find(|a| a.name == name)
            } else {
                None
            }
        };
        let mut seen_links: HashSet<(String, String)> = HashSet::new();
        for loop_item in detected_loops {
            for link in &loop_item.links {
                // Loop links can carry element-level subscripts on either
                // end -- `link.from` for a per-source-element FixedIndex /
                // cross-dimensional edge ("pop[nyc]"), an agg-routed source
                // element, or (for cross-element loops) `link.to` for an A2A
                // target visited at a single element. The link-score helpers
                // and `source_vars` / `agg_nodes` lookups all key on the
                // variable-level name (the agg name has no subscript), so
                // strip the subscript from both ends for the dedup key and
                // the helper calls. Each helper emits the *full* link score
                // for the (var_from, var_to) edge -- per-element when the
                // target is arrayed -- so the loop-score equation's `[elem]`
                // subscript picks the slot the loop actually visits.
                let from_var_level = strip_subscript(link.from.as_str());
                let to_var_level = strip_subscript(link.to.as_str());
                let key = (from_var_level.to_string(), to_var_level.to_string());
                if !seen_links.insert(key) {
                    continue;
                }
                // A loop link whose target is a synthetic agg node is the
                // `source[d] → agg` half; one whose source is a synthetic agg
                // is the `agg → target` half. (The two halves of a hoisted
                // reducer reference appear as two consecutive loop links
                // `X → agg`, `agg → Y` -- the original `(X, Y)` causal edge
                // never appears in an element-level loop once routed.)
                if let Some(agg) = agg_by_name(to_var_level) {
                    emit_source_to_agg_link_scores(
                        db,
                        source_vars,
                        from_var_level,
                        agg,
                        model,
                        project,
                        &mut vars,
                    );
                } else if let Some(agg) = agg_by_name(from_var_level) {
                    emit_agg_to_target_link_scores(
                        db,
                        source_vars,
                        agg_nodes,
                        agg,
                        to_var_level,
                        model,
                        project,
                        &mut vars,
                    );
                } else {
                    emit_link_scores_for_edge(
                        db,
                        source_vars,
                        agg_nodes,
                        from_var_level,
                        to_var_level,
                        model,
                        project,
                        dm_dims,
                        /* skip_agg_halves = */ true,
                        &mut vars,
                    );
                }
            }
        }
    }

    // Part 2: Loop scores and relative loop scores (exhaustive mode only).
    // Generated for any model with feedback loops, regardless of whether
    // it also has input ports. A model can be both a reusable sub-model
    // AND have internal loops that need scoring.
    //
    // Uses element-level cycle partitions so that cross-element feedback
    // is detected correctly (e.g., population[NYC] and population[Boston]
    // in the same partition when connected through migration).
    if let Some(ref detected_loops) = loops {
        let partitions_result = model_element_cycle_partitions(db, model, project);
        let partitions = CyclePartitions {
            partitions: partitions_result
                .partitions
                .iter()
                .map(|p| p.iter().map(|s| Ident::new(s)).collect())
                .collect(),
            stock_partition: partitions_result
                .stock_partition
                .iter()
                .map(|(k, v)| (Ident::new(k), *v))
                .collect(),
        };

        // Capture each loop's per-slot partition vector before consuming
        // `partitions` so post-sim `compute_rel_loop_scores*` can group slots
        // into the same `(partition, slot)` denominator bins.  The vector's
        // length must match the loop_score series' slot count -- 1 for a
        // scalar/cross-element/mixed loop, the dimension-element-space size
        // for an A2A loop -- which is the same `n_slots` that
        // `ltm_post::build_loop_element_index` derives from
        // `LtmSyntheticVar.dimensions` + the project dims; both feed
        // `compute_rel_loop_scores_per_element`, so a length mismatch would
        // desync the per-element normalization.
        for l in detected_loops.iter() {
            let parts = partitions.partition_for_loop(l, dm_dims);
            debug_assert!(
                {
                    let expected = if l.dimensions.is_empty() {
                        Some(1usize)
                    } else {
                        let n =
                            crate::ltm::loop_dimension_element_tuples(&l.dimensions, dm_dims).len();
                        // n == 0 only when `dm_dims` doesn't cover the loop's
                        // declared dimensions (a mid-edit inconsistency);
                        // `partition_for_loop` then falls back to the present
                        // suffixes and the length is not predictable here.
                        if n == 0 { None } else { Some(n) }
                    };
                    expected.is_none_or(|n| parts.len() == n)
                },
                "loop {:?}: per-slot partition vector length {} disagrees with the loop's slot \
                 count; it must equal `build_loop_element_index`'s n_slots (both feed \
                 `compute_rel_loop_scores_per_element`)",
                l.id,
                parts.len(),
            );
            loop_partitions.insert(l.id.clone(), parts);
        }

        // Build the set of link-score variable names emitted so far so
        // generate_loop_score_equation can resolve each loop link to a
        // name that actually exists. Without this, loops traversing
        // edges whose only AST shape is Wildcard or DynamicIndex would
        // reference a never-emitted Bare canonical name and the
        // fragment compiler would silently fall back to a stub dep.
        let emitted_link_score_names: HashSet<String> = vars
            .iter()
            .filter(|v| v.name.contains("\u{205A}link_score\u{205A}"))
            .map(|v| v.name.clone())
            .collect();
        let loop_vars = crate::ltm_augment::generate_loop_score_variables(
            detected_loops,
            &partitions,
            &emitted_link_score_names,
        );
        for (name, var) in loop_vars {
            let equation_text = match var.get_equation() {
                Some(crate::datamodel::Equation::Scalar(eq)) => eq.clone(),
                _ => String::new(),
            };
            // Carry forward dimensions from the Loop struct for A2A loops.
            let loop_item = detected_loops
                .iter()
                .find(|l| name.as_str().ends_with(&l.id));
            let dimensions = loop_item
                .and_then(|l| {
                    if l.dimensions.is_empty() {
                        None
                    } else {
                        Some(l.dimensions.clone())
                    }
                })
                .unwrap_or_default();
            vars.push(LtmSyntheticVar {
                name: name.to_string(),
                equation: ltm_synthetic_equation(equation_text, &dimensions),
                dimensions,
                // Loop scores aren't link scores; `assemble_module` compiles
                // them directly via the non-link-score branch already.
                compile_directly: false,
            });
        }
    }

    // Pathway and composite scores for models with input ports.
    //
    // Each pathway is a product of link-score references. Since
    // emit_per_shape_link_scores only emits the names that appear in
    // each target's AST, we resolve each link against the set of
    // already-emitted names with shape priority (Bare > FixedIndex >
    // Wildcard > DynamicIndex). Without this, an input-port model with
    // an edge like `share[r] = SUM(x[*])` would reference the never-
    // emitted Bare canonical name `x→share`, and the fragment
    // compiler's stub-dep fallback would silently drop that pathway's
    // contribution to the composite score.
    let pathway_emitted_names: HashSet<String> = vars
        .iter()
        .filter(|v| v.name.contains("\u{205A}link_score\u{205A}"))
        .map(|v| v.name.clone())
        .collect();
    for (input_port, port_pathways) in &pathways {
        let mut pathway_names = Vec::new();
        for (idx, pathway_links) in port_pathways.iter().enumerate() {
            let path_var_name = format!(
                "$\u{205A}ltm\u{205A}path\u{205A}{}\u{205A}{}",
                input_port.as_str(),
                idx
            );

            let link_score_refs: Vec<String> = pathway_links
                .iter()
                .map(|link| {
                    // Pathway links come from the variable-level causal
                    // graph (`enumerate_pathways_to_outputs`), so `link.to`
                    // never carries an element subscript and there is no
                    // visited-element to pass.
                    let resolved = crate::ltm_augment::resolve_link_score_name_for_loop(
                        link.from.as_str(),
                        link.to.as_str(),
                        &pathway_emitted_names,
                        None,
                    );
                    format!("\"{resolved}\"")
                })
                .collect();

            let equation = if link_score_refs.is_empty() {
                "0".to_string()
            } else {
                link_score_refs.join(" * ")
            };

            pathway_names.push(path_var_name.clone());
            vars.push(LtmSyntheticVar {
                name: path_var_name,
                equation: datamodel::Equation::Scalar(equation),
                dimensions: vec![],
                compile_directly: false,
            });
        }

        let composite_name = format!(
            "$\u{205A}ltm\u{205A}composite\u{205A}{}",
            input_port.as_str()
        );
        let equation = generate_max_abs_chain_str(&pathway_names);
        vars.push(LtmSyntheticVar {
            name: composite_name,
            equation: datamodel::Equation::Scalar(equation),
            dimensions: vec![],
            compile_directly: false,
        });
    }

    // Sort by evaluation-order category so the VM's sequential flow
    // evaluation respects the dependency chain: composites reference paths
    // which reference loop scores which reference link scores, and link
    // scores referencing an aggregate node read its current-step value, so
    // the agg fragment must run first. Within each category, sort lexically
    // for determinism. (`compute_layout` section 3 re-sorts LTM vars purely
    // by name -- `$⁚ltm⁚agg⁚{n}` < `$⁚ltm⁚link_score⁚...` lexically, so the
    // agg gets its layout slot before any consumer there too -- but the
    // runlist order is what the same-timestep ordering hazard turns on, and
    // that comes from this sort.)
    vars.sort_by(|a, b| {
        fn category(name: &str) -> u8 {
            // The agg check uses the `$⁚ltm⁚agg⁚` *prefix*, not a substring
            // search: the `agg → target` link score is named
            // `$⁚ltm⁚link_score⁚$⁚ltm⁚agg⁚{n}→{to}` and contains `⁚agg⁚`,
            // but it is a link score (category 1) that must run *after* the
            // agg aux it references.
            if crate::ltm_agg::is_synthetic_agg_name(name) {
                0 // aggregate nodes: before everything that may reference them
            } else if name.contains("\u{205A}composite\u{205A}") {
                4
            } else if name.contains("\u{205A}path\u{205A}") {
                3
            } else if name.contains("\u{205A}loop_score\u{205A}") {
                2
            } else {
                1 // link_score and anything else
            }
        }
        category(&a.name)
            .cmp(&category(&b.name))
            .then_with(|| a.name.cmp(&b.name))
    });
    LtmVariablesResult {
        vars,
        loop_partitions,
        agg_recovery_truncated,
    }
}

#[cfg(test)]
#[path = "db_ltm_tests.rs"]
mod db_ltm_tests;
