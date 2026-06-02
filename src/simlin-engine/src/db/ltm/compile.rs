// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Per-equation compilation of LTM synthetic variables to symbolic
//! bytecodes.
//!
//! This is the emission side of the LTM pipeline: the per-link salsa
//! fragment (`compile_ltm_var_fragment`), the shape-aware link-score
//! equation-text query (`link_score_equation_text_shaped`), the shared
//! equation-to-bytecode compiler (`compile_ltm_equation_fragment`), the
//! synthetic-fragment selector (`compile_ltm_synthetic_fragment`), the
//! compile-failure diagnostic pass (`model_ltm_fragment_diagnostics`), and
//! the implicit-variable fragment compiler (`compile_ltm_implicit_var_fragment`).

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::canonicalize;
use crate::common::{Canonical, Ident};
use crate::datamodel;

use crate::db::{
    Db, LtmLinkId, LtmSyntheticVar, ModelDepGraphResult, RefShape, SourceModel, SourceProject,
    SourceVariableKind, VarFragmentResult, build_module_inputs, build_stub_variable,
    build_submodel_metadata, canonical_module_input_set, compute_layout,
    extract_tables_from_source_var, link_score_equation_text, model_implicit_var_info,
    model_module_ident_context, model_module_map, parse_source_variable_with_module_context,
    project_converted_dimensions, project_datamodel_dims, project_dimensions_context,
    project_units_context, reconstruct_single_variable, variable_dimensions, variable_size,
};

