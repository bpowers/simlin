// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Module/simulation assembly: turning per-variable symbolic fragments into
//! a concrete `CompiledModule`/`CompiledSimulation`.
//!
//! Holds the table/metadata extraction helpers
//! (`extract_tables_from_source_var`, `build_module_inputs`,
//! `build_stub_variable`, `build_submodel_metadata`), the per-variable
//! emission tail (`compile_phase_to_per_var_bytecodes` and the
//! `VarFragmentResult`/`PerVarSizes` values), the production
//! element-graph source `var_phase_symbolic_fragment_prod`, the resolved
//! recurrence-SCC interleaver (`segment_member_by_element` /
//! `combine_scc_fragment`), the salsa-tracked `assemble_module` /
//! `assemble_simulation`, module-instance enumeration
//! (`enumerate_module_instances`), and the flattened-offset map builder
//! (`calc_flattened_offsets_incremental`).

use std::collections::{BTreeSet, HashMap, HashSet};

use super::*;
use crate::common::{Canonical, Ident};

/// Extract compiler::Table data directly from a SourceVariable's graphical
/// function fields. Used to populate the mini-Module's tables map for
/// dependency variables that define lookup tables.
pub(crate) fn extract_tables_from_source_var(
    db: &dyn Db,
    source_var: &SourceVariable,
    project: SourceProject,
) -> Vec<crate::compiler::Table> {
    let ident = source_var.ident(db);
    let eq = source_var.equation(db);

    // For arrayed equations with per-element graphical functions, build one
    // table per element (matching variable.rs build_tables). Each element's
    // table is laid out at the element's flat declared dimension index (not
    // its `elems` Vec position), because the runtime selects a per-element
    // table by the row-major dimension offset (vm.rs Lookup/LookupArray); see
    // `crate::variable::reorder_arrayed_element_tables`. Elements without a GF
    // get an empty placeholder so that table[element_offset] stays aligned.
    if let datamodel::Equation::Arrayed(_, elements, _, _) = eq {
        // The per-element gf is the 4th tuple field
        // `(subscript, equation, gf_equation, gf)`.
        let has_element_gfs = elements.iter().any(|(_, _, _, gf)| gf.is_some());
        if has_element_gfs {
            // Parse present element tables, keyed by canonical (comma-joined)
            // subscript name.
            let mut present: HashMap<crate::common::CanonicalElementName, crate::compiler::Table> =
                HashMap::new();
            for (subscript, _, _, gf) in elements {
                if let Some(gf) = gf.as_ref()
                    && let Some(var_table) = crate::variable::parse_table(&Some(gf.clone()))
                        .ok()
                        .flatten()
                    && let Ok(table) = crate::compiler::Table::new(ident, &var_table)
                {
                    present.insert(
                        crate::common::CanonicalElementName::from_raw(subscript),
                        table,
                    );
                }
            }

            // Resolve the variable's dimensions so the reorder maps each
            // element name to its row-major declared-order flat offset. If the
            // dimensions cannot be resolved, fall back to the original
            // Vec-positional layout rather than dropping tables.
            let dims = variable_dimensions(db, *source_var, project);
            if dims.is_empty() {
                return elements
                    .iter()
                    .map(|(subscript, _, _, _)| {
                        present
                            .get(&crate::common::CanonicalElementName::from_raw(subscript))
                            .cloned()
                            .unwrap_or(crate::compiler::Table { data: vec![] })
                    })
                    .collect();
            }
            return crate::variable::reorder_arrayed_element_tables(
                dims,
                &present,
                || crate::compiler::Table { data: vec![] },
                |t: &crate::compiler::Table| t.clone(),
            );
        }
    }

    // Scalar or apply-to-all: use the variable-level graphical function.
    let gf = source_var.gf(db);
    match gf {
        Some(gf) => crate::variable::parse_table(&Some(gf.clone()))
            .ok()
            .flatten()
            .and_then(|vt| crate::compiler::Table::new(ident, &vt).ok())
            .into_iter()
            .collect(),
        None => vec![],
    }
}

/// Build module input mappings from raw (src, dst) reference pairs.
///
/// Filters out references where src is an internal module input (starts
/// with the module's own prefix), strips the module prefix from dst,
/// and strips leading middots from src in the "main" model (where parent
/// scope refs are represented as `·var` after canonicalization).
pub(crate) fn build_module_inputs<S1: AsRef<str>, S2: AsRef<str>>(
    model_name: &str,
    module_var_prefix: &str,
    refs: impl Iterator<Item = (S1, S2)>,
) -> Vec<crate::variable::ModuleInput> {
    refs.filter_map(|(src, dst)| {
        let src = src.as_ref();
        let dst = dst.as_ref();
        // Skip internal module inputs (src within the module's own namespace)
        if src.starts_with(module_var_prefix) {
            return None;
        }
        let dst_stripped = dst.strip_prefix(module_var_prefix)?;
        let src_str = if model_name == "main" && src.starts_with('\u{00B7}') {
            &src['\u{00B7}'.len_utf8()..]
        } else {
            src
        };
        Some(crate::variable::ModuleInput {
            src: Ident::new(src_str),
            dst: Ident::new(dst_stripped),
        })
    })
    .collect()
}

/// Build a dimension-only stub Variable for use in a minimal compilation
/// context. Only get_dimensions() is called on these by Context.
pub(crate) fn build_stub_variable(
    db: &dyn Db,
    source_var: &SourceVariable,
    ident: &Ident<Canonical>,
    dims: &[crate::dimensions::Dimension],
) -> crate::variable::Variable {
    let dummy_ast = if dims.is_empty() {
        None
    } else {
        Some(crate::ast::Ast::ApplyToAll(
            dims.to_vec(),
            crate::ast::Expr2::Const(
                "0".to_string(),
                crate::ast::Literal::new(0.0),
                crate::ast::Loc::default(),
            ),
        ))
    };

    match source_var.kind(db) {
        SourceVariableKind::Stock => crate::variable::Variable::Stock {
            ident: ident.clone(),
            init_ast: dummy_ast,
            eqn: None,
            units: None,
            inflows: vec![],
            outflows: vec![],
            non_negative: false,
            errors: vec![],
            unit_errors: vec![],
        },
        SourceVariableKind::Module => crate::variable::Variable::Module {
            ident: ident.clone(),
            model_name: Ident::new(source_var.model_name(db)),
            units: None,
            inputs: vec![],
            errors: vec![],
            unit_errors: vec![],
        },
        _ => crate::variable::Variable::Var {
            ident: ident.clone(),
            ast: dummy_ast,
            init_ast: None,
            eqn: None,
            units: None,
            tables: vec![],
            non_negative: false,
            is_flow: source_var.kind(db) == SourceVariableKind::Flow,
            is_table_only: false,
            errors: vec![],
            unit_errors: vec![],
        },
    }
}

/// Populate sub-model metadata in `all_metadata` for module variable compilation.
/// Mirrors the monolithic `build_metadata` but works with salsa SourceModel/SourceVariable.
/// Recursively populates metadata for nested modules.
pub(crate) fn build_submodel_metadata<'arena>(
    arena: &'arena bumpalo::Bump,
    db: &dyn Db,
    sub_model: SourceModel,
    project: SourceProject,
    all_metadata: &mut crate::common::IdentMap<
        Ident<Canonical>,
        crate::common::IdentMap<Ident<Canonical>, crate::compiler::VariableMetadata<'arena>>,
    >,
) {
    let sub_model_name: Ident<Canonical> = Ident::new(sub_model.name(db));

    if all_metadata.contains_key(&sub_model_name) {
        return;
    }

    let layout = compute_layout(db, sub_model, project);
    let source_vars = sub_model.variables(db);
    let project_models = project.models(db);

    let mut sub_metadata: crate::common::IdentMap<
        Ident<Canonical>,
        crate::compiler::VariableMetadata<'arena>,
    > = Default::default();

    let mut sorted_names: Vec<&String> = source_vars.keys().collect();
    sorted_names.sort_unstable();

    for name in &sorted_names {
        let svar = &source_vars[name.as_str()];
        let var_ident: Ident<Canonical> = Ident::new(name.as_str());
        let entry = layout.get(name.as_str());
        let (offset, size) = entry.map_or((0, 1), |e| (e.offset, e.size));

        // Build a stub variable with correct dimensions for the sub-model context
        let dims = variable_dimensions(db, *svar, project);
        let stub = build_stub_variable(db, svar, &var_ident, dims);
        let stub: &'arena crate::variable::Variable = arena.alloc(stub);

        sub_metadata.insert(
            var_ident.clone(),
            crate::compiler::VariableMetadata {
                offset: Some(offset),
                size,
                var: stub,
            },
        );

        // Recurse into nested module variables
        if svar.kind(db) == SourceVariableKind::Module {
            let nested_model_name = svar.model_name(db);
            let nested_canonical = canonicalize(nested_model_name);
            if let Some(nested_model) = project_models.get(nested_canonical.as_ref()) {
                build_submodel_metadata(arena, db, *nested_model, project, all_metadata);
            }
        }
    }

    // When LTM is enabled the sub-model is itself LTM-augmented: its layout
    // (from `compute_layout`) carries the synthetic LTM variables, most
    // importantly the per-input-port composite score `$⁚ltm⁚composite⁚{port}`.
    // A parent equation can reference one of these across the module boundary
    // -- the exhaustive-mode input→macro link score is the composite-reference
    // form `"{module}·$⁚ltm⁚composite⁚{port}"` (GH #548) -- and `Context::
    // get_submodel_offset` resolves that by looking the bare LTM var up in
    // *this* sub-model's metadata. Without an entry here the lookup returns
    // `DoesNotExist`, the parent fragment fails to compile, `assemble_module`
    // drops it, and the link score reads a constant 0 -- silently zeroing every
    // loop that runs through the macro. Register the LTM vars (and their
    // implicit helpers) at their `compute_layout` offsets so the cross-module
    // reference resolves the same way the full flattened-offset assembly does.
    if project.ltm_enabled(db) {
        let ltm_vars = model_ltm_variables(db, sub_model, project);
        let dim_context = project_dimensions_context(db, project);
        for ltm_var in &ltm_vars.vars {
            let var_ident: Ident<Canonical> = Ident::new(&ltm_var.name);
            if sub_metadata.contains_key(&var_ident) {
                continue;
            }
            let Some(entry) = layout.get(&ltm_var.name) else {
                continue;
            };
            // A2A link/loop scores carry dimensions; the stub's dummy AST
            // mirrors the layout so any subscripted cross-module read resolves
            // an element offset rather than collapsing to slot 0. Scalar LTM
            // vars (the composite among them) get a plain `Var` stub.
            let dummy_ast = if ltm_var.dimensions.is_empty() {
                None
            } else {
                let dims: Vec<crate::dimensions::Dimension> = ltm_var
                    .dimensions
                    .iter()
                    .filter_map(|name| {
                        let canonical = crate::common::CanonicalDimensionName::from_raw(name);
                        dim_context.get(&canonical).cloned()
                    })
                    .collect();
                Some(crate::ast::Ast::ApplyToAll(
                    dims,
                    crate::ast::Expr2::Const(
                        "0".to_string(),
                        crate::ast::Literal::new(0.0),
                        crate::ast::Loc::default(),
                    ),
                ))
            };
            let stub: &'arena crate::variable::Variable =
                arena.alloc(crate::variable::Variable::Var {
                    ident: var_ident.clone(),
                    ast: dummy_ast,
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
            sub_metadata.insert(
                var_ident,
                crate::compiler::VariableMetadata {
                    offset: Some(entry.offset),
                    size: entry.size,
                    var: stub,
                },
            );
        }

        let ltm_implicit = model_ltm_implicit_var_info(db, sub_model, project);
        for (im_name, meta) in ltm_implicit.iter() {
            let var_ident: Ident<Canonical> = Ident::new(im_name);
            if sub_metadata.contains_key(&var_ident) {
                continue;
            }
            let Some(entry) = layout.get(im_name) else {
                continue;
            };
            // Module-type LTM implicit helpers (PREVIOUS-of-module-output
            // instances) need the `Module` variant and a recursion into their
            // sub-model so a nested cross-module reference resolves; scalar
            // helpers use a plain `Var` stub.
            let stub: &'arena crate::variable::Variable = if meta.is_module {
                let model_name = meta.model_name.as_deref().unwrap_or("");
                if !model_name.is_empty() {
                    let nested_canonical = canonicalize(model_name);
                    if let Some(nested_model) = project_models.get(nested_canonical.as_ref()) {
                        build_submodel_metadata(arena, db, *nested_model, project, all_metadata);
                    }
                }
                arena.alloc(crate::variable::Variable::Module {
                    ident: var_ident.clone(),
                    model_name: Ident::new(model_name),
                    units: None,
                    inputs: vec![],
                    errors: vec![],
                    unit_errors: vec![],
                })
            } else {
                arena.alloc(crate::variable::Variable::Var {
                    ident: var_ident.clone(),
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
                })
            };
            sub_metadata.insert(
                var_ident,
                crate::compiler::VariableMetadata {
                    offset: Some(entry.offset),
                    size: entry.size,
                    var: stub,
                },
            );
        }
    }

    all_metadata.insert(sub_model_name, sub_metadata);
}

/// Pre-computed invariance data for the flow phase, stored on
/// `VarFragmentResult` so `model_flows_invariant` can run its topological
/// fixpoint pass without re-calling `lower_var_fragment` (the compile-time
/// regression fix, GH #712).
///
/// `locally_pure`: the variable's flow-phase expression is invariant assuming
/// every dependency is invariant — i.e., no `TIME`/`PULSE`/`RAMP`/`STEP`/
/// `PREVIOUS`/`EvalModule`/`ModuleInput` appears anywhere in the AST. If
/// `false`, the variable is definitely variant regardless of deps.
///
/// `dep_names`: the canonical names of every dependency whose offset is
/// referenced in the flow-phase expression (excluding the variable's own
/// offset, which is a self-reference). `model_flows_invariant` checks that
/// all of these are in the accumulated invariant set.
///
/// Together, `locally_pure && dep_names ⊆ invariant` is exactly the
/// per-variable verdict the topological pass needs, with no re-lowering.
#[derive(Clone, Debug, PartialEq, salsa::Update)]
pub(crate) struct FlowInvarianceSupport {
    pub locally_pure: bool,
    pub dep_names: std::sync::Arc<std::collections::BTreeSet<String>>,
}

/// Result of per-variable compilation: symbolic bytecodes for each phase.
#[derive(Clone, Debug, PartialEq, salsa::Update)]
pub(crate) struct VarFragmentResult {
    pub fragment: crate::compiler::symbolic::CompiledVarFragment,
    /// Invariance support for the flow phase. `None` when the variable has
    /// no flow phase (not in the flows runlist) or when the noninitial
    /// lowering failed. Used by `model_flows_invariant` to avoid re-lowering.
    pub flow_invariance: Option<FlowInvarianceSupport>,
}

/// Walk `exprs` and push the NAME of every variable referenced by a `Var`,
/// `Subscript`, or `StaticSubscript` node into `out`.
///
/// This is NOT the same as calling `exprs_are_invariant`: it collects names
/// without returning a verdict. It is used by `compute_flow_invariance_support`
/// to determine which variables are actually referenced in the *flow*
/// expression (as opposed to the init expression, which must not pollute the
/// dep_names set).
///
/// The walk is exhaustive over every `Expr`/`BuiltinFn` variant.
fn collect_expr_refs(exprs: &[crate::compiler::Expr], out: &mut HashSet<Ident<Canonical>>) {
    use crate::builtins::BuiltinFn;
    use crate::compiler::Expr;
    use crate::compiler::expr::SubscriptIndex;

    fn walk(expr: &Expr, out: &mut HashSet<Ident<Canonical>>) {
        match expr {
            // Leaf: referenced variable.
            Expr::Var(var, _)
            | Expr::Subscript(var, _, _, _)
            | Expr::StaticSubscript(var, _, _) => {
                out.insert(var.name.clone());
                // For Subscript, also walk the index expressions (they may
                // reference other variables).
                if let Expr::Subscript(_, indices, _, _) = expr {
                    for idx in indices {
                        match idx {
                            SubscriptIndex::Single(e) => walk(e, out),
                            SubscriptIndex::Range(s, e) => {
                                walk(s, out);
                                walk(e, out);
                            }
                        }
                    }
                }
            }

            // No slot reference in these leaves.
            Expr::Const(_, _)
            | Expr::Dt(_)
            | Expr::TempArray(_, _, _)
            | Expr::TempArrayElement(_, _, _, _)
            | Expr::ModuleInput(_, _) => {}

            // Compound expressions: recurse into subexpressions.
            Expr::Op2(_, l, r, _) => {
                walk(l, out);
                walk(r, out);
            }
            Expr::Op1(_, operand, _) => walk(operand, out),
            Expr::If(cond, t, f, _) => {
                walk(cond, out);
                walk(t, out);
                walk(f, out);
            }
            Expr::AssignCurr(_, rhs) | Expr::AssignNext(_, rhs) => walk(rhs, out),
            Expr::AssignTemp(_, rhs, _) => walk(rhs, out),

            // Module evaluation: walk the argument expressions.
            Expr::EvalModule(_, _, _, args) => {
                for arg in args {
                    walk(arg, out);
                }
            }

            // Builtins: walk every argument expression.
            Expr::App(builtin, _) => match builtin {
                BuiltinFn::Inf
                | BuiltinFn::Pi
                | BuiltinFn::Time
                | BuiltinFn::TimeStep
                | BuiltinFn::StartTime
                | BuiltinFn::FinalTime
                | BuiltinFn::IsModuleInput(_, _) => {}

                BuiltinFn::Lookup(table, index, _)
                | BuiltinFn::LookupForward(table, index, _)
                | BuiltinFn::LookupBackward(table, index, _) => {
                    walk(table, out);
                    walk(index, out);
                }

                // Init(a): the initial-values buffer is frozen after the
                // initials phase -- `INIT(dynamic_var)` is run-invariant
                // regardless of what `a` references. Do NOT recurse into `a`
                // (mirroring `builtin_is_invariant` which returns `true`
                // without walking the argument).
                BuiltinFn::Init(_) => {}

                // Previous is already variant (caught by locally_pure), but
                // we still avoid recursing into its argument so the walk stays
                // consistent with the invariance classifier's treatment.
                BuiltinFn::Previous(_, _) => {}

                BuiltinFn::Abs(a)
                | BuiltinFn::Arccos(a)
                | BuiltinFn::Arcsin(a)
                | BuiltinFn::Arctan(a)
                | BuiltinFn::Cos(a)
                | BuiltinFn::Exp(a)
                | BuiltinFn::Int(a)
                | BuiltinFn::Ln(a)
                | BuiltinFn::Log10(a)
                | BuiltinFn::Sign(a)
                | BuiltinFn::Sin(a)
                | BuiltinFn::Sqrt(a)
                | BuiltinFn::Tan(a)
                | BuiltinFn::Size(a)
                | BuiltinFn::Stddev(a)
                | BuiltinFn::Sum(a) => walk(a, out),

                BuiltinFn::Max(a, b) | BuiltinFn::Min(a, b) => {
                    walk(a, out);
                    if let Some(b) = b {
                        walk(b, out);
                    }
                }
                BuiltinFn::SafeDiv(a, b, c) => {
                    walk(a, out);
                    walk(b, out);
                    if let Some(c) = c {
                        walk(c, out);
                    }
                }
                BuiltinFn::Step(a, b) | BuiltinFn::Quantum(a, b) => {
                    walk(a, out);
                    walk(b, out);
                }
                BuiltinFn::Pulse(a, b, c) => {
                    walk(a, out);
                    walk(b, out);
                    if let Some(c) = c {
                        walk(c, out);
                    }
                }
                BuiltinFn::Ramp(a, b, c) => {
                    walk(a, out);
                    walk(b, out);
                    if let Some(c) = c {
                        walk(c, out);
                    }
                }
                BuiltinFn::Sshape(a, b, c) => {
                    walk(a, out);
                    walk(b, out);
                    walk(c, out);
                }
                BuiltinFn::Mean(args) => {
                    for arg in args {
                        walk(arg, out);
                    }
                }
                BuiltinFn::Rank(a, b)
                | BuiltinFn::VectorElmMap(a, b)
                | BuiltinFn::VectorSortOrder(a, b) => {
                    walk(a, out);
                    walk(b, out);
                }
                BuiltinFn::AllocateAvailable(a, b, c) => {
                    walk(a, out);
                    walk(b, out);
                    walk(c, out);
                }
                BuiltinFn::VectorSelect(a, b, c, d, e)
                | BuiltinFn::AllocateByPriority(a, b, c, d, e) => {
                    walk(a, out);
                    walk(b, out);
                    walk(c, out);
                    walk(d, out);
                    walk(e, out);
                }
            },
        }
    }

    for expr in exprs {
        walk(expr, out);
    }
}

/// Compute `FlowInvarianceSupport` for a variable's flow phase, for use by
/// `model_flows_invariant` (GH #712).
///
/// `locally_pure` is determined by running `exprs_are_invariant` with a
/// callback that always returns `Invariant` (so only TIME/PULSE/etc. in the
/// expression can make it `false`).
///
/// `dep_names` is determined by walking the *flow* expression (`flow_var.ast`)
/// and reading the owning variable's name off every reference. This is precise:
/// it considers only flow-expression references, not init-only deps that were
/// never read at dt time. Using the fragment's whole dependency set would
/// over-approximate: a variable `v` with `y = INIT(k)` in its init equation
/// would include `k` in dep_names even though `k` does not appear in `v`'s flow
/// expression, causing `k` being variant to incorrectly classify `v` as variant
/// too.
///
/// Returns `None` if `flow_var` is an `Err` (noninitial lowering failed) or
/// the expression list is empty.
pub(crate) fn compute_flow_invariance_support(
    flow_var: &Result<crate::compiler::Var, crate::common::Error>,
    var_ident_canonical: &Ident<Canonical>,
) -> Option<FlowInvarianceSupport> {
    use crate::compiler::invariance::{RefClass, exprs_are_invariant};
    use std::collections::BTreeSet;
    use std::sync::Arc;

    let flow_var = match flow_var {
        Ok(v) => v,
        Err(_) => return None,
    };
    if flow_var.ast.is_empty() {
        return None;
    }

    // Structural purity check: does the expression contain any variant
    // builtins (TIME, PULSE, RAMP, STEP, PREVIOUS, EvalModule, ModuleInput)?
    // All reference lookups return Invariant so only the builtin arms matter.
    let locally_pure = exprs_are_invariant(&flow_var.ast, &|_var| RefClass::Invariant);

    // Walk the flow expression to collect only the variables actually
    // referenced there (not init-only deps).
    //
    // This used to reverse-map each referenced slot offset through the
    // fragment's private offset layout, with a `debug_assert!` guarding the case
    // where an offset resolved to no owner -- a silently dropped dependency
    // would be INVISIBLE to the invariance fixpoint, the over-classification
    // direction. Reading the name off the reference removes the failure mode
    // rather than guarding it: there is no lookup left to fail.
    let mut referenced: HashSet<Ident<Canonical>> = HashSet::new();
    collect_expr_refs(&flow_var.ast, &mut referenced);

    // The variable's own references are self-references, never dependencies.
    let dep_names: BTreeSet<String> = referenced
        .iter()
        .filter(|name| *name != var_ident_canonical)
        .map(|name| name.as_str().to_string())
        .collect();

    Some(FlowInvarianceSupport {
        locally_pure,
        dep_names: Arc::new(dep_names),
    })
}

/// `reference -> extent of the variable it addresses in whole`: the per-fragment
/// size table `lower_var_fragment` produces, and which BOTH halves of the
/// compile borrow -- the lowering context (`compiler::ContextCore`) and the
/// per-phase emission context (`compiler::ModuleCtx`). This IS
/// `compiler::VarSizes` -- the fragment-side name for the same type, kept so the
/// db-side signatures read in fragment terms.
///
/// Its predecessor carried `(offset, size)` per variable over a private
/// per-fragment layout that existed only so symbolization could undo it. There
/// are no fragment-local offsets any more: emission reads names, and the only
/// surviving question is how big a variable is.
pub(crate) type PerVarSizes = crate::compiler::VarSizes;

/// Flatten a phase's temp-id -> size map into the `(temp_id, size)` vector
/// `PerVarBytecodes::temp_sizes` carries, **ordered by temp id**.
///
/// The ordering is load-bearing, not cosmetic. `temp_sizes` rides on a
/// `PerVarBytecodes`, which is a salsa-cached value with a *derived*
/// `PartialEq`, and `Vec` equality is order-sensitive. Building it straight
/// out of `HashMap::iter` therefore made two otherwise-identical compiles of
/// the same fragment compare unequal whenever the per-process hash seed
/// reordered the map: salsa's backdating stops firing (every downstream
/// consumer re-executes), and the compiled artifact itself stops being
/// reproducible run to run -- the nondeterminism class GH #595 tracks.
///
/// Nothing downstream ever *depended* on the order: the sole consumer,
/// `FragmentMerger::absorb`, folds each entry into a resize-and-`max` over a
/// dense `merged_temp_sizes` vector, which is order-independent, and the
/// merged result is re-emitted densely by `into_per_var_bytecodes`. So this is
/// a pure determinism fix with no bytecode consequence -- which is exactly why
/// it survived undetected.
pub(crate) fn temp_sizes_by_id(temp_sizes_map: &HashMap<u32, usize>) -> Vec<(u32, usize)> {
    let mut temp_sizes: Vec<(u32, usize)> = temp_sizes_map
        .iter()
        .map(|(&id, &size)| (id, size))
        .collect();
    temp_sizes.sort_unstable_by_key(|(id, _)| *id);
    temp_sizes
}

/// Assemble the phase-INVARIANT emission context for one variable's
/// fragment: everything `compile_phase_to_per_var_bytecodes` needs except
/// the phase's own lowered expressions (and the temp sizes derived from
/// them, which that function fills in).
///
/// Every field is a borrow with the caller's lifetime -- the fragment's size
/// table and tables, and the salsa-cached project-global dimension context and
/// converted dimensions, are read in place, never copied into a per-fragment
/// container.
///
/// The empty `runlist_flows`/`temp_sizes` placeholders exist in exactly one
/// place, here, so no emission site can forget which runlist a fragment's
/// expressions belong in.
pub(crate) fn fragment_emit_ctx<'a>(
    model_name: &'a Ident<Canonical>,
    inputs: &'a BTreeSet<Ident<Canonical>>,
    var_sizes: &'a PerVarSizes,
    tables: &'a HashMap<Ident<Canonical>, Vec<crate::compiler::Table>>,
    dimensions: &'a [crate::dimensions::Dimension],
    dimensions_ctx: &'a crate::dimensions::DimensionsContext,
) -> crate::compiler::ModuleCtx<'a> {
    crate::compiler::ModuleCtx {
        ident: model_name,
        inputs,
        temp_sizes: &[],
        runlist_initials_by_var: &[],
        runlist_flows: &[],
        runlist_stocks: &[],
        var_sizes,
        tables,
        dimensions,
        dimensions_ctx,
    }
}