use super::parse::{ltm_equation_dimensions, parse_ltm_equation};
use super::{
    LtmImplicitVarMeta, ltm_module_idents, model_ltm_implicit_module_refs,
    model_ltm_implicit_var_info, model_ltm_var_name_index, model_ltm_variables,
};

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
/// Sibling of [`crate::db::link_score_equation_text`]. Where the legacy
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
/// Lives in `db/ltm/compile.rs` rather than `db.rs` so the latter stays
/// under the project's per-file line cap.
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

    let from_var = reconstruct_single_variable(db, model, project, from_name);
    let to_var = reconstruct_single_variable(db, model, project, to_name)?;

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
    // `Region`); the cached context depends only on the salsa-tracked
    // dimensions input, so this fn is recomputed when a dimension's
    // mappings change.
    let dim_ctx = project_dimensions_context(db, project);
    // The generator returns the equation already tagged with the target's
    // dimensionality (`Scalar`, `ApplyToAll`, or `Arrayed`). `dimensions`
    // and `compile_directly` are left at defaults here; the emission loop
    // in `model_ltm_variables` (`emit_per_shape_link_scores`) overwrites
    // `dimensions`, the equation's dimension names, and `compile_directly`
    // (set when `shape` is not `Bare`) with the per-shape policy result.
    // A `PartialEquationError` means the target's equation text did not parse
    // for the ceteris-paribus partial (GH #311). Skip the variable and warn
    // rather than emit a silently non-ceteris-paribus link score; the bad
    // equation would compile cleanly, so `model_ltm_fragment_diagnostics`
    // would not catch it.
    let equation = match crate::ltm_augment::generate_link_score_equation_for_link(
        &from_ident,
        &to_ident,
        &shape,
        &source_dim_elements,
        &to_var,
        &all_vars,
        Some(dim_ctx),
    ) {
        Ok(eqn) => eqn,
        Err(err) => {
            super::emit_ltm_partial_equation_warning(db, model, &var_name, &err);
            return None;
        }
    };

    Some(LtmSyntheticVar {
        name: var_name,
        equation,
        dimensions: vec![],
        compile_directly: false,
    })
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
pub(crate) fn compile_ltm_equation_fragment(
    db: &dyn Db,
    var_name: &str,
    equation: &datamodel::Equation,
    model: SourceModel,
    project: SourceProject,
) -> Option<VarFragmentResult> {
    use crate::compiler::symbolic::{
        CompiledVarFragment, PerVarBytecodes, ReverseOffsetMap, VariableLayout,
    };

    // Project-global dims (datamodel form needed by `parse_ltm_equation`) plus
    // the canonicalized context + converted dims, all from the salsa-cached
    // queries rather than rebuilt per LTM fragment.
    let dims = project_datamodel_dims(db, project);
    let dim_context = project_dimensions_context(db, project);
    let converted_dims = project_converted_dimensions(db, project);

    let units_ctx = project_units_context(db, project);
    let module_idents = ltm_module_idents(db, model, project);
    let model_var_names = super::ltm_model_var_names(db, model, project);

    let var_dimensions = ltm_equation_dimensions(equation);

    let parsed = parse_ltm_equation(
        var_name,
        equation,
        dims,
        units_ctx,
        Some(module_idents),
        Some(model_var_names),
    );

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
        dimensions: dim_context,
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

    // Collect dependency variable names from the lowered AST. Lookup-table
    // references are not data-flow deps (issue #606 keeps them in
    // `referenced_tables`, off the causal graph), but the fragment still
    // needs them: see the referenced-tables pass below.
    let (dep_idents, referenced_tables) = if let Some(ast) = lowered.ast() {
        let classification = crate::variable::classify_dependencies(ast, &[], None);
        (classification.all, classification.referenced_tables)
    } else {
        (HashSet::new(), std::collections::BTreeSet::new())
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
                        .map(|sm| compute_layout(db, *sm, project).n_slots)
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
                        .map(|sm| compute_layout(db, *sm, project).n_slots)
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
                        .map(|sm| compute_layout(db, *sm, project).n_slots)
                        .unwrap_or(1);

                    let module_ctx = model_module_ident_context(db, model, project, vec![]);
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
                        .map(|sm| compute_layout(db, *sm, project).n_slots)
                        .unwrap_or(1);

                    // The implicit module variable rides on its meta (captured
                    // at LTM-equation parse time), so the parent equation is
                    // not re-parsed to recover its input references.
                    let module_inputs =
                        if let datamodel::Variable::Module(dm_module) = &ltm_im_meta.variable {
                            let input_prefix = format!("{module_var_name}\u{00B7}");
                            build_module_inputs(
                                model.name(db),
                                &input_prefix,
                                dm_module
                                    .references
                                    .iter()
                                    .map(|mr| (canonicalize(&mr.src), canonicalize(&mr.dst))),
                            )
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
                        .map(|sm| compute_layout(db, *sm, project).n_slots)
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
        // model_ltm_variables and the name index are salsa-cached, so this
        // lookup is cheap and safe to call from within
        // compile_ltm_equation_fragment -- the same pattern is used by the
        // implicit-module branch above. The indexed lookup matters: most
        // unresolved deps here are PREVIOUS-helper names that are NOT LTM
        // vars, and a linear scan over all LTM vars per dep was O(N^2)
        // across a model's compile (~145k lookups over 6.7k vars on
        // C-LEARN).
        else {
            let ltm_dep = model_ltm_var_name_index(db, model, project)
                .get(effective)
                .map(|&i| model_ltm_variables(db, model, project).vars[i].clone());
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

    // Lookup-table references (issue #606): a `LOOKUP(table, x)` call's table
    // argument is not a data-flow dep, but the fragment still needs (a) a
    // metadata stub so lowering resolves the table ident to a layout offset
    // (the lookup codegen recovers the table identity by reverse offset
    // lookup), and (b) the table's graphical-function data in the
    // mini-Module's tables map so the Lookup opcode gets a base_gf. Without
    // both, the fragment fails to compile and the link score silently reads a
    // constant 0 -- the failure mode behind WRLD3's identically-zero
    // table-mediated link scores (food_per_capita -> lifetime_multiplier_from_food
    // and 50+ siblings).
    let mut tables: HashMap<Ident<Canonical>, Vec<crate::compiler::Table>> = HashMap::new();
    for table_name in &referenced_tables {
        let effective = table_name
            .strip_prefix('\u{00B7}')
            .unwrap_or(table_name.as_str());
        // Module-namespaced tables can't be referenced from LTM equations.
        if effective.contains('\u{00B7}') {
            continue;
        }
        let table_ident: Ident<Canonical> = Ident::new(effective);
        let Some(table_sv) = source_vars.get(effective) else {
            continue;
        };
        let table_data = extract_tables_from_source_var(db, table_sv, project);
        if !table_data.is_empty() {
            tables.insert(table_ident.clone(), table_data);
        }
        let already_present = mini_metadata.contains_key(&table_ident)
            || dep_variables.iter().any(|(id, _, _)| id == &table_ident);
        if !already_present {
            let table_dims = variable_dimensions(db, *table_sv, project);
            let table_size = variable_size(db, *table_sv, project);
            let table_var = build_stub_variable(db, table_sv, &table_ident, table_dims);
            dep_variables.push((table_ident, table_var, table_size));
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

    let inputs = BTreeSet::new();

    // Merge LTM implicit module references from LTM equation parsing into the
    // module_models map so the compiler context can resolve module_var_name ->
    // sub_model_name lookups. Copy-on-write: the salsa-cached base map is only
    // cloned when this equation actually has implicit module refs (this
    // function runs once per LTM synthetic var, ~6.7k times on C-LEARN).
    let base_module_models = model_module_map(db, model, project);
    let merged_module_models;
    let module_models: &HashMap<Ident<Canonical>, HashMap<Ident<Canonical>, Ident<Canonical>>> =
        if implicit_module_refs.is_empty() {
            base_module_models
        } else {
            merged_module_models = {
                let mut merged = base_module_models.clone();
                let current_model_modules = merged.entry(model_name_ident.clone()).or_default();
                for (var_ident, (sub_model_name, _input_set)) in &implicit_module_refs {
                    current_model_modules.insert(var_ident.clone(), sub_model_name.clone());
                }
                merged
            };
            &merged_module_models
        };

    let core = crate::compiler::ContextCore {
        dimensions: converted_dims,
        dimensions_ctx: dim_context,
        model_name: &model_name_ident,
        metadata: &all_metadata,
        module_models,
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
            // Owned Module fields: the slice/context come from the cached
            // queries by reference, so materialize owned copies here. The
            // interned-backed `Dimension`s clone cheaply -- the expensive
            // rebuild (re-canonicalizing every element) is what the cache
            // removes; only the relationship-cache memo is rebuilt cold.
            dimensions: converted_dims.to_vec(),
            dimensions_ctx: (*dim_context).clone(),
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
pub(crate) fn compile_ltm_synthetic_fragment(
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

    use crate::db::{CompilationDiagnostic, Diagnostic, DiagnosticError, DiagnosticSeverity};

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
pub(crate) fn compile_ltm_implicit_var_fragment(
    db: &dyn Db,
    meta: &LtmImplicitVarMeta,
    model: SourceModel,
    project: SourceProject,
    _dep_graph: &ModelDepGraphResult,
    module_input_names: &[String],
) -> Option<VarFragmentResult> {
    use crate::compiler::symbolic::{
        CompiledVarFragment, PerVarBytecodes, ReverseOffsetMap, VariableLayout,
    };

    // The implicit variable rides on the meta (captured at LTM-equation parse
    // time by `model_ltm_implicit_var_info`), so no parent re-parse is needed.
    let implicit_dm_var = &meta.variable;
    let implicit_name = canonicalize(implicit_dm_var.get_ident()).into_owned();

    // Project-global dims (datamodel form needed by `parse_var`) plus the
    // canonicalized context + converted dims, from the salsa-cached queries.
    let dims = project_datamodel_dims(db, project);
    let dim_context = project_dimensions_context(db, project);
    let converted_dims = project_converted_dimensions(db, project);

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
            dimensions: dim_context,
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
    // Lookup tables referenced by this implicit var's equation (issue #606):
    // populated by the non-module dependency pass below, consumed by the
    // mini-Module construction.
    let mut fragment_tables: HashMap<Ident<Canonical>, Vec<crate::compiler::Table>> =
        HashMap::new();

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
        // Lookup-table references are handled separately below (they are not
        // data-flow deps -- issue #606 -- but the fragment needs their layout
        // stub and graphical-function data so a `lookup(table, ...)` inside a
        // synthesized helper compiles; see compile_ltm_equation_fragment).
        let (dep_idents, referenced_tables) = if let Some(ast) = lowered.ast() {
            let classification = crate::variable::classify_dependencies(ast, &[], None);
            (classification.all, classification.referenced_tables)
        } else {
            (HashSet::new(), std::collections::BTreeSet::new())
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
                        .map(|sm| compute_layout(db, *sm, project).n_slots)
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

        // Referenced lookup tables: layout stub + graphical-function data
        // (mirrors compile_ltm_equation_fragment's referenced-tables pass).
        for table_name in &referenced_tables {
            let effective = table_name
                .strip_prefix('\u{00B7}')
                .unwrap_or(table_name.as_str());
            if effective.contains('\u{00B7}') {
                continue;
            }
            let table_ident: Ident<Canonical> = Ident::new(effective);
            let Some(table_sv) = source_vars.get(effective) else {
                continue;
            };
            let table_data = extract_tables_from_source_var(db, table_sv, project);
            if !table_data.is_empty() {
                fragment_tables.insert(table_ident.clone(), table_data);
            }
            let already_present = mini_metadata.contains_key(&table_ident)
                || dep_variables.iter().any(|(id, _, _)| id == &table_ident);
            if !already_present {
                let table_dims = variable_dimensions(db, *table_sv, project);
                let table_size = variable_size(db, *table_sv, project);
                let table_var = build_stub_variable(db, table_sv, &table_ident, table_dims);
                dep_variables.push((table_ident, table_var, table_size));
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

    let tables = fragment_tables;
    let inputs = canonical_module_input_set(module_input_names);

    // Merge the current variable's own module refs plus the module-typed LTM
    // implicit refs into the model module map so cross-references resolve (a
    // module's inputs may reference outputs from OTHER LTM implicit modules,
    // e.g. one PREVIOUS instance reading another's output).
    //
    // Both source maps are salsa-cached and the merge is copy-on-write. This
    // function runs once per LTM implicit variable -- ~145k times on a model
    // like C-LEARN -- so the previous per-call clone-and-rescan of the full
    // `model_ltm_implicit_var_info` map was O(K^2) in the implicit-var count
    // and dominated LTM compile time (tens of seconds of HashMap iteration).
    let base_module_models = model_module_map(db, model, project);
    let ltm_module_refs = model_ltm_implicit_module_refs(db, model, project);
    let merged_module_models;
    let module_models: &HashMap<Ident<Canonical>, HashMap<Ident<Canonical>, Ident<Canonical>>> =
        if module_refs.is_empty() && ltm_module_refs.is_empty() {
            base_module_models
        } else {
            merged_module_models = {
                let mut merged = base_module_models.clone();
                let current_model_modules = merged.entry(model_name_ident.clone()).or_default();
                for (var_ident, (sub_model_name, _input_set)) in &module_refs {
                    current_model_modules.insert(var_ident.clone(), sub_model_name.clone());
                }
                for (im_ident, sub_model_name) in ltm_module_refs.iter() {
                    current_model_modules.insert(im_ident.clone(), sub_model_name.clone());
                }
                merged
            };
            &merged_module_models
        };

    let core = crate::compiler::ContextCore {
        dimensions: converted_dims,
        dimensions_ctx: dim_context,
        model_name: &model_name_ident,
        metadata: &all_metadata,
        module_models,
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
            // Owned Module fields: the slice/context come from the cached
            // queries by reference, so materialize owned copies here. The
            // interned-backed `Dimension`s clone cheaply -- the expensive
            // rebuild (re-canonicalizing every element) is what the cache
            // removes; only the relationship-cache memo is rebuilt cold.
            dimensions: converted_dims.to_vec(),
            dimensions_ctx: (*dim_context).clone(),
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