/// Compile one phase's lowered `Vec<Expr>` for a single variable into a
/// layout-independent `PerVarBytecodes`.
///
/// **This is the single fragment emission entry point** (GH #964's "explicit,
/// implicit, and LTM variables share one fragment emission implementation").
/// Five call sites reach it: `compile_var_fragment` (explicit variables),
/// `compile_implicit_var_phase_bytecodes` (SMOOTH/DELAY/TREND helpers, and
/// through it `var_phase_symbolic_fragment_prod`'s parent-sourced arm),
/// `var_phase_symbolic_fragment_prod` itself (the element-cycle SCC graph
/// builder, which must reuse the *exact* production path rather than a
/// re-derivation), and both LTM emitters in `db/ltm/compile.rs`. The two LTM
/// sites used to carry hand-copied 97-line duplicates of this body, which is
/// how the same `temp_sizes` ordering defect came to need fixing in three
/// places at once.
///
/// `base` is the phase-INVARIANT half of the emission context, built once
/// per variable by the caller: its `runlist_flows`/`temp_sizes` are ignored
/// (this function fills both in per phase), and everything else -- the
/// fragment's `var_sizes` and `tables`, the project-global
/// `dimensions`/`dimensions_ctx`, and the module-input set -- is borrowed for
/// the call and never cloned.
///
/// What comes back is codegen's output verbatim: it is already symbolic, so
/// there is nothing between emission and the salsa-cached fragment.
///
/// Returns `None` (loud-safe, never panics) when `exprs` is empty or codegen
/// fails.
pub(crate) fn compile_phase_to_per_var_bytecodes(
    base: &crate::compiler::ModuleCtx<'_>,
    exprs: &[crate::compiler::Expr],
) -> Option<crate::compiler::symbolic::PerVarBytecodes> {
    compile_phase_to_per_var_bytecodes_reporting(base, exprs).ok()
}

/// [`compile_phase_to_per_var_bytecodes`], keeping the emit error instead of
/// discarding it.
///
/// The `Option`-returning form above is what every production emitter wants:
/// a fragment that will not emit is dropped and the caller carries on. But a
/// *diagnostic* caller needs to say which construct codegen refused, and
/// `.ok()?` erased that -- which is why ~1,600 dropped fragments on one real
/// model reported no cause at all. Behavior is unchanged; only the error's
/// visibility is.
pub(crate) fn compile_phase_to_per_var_bytecodes_reporting(
    base: &crate::compiler::ModuleCtx<'_>,
    exprs: &[crate::compiler::Expr],
) -> Result<crate::compiler::symbolic::PerVarBytecodes, String> {
    use crate::compiler::symbolic::PerVarBytecodes;

    if exprs.is_empty() {
        return Err("nothing to emit: the phase lowered to zero expressions".to_string());
    }

    // Extract temp sizes from expressions
    let mut temp_sizes_map: HashMap<u32, usize> = HashMap::new();
    for expr in exprs {
        crate::compiler::extract_temp_sizes_pub(expr, &mut temp_sizes_map);
    }
    let mut temp_sizes: Vec<usize> = vec![0; temp_sizes_map.len()];
    for (id, size) in &temp_sizes_map {
        if (*id as usize) < temp_sizes.len() {
            temp_sizes[*id as usize] = *size;
        }
    }

    // A fragment is one variable's one phase, so the whole phase goes in the
    // flows runlist; the initials/stocks runlists stay empty and the phase
    // distinction is the caller's (which lowered `Vec<Expr>` it passes).
    let emit_ctx = crate::compiler::ModuleCtx {
        runlist_flows: exprs,
        temp_sizes: &temp_sizes,
        ..*base
    };

    let compiled = emit_ctx
        .compile()
        .map_err(|err| format!("codegen rejected the lowered expressions: {err}"))?;

    Ok(PerVarBytecodes {
        symbolic: compiled.compiled_flows,
        graphical_functions: compiled.graphical_functions,
        module_decls: compiled.module_decls,
        static_views: compiled.static_views,
        temp_sizes: temp_sizes_by_id(&temp_sizes_map),
        dim_lists: compiled
            .dim_lists
            .iter()
            .map(|(n, arr)| arr[..(*n as usize)].to_vec())
            .collect(),
    })
}

/// A variable's *symbolic* `PerVarBytecodes` for a phase, sourced through
/// the exact production lowering + emission path (`lower_var_fragment` +
/// `compile_phase_to_per_var_bytecodes`), never a re-derivation.
///
/// This is the cross-member-comparable substrate the element-cycle SCC
/// graph builder consumes: every variable reference in the returned
/// bytecode is a layout-independent
/// `SymVarRef { name, element_offset }`, so a multi-member recurrence
/// SCC's induced element graph can be built across members (the fix for
/// GH #575 -- the prior `Expr::AssignCurr`-mini-slot builder was
/// structurally incapable of cross-member edges). It is the production
/// element-graph source consumed by `symbolic_phase_element_order` and
/// `combine_scc_fragment` (the Phase 2 GH #575 rebuild replaced the prior
/// `Expr`-based accessor entirely).
///
/// This accessor returns the *whole* per-phase symbolic stream verbatim
/// (PREVIOUS/INIT reads included). Which opcodes become element-graph
/// *edges* is the consumer's concern: `symbolic_phase_element_order`'s
/// read-opcode arm inherits `build_var_info`'s exact per-phase
/// PREVIOUS/INIT strip (`SymLoadPrev` -> no edge in either phase;
/// `SymLoadInitial` -> no edge in `Dt`, edge in `Initial`; current-value
/// reads kept), so the element graph MATCHES the engine's actual
/// per-phase data-flow relation rather than over-collecting lagged reads.
/// See that function's rustdoc for the AC4 soundness argument and the
/// exact `db/dep_graph.rs` `build_var_info` line citations. The loud-safe
/// contract documented *here* is a distinct concern -- it is about a
/// node failing to be element-*sourced* (always `None`, never a panic),
/// not about which sourced opcodes are ordering edges.
///
/// The caller-owned, lowering-independent context is built byte-identically
/// to `compile_var_fragment` (same helpers, same order, the default
/// no-module-input wiring `build_var_info(.., &[])` uses):
/// `SccPhase::Dt` selects `per_phase_lowered.noninitial`,
/// `SccPhase::Initial` selects `.initial`.
///
/// A synthetic helper (`$\u{205A}` prefix, absent from `model.variables`)
/// that lands in a recurrence SCC is **parent-sourced**: its symbolic
/// `PerVarBytecodes` is the parent variable's `implicit_vars[index]`
/// compiled through the shared per-phase relation
/// `compile_implicit_var_phase_bytecodes` (the same chain
/// `compile_implicit_var_fragment` runs), so the element-graph builder
/// consumes it exactly like a real member (element-cycle Phase 3 Task 2 /
/// AC3.1, pinned by `synthetic_helper_symbolic_fragment_is_parent_sourced`).
///
/// **Loud-safe contract (the load-bearing invariant -- formalized here).**
/// This accessor returns `None` -- *never* panics, `expect`s, or `unwrap`s
/// on a sourcing failure -- on EVERY way a node fails to be
/// element-sourced:
/// - no `SourceVariable` AND not a parent-sourceable synthetic helper
///   (absent from `model_implicit_var_info`, or the shared per-phase
///   compile failed): `None` (the loud-safe signal -- AC3.2);
/// - `LoweredVarFragment::Fatal` (the variable did not lower at all):
///   explicit `return None`;
/// - the requested phase's `Var::new` errored (`phase_var.ok()?`);
/// - any `compile_phase_to_per_var_bytecodes` failure (empty exprs, the
///   codegen) -- that function is itself total-and-`None`-on-failure.
///
/// `None` propagates loud-safe and all-or-nothing: any in-SCC node that
/// cannot be element-sourced makes `symbolic_phase_element_order` return
/// `None` (its `?` on this call), so `refine_scc_to_element_verdict`
/// yields `SccVerdict::Unresolved`, `resolve_recurrence_sccs` sets
/// `has_unresolved`, and `model_dependency_graph_impl` keeps `has_cycle`
/// and accumulates the `CircularDependency` diagnostic
/// (`dt_scc_map`/`init_scc_map` stays empty, `resolved_sccs` stays empty).
/// The model is rejected loudly -- no panic, no silent miscompile, and the
/// other SCC members are **not** partially resolved (the SCC is rejected
/// as a unit). This contract is regression-pinned by
/// `unsourceable_in_scc_node_falls_back_to_circular_no_panic` (AC3.2,
/// driven through the production `model_dependency_graph` path via the
/// `#[cfg(test)]` `UnsourceableVarsGuard` override) and
/// `var_phase_symbolic_fragment_prod_none_for_absent_var_no_panic`.
pub(crate) fn var_phase_symbolic_fragment_prod(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
    var_name: &str,
    phase: SccPhase,
) -> Option<crate::compiler::symbolic::PerVarBytecodes> {
    use crate::db::var_fragment::{LoweredVarFragment, lower_var_fragment};

    // `#[cfg(test)]` only: an active `UnsourceableVarsGuard` forces this
    // node to take the loud-safe `None` arm, so the AC3.2 regression test
    // can exercise the genuinely-unsourceable in-SCC path through the
    // PRODUCTION `model_dependency_graph` chain (an organic orphan that is
    // neither in `source_vars` nor resolvable via `model_implicit_var_info`
    // is hard to construct deterministically; this is the reliable
    // trigger). It returns the SAME `None` a real no-`SourceVariable`
    // node returns, so the test observes the real loud-safe behavior, not
    // a shim. No effect in non-test builds.
    #[cfg(test)]
    if crate::db::dep_graph::var_is_forced_unsourceable(var_name) {
        return None;
    }

    let source_vars = model.variables(db);
    // No `SourceVariable` (a synthetic INIT/PREVIOUS/SMOOTH/macro-expansion
    // helper, `$\u{205A}` prefix, absent from `model.variables`): before
    // the loud-safe `None`, attempt parent-`implicit_vars` sourcing
    // (element-cycle Phase 3 Task 2 / AC3.1). A synthetic helper that
    // lands in a recurrence SCC has no `SourceVariable` but DOES resolve
    // in `model_implicit_var_info`; its symbolic `PerVarBytecodes` is the
    // parent variable's `implicit_vars[index]` compiled through
    // the SAME shared per-phase relation the production per-variable
    // assembly uses (`compile_implicit_var_phase_bytecodes` -- the exact
    // `parent → parsed.implicit_vars[i] → parse_var → lower_variable →
    // compile` chain `compile_implicit_var_fragment` runs), so
    // the element-graph builder consumes it exactly like a real member
    // (same layout-independent `SymVarRef` form). The element-cycle SCC
    // identification uses the default no-module-input wiring, so source the
    // helper with `module_input_names = &[]` (matching the real-var arm's
    // `lower_var_fragment(.., &[], ..)` below; the symbolic fragment is
    // role-independent, so there is no longer an `is_root` selector).
    // Genuinely unsourceable (absent from `model_implicit_var_info`
    // too, or the shared compile failed) ⇒ `None`, the loud-safe signal
    // (see the rustdoc's loud-safe contract): the SCC stays unresolved and
    // `CircularDependency` is kept -- no panic, no silent miscompile
    // (AC3.2).
    let Some(sv) = source_vars.get(var_name) else {
        let canonical_name = canonicalize(var_name).into_owned();
        let info = model_implicit_var_info(db, model, project);
        let meta = info.get(&canonical_name)?;
        let is_initial = matches!(phase, SccPhase::Initial);
        // The cycle-gate probe wants only the fragment; failures stay silent
        // here (the production assembly path attributes them, GH #1000).
        return compile_implicit_var_phase_bytecodes(
            db,
            meta,
            model,
            project,
            &[],
            is_initial,
            None,
        );
    };

    // Caller-owned, lowering-independent context, read EXACTLY as
    // `compile_var_fragment` reads it (mirror byte-for-byte): the
    // salsa-cached project-global dimension context and converted dims.
    let dim_context = project_dimensions_context(db, project);
    let converted_dims = project_converted_dimensions(db, project);
    let model_name_ident = Ident::new(model.name(db));
    let inputs: BTreeSet<Ident<Canonical>> = BTreeSet::new();
    let module_models = model_module_map(db, model, project).clone();

    let lowered = lower_var_fragment(
        db,
        *sv,
        model,
        project,
        &[],
        converted_dims,
        dim_context,
        &model_name_ident,
        &module_models,
        &inputs,
    );

    let (per_phase_lowered, tables, var_sizes) = match lowered {
        LoweredVarFragment::Lowered {
            per_phase_lowered,
            tables,
            var_sizes,
            ..
        } => (per_phase_lowered, tables, var_sizes),
        // The variable did not lower at all => `None` (loud-safe).
        LoweredVarFragment::Fatal { .. } => return None,
    };

    // `SccPhase::Dt` selects the non-initial (dt/flow) lowering;
    // `SccPhase::Initial` selects the initial lowering -- the same
    // selection `compile_var_fragment` makes per phase.
    let phase_var = match phase {
        SccPhase::Dt => per_phase_lowered.noninitial,
        SccPhase::Initial => per_phase_lowered.initial,
    };
    // The phase's `Var::new` errored => cannot source its production
    // lowered exprs => `None` (loud-safe).
    let var = phase_var.ok()?;

    let base_ctx = fragment_emit_ctx(
        &model_name_ident,
        &inputs,
        &var_sizes,
        &tables,
        converted_dims,
        dim_context,
    );
    compile_phase_to_per_var_bytecodes(&base_ctx, &var.ast)
}

/// Segment one member's symbolic opcode stream into per-element slices,
/// keyed by `element_offset`.
///
/// A per-element slice for element `e` is the run of opcodes up to and
/// including the **write** opcode whose `var.name == member` and
/// `var.element_offset == e` (`AssignCurr | AssignConstCurr |
/// BinOpAssignCurr`). This is the *exact* segmentation
/// `crate::db::dep_graph::symbolic_phase_element_order` performs to build
/// the SCC element graph (GH #575) -- the verdict and the combined
/// fragment MUST agree on segment boundaries or `element_order` would
/// reference a slice the combiner cannot reproduce, so the two share this
/// definition's contract.
///
/// A trailing `Ret` is stripped first (the combined fragment carries one
/// terminal `Ret`). Any opcodes after the member's final per-element write
/// (before the stripped `Ret`) are appended to the last element's slice so
/// no opcode is silently dropped -- a tail with no write is a malformed
/// fragment (`Err`).
///
/// Loud-safe failures (return `Err`, caller keeps `CircularDependency` --
/// NEVER a panic, NEVER a silently-malformed slice):
/// - a duplicate write for the same element (ambiguous segmentation);
/// - opcodes present but no per-element write at all (not element-
///   sourceable in the simple per-element shape, mirroring
///   `symbolic_phase_element_order`'s `saw_write` guard);
/// - a backward jump whose target lies in an EARLIER segment. Segments are
///   emitted in `element_order`, not in their original order, and a jump
///   offset is relative, so a jump that escaped its own segment would land on
///   whatever opcode happened to sit that far back after the interleave -- a
///   silent miscompile with no bad id and no bad reference to notice. Codegen
///   cannot currently produce one (a `BeginIter` loop writes a TEMP through
///   `StoreIterElement`, and a member's per-element `AssignCurr` is emitted
///   after `EndIter`, so a loop is always wholly inside one segment), which is
///   exactly why it is worth checking rather than asserting in prose: this is
///   an assumption about a DIFFERENT file's emission shape, and nothing else
///   would notice it changing.
///
/// Consumed by `combine_scc_fragment`, which `assemble_module` invokes
/// for every resolved recurrence SCC (the Subcomponent B Task 6
/// production consumer -- the dt flows runlist and the synthetic-ident
/// init `SymbolicCompiledInitial` path).
fn segment_member_by_element(
    member: &str,
    code: &[crate::compiler::symbolic::SymbolicOpcode],
) -> Result<HashMap<usize, Vec<crate::compiler::symbolic::SymbolicOpcode>>, String> {
    use crate::compiler::symbolic::SymbolicOpcode;

    // Strip a trailing Ret -- the combined fragment appends a single Ret.
    let end = if code.last() == Some(&SymbolicOpcode::Ret) {
        code.len() - 1
    } else {
        code.len()
    };
    let body = &code[..end];

    // The opcode that terminates one of THIS member's per-element segments. A
    // write to a *different* member, or a `BinOpAssignNext` (a stock update,
    // not a per-element current-value write of this member), does not --
    // exactly the `symbolic_phase_element_order` rule.
    let write_element = |op: &SymbolicOpcode| -> Option<usize> {
        match op {
            SymbolicOpcode::AssignCurr { var }
            | SymbolicOpcode::AssignConstCurr { var, .. }
            | SymbolicOpcode::BinOpAssignCurr { var, .. }
                if var.name.as_str() == member =>
            {
                Some(var.element_offset)
            }
            _ => None,
        }
    };

    // Jump containment. `lower_bound[pc]` is the first index of the segment
    // `pc` ends up in: segments start just after the previous per-element
    // write, except that opcodes trailing the FINAL write are appended to the
    // last segment rather than starting a new one (see below), so their bound
    // is that segment's start.
    let mut lower_bound: Vec<usize> = Vec::with_capacity(body.len());
    let mut start = 0usize;
    for (pc, op) in body.iter().enumerate() {
        lower_bound.push(start);
        if write_element(op).is_some() {
            start = pc + 1;
        }
    }
    if let Some(last_write) = body.iter().rposition(|op| write_element(op).is_some())
        && last_write + 1 < body.len()
    {
        let last_segment_start = lower_bound[last_write];
        for bound in &mut lower_bound[last_write + 1..] {
            *bound = last_segment_start;
        }
    }
    for (pc, op) in body.iter().enumerate() {
        let Some(offset) = op.jump_offset() else {
            continue;
        };
        let target = pc as isize + offset as isize;
        // Backward-or-self and not before the segment's own start. The second
        // half is what segment reordering needs; the first is what makes the
        // lower bound sufficient, and it holds for every jump the instruction
        // set can express (`jump_offset` reports only backward jumps). A
        // future forward jump would need its own upper-bound reasoning, and
        // should trip this rather than silently inherit an argument that does
        // not cover it.
        if target < lower_bound[pc] as isize || target > pc as isize {
            return Err(format!(
                "SCC member `{member}` has a jump at opcode {pc} targeting \
                 {target}, outside the per-element segment starting at {}; \
                 the segments are reordered by element_order, so a jump that \
                 leaves its own segment cannot be relocated safely",
                lower_bound[pc]
            ));
        }
    }

    let mut segments: HashMap<usize, Vec<SymbolicOpcode>> = HashMap::new();
    let mut current: Vec<SymbolicOpcode> = Vec::new();
    let mut last_written_elem: Option<usize> = None;

    for op in body {
        current.push(op.clone());
        if let Some(elem) = write_element(op) {
            if segments.contains_key(&elem) {
                return Err(format!(
                    "SCC member `{member}` has a duplicate per-element \
                     write for element {elem}; combined fragment cannot \
                     be unambiguously segmented"
                ));
            }
            segments.insert(elem, std::mem::take(&mut current));
            last_written_elem = Some(elem);
        }
    }

    // Any trailing opcodes after the last write belong to the last
    // element's segment (dropping them would change semantics). With no
    // write at all this member is not element-sourceable -- loud-safe.
    if !current.is_empty() {
        match last_written_elem {
            Some(elem) => {
                segments
                    .get_mut(&elem)
                    .expect("last_written_elem indexes an inserted segment")
                    .extend(current);
            }
            None => {
                return Err(format!(
                    "SCC member `{member}` has no per-element write \
                     opcode; not element-sourceable for the combined \
                     fragment"
                ));
            }
        }
    }

    Ok(segments)
}

/// Interleave a multi-member recurrence SCC's per-element symbolic
/// segments into ONE combined `PerVarBytecodes`, following the SCC's
/// element-acyclic `element_order`.
///
/// `member_fragments` maps each SCC member's canonical name to its
/// *symbolic* `PerVarBytecodes` for the SCC's phase (obtained by the
/// caller via `var_phase_symbolic_fragment_prod(.., scc.phase)` -- the
/// exact production emission path, never a re-derivation). The
/// result is a single fragment whose per-element writes appear in
/// `scc.element_order`, with each write keeping its **original**
/// `SymVarRef { name, element_offset }` (only segment ordering changes).
/// `resolve_module` therefore maps every write to the same model slot it
/// would have without the SCC, so variable layout offsets and the results
/// offset map are unchanged and per-variable result series stay
/// individually addressable (AC2.3).
///
/// **This is the per-element-granular generalization of
/// `concatenate_fragments`.** Resources are MEMBER-scoped, not
/// element-scoped: each member's fragment is absorbed into the shared
/// `FragmentMerger` exactly ONCE (in `element_order`'s member
/// first-encounter order, so the offset assignment is deterministic),
/// yielding that member's resource base offsets and merging its
/// side-channels (literals, GFs, modules, views, temps, dim-lists) the
/// same way `concatenate_fragments` merges a fragment. Every segment of
/// that member is then renumbered by the member's offsets. The two
/// consumers share `FragmentMerger`/`renumber_opcode` so the multi-layer
/// resource accounting cannot drift.
///
/// Loud-safe (`Err`, caller keeps `CircularDependency` -- never a panic,
/// never a malformed fragment):
/// - a member named in `element_order` has no supplied fragment (the Task
///   4 accessor returned `None` -- unsourceable);
/// - a member's fragment cannot be cleanly segmented (missing / duplicate
///   / no-write element segment -- `segment_member_by_element`);
/// - an `(member, element)` entry in `element_order` has no matching
///   segment;
/// - a resource-ID renumber overflows its target ID type.
///
/// `assemble_module` (Subcomponent B Task 6) invokes this for every
/// resolved recurrence SCC: it skips each member's per-variable fragment
/// in the dt-flows and init collection loops and injects this combined
/// fragment at the first member's runlist slot (the dt fragment into
/// `flow_frags`, the init fragment as one synthetic-ident
/// `SymbolicCompiledInitial`).
pub(crate) fn combine_scc_fragment(
    scc: &ResolvedScc,
    member_fragments: &HashMap<Ident<Canonical>, crate::compiler::symbolic::PerVarBytecodes>,
) -> Result<crate::compiler::symbolic::PerVarBytecodes, String> {
    use crate::compiler::symbolic::{
        ContextResourceCounts, FragmentMerger, FragmentResourceOffsets, SymbolicOpcode,
        renumber_opcode,
    };

    // Absorb each member ONCE, in `element_order`'s member first-encounter
    // order, so per-member resource offsets are assigned deterministically
    // (the interleave is a pure reordering => byte-stable output, AC2.3).
    // The combined fragment is itself a fragment re-fed to
    // `concatenate_fragments` at assembly, so it is built in an isolated
    // resource namespace (`ctx_base = default`), exactly as a per-variable
    // fragment is.
    let mut merger = FragmentMerger::new(&ContextResourceCounts::default());
    let mut absorbed: HashMap<Ident<Canonical>, FragmentResourceOffsets> = HashMap::new();
    // Per-member, per-element renumbered segments. Keyed by the same
    // `(member, element)` identity `element_order` carries.
    let mut renumbered_segments: HashMap<(Ident<Canonical>, usize), Vec<SymbolicOpcode>> =
        HashMap::new();

    for (member, _elem) in &scc.element_order {
        if absorbed.contains_key(member) {
            continue;
        }
        let frag = member_fragments.get(member).ok_or_else(|| {
            format!(
                "SCC member `{}` has no supplied symbolic fragment \
                 (unsourceable); keeping CircularDependency",
                member.as_str()
            )
        })?;
        // `absorb` merges this member's side-channels (de-duplicating its
        // GF blocks against the running merge -- #582) and returns its flat
        // resource base offsets plus the per-slot GF remap -- the exact
        // per-fragment prologue `concatenate_fragments` runs.
        let (off, gf_remap) = merger.absorb(frag)?;
        absorbed.insert(member.clone(), off);

        // Segment the member's symbolic code on its per-element write
        // opcodes (identical contract to the Task 4 verdict builder), then
        // renumber every opcode of every segment by THIS member's offsets
        // and GF remap.
        let segments = segment_member_by_element(member.as_str(), &frag.symbolic.code)?;
        for (elem, ops) in segments {
            let mut renumbered = Vec::with_capacity(ops.len());
            for op in &ops {
                renumbered.push(renumber_opcode(
                    op,
                    off.lit_offset,
                    &gf_remap,
                    off.mod_offset,
                    off.view_offset,
                    off.temp_offset,
                    off.dl_offset,
                )?);
            }
            renumbered_segments.insert((member.clone(), elem), renumbered);
        }
    }

    // Emit the renumbered segments in `element_order`. Every entry must
    // map to exactly one segment (a missing one is loud-safe). Each
    // segment is consumed exactly once: a duplicate `(member, element)` in
    // `element_order` (which the Task 4 builder cannot produce -- nodes
    // are unique) would try to reuse a removed segment and fail loud-safe.
    let mut combined_code: Vec<SymbolicOpcode> = Vec::new();
    for (member, elem) in &scc.element_order {
        let seg = renumbered_segments
            .remove(&(member.clone(), *elem))
            .ok_or_else(|| {
                format!(
                    "SCC element_order references `{}`[{}] but no such \
                     per-element segment exists in its fragment; keeping \
                     CircularDependency",
                    member.as_str(),
                    elem
                )
            })?;
        combined_code.extend(seg);
    }

    Ok(merger.into_per_var_bytecodes(combined_code))
}

/// Renumber the INITIALS phase into one `SymbolicCompiledInitial` per fragment.
///
/// The other two phases go through `concatenate_fragments_with_gf`, which
/// merges their fragments into a single stream. Initials cannot: `eval_initials`
/// runs them one at a time, so each keeps its own bytecode -- and therefore its
/// own literal pool, which is why every initial is renumbered at literal offset
/// 0 rather than at a running one.
///
/// Everything else must still land where the all-phases merge put it (M8): the
/// module / view / dim-list offsets are the running counts over the preceding
/// initials, exactly the bases `FragmentMerger::absorb_non_gf` would hand out,
/// and the GF base comes from the shared dedup (initial `i` is `all_frags[i]`,
/// since initials come first). Temps are NOT advanced (#583) -- they recycle
/// into the one identity pool every phase shares.
///
/// This is a third hand-rolled copy of the merger's flat accounting, kept
/// because of the literal-pool difference above. It is a free function rather
/// than an inline loop specifically so the M8 property can drive it: as an
/// inline loop, freezing `init_view_off` or `init_dl_off` left the entire
/// repository green.
///
/// `Err` on M3, for the same reason `absorb_non_gf` checks its own bases: these
/// offsets index the all-phases tables the module reports, so a silently
/// truncated one produces a well-formed initial that names a different module,
/// view or dimension list. The bound itself is not restated here -- the running
/// offsets are `usize` and each is narrowed by `symbolic::resource_base`, the
/// one place that knows both a base and the length of the fragment about to use
/// it, so an initial that carries none of a resource is exempt exactly as a
/// merged fragment is.
pub(crate) fn renumber_initials_phase(
    initial_frags: &[(String, &crate::compiler::symbolic::PerVarBytecodes)],
    gf_dedup: &crate::compiler::symbolic::GfDedup,
) -> Result<Vec<crate::compiler::symbolic::SymbolicCompiledInitial>, String> {
    use crate::compiler::symbolic::{
        SymbolicByteCode, SymbolicCompiledInitial, renumber_opcode, resource_base,
    };

    let mut compiled_initials: Vec<SymbolicCompiledInitial> = Vec::new();
    let mut mod_off: usize = 0;
    let mut view_off: usize = 0;
    let temp_off: u32 = 0;
    let mut dl_off: usize = 0;
    for (i, (name, bc)) in initial_frags.iter().enumerate() {
        let gf_remap = gf_dedup.remap(i);
        let mod_base = resource_base(0, mod_off, bc.module_decls.len(), "module declaration")?;
        let view_base = resource_base(0, view_off, bc.static_views.len(), "static view")?;
        let dl_base = resource_base(0, dl_off, bc.dim_lists.len(), "dimension list")?;
        let renumbered_code = bc
            .symbolic
            .code
            .iter()
            .map(|op| renumber_opcode(op, 0, gf_remap, mod_base, view_base, temp_off, dl_base))
            .collect::<Result<Vec<_>, _>>()?;
        compiled_initials.push(SymbolicCompiledInitial {
            ident: Ident::new(name),
            bytecode: SymbolicByteCode {
                literals: bc.symbolic.literals.clone(),
                code: renumbered_code,
            },
        });
        mod_off += bc.module_decls.len();
        view_off += bc.static_views.len();
        dl_off += bc.dim_lists.len();
    }
    Ok(compiled_initials)
}

/// Assemble a complete CompiledModule from per-variable fragments.
///
/// Salsa-tracked: the per-module assembly (fragment concatenation, SCC
/// combined-fragment build, GF dedup, resolve) is memoized so an unchanged
/// module (same `model`/`project`/`is_root`/`module_inputs`) is a pure
/// cache hit -- no re-concatenation, no re-resolve. The success payload rides
/// behind an `Arc` so the tracked-fn return value is `salsa::Update` (its
/// inner `CompiledModule` derives `Update` via the per-field `PartialEq`
/// fallback for the opaque bytecode side-channels) and salsa's clone-out on
/// each cache-hit read is a single refcount bump rather than a deep bytecode
/// clone.
///
/// `module_inputs` is an interned `ModuleInputSet` (the sorted canonical input
/// names). The empty set is the no-inputs case and, being a single interned
/// id, shares one cache entry across all no-input callers.
#[salsa::tracked]
pub fn assemble_module<'db>(
    db: &'db dyn Db,
    model: SourceModel,
    project: SourceProject,
    is_root: bool,
    module_inputs: ModuleInputSet<'db>,
) -> Result<std::sync::Arc<crate::bytecode::CompiledModule>, String> {
    use crate::compiler::symbolic::{
        ContextResourceCounts, SymbolicCompiledModule, concatenate_fragments_with_gf,
        merge_context_side_channels, resolve_module,
    };

    // The interned set stores the sorted canonical names; the plain lowering
    // helpers (`compile_implicit_var_fragment` and friends) still take
    // `&[String]`, so read it back as a slice.
    let module_input_names = module_inputs.names(db);
    // Reconstruct the `BTreeSet<Ident<Canonical>>` the assembly logic (the
    // `is_module_input` predicate, the module-input exclusion in the stocks
    // phase) consumes -- the exact inverse of the input set's key derivation.
    let canonical_inputs = module_inputs.canonical_input_set(db);
    let dep_graph = model_dependency_graph(db, model, project, module_inputs);
    if dep_graph.has_cycle {
        let msg = format!("model '{}' has circular dependencies", model.name(db));
        return Err(msg);
    }
    // `compute_layout` returns the role-independent *body* layout (offsets
    // from 0). The root module relocates it by `IMPLICIT_VAR_COUNT` to
    // reserve the implicit-global slots; every fragment `SymVarRef` and
    // module-decl `off` is resolved against this final layout, so the root
    // shift lands once here and the submodule path uses the body layout
    // verbatim (the parent relocates a submodule via its module-decl `off`,
    // which already comes from the parent's shifted layout). The shift logic
    // lives in `VariableLayout::root_shifted`, shared with
    // `calc_flattened_offsets_incremental` so the two stay in lockstep.
    let body_layout = compute_layout(db, model, project);
    let root_layout;
    let layout: &crate::compiler::symbolic::VariableLayout = if is_root {
        root_layout = body_layout.root_shifted();
        &root_layout
    } else {
        body_layout
    };
    // Fail fast (before compiling thousands of fragments) when the layout
    // exceeds the bytecode's u16-addressable slot range. resolve_var_ref has
    // a defense-in-depth checked cast, but by then the expensive per-variable
    // compilation has already run; checking here surfaces one clear error
    // immediately. See `check_layout_addressable` for why a silent overflow
    // corrupts every result.
    crate::compiler::symbolic::check_layout_addressable(layout.n_slots, model.name(db))?;
    let source_vars = model.variables(db);
    let implicit_info = model_implicit_var_info(db, model, project);
    let model_name = model.name(db).clone();

    // Pre-compile all fragments (explicit + implicit) into a combined map
    let mut all_fragments: HashMap<String, VarFragmentResult> = HashMap::new();

    for (name, svar) in source_vars.iter() {
        if let Some(result) = compile_var_fragment(db, *svar, model, project, module_inputs) {
            all_fragments.insert(name.clone(), result.clone());
        }
    }

    for (name, meta) in implicit_info.iter() {
        if let Some(result) =
            compile_implicit_var_fragment(db, meta, model, project, dep_graph, module_input_names)
        {
            all_fragments.insert(name.clone(), result);
        }
    }

    // Pass 3: LTM synthetic variables (only when ltm_enabled).
    //
    // LTM link-score, loop-score, and relative-score equations are
    // compiled here and appended to the flows runlist. When ltm_enabled
    // is false this pass is skipped entirely (AC1.5). When the model
    // has no feedback loops the LTM variable list is empty (AC1.4).
    //
    // LTM vars have no dt-phase ordering constraints with regular
    // variables because PREVIOUS reads from the previous timestep's
    // committed values. They can be appended to the end of the flows
    // runlist.
    let mut ltm_flow_names: Vec<String> = Vec::new();
    if project.ltm_enabled(db) {
        // GH #486's non-Euler rejection is NOT enforced per-module here: the
        // integration method that actually runs is a single, main-model-
        // governed property of the whole assembled simulation (see
        // `assemble_simulation`'s `Specs` selection and the
        // `ltm_non_euler_guard` it calls), so a per-module check against each
        // module's own specs would both miss the real hazard (a stock-free
        // main overriding to RK4 with the stock in a submodel) and fire
        // spuriously (a submodel overriding to RK4 under a Euler main). The
        // rejection is resolved once, against the main-governed method, in
        // `assemble_simulation`.
        let ltm_vars = model_ltm_variables(db, model, project);

        for ltm_var in &ltm_vars.vars {
            let ltm_var_canonical = canonicalize(&ltm_var.name).into_owned();

            // Select and compile this LTM var's fragment. The
            // selection logic (salsa-cached `(from, to)` path vs.
            // direct compilation of the prepared equation) lives in
            // `compile_ltm_synthetic_fragment` so the diagnostic pass
            // (`model_ltm_fragment_diagnostics`) detects the exact same
            // compile failures this assembly pass would silently drop.
            let fragment_result = compile_ltm_synthetic_fragment(db, ltm_var, model, project);

            if let Some(result) = fragment_result {
                // Drop LTM fragments whose symbolic variable references can't
                // be resolved in this model's layout.  This happens when
                // sub-model LTM equations reference implicit stdlib module
                // instance names (e.g. "smth1") that only exist in the root
                // model's namespace under qualified names like
                // "$:var_name:0:smth1".  Silently dropping these is correct:
                // the root model generates its own LTM vars using the
                // qualified names, so sub-model LTM vars for the same modules
                // would be duplicates anyway.
                if crate::compiler::symbolic::fragment_vars_in_layout(&result.fragment, layout) {
                    all_fragments.insert(ltm_var_canonical.clone(), result);
                    ltm_flow_names.push(ltm_var_canonical);
                }
            }
        }

        // Also compile the implicit helpers / modules (PREVIOUS instances)
        // from LTM equations. Each implicit variable rides on its
        // `LtmImplicitVarMeta` (captured when `model_ltm_implicit_var_info`
        // parsed the LTM equations), so no parent equation is re-parsed here.
        // Iteration is name-sorted for deterministic compile order.
        let ltm_implicit = model_ltm_implicit_var_info(db, model, project);
        let mut implicit_names: Vec<&String> = ltm_implicit.keys().collect();
        implicit_names.sort();
        for im_name in implicit_names {
            if all_fragments.contains_key(im_name.as_str()) {
                continue;
            }
            let meta = &ltm_implicit[im_name];
            // Since LTM implicit vars don't have a parent SourceVariable, we
            // compile them directly from the captured implicit variable.
            let im_fragment = compile_ltm_implicit_var_fragment(
                db,
                meta,
                model,
                project,
                dep_graph,
                module_input_names,
                None,
            );
            if let Some(result) = im_fragment {
                // Same layout check as for main LTM vars above.
                if crate::compiler::symbolic::fragment_vars_in_layout(&result.fragment, layout) {
                    all_fragments.insert(im_name.clone(), result);
                }
            }
        }
    }

    // Module input variables have their values provided by the parent
    // model via EvalModule/LoadModuleInput. Their compiled bytecodes
    // consist of LoadModuleInput -> AssignCurr, which copies the
    // parent-provided value into the sub-model's local slot. This must
    // happen during initials and flows phases. Only the stocks phase
    // excludes module inputs (matching the monolithic path which uses
    // `!instantiation.contains(id) && (is_stock || is_module)` for stocks).
    let is_module_input =
        |var_name: &str| -> bool { canonical_inputs.contains(&*canonicalize(var_name)) };

    // ── Combined per-element fragments for resolved recurrence SCCs ─────
    //
    // A multi-member (or single-variable) recurrence SCC whose induced
    // element graph the cycle gate proved acyclic (`dep_graph
    // .resolved_sccs`, populated by the Task 4 symbolic verdict) is
    // lowered as ONE combined `PerVarBytecodes` whose per-element writes
    // follow the SCC's verified `element_order` (Task 5
    // `combine_scc_fragment`), instead of the members' individual
    // one-contiguous-block-per-variable fragments -- the latter cannot
    // express the required cross-member per-element interleaving. Each
    // member's symbolic fragment is sourced via the EXACT production
    // emission path (`var_phase_symbolic_fragment_prod`, the
    // Task 4 accessor -- never a re-derivation), so every write keeps its
    // original `SymVarRef { name, element_offset }`; `resolve_module`
    // therefore maps each write to the same model slot the acyclic layout
    // assigns and the results offset map is unchanged (AC2.3).
    //
    // Two combined fragments per SCC are built up-front so they OUTLIVE
    // the `concatenate_fragments` / init-renumber calls below (the
    // `flow_frags`/`initial_frags` vectors hold `&` borrows into these):
    //  * the DT combined fragment (sourced from each member's
    //    `SccPhase::Dt` symbolic fragment), injected into the flows
    //    runlist -- only `phase == Dt` SCCs (an `Initial`-phase SCC is
    //    stock-backed and stocks are not flow variables).
    //  * the INIT combined fragment (sourced from each member's
    //    `SccPhase::Initial` symbolic fragment), injected into the
    //    initials runlist via the Task 1 spike's single synthetic-ident
    //    `SymbolicCompiledInitial` mechanism -- built for EVERY resolved
    //    SCC (both phases), because a `Dt`-phase aux SCC's members carry
    //    the SAME recurrence in their init equations and the initials
    //    runlist groups BOTH phases contiguously (see the
    //    `build_scc_grouping(false)` runlist comment). The SCC's
    //    `element_order` (dt order for a `phase: Dt` SCC) is valid for
    //    the init interleave because a same-equation aux's init and dt
    //    element graphs are structurally identical; if they ever diverge
    //    (a member's init fragment cannot be segmented to match
    //    `element_order`) `combine_scc_fragment` returns a loud-safe
    //    `Err` and assembly fails with an Assembly diagnostic rather than
    //    miscompiling.
    //
    // Loud-safe: an unsourceable member (`var_phase_symbolic_fragment_prod`
    // returned `None`) or a `combine_scc_fragment` error accumulates an
    // Assembly diagnostic and aborts assembly (mirrors the existing
    // missing-fragment / concatenate-error pattern); the combined
    // fragment is NEVER silently dropped or partially injected.
    let resolved_sccs = &dep_graph.resolved_sccs;
    let combine_scc_for_phase = |scc: &ResolvedScc,
                                 phase: SccPhase|
     -> Result<crate::compiler::symbolic::PerVarBytecodes, String> {
        let mut member_fragments: HashMap<
            Ident<Canonical>,
            crate::compiler::symbolic::PerVarBytecodes,
        > = HashMap::with_capacity(scc.members.len());
        for member in &scc.members {
            let frag = var_phase_symbolic_fragment_prod(
                db,
                model,
                project,
                member.as_str(),
                phase.clone(),
            )
            .ok_or_else(|| {
                format!(
                    "resolved recurrence SCC member `{}` has no \
                         sourceable symbolic fragment for its phase; \
                         cannot build the combined per-element fragment",
                    member.as_str()
                )
            })?;
            member_fragments.insert(member.clone(), frag);
        }
        combine_scc_fragment(scc, &member_fragments)
    };

    // DT combined fragments, indexed parallel to `resolved_sccs`
    // (`None` for an `Initial`-phase SCC -- not a flow). INIT combined
    // fragments for every SCC. Both owned here to the end of
    // `assemble_module`.
    let mut dt_combined: Vec<Option<crate::compiler::symbolic::PerVarBytecodes>> =
        Vec::with_capacity(resolved_sccs.len());
    let mut init_combined: Vec<crate::compiler::symbolic::PerVarBytecodes> =
        Vec::with_capacity(resolved_sccs.len());
    for scc in resolved_sccs.iter() {
        let dt = if scc.phase == SccPhase::Dt {
            Some(combine_scc_for_phase(scc, SccPhase::Dt)?)
        } else {
            None
        };
        let init = combine_scc_for_phase(scc, SccPhase::Initial)?;
        dt_combined.push(dt);
        init_combined.push(init);
    }

    // Member-name -> resolved-SCC index. A member is in at most one SCC
    // (the SCCs in `resolved_sccs` are pairwise disjoint -- see
    // `scc_map_from_resolved`), so this is well-defined.
    let scc_of_member: HashMap<&str, usize> = resolved_sccs
        .iter()
        .enumerate()
        .flat_map(|(idx, scc)| scc.members.iter().map(move |m| (m.as_str(), idx)))
        .collect();

    // Collect fragments for each phase, tracking missing variables.
    //
    // Flow fragments carry a run-invariance flag (GH #712). The fragments are
    // collected in runlist (topological) order; after collection they are
    // stably partitioned so every invariant fragment precedes every dynamic
    // one, and the invariant prefix's opcode length is recorded as the split
    // boundary. Only ordinary source flow variables (in `flows_invariant`) are
    // invariant; SCC-combined, LTM, and implicit-helper fragments are always
    // dynamic (variant), so they keep `false`.
    // `model_flows_invariant` guards internally (returns empty for
    // non-root); call it unconditionally so the check lives in one place.
    let flows_invariant = model_flows_invariant(db, model, project, is_root, module_inputs);
    let mut initial_frags: Vec<(String, &crate::compiler::symbolic::PerVarBytecodes)> = Vec::new();
    let mut flow_frags: Vec<(bool, &crate::compiler::symbolic::PerVarBytecodes)> = Vec::new();
    let mut stock_frags: Vec<&crate::compiler::symbolic::PerVarBytecodes> = Vec::new();
    let mut missing_vars: Vec<String> = Vec::new();

    // Track which SCCs have already had their combined fragment injected
    // in each runlist. Task 5b guarantees a resolved SCC's members are a
    // contiguous, byte-stable block at the SCC's topological slot, so
    // "inject at the first member encountered, skip the rest" lands the
    // combined fragment in the correct relative position. The runlist
    // `Vec<String>` itself is salsa-owned and NOT mutated (we skip during
    // collection, never remove).
    let mut injected_init_sccs: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let mut injected_flow_sccs: std::collections::HashSet<usize> = std::collections::HashSet::new();

    for var_name in &dep_graph.runlist_initials {
        if let Some(&scc_idx) = scc_of_member.get(var_name.as_str()) {
            // A resolved-SCC member: its per-ident init fragment is
            // SUBSUMED by the SCC's single combined init fragment. Inject
            // that combined fragment once, at the first member of this
            // SCC seen in the initials runlist, under a synthetic ident
            // (`$⁚scc⁚init⁚{n}`). The spike verified `resolve_module` /
            // `eval_initials` consume `compiled_initials` positionally
            // (ident-agnostic; offsets re-derived from the bytecode's
            // `AssignCurr` operands), so one `SymbolicCompiledInitial`
            // may write every member's init slots.
            if injected_init_sccs.insert(scc_idx) {
                let synthetic_ident = format!("$\u{205A}scc\u{205A}init\u{205A}{scc_idx}");
                initial_frags.push((synthetic_ident, &init_combined[scc_idx]));
            }
            // Non-first members (and the first, after injection): skip
            // the per-ident push entirely.
            continue;
        }
        if let Some(result) = all_fragments.get(var_name)
            && let Some(ref bc) = result.fragment.initial_bytecodes
        {
            initial_frags.push((var_name.clone(), bc));
        } else if !is_module_input(var_name) {
            missing_vars.push(var_name.clone());
        }
    }

    for var_name in &dep_graph.runlist_flows {
        if let Some(&scc_idx) = scc_of_member.get(var_name.as_str())
            && let Some(ref combined) = dt_combined[scc_idx]
        {
            // A `phase == Dt` resolved-SCC member: its per-variable flow
            // fragment is subsumed by the SCC's combined dt fragment.
            // Push the combined fragment once, at the first member of
            // this SCC encountered in the flows runlist; skip the rest.
            // An SCC reads co-member current values within the dt phase, so it
            // is conservatively dynamic (`false`).
            if injected_flow_sccs.insert(scc_idx) {
                flow_frags.push((false, combined));
            }
            continue;
        }
        if let Some(result) = all_fragments.get(var_name)
            && let Some(ref bc) = result.fragment.flow_bytecodes
        {
            let invariant = flows_invariant.contains(var_name);
            flow_frags.push((invariant, bc));
        } else if !is_module_input(var_name) {
            missing_vars.push(var_name.clone());
        }
    }

    for var_name in &dep_graph.runlist_stocks {
        if is_module_input(var_name) {
            continue;
        }
        if let Some(result) = all_fragments.get(var_name)
            && let Some(ref bc) = result.fragment.stock_bytecodes
        {
            stock_frags.push(bc);
        } else {
            missing_vars.push(var_name.clone());
        }
    }

    // Append LTM flow fragments (link scores, loop scores, relative
    // loop scores). These go at the end of the flows runlist since
    // they have no ordering constraints with regular variables.
    for ltm_name in &ltm_flow_names {
        if let Some(result) = all_fragments.get(ltm_name)
            && let Some(ref bc) = result.fragment.flow_bytecodes
        {
            // LTM synthetic vars use PREVIOUS -> always dynamic.
            flow_frags.push((false, bc));
        }
    }

    // Append LTM implicit var fragments to the relevant runlists.
    // Some implicit vars participate in initials and/or stocks even
    // though they are not part of the original model.
    if project.ltm_enabled(db) {
        let ltm_implicit = model_ltm_implicit_var_info(db, model, project);
        let mut ltm_im_names: Vec<&String> = ltm_implicit.keys().collect();
        ltm_im_names.sort_unstable();
        for im_name in ltm_im_names {
            if let Some(result) = all_fragments.get(im_name) {
                if let Some(ref bc) = result.fragment.initial_bytecodes {
                    initial_frags.push((im_name.clone(), bc));
                }
                if let Some(ref bc) = result.fragment.flow_bytecodes {
                    // LTM implicit helpers (PREVIOUS instances) -> dynamic.
                    flow_frags.push((false, bc));
                }
                if let Some(ref bc) = result.fragment.stock_bytecodes {
                    stock_frags.push(bc);
                }
            }
        }
    }

    if !missing_vars.is_empty() {
        let msg = format!(
            "failed to compile fragments for variables: {}",
            missing_vars.join(", ")
        );
        return Err(msg);
    }

    // ── Run-invariant flow partition (GH #712) ─────────────────────────
    //
    // Stably partition the flow fragments so every run-invariant fragment
    // precedes every dynamic one, preserving each group's original relative
    // (topological) order. This is a valid topological order: the invariant
    // subgraph is closed under its dependencies (an invariant var cannot
    // depend on a dynamic var, by construction), so no reader is moved ahead
    // of a dependency. The split boundary is the invariant prefix's opcode
    // length in the concatenated flow stream.
    //
    // `concatenate_fragments_with_gf` is opcode-count-preserving: it strips
    // each fragment's single trailing `Ret` and copies every remaining opcode
    // 1:1, appending one terminal `Ret`. So the prefix opcode length is
    // exactly the sum of the invariant fragments' Ret-stripped code lengths.
    // `resolve_module` (symbolic -> concrete) is likewise 1:1 and does no
    // fusion, so this count is the boundary in the final `compiled_flows.code`.
    // The boundary is fusion-proof at `Vm::new` (no `fuse_three_address` window
    // crosses a fragment boundary -- every fragment ends in an `Assign*`, and
    // no window starts with or uses an `Assign*` as a combiner); see the design
    // note `docs/design-plans/2026-06-04-time-invariant-hoisting.md`.
    let ret_stripped_len = |frag: &crate::compiler::symbolic::PerVarBytecodes| -> usize {
        let code = &frag.symbolic.code;
        if code.last() == Some(&crate::compiler::symbolic::SymbolicOpcode::Ret) {
            code.len() - 1
        } else {
            code.len()
        }
    };
    // Stable partition: invariant first, dynamic after, each in original order.
    let mut invariant_flow_frags: Vec<&crate::compiler::symbolic::PerVarBytecodes> = Vec::new();
    let mut dynamic_flow_frags: Vec<&crate::compiler::symbolic::PerVarBytecodes> = Vec::new();
    for &(is_invariant, bc) in &flow_frags {
        if is_invariant {
            invariant_flow_frags.push(bc);
        } else {
            dynamic_flow_frags.push(bc);
        }
    }
    let flows_invariant_opcode_len: usize = invariant_flow_frags
        .iter()
        .map(|bc| ret_stripped_len(bc))
        .sum();
    let flow_frags: Vec<&crate::compiler::symbolic::PerVarBytecodes> = invariant_flow_frags
        .into_iter()
        .chain(dynamic_flow_frags)
        .collect();

    // Compute context resource base offsets for each phase so that flows
    // and stocks reference the same resource namespace as the all-phases
    // merge. The all-phases ordering is: initials, then flows, then stocks.
    let initial_refs: Vec<&crate::compiler::symbolic::PerVarBytecodes> =
        initial_frags.iter().map(|(_, bc)| *bc).collect();
    let initial_counts = ContextResourceCounts::from_fragments(&initial_refs);
    let flow_counts = ContextResourceCounts::from_fragments(&flow_frags);

    // #583: temps are NOT a per-phase-offset resource. The plain-phase
    // concat recycles every fragment's 0-based temps into ONE shared
    // identity pool (matching the monolithic `Module::compile` keyed
    // max-merge over the flattened initials+flows+stocks runlists), so the
    // `ctx_base.temps` is 0 for EVERY phase -- the pool is not partitioned by
    // phase. (Summing per phase, as before, drove the renumbered `temp_id`
    // past `u8::MAX` and diverged `flows_concat` from the all-phases `merged`
    // temp_offsets table the VM consumes.) Modules/views/dim-lists DO stay
    // per-phase summed: each is a distinct resource, laid out disjointly
    // across phases exactly as the all-phases `merged` lays them out.
    let no_base = ContextResourceCounts::default();
    let flow_base = ContextResourceCounts {
        temps: 0,
        ..initial_counts.clone()
    };
    // Summed in `usize` and left there. M3 -- every ASSIGNED id is
    // representable -- is discharged by `symbolic::resource_base` when a stock
    // fragment actually consumes one of these bases, which is the only point
    // where "the table is full" and "this fragment wants an id past the end"
    // can be told apart. Bounding the sum here instead would reject a model
    // whose initials and flows fill the table exactly and whose stocks phase
    // then names nothing -- a program in which every id is valid.
    let stock_base = ContextResourceCounts {
        modules: initial_counts.modules + flow_counts.modules,
        views: initial_counts.views + flow_counts.views,
        temps: 0,
        dim_lists: initial_counts.dim_lists + flow_counts.dim_lists,
    };

    // #582: graphical functions are content-de-duplicated across ALL
    // fragments of the model (one block per distinct table, matching the
    // monolithic `Compiler::new`), so -- unlike the flat literal/module/
    // view/temp/dim-list resources -- their `base_gf`s cannot be a per-phase
    // running count. Build the dedup ONCE over the union of every phase's
    // fragments (in the all-phases order initials, flows, stocks) and feed
    // each phase the corresponding per-fragment GF remap. A dependency
    // arrayed GF referenced by hundreds of consumer fragments now lands in
    // `graphical_functions` exactly once instead of once per consumer,
    // which both fixes the `GraphicalFunctionId = u8` overflow and matches
    // the monolithic GF-table layout.
    let all_frags: Vec<&crate::compiler::symbolic::PerVarBytecodes> = initial_frags
        .iter()
        .map(|(_, bc)| *bc)
        .chain(flow_frags.iter().copied())
        .chain(stock_frags.iter().copied())
        .collect();
    let gf_dedup = crate::compiler::symbolic::GfDedup::build(&all_frags)?;
    // Phase offsets into `all_frags` so each phase's fragments map to their
    // remap entry.
    let n_init = initial_frags.len();
    let n_flow = flow_frags.len();

    let flows_concat = concatenate_fragments_with_gf(&flow_frags, &flow_base, &gf_dedup, n_init)?;
    let stocks_concat =
        concatenate_fragments_with_gf(&stock_frags, &stock_base, &gf_dedup, n_init + n_flow)?;

    let compiled_initials = renumber_initials_phase(&initial_frags, &gf_dedup)?;

    // The all-phases aggregation of the shared context side-channels (modules,
    // views, temps, dim_lists); its `graphical_functions` is the dedup's single
    // table, shared by all three phases.
    //
    // This is `merge_context_side_channels` and not a full merge because the
    // three phases above already keep every stream this module retains. A full
    // merge additionally built an opcode stream and a literal pool spanning the
    // whole model, both dropped on the floor here -- and bounded the model's
    // AGGREGATE literal count against the `u16` id capacity in the process,
    // failing assembly for models whose every retained pool was fine.
    let merged = merge_context_side_channels(&all_frags, &no_base, &gf_dedup)?;

    // Build dimension metadata from project dimensions (mirrors
    // Compiler::populate_dimension_metadata). Read the project-global converted
    // dims from the salsa-cached query instead of rebuilding them here.
    let converted_dims = project_converted_dimensions(db, project);

    let mut dim_names: Vec<String> = Vec::new();
    let mut dim_infos: Vec<crate::bytecode::DimensionInfo> = Vec::new();

    let intern_name = |names: &mut Vec<String>, name: &str| -> crate::bytecode::NameId {
        if let Some(idx) = names.iter().position(|n| n == name) {
            return idx as crate::bytecode::NameId;
        }
        let id = names.len() as crate::bytecode::NameId;
        names.push(name.to_string());
        id
    };

    for dim in converted_dims {
        match dim {
            crate::dimensions::Dimension::Indexed(dim_name, size) => {
                let name_id = intern_name(&mut dim_names, dim_name.as_str());
                dim_infos.push(crate::bytecode::DimensionInfo::indexed(
                    name_id,
                    *size as u16,
                ));
            }
            crate::dimensions::Dimension::Named(dim_name, named_dim) => {
                let name_id = intern_name(&mut dim_names, dim_name.as_str());
                let element_name_ids: smallvec::SmallVec<[crate::bytecode::NameId; 8]> = named_dim
                    .elements
                    .iter()
                    .map(|elem| intern_name(&mut dim_names, elem.as_str()))
                    .collect();
                dim_infos.push(crate::bytecode::DimensionInfo::named(
                    name_id,
                    element_name_ids,
                ));
            }
        }
    }

    // Build the symbolic compiled module
    let sym_module = SymbolicCompiledModule {
        ident: Ident::new(&model_name),
        compiled_initials,
        compiled_flows: flows_concat.bytecode,
        compiled_stocks: stocks_concat.bytecode,
        graphical_functions: merged.graphical_functions,
        module_decls: merged.module_decls,
        static_views: merged.static_views,
        arrays: vec![],
        dimensions: dim_infos,
        subdim_relations: vec![],
        names: dim_names,
        temp_offsets: merged.temp_offsets,
        temp_total_size: merged.temp_total_size,
        dim_lists: merged.dim_lists,
        flows_invariant_opcode_len,
    };

    // Resolve symbolic -> concrete offsets. The CompiledModule stays a pure,
    // salsa-cached artifact; the 3-address fusion (R2) is applied later, at
    // Vm::new, to the execution copy of the bytecode. The success payload is
    // wrapped in an `Arc` so this tracked fn's return type is `salsa::Update`
    // and salsa's clone-out is a refcount bump (the inner bytecode is large).
    resolve_module(&sym_module, layout).map(std::sync::Arc::new)
}

/// Assemble a full CompiledSimulation from assembled modules.
///
/// Salsa-tracked: enumerating module instances, assembling each unique
/// `(model, input_set)` module, building the `Specs`, and computing the
/// flattened offset map are all memoized, so a recompile with no input
/// changes is a pure cache hit (zero re-assembly). When one variable
/// changes, only the affected `assemble_module` instances re-execute;
/// unchanged submodules cache-hit. `main_model_name` is an owned `String`
/// (a salsa-compatible by-value key); the success payload rides behind an
/// `Arc` so the return type is `salsa::Update` and clone-out is a refcount
/// bump rather than a deep clone of the modules/offsets maps.
#[salsa::tracked]
pub fn assemble_simulation(
    db: &dyn Db,
    project: SourceProject,
    main_model_name: String,
) -> Result<std::sync::Arc<crate::vm::CompiledSimulation>, String> {
    use crate::common::{Canonical, Ident};
    use crate::vm::CompiledSimulation;

    let project_models = project.models(db);
    let main_model_canonical = canonicalize(&main_model_name);

    if !project_models.contains_key(main_model_canonical.as_ref()) {
        let msg = format!("no model named '{}' to simulate", main_model_name);
        return Err(msg);
    }

    // A cyclic / self-referential module graph reachable FROM THE MAIN MODEL
    // would drive the recursive instance-enumeration, layout, and module-map
    // queries into a salsa dependency-graph cycle panic. Reject it cleanly first
    // (GH #806). Scoped to reachability from `main`: assembly starts from main
    // and only recurses into its instantiated modules, so an unrelated draft
    // cycle elsewhere in the project must not block compiling a valid main.
    if let Some((_code, msg)) =
        project_module_graph(db, project).cycle_error_from(main_model_canonical.as_ref())
    {
        return Err(msg);
    }

    // Enumerate module instances by walking module variables recursively.
    // Each unique (model_name, input_set) pair gets its own CompiledModule.
    let module_instances = enumerate_module_instances(db, project, &main_model_name)?;

    // GH #486: the LTM flow-to-stock link-score formula is only valid under
    // Euler integration; under RK2/RK4 the scores are mathematically
    // meaningless, and non-Euler IS honored at runtime (the VM and wasm
    // backends both have distinct RK2/RK4 stepping loops), so it would
    // silently produce plausible-but-wrong scores. The method that actually
    // runs is the SINGLE main-model-governed `Specs.method` resolved below
    // (root override else project specs); a submodel's own override is dead.
    // Resolve it once and reject only when the assembled sim actually produces
    // a flow-to-stock score against that main-governed method.
    //
    // GH #663 refinement: the old guard rejected on the mere presence of a
    // stock in any instantiated model. That is a false positive for a loop-free
    // model (an open-loop accumulation -- a constant inflow that never reads the
    // stock back): in exhaustive mode LTM scores only the edges of detected
    // feedback loops, so such a stock emits NO flow-to-stock score and the
    // non-Euler method has nothing to corrupt. The refined precondition is the
    // EXACT thing #486 protects against: "an instantiated model actually emits a
    // flow-to-stock link score" (`model_emits_flow_to_stock_score`).
    //
    // The check iterates the instantiated set (root + every transitively-
    // instantiated submodule) and asks each model's own
    // `model_emits_flow_to_stock_score`, so a flow-to-stock score produced
    // entirely inside a submodel instance is caught -- the assembly emits that
    // submodel's LTM vars too. The root may be stock-free while a submodel under
    // the main-governed method scores a flow-to-stock link, exactly the hazard a
    // per-submodel-specs check missed. Unused model definitions sitting in the
    // project are never instantiated, so they are irrelevant.
    //
    // Reading the emitted var set (rather than a loop-presence proxy) is what
    // makes the refinement SOUND across modes: in discovery mode (user-forced or
    // auto-flipped) and in any model with input ports, `model_ltm_variables`
    // scores ALL causal edges, so it emits a flow-to-stock score for an
    // open-loop stock's `flow → stock` edge even though no loop contains that
    // stock. A "has any feedback loop" proxy would under-reject those models; the
    // direct var-set test cannot. `model_ltm_variables` is already salsa-computed
    // on this LTM-enabled assembly path, so this is a cache hit plus a linear
    // scan.
    //
    // This rejection rides the `assemble_simulation` `Err`, so it reaches
    // `simlin_sim_new`, `simlin_project_get_errors` (the `vm_error` channel),
    // and the wasm backend -- the sim-compile path that, unlike
    // `collect_all_diagnostics`, is what every runnable consumer goes through.
    if project.ltm_enabled(db)
        && let Some(root_model) = project_models.get(main_model_canonical.as_ref())
        && let Some(method) = ltm::effective_non_euler_method(db, *root_model, project)
    {
        let any_flow_to_stock_score = module_instances.keys().any(|name| {
            let canonical = canonicalize(name.as_str());
            project_models
                .get(canonical.as_ref())
                .is_some_and(|sm| ltm::model_emits_flow_to_stock_score(db, *sm, project))
        });
        if any_flow_to_stock_score {
            return Err(ltm::ltm_non_euler_diagnostic_message(method));
        }
    }

    // Sort module names: main first, then all others alphabetically
    let main_ident = Ident::<Canonical>::new(&main_model_name);
    let mut module_names: Vec<&Ident<Canonical>> = module_instances.keys().collect();
    module_names.sort_unstable();
    let mut sorted_names = vec![&main_ident];
    sorted_names.extend(
        module_names
            .into_iter()
            .filter(|n| n.as_str() != main_model_name),
    );

    let root_input_set: BTreeSet<Ident<Canonical>> = BTreeSet::new();
    let root_key: crate::vm::ModuleKey = (main_ident.clone(), root_input_set);

    let mut compiled_modules: HashMap<crate::vm::ModuleKey, crate::bytecode::CompiledModule> =
        HashMap::new();

    for name in &sorted_names {
        let distinct_inputs = &module_instances[*name];
        for inputs in distinct_inputs.iter() {
            let model_name_str = name.as_str();
            let canonical_name = canonicalize(model_name_str);
            let source_model = project_models.get(canonical_name.as_ref()).ok_or_else(|| {
                format!(
                    "model '{}' referenced as module but not found in project",
                    model_name_str,
                )
            })?;

            let is_root = canonicalize(name.as_str()) == main_model_canonical;
            // The tracked `assemble_module` keys on an interned `ModuleInputSet`
            // (the sorted canonical input names). `inputs` is already a
            // `BTreeSet<Ident<Canonical>>`, so this is the canonical round-trip.
            let module_inputs = ModuleInputSet::from_canonical_set(db, inputs);
            let compiled = assemble_module(db, *source_model, project, is_root, module_inputs)?;
            let module_key: crate::vm::ModuleKey = ((*name).clone(), inputs.clone());
            // Clone the `CompiledModule` out of the salsa-owned `Arc`: the
            // `CompiledSimulation.modules` map stores it by value (its bytecode
            // is itself `Arc`-backed, so this clone is cheap refcount bumps).
            compiled_modules.insert(module_key, (*compiled).clone());
        }
    }

    // Build Specs, preferring model-level sim_specs override when present
    let specs = if let Some(source_model) = project_models.get(main_model_canonical.as_ref())
        && let Some(ref model_specs) = *source_model.model_sim_specs(db)
    {
        crate::vm::Specs::from(model_specs)
    } else {
        crate::vm::Specs::from(project.sim_specs(db))
    };

    // Compute flattened offsets for variable name -> offset mapping
    let offsets = calc_flattened_offsets_incremental(db, project, &main_model_name, true);
    let offsets: HashMap<Ident<Canonical>, usize> =
        offsets.into_iter().map(|(k, (off, _))| (k, off)).collect();

    Ok(std::sync::Arc::new(CompiledSimulation::new(
        compiled_modules,
        specs,
        root_key,
        offsets,
    )))
}

type ModuleInstanceMap = HashMap<Ident<Canonical>, BTreeSet<BTreeSet<Ident<Canonical>>>>;

/// Enumerate all module instances in a project, starting from the main model.
/// Returns a map from model name to the set of distinct input sets that model
/// is instantiated with.
/// The input sets one model is instantiated with, as PRODUCTION enumerates
/// them (`#[cfg(test)]` accessor only, mirroring `db::dep_graph`'s
/// `dt_cycle_sccs` idiom).
///
/// A test that needs "the module input set this sub-model actually gets" must
/// not spell it by hand: a hand-written set is an assumption about the wiring,
/// and the assumption is exactly what such a test is trying to hold the fixture
/// to. Routing through `enumerate_module_instances` makes the test's input the
/// engine's input by construction, so degrading the fixture's wiring changes
/// the test's answer instead of being silently ignored.
#[cfg(test)]
pub(crate) fn module_input_sets_for(
    db: &dyn Db,
    project: SourceProject,
    main_model_name: &str,
    model_name: &str,
) -> Vec<BTreeSet<Ident<Canonical>>> {
    let modules = enumerate_module_instances(db, project, main_model_name)
        .expect("fixture project must enumerate");
    modules
        .get(&Ident::<Canonical>::new(model_name))
        .map(|sets| sets.iter().cloned().collect())
        .unwrap_or_default()
}

fn enumerate_module_instances(
    db: &dyn Db,
    project: SourceProject,
    main_model_name: &str,
) -> Result<ModuleInstanceMap, String> {
    use crate::common::{Canonical, Ident};

    let main_ident = Ident::<Canonical>::new(main_model_name);

    let mut modules: ModuleInstanceMap = HashMap::new();

    // Main model with no inputs
    let no_inputs = BTreeSet::new();
    modules.insert(main_ident, [no_inputs].into_iter().collect());

    enumerate_module_instances_inner(db, project, main_model_name, &mut modules)?;

    Ok(modules)
}

fn enumerate_module_instances_inner(
    db: &dyn Db,
    project: SourceProject,
    model_name: &str,
    modules: &mut ModuleInstanceMap,
) -> Result<(), String> {
    use crate::common::{Canonical, Ident};

    let project_models = project.models(db);
    let canonical_name = canonicalize(model_name);
    let source_model = project_models
        .get(canonical_name.as_ref())
        .ok_or_else(|| format!("model '{}' not found", model_name))?;

    let source_vars = source_model.variables(db);
    for (var_name, source_var) in source_vars.iter() {
        if source_var.kind(db) != SourceVariableKind::Module {
            continue;
        }

        let sub_model_name = source_var.model_name(db);
        let sub_canonical = canonicalize(sub_model_name);

        if !project_models.contains_key(sub_canonical.as_ref()) {
            return Err(format!(
                "model '{}' referenced as module but not found",
                sub_model_name,
            ));
        }

        // Strip module ident prefix from dst to get bare sub-model variable
        // names, matching how resolve_module_input works in the monolithic path
        let input_prefix = format!("{var_name}\u{00B7}");
        let inputs: BTreeSet<Ident<Canonical>> = source_var
            .module_refs(db)
            .iter()
            .filter_map(|mr| {
                let dst_canonical = canonicalize(&mr.dst);
                let bare = dst_canonical.strip_prefix(&input_prefix)?;
                Some(Ident::new(bare))
            })
            .collect();

        let key = Ident::<Canonical>::new(sub_model_name);
        let is_new = !modules.contains_key(&key);

        modules.entry(key).or_default().insert(inputs);

        if is_new {
            enumerate_module_instances_inner(db, project, sub_model_name, modules)?;
        }
    }

    // Include implicit MODULE variables (e.g. from SMOOTH, DELAY builtins)
    let implicit_info = model_implicit_var_info(db, *source_model, project);
    for (name, meta) in implicit_info.iter() {
        if !meta.is_module {
            continue;
        }
        let sub_model_name = match &meta.model_name {
            Some(n) => n,
            None => continue,
        };
        let sub_canonical = canonicalize(sub_model_name);
        if !project_models.contains_key(sub_canonical.as_ref()) {
            return Err(format!(
                "implicit module '{}' references model '{}' which was not found",
                name, sub_model_name,
            ));
        }
        let module_ident_context = model_module_ident_context(db, *source_model, project, vec![]);
        let parsed = parse_source_variable_with_module_context(
            db,
            meta.parent_source_var,
            project,
            module_ident_context,
        );
        let input_prefix = format!("{name}\u{00B7}");
        let inputs: BTreeSet<Ident<Canonical>> =
            if let Some(datamodel::Variable::Module(dm_module)) = meta.find_in(parsed) {
                dm_module
                    .references
                    .iter()
                    .filter_map(|mr| {
                        let dst_canonical = canonicalize(&mr.dst);
                        let bare = dst_canonical.strip_prefix(&input_prefix)?;
                        Some(Ident::new(bare))
                    })
                    .collect()
            } else {
                BTreeSet::new()
            };

        let key = Ident::<Canonical>::new(sub_model_name);
        let is_new = !modules.contains_key(&key);

        modules.entry(key).or_default().insert(inputs);

        if is_new {
            enumerate_module_instances_inner(db, project, sub_model_name, modules)?;
        }
    }

    // Include LTM implicit MODULE variables (e.g. PREVIOUS instances from
    // feedback loop instrumentation). These are only present when LTM is
    // enabled. Models without feedback loops produce empty lists.
    //
    // Module-typed LTM implicit vars are the only ones that contribute module
    // instances, and they are rare (in the current architecture LTM equations
    // never contain module-function calls, so there are usually none). Drive
    // the loop from the salsa-cached module-typed projection; each implicit
    // variable rides on its meta, so no parent equation is (re-)parsed here.
    if project.ltm_enabled(db) {
        let ltm_implicit = ltm::model_ltm_implicit_var_info(db, *source_model, project);
        let mut module_typed: Vec<(&String, &crate::db::LtmImplicitVarMeta)> = ltm_implicit
            .iter()
            .filter(|(_, meta)| meta.is_module)
            .collect();
        // Deterministic processing order: the recursive sub-model discovery
        // below allocates entries in `modules` as it goes.
        module_typed.sort_unstable_by(|a, b| a.0.cmp(b.0));

        if !module_typed.is_empty() {
            for (im_name, im_meta) in module_typed {
                let sub_model_name = match &im_meta.model_name {
                    Some(n) => n,
                    None => continue,
                };
                let sub_canonical = canonicalize(sub_model_name);
                if !project_models.contains_key(sub_canonical.as_ref()) {
                    continue;
                }

                let implicit_dm_var = &im_meta.variable;

                // Extract input set from the implicit module's references
                let input_prefix = format!("{im_name}\u{00B7}");
                let inputs: BTreeSet<Ident<Canonical>> =
                    if let datamodel::Variable::Module(dm_module) = implicit_dm_var {
                        dm_module
                            .references
                            .iter()
                            .filter_map(|mr| {
                                let dst_canonical = canonicalize(&mr.dst);
                                let bare = dst_canonical.strip_prefix(&input_prefix)?;
                                Some(Ident::new(bare))
                            })
                            .collect()
                    } else {
                        BTreeSet::new()
                    };

                let key = Ident::<Canonical>::new(sub_model_name);
                let is_new = !modules.contains_key(&key);

                modules.entry(key).or_default().insert(inputs);

                if is_new {
                    enumerate_module_instances_inner(db, project, sub_model_name, modules)?;
                }
            }
        }
    }

    Ok(())
}

/// Compute flattened offsets for each variable in a model, mapping
/// canonical variable names to (start_offset, size) pairs.
/// Works with SourceModel/SourceVariable from the salsa database.
pub(crate) fn calc_flattened_offsets_incremental(
    db: &dyn Db,
    project: SourceProject,
    model_name: &str,
    is_root: bool,
) -> HashMap<Ident<Canonical>, (usize, usize)> {
    use crate::common::{Canonical, Ident};
    let project_models = project.models(db);
    let canonical_name = canonicalize(model_name);

    let source_model = match project_models.get(canonical_name.as_ref()) {
        Some(m) => m,
        None => return HashMap::new(),
    };

    let mut offsets: HashMap<Ident<Canonical>, (usize, usize)> = HashMap::new();
    let mut i = 0;
    if is_root {
        offsets.insert(Ident::new("time"), (0, 1));
        offsets.insert(Ident::new("dt"), (1, 1));
        offsets.insert(Ident::new("initial_time"), (2, 1));
        offsets.insert(Ident::new("final_time"), (3, 1));
        i += crate::vm::IMPLICIT_VAR_COUNT;
    }

    let source_vars = source_model.variables(db);
    let var_names = source_model.variable_names(db);
    let mut sorted_names: Vec<&String> = var_names.iter().collect();
    sorted_names.sort_unstable();

    for ident in &sorted_names {
        let ident_canonical = Ident::new(ident.as_str());
        let size = if let Some(svar) = source_vars.get(ident.as_str()) {
            if svar.kind(db) == SourceVariableKind::Module {
                let sub_model_name = svar.model_name(db);
                let sub_offsets =
                    calc_flattened_offsets_incremental(db, project, sub_model_name, false);
                let mut sub_var_names: Vec<&Ident<Canonical>> = sub_offsets.keys().collect();
                sub_var_names.sort_unstable();
                for sub_name in &sub_var_names {
                    let (sub_off, sub_size) = sub_offsets[*sub_name];
                    offsets.insert(
                        Ident::join(
                            &ident_canonical.as_canonical_str(),
                            &sub_name.as_canonical_str(),
                        ),
                        (i + sub_off, sub_size),
                    );
                }
                let sub_size: usize = sub_offsets.iter().map(|(_, (_, size))| size).sum();
                sub_size
            } else {
                let var_sz = variable_size(db, *svar, project);
                // A lookup-only table is not a saved output variable: reserve
                // its layout slot (so these offsets stay in lockstep with
                // `compute_layout`, whose map codegen's table-identity
                // reverse-map resolves against) but do NOT expose its name in
                // this VM/Results map -- it produces no series (issue #606).
                if !source_var_is_table_only(db, *svar) {
                    if var_sz > 1 {
                        // Array variable: produce per-element offsets
                        let dims = variable_dimensions(db, *svar, project);
                        if !dims.is_empty() {
                            for (j, subscripts) in
                                crate::dimensions::SubscriptIterator::new(dims).enumerate()
                            {
                                let subscript = subscripts.join(",");
                                let subscripted_ident = Ident::<Canonical>::from_unchecked(
                                    format!("{}[{}]", ident_canonical.as_str(), subscript),
                                );
                                offsets.insert(subscripted_ident, (i + j, 1));
                            }
                        }
                    } else {
                        offsets.insert(ident_canonical.clone(), (i, 1));
                    }
                }
                var_sz
            }
        } else {
            offsets.insert(ident_canonical.clone(), (i, 1));
            1
        };
        i += size;
    }

    // Include implicit variables (SMOOTH, DELAY, TREND builtins) after explicit variables.
    // Implicit MODULE vars (from builtin expansion) occupy their sub-model's full
    // slot count, mirroring compute_layout's handling at the VariableLayout level.
    let implicit_info = model_implicit_var_info(db, *source_model, project);
    let mut implicit_names: Vec<&String> = implicit_info.keys().collect();
    implicit_names.sort_unstable();
    for name in implicit_names {
        let info = &implicit_info[name];
        let ident_canonical = Ident::new(name.as_str());

        if info.is_module {
            if let Some(sub_model_name) = &info.model_name {
                let sub_offsets =
                    calc_flattened_offsets_incremental(db, project, sub_model_name, false);
                let mut sub_var_names: Vec<&Ident<Canonical>> = sub_offsets.keys().collect();
                sub_var_names.sort_unstable();
                for sub_name in &sub_var_names {
                    let (sub_off, sub_size) = sub_offsets[*sub_name];
                    offsets.insert(
                        Ident::join(
                            &ident_canonical.as_canonical_str(),
                            &sub_name.as_canonical_str(),
                        ),
                        (i + sub_off, sub_size),
                    );
                }
                let sub_size: usize = sub_offsets.iter().map(|(_, (_, size))| size).sum();
                i += sub_size;
            } else {
                offsets.insert(ident_canonical.clone(), (i, info.size));
                i += info.size;
            }
        } else {
            offsets.insert(ident_canonical.clone(), (i, info.size));
            i += info.size;
        }
    }

    // Include LTM variables (loop scores, relative loop scores, and their
    // implicit helper/module vars) when LTM is enabled. Models without
    // feedback loops get empty LTM var lists. These occupy slots after the
    // implicit variables, matching compute_layout's Section 3 ordering.
    //
    // `compute_layout` now returns the body layout (0-based); the running
    // `i` above is already root-shifted (it reserves `IMPLICIT_VAR_COUNT`
    // when `is_root`), so the LTM section must read the SAME root-shifted
    // entry offsets the assembled module resolves against. `root_shifted`
    // is the single shared shift, so this stays in lockstep with
    // `assemble_module`'s root path.
    if project.ltm_enabled(db) {
        let body_layout = compute_layout(db, *source_model, project);
        let shifted_layout;
        let layout: &crate::compiler::symbolic::VariableLayout = if is_root {
            shifted_layout = body_layout.root_shifted();
            &shifted_layout
        } else {
            body_layout
        };

        let ltm_vars = model_ltm_variables(db, *source_model, project);

        let ltm_implicit = ltm::model_ltm_implicit_var_info(db, *source_model, project);

        // Add explicit LTM variables (loop scores, relative loop scores)
        for ltm_var in &ltm_vars.vars {
            let canonical_name = canonicalize(&ltm_var.name);
            if let Some(entry) = layout.get(&canonical_name) {
                offsets.insert(
                    Ident::<Canonical>::from_unchecked(
                        Ident::<Canonical>::new(&canonical_name).to_source_repr(),
                    ),
                    (entry.offset, entry.size),
                );
            }
        }

        // Add implicit variables from LTM equations. `model_ltm_implicit_var_info`
        // already maps every parse-synthesized implicit var (keyed by its
        // parent-embedding unique name) to its metadata, so iterate it directly --
        // re-parsing every LTM equation here just to re-discover the same names
        // was a full pass over ~20 MB of equation text on C-LEARN.
        for (im_name, im_meta) in ltm_implicit.iter() {
            let Some(entry) = layout.get(im_name) else {
                continue;
            };
            if im_meta.is_module {
                // Module-type: include sub-model variable offsets
                if let Some(sub_model_name) = &im_meta.model_name {
                    let sub_offsets =
                        calc_flattened_offsets_incremental(db, project, sub_model_name, false);
                    let mut sub_var_names: Vec<&Ident<Canonical>> = sub_offsets.keys().collect();
                    sub_var_names.sort_unstable();
                    let im_ident = Ident::new(im_name.as_str());
                    for sub_name in &sub_var_names {
                        let (sub_off, sub_size) = sub_offsets[*sub_name];
                        let sub_canonical = Ident::new(sub_name.as_str());
                        offsets.insert(
                            Ident::<Canonical>::from_unchecked(format!(
                                "{}.{}",
                                im_ident.to_source_repr(),
                                sub_canonical.to_source_repr()
                            )),
                            (entry.offset + sub_off, sub_size),
                        );
                    }
                }
            } else {
                offsets.insert(
                    Ident::<Canonical>::from_unchecked(
                        Ident::<Canonical>::new(im_name.as_str()).to_source_repr(),
                    ),
                    (entry.offset, entry.size),
                );
            }
        }
    }

    offsets
}
