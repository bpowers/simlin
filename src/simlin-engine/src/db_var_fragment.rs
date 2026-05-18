// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Per-variable lowering to the pre-bytecode `Var` form.
//!
//! `lower_var_fragment` performs the lowering half of per-variable
//! compilation: parse the source variable, lower its equation, build the
//! minimal symbol-table / metadata context it needs, and run the
//! per-phase `Var` construction (`crate::compiler::Var::new`) that yields
//! the lowered `Vec<Expr>` for each phase. The bytecode-emission half
//! (`compile_phase`) stays with the salsa-tracked caller in
//! `crate::db::compile_var_fragment`, which consumes the owned,
//! lowering-independent values this returns.
//!
//! The split exists for two reasons. First, the lowered `Vec<Expr>` is
//! the natural reuse surface for read-only structural probes that need
//! the engine's *own* per-variable production lowering without re-running
//! it with a reconstructed context. Second, `Vec<Expr>` does not
//! implement `salsa::Update`, so the lowering step must be a plain
//! function while the caller remains the salsa-tracked query.
//!
//! Because a plain function cannot accumulate salsa diagnostics, the
//! diagnostics this step would emit are returned **as data**
//! (`LoweredVarFragment`) and replayed by the caller. There are six
//! distinct diagnostic outcomes, preserved exactly:
//!
//! * a malformed unit-string error is *non-fatal* -- it is recorded but
//!   compilation of the variable continues (carried in `unit_diags`,
//!   present in both result variants);
//! * an equation parse error, an AST-lowering error, an unknown
//!   dependency, and a graphical-function table-build error are each
//!   *fatal* -- they abort this variable's compilation (folded into
//!   `Fatal { fatal_diags }`, whole-variable `None` at the caller);
//! * a per-phase `Var::new` failure is *phase-local* -- only that phase's
//!   bytecode is dropped while the other phases still compile (carried in
//!   `per_phase_lowered` as a per-phase `Err`, not a whole-variable
//!   failure).
//!
//! The coupled cluster `{lowered, all_metadata, arena, ContextCore,
//! Context, Var::new}` is internal to `lower_var_fragment` and drops
//! together at return; only owned, lifetime-free values
//! (`per_phase_lowered`, `tables`, `offsets`, `rmap`, `mini_offset`)
//! cross back to the caller. The metadata map borrows the lowered
//! variable (its self-entry is `&lowered`) and the sub-model stub arena,
//! so it must not outlive them; the caller never sees it -- it consumes
//! only the owned `offsets` projection (variable -> (offset, size)).
//!
//! This is a top-level module (a sibling of `db`, like `db_dep_graph` /
//! `db_ltm_ir` / `db_macro_registry`) rather than a submodule of `db.rs`
//! purely to keep `db.rs` under the per-file line cap; the caller in
//! `db` reaches it via `crate::db_var_fragment::lower_var_fragment`.

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::canonicalize;
use crate::common::{Canonical, Error, Ident};
use crate::datamodel;
use crate::db::{
    Db, Diagnostic, DiagnosticError, DiagnosticSeverity, SourceModel, SourceProject,
    SourceVariable, SourceVariableKind, build_module_inputs, build_stub_variable,
    build_submodel_metadata, compute_layout, extract_tables_from_source_var,
    model_implicit_var_info, model_module_ident_context, parse_source_variable_with_module_context,
    variable_dimensions, variable_direct_dependencies_with_context, variable_size,
};

/// Per-model variable -> (offset, size) projection of the minimal
/// metadata map. This is the owned, borrow-free view of the symbol
/// layout the caller's `compile_phase` consumes (it never reads the
/// borrowed variable, only the offset/size pair), mirroring the
/// `VariableOffsetMap` shape used inside the compiler.
type VarOffsets = HashMap<Ident<Canonical>, HashMap<Ident<Canonical>, (usize, usize)>>;

/// Result of `crate::compiler::Var::new` for each compilation phase.
///
/// `initial` is `Var::new(.., is_initial = true)`; `noninitial` is
/// `Var::new(.., is_initial = false)` and is shared by the flow and
/// stock phases (the original code calls `build_var(false)` for both --
/// `Var::new` is deterministic, so computing it once and reusing it is
/// observably identical). Each phase carries its own `Result`: an `Err`
/// means that phase's `Var::new` failed and the caller drops only that
/// phase's bytecode (replaying the per-phase diagnostic), exactly as the
/// original `match build_var(..)` arms did.
pub(crate) struct PerPhaseLowered {
    pub(crate) initial: Result<crate::compiler::Var, Error>,
    pub(crate) noninitial: Result<crate::compiler::Var, Error>,
}

/// Owned, lifetime-free outcome of `lower_var_fragment`, returned to the
/// salsa-tracked caller so it can replay diagnostics and run bytecode
/// emission without the borrowed lowering context.
pub(crate) enum LoweredVarFragment {
    /// The variable failed at a fatal site (equation parse, AST lowering,
    /// unknown dependency, or table build). `unit_diags` carries any
    /// non-fatal malformed-unit diagnostics that were recorded before the
    /// fatal site (replayed first, preserving emission order);
    /// `fatal_diags` carries the fatal diagnostic(s). The caller
    /// accumulates both and returns whole-variable `None`.
    Fatal {
        unit_diags: Vec<Diagnostic>,
        fatal_diags: Vec<Diagnostic>,
    },
    /// The variable lowered successfully. `unit_diags` carries any
    /// non-fatal malformed-unit diagnostics (replayed, compilation
    /// continues). The remaining fields are the owned, lowering-
    /// independent values the caller's `compile_phase` consumes.
    Lowered {
        unit_diags: Vec<Diagnostic>,
        per_phase_lowered: PerPhaseLowered,
        tables: HashMap<Ident<Canonical>, Vec<crate::compiler::Table>>,
        offsets: VarOffsets,
        rmap: crate::compiler::symbolic::ReverseOffsetMap,
        mini_offset: usize,
    },
}

/// Dependency-collection outputs for a single variable.
///
/// The dependency walk produces both lowering-coupled values (the stub
/// `dep_variables` / `implicit_module_vars` woven into the minimal
/// metadata map, and the sub-model lists feeding sub-model metadata) and
/// lowering-*independent* values (`extra_module_refs` /
/// `implicit_module_refs`, which the caller's `compile_phase` needs).
/// The walk logic is defined exactly once (`collect_var_dependencies`).
/// It is invoked from two call sites per variable -- `lower_var_fragment`
/// and the caller's module-ref reconstruction (`build_caller_module_refs`)
/// -- but `collect_var_dependencies` is pure over salsa-tracked inputs,
/// so the second invocation is a memoized cache hit with no
/// recomputation. `unknown_dependency` is `Some` when the walk hit a
/// reference that is neither a source nor an implicit variable (the fatal
/// unknown-dependency site); the walk stops there (a fatal unknown
/// dependency short-circuits the rest of the walk).
pub(crate) struct VarDepCollection {
    pub(crate) dep_variables: Vec<(Ident<Canonical>, crate::variable::Variable, usize)>,
    pub(crate) extra_module_refs: HashMap<Ident<Canonical>, crate::vm::ModuleKey>,
    pub(crate) extra_submodels: Vec<(String, SourceModel)>,
    pub(crate) implicit_module_vars: Vec<(Ident<Canonical>, crate::variable::Variable, usize)>,
    pub(crate) implicit_module_refs: HashMap<Ident<Canonical>, crate::vm::ModuleKey>,
    pub(crate) implicit_submodels: Vec<(String, SourceModel)>,
    pub(crate) unknown_dependency: Option<Diagnostic>,
}

/// Walk a variable's direct dependencies (plus stock inflows/outflows and
/// implicit module instances) and build the per-dependency stub
/// variables, module-ref entries, and sub-model lists.
///
/// This is a faithful relocation of the dependency loop that previously
/// lived inline in `compile_var_fragment`. The original consulted the
/// incrementally-built minimal metadata map (`mini_metadata`) to skip
/// already-present entries; this relocation instead consults
/// `existing_keys` = `{self} ∪ {time, dt, initial_time, final_time}`
/// (the implicit-time entries present only when `is_root`). That
/// substitution is byte-equivalent for both loops, but for two
/// *different* reasons -- the original map was not a single frozen set
/// across both:
///
/// * Dependency loop: when the inline dep loop ran its `contains_key`
///   skip, `mini_metadata` held *exactly* `{self} ∪ {implicit-if-root}`
///   -- the inline code inserts the `dep_variables` keys only *after*
///   the dep loop finishes -- so `existing_keys` is precisely that map
///   and the skip outcome is identical.
/// * Implicit-module loop: the inline code inserts the `dep_variables`
///   keys *between* the two loops, so at the inline implicit-module
///   skip `mini_metadata` *additionally* held those dep keys -- it was
///   NOT frozen here. The skip outcome is nonetheless identical because
///   the implicit-module idents are synthetic `$⁚`-prefixed module
///   idents drawn from `parsed.implicit_vars`, whereas the dep keys are
///   real source / non-module implicit-helper / source-module names:
///   the two name spaces are disjoint, so an `im_ident` is never a dep
///   key and `existing_keys.contains` therefore agrees with the
///   original `mini_metadata.contains_key` on every `im_ident`.
///
/// First-inserted-wins among the collected `dep_variables` /
/// `implicit_module_vars` is preserved downstream by the
/// `!mini_metadata.contains_key` guards on the two `mini_metadata`
/// insertion loops in `lower_var_fragment`, not here. No `db` diagnostic
/// is accumulated here; the unknown-dependency diagnostic is returned as
/// data.
fn collect_var_dependencies(
    db: &dyn Db,
    var: SourceVariable,
    model: SourceModel,
    project: SourceProject,
    is_root: bool,
    module_input_names: &[String],
) -> VarDepCollection {
    let var_ident = var.ident(db).clone();
    let var_ident_canonical: Ident<Canonical> = Ident::new(&var_ident);
    let module_ident_context =
        model_module_ident_context(db, model, project, module_input_names.to_vec());
    let parsed = parse_source_variable_with_module_context(db, var, project, module_ident_context);
    let deps = variable_direct_dependencies_with_context(db, var, project, module_ident_context);
    let project_models = project.models(db);

    // `existing_keys` is the exact pre-loop key set: `{self}` plus the
    // implicit `time`/`dt`/`initial_time`/`final_time` entries when
    // `is_root`. The original inline `mini_metadata.contains_key(..)`
    // skip checks are equivalent to `existing_keys` membership for both
    // loops, but not because the map is frozen across both: for the
    // dependency loop `existing_keys` IS `mini_metadata` at that skip;
    // for the implicit-module loop `mini_metadata` additionally holds the
    // dep keys, yet the outcome is unchanged by the name-space
    // disjointness argued on `collect_var_dependencies` above.
    let mut existing_keys: HashSet<Ident<Canonical>> = HashSet::new();
    existing_keys.insert(var_ident_canonical.clone());
    if is_root {
        existing_keys.insert(Ident::new("time"));
        existing_keys.insert(Ident::new("dt"));
        existing_keys.insert(Ident::new("initial_time"));
        existing_keys.insert(Ident::new("final_time"));
    }

    // Collect all dep names from both dt and initial deps
    let all_dep_names: BTreeSet<&String> = deps
        .dt_deps
        .iter()
        .chain(deps.initial_deps.iter())
        .collect();

    // For each dep, build a dimension-only Variable for context.
    // We need these to live long enough for the metadata references.
    let source_vars = model.variables(db);
    let mut dep_variables: Vec<(Ident<Canonical>, crate::variable::Variable, usize)> = Vec::new();

    // Also add inflows/outflows for stocks (needed by stock update expressions)
    let mut extra_dep_names: Vec<String> = Vec::new();
    if var.kind(db) == SourceVariableKind::Stock {
        for flow_name in var.inflows(db).iter().chain(var.outflows(db).iter()) {
            let canonical = canonicalize(flow_name).into_owned();
            if !all_dep_names.contains(&canonical) {
                extra_dep_names.push(canonical);
            }
        }
    }

    let all_names: Vec<&String> = all_dep_names
        .iter()
        .copied()
        .chain(extra_dep_names.iter())
        .collect();

    // Track module deps that need module_refs and sub-model metadata
    let mut extra_module_refs: HashMap<Ident<Canonical>, crate::vm::ModuleKey> = HashMap::new();
    let mut extra_submodels: Vec<(String, SourceModel)> = Vec::new();
    let implicit_var_info = model_implicit_var_info(db, model, project);

    for dep_name in &all_names {
        // Skip self and implicit vars
        if dep_name.as_str() == var_ident.as_str()
            || matches!(
                dep_name.as_str(),
                "time" | "dt" | "initial_time" | "final_time"
            )
        {
            continue;
        }

        // Handle leading middle-dot (parent model reference in XMILE)
        let effective_name = dep_name
            .as_str()
            .strip_prefix('\u{00B7}')
            .unwrap_or(dep_name.as_str());

        // Check for composite module output reference (contains middle dot)
        if let Some(dot_pos) = effective_name.find('\u{00B7}') {
            let module_var_name = &effective_name[..dot_pos];
            let module_ident: Ident<Canonical> = Ident::new(module_var_name);

            if existing_keys.contains(&module_ident) {
                continue;
            }

            // Look up the module variable in source_vars or implicit vars
            if let Some(mod_source_var) = source_vars.get(module_var_name) {
                if mod_source_var.kind(db) == SourceVariableKind::Module {
                    let mod_model_name = mod_source_var.model_name(db);
                    let sub_canonical = canonicalize(mod_model_name);
                    let sub_size = project_models
                        .get(sub_canonical.as_ref())
                        .map(|sm| compute_layout(db, *sm, project, false).n_slots)
                        .unwrap_or(1);

                    // Build Module variable with resolved inputs
                    let mod_input_prefix = format!("{module_var_name}\u{00B7}");
                    let module_inputs: Vec<crate::variable::ModuleInput> = mod_source_var
                        .module_refs(db)
                        .iter()
                        .filter_map(|mr| {
                            let src = canonicalize(&mr.src);
                            let dst = canonicalize(&mr.dst);
                            if src.starts_with(&mod_input_prefix) {
                                return None;
                            }
                            let dst_stripped = dst.strip_prefix(&mod_input_prefix)?;
                            let src_str = if model.name(db) == "main" && src.starts_with('\u{00B7}')
                            {
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

                    let mod_var = crate::variable::Variable::Module {
                        ident: module_ident.clone(),
                        model_name: Ident::new(mod_model_name),
                        units: None,
                        inputs: module_inputs.clone(),
                        errors: vec![],
                        unit_errors: vec![],
                    };
                    dep_variables.push((module_ident.clone(), mod_var, sub_size));

                    // Build module_refs entry
                    let input_set: BTreeSet<Ident<Canonical>> =
                        module_inputs.iter().map(|mi| mi.dst.clone()).collect();
                    extra_module_refs.insert(module_ident, (Ident::new(mod_model_name), input_set));

                    if let Some(sub_model) = project_models.get(sub_canonical.as_ref()) {
                        extra_submodels.push((mod_model_name.to_string(), *sub_model));
                    }
                }
            } else if let Some(meta) = implicit_var_info.get(module_var_name)
                && meta.is_module
            {
                // Implicit module already handled in the implicit_module_vars section below
            }
            continue;
        }

        let dep_ident = Ident::new(effective_name);
        if existing_keys.contains(&dep_ident) {
            continue;
        }

        if let Some(dep_source_var) = source_vars.get(effective_name) {
            let dep_dims = variable_dimensions(db, *dep_source_var, project);
            let dep_size = variable_size(db, *dep_source_var, project);

            let dep_var = build_stub_variable(db, dep_source_var, &dep_ident, dep_dims);

            dep_variables.push((dep_ident, dep_var, dep_size));
        } else if let Some(meta) = implicit_var_info.get(effective_name) {
            if !meta.is_module {
                let dep_var = if meta.is_stock {
                    crate::variable::Variable::Stock {
                        ident: dep_ident.clone(),
                        init_ast: None,
                        eqn: None,
                        units: None,
                        inflows: vec![],
                        outflows: vec![],
                        non_negative: false,
                        errors: vec![],
                        unit_errors: vec![],
                    }
                } else {
                    crate::variable::Variable::Var {
                        ident: dep_ident.clone(),
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
                    }
                };
                dep_variables.push((dep_ident, dep_var, meta.size));
            }
        } else {
            // Dependency is not a source variable or implicit variable --
            // this is an unknown dependency. Look up the source location
            // from the AST so the error points to the reference site.
            let loc = parsed
                .variable
                .ast()
                .and_then(|ast| ast.get_var_loc(effective_name))
                .unwrap_or_default();
            return VarDepCollection {
                dep_variables,
                extra_module_refs,
                extra_submodels,
                implicit_module_vars: Vec::new(),
                implicit_module_refs: HashMap::new(),
                implicit_submodels: Vec::new(),
                unknown_dependency: Some(Diagnostic {
                    model: model.name(db).clone(),
                    variable: Some(var.ident(db).clone()),
                    error: DiagnosticError::Equation(crate::common::EquationError {
                        start: loc.start,
                        end: loc.end,
                        code: crate::common::ErrorCode::UnknownDependency,
                    }),
                    severity: DiagnosticSeverity::Error,
                }),
            };
        }
    }

    // Add implicit module variables that this variable's AST references.
    // E.g., INIT(x) creates implicit module $⁚x⁚0⁚init and the variable's
    // AST references $⁚x⁚0⁚init·output -- the compiler needs the implicit
    // module in mini_metadata to resolve the sub-model offset.
    let mut implicit_module_vars: Vec<(Ident<Canonical>, crate::variable::Variable, usize)> =
        Vec::new();
    let mut implicit_module_refs: HashMap<Ident<Canonical>, crate::vm::ModuleKey> = HashMap::new();
    let mut implicit_submodels: Vec<(String, SourceModel)> = Vec::new();

    for implicit_dm_var in &parsed.implicit_vars {
        if let datamodel::Variable::Module(dm_module) = implicit_dm_var {
            let im_name = canonicalize(dm_module.ident.as_str()).into_owned();
            let im_ident: Ident<Canonical> = Ident::new(&im_name);
            if existing_keys.contains(&im_ident) {
                continue;
            }

            let sub_canonical = canonicalize(&dm_module.model_name);
            let sub_size = project_models
                .get(sub_canonical.as_ref())
                .map(|sm| compute_layout(db, *sm, project, false).n_slots)
                .unwrap_or(1);

            let im_var = crate::variable::Variable::Module {
                ident: im_ident.clone(),
                model_name: Ident::new(&dm_module.model_name),
                units: None,
                inputs: vec![],
                errors: vec![],
                unit_errors: vec![],
            };
            implicit_module_vars.push((im_ident.clone(), im_var, sub_size));

            // Build module_refs entry for the implicit module, stripping
            // the module ident prefix from dst (same as resolve_module_input)
            let im_input_prefix = format!("{im_name}\u{00B7}");
            let input_set: BTreeSet<Ident<Canonical>> = dm_module
                .references
                .iter()
                .filter_map(|mr| {
                    let dst_canonical = canonicalize(&mr.dst);
                    let bare = dst_canonical.strip_prefix(&im_input_prefix)?;
                    Some(Ident::new(bare))
                })
                .collect();
            implicit_module_refs.insert(im_ident, (Ident::new(&dm_module.model_name), input_set));

            if let Some(sub_model) = project_models.get(sub_canonical.as_ref()) {
                implicit_submodels.push((dm_module.model_name.clone(), *sub_model));
            }
        }
    }

    VarDepCollection {
        dep_variables,
        extra_module_refs,
        extra_submodels,
        implicit_module_vars,
        implicit_module_refs,
        implicit_submodels,
        unknown_dependency: None,
    }
}

/// Reconstruct the `module_refs` map the caller's `compile_phase` needs.
///
/// `module_refs` is built only from project/variable data (the variable's
/// own module references plus the module/implicit-module references found
/// during the dependency walk) -- it does not depend on the lowered
/// equation, the metadata arena, or the symbol-table context, so it is
/// rebuilt on the caller side from the same single dependency walk
/// (`collect_var_dependencies`) rather than threaded back across the
/// lowering boundary. This is the exact `module_refs` assembly that
/// previously followed the dependency loop inline.
pub(crate) fn build_caller_module_refs(
    db: &dyn Db,
    var: SourceVariable,
    model: SourceModel,
    project: SourceProject,
    is_root: bool,
    module_input_names: &[String],
) -> HashMap<Ident<Canonical>, crate::vm::ModuleKey> {
    let var_ident = var.ident(db).clone();
    let var_ident_canonical: Ident<Canonical> = Ident::new(&var_ident);
    let is_module = var.kind(db) == SourceVariableKind::Module;

    let deps = collect_var_dependencies(db, var, model, project, is_root, module_input_names);

    // We need module_refs for module variables (explicit or implicit)
    let mut module_refs: HashMap<Ident<Canonical>, crate::vm::ModuleKey> = if is_module {
        let ref_prefix = format!("{var_ident}\u{00B7}");
        let input_set: BTreeSet<Ident<Canonical>> = var
            .module_refs(db)
            .iter()
            .filter_map(|mr| {
                let dst_canonical = canonicalize(&mr.dst);
                let bare = dst_canonical.strip_prefix(&ref_prefix)?;
                Some(Ident::new(bare))
            })
            .collect();
        let mut refs = HashMap::new();
        refs.insert(
            var_ident_canonical.clone(),
            (Ident::new(var.model_name(db)), input_set),
        );
        refs
    } else {
        HashMap::new()
    };
    module_refs.extend(deps.implicit_module_refs);
    module_refs.extend(deps.extra_module_refs);
    module_refs
}

/// Lower a single source variable to its per-phase `Var` form.
///
/// Performs parsing, equation lowering, minimal metadata/context
/// construction, and per-phase `Var::new`, returning the owned values the
/// salsa-tracked caller needs plus its diagnostics as data. The caller-
/// owned, lowering-independent values (`converted_dims`, `dim_context`,
/// `model_name_ident`, `module_models`, `inputs`) are borrowed in; the
/// internal coupled cluster (`lowered`, the metadata map, the sub-model
/// arena, the symbol-table context) drops at return.
#[allow(clippy::too_many_arguments)]
pub(crate) fn lower_var_fragment(
    db: &dyn Db,
    var: SourceVariable,
    model: SourceModel,
    project: SourceProject,
    is_root: bool,
    module_input_names: &[String],
    converted_dims: &[crate::dimensions::Dimension],
    dim_context: &crate::dimensions::DimensionsContext,
    model_name_ident: &Ident<Canonical>,
    module_models: &HashMap<Ident<Canonical>, HashMap<Ident<Canonical>, Ident<Canonical>>>,
    inputs: &BTreeSet<Ident<Canonical>>,
) -> LoweredVarFragment {
    use crate::compiler::symbolic::{ReverseOffsetMap, VariableLayout};

    let var_ident = var.ident(db).clone();
    let module_ident_context =
        model_module_ident_context(db, model, project, module_input_names.to_vec());
    let parsed = parse_source_variable_with_module_context(db, var, project, module_ident_context);

    // Accumulate unit definition errors from the parsed variable.
    // These are syntax errors in the unit string (e.g., "bad units
    // here!!!") that are stored in the variable's unit_errors field
    // during parsing but not checked during compilation.
    let mut unit_diags: Vec<Diagnostic> = Vec::new();
    if let Some(unit_errs) = parsed.variable.unit_errors() {
        let model_name = model.name(db).clone();
        for err in unit_errs {
            unit_diags.push(Diagnostic {
                model: model_name.clone(),
                variable: Some(var_ident.clone()),
                error: DiagnosticError::Unit(err),
                severity: DiagnosticSeverity::Error,
            });
        }
    }

    // Check for parse errors -- accumulate each one before bailing out
    if let Some(errors) = parsed.variable.equation_errors()
        && !errors.is_empty()
    {
        let mut fatal_diags: Vec<Diagnostic> = Vec::new();
        for err in &errors {
            fatal_diags.push(Diagnostic {
                model: model.name(db).clone(),
                variable: Some(var.ident(db).clone()),
                error: DiagnosticError::Equation(err.clone()),
                severity: DiagnosticSeverity::Error,
            });
        }
        return LoweredVarFragment::Fatal {
            unit_diags,
            fatal_diags,
        };
    }

    // Build metadata from the full, input-agnostic dependency set so both
    // branches of `if isModuleInput(...)` remain compilable in the mini-context.
    let deps = variable_direct_dependencies_with_context(db, var, project, module_ident_context);

    let project_models = project.models(db);

    // Lower the variable for compilation. Module-type variables need
    // direct construction because lower_variable's resolve_module_input
    // requires a populated models map.
    let lowered = if var.kind(db) == SourceVariableKind::Module {
        let var_name_canonical = canonicalize(&var_ident);
        let input_prefix = format!("{var_name_canonical}\u{00B7}");
        let module_inputs = build_module_inputs(
            model.name(db),
            &input_prefix,
            var.module_refs(db)
                .iter()
                .map(|mr| (canonicalize(&mr.src), canonicalize(&mr.dst))),
        );
        crate::variable::Variable::Module {
            ident: Ident::new(&var_ident),
            model_name: Ident::new(var.model_name(db)),
            units: None,
            inputs: module_inputs,
            errors: vec![],
            unit_errors: vec![],
        }
    } else {
        // Build a minimal ModelStage0 so that ArrayContext::get_dimensions
        // can resolve dependency dimensions during Expr2 lowering. Without
        // this, SUM(arr[*] + 1) fails because the Op2's ArrayBounds are
        // never computed (get_dimensions returns None for dependencies).
        let model_name_str = model.name(db);
        let source_vars = model.variables(db);
        let mut stage0_vars: HashMap<Ident<Canonical>, crate::model::VariableStage0> =
            HashMap::new();

        // Add the current variable
        stage0_vars.insert(Ident::new(&var_ident), parsed.variable.clone());

        // Add dependency variables so get_dimensions can resolve them
        let dep_names: BTreeSet<&String> = deps
            .dt_deps
            .iter()
            .chain(deps.initial_deps.iter())
            .collect();
        for dep_name in &dep_names {
            let effective = dep_name
                .as_str()
                .strip_prefix('\u{00B7}')
                .unwrap_or(dep_name.as_str());
            if effective.contains('\u{00B7}') {
                continue;
            }
            if let Some(dep_sv) = source_vars.get(effective) {
                let dep_parsed = parse_source_variable_with_module_context(
                    db,
                    *dep_sv,
                    project,
                    module_ident_context,
                );
                stage0_vars.insert(Ident::new(effective), dep_parsed.variable.clone());
            }
        }

        let mini_model = crate::model::ModelStage0 {
            ident: Ident::new(model_name_str),
            display_name: model_name_str.to_string(),
            variables: stage0_vars,
            errors: None,
            implicit: false,
        };

        let mut models: HashMap<Ident<Canonical>, &crate::model::ModelStage0> = HashMap::new();
        models.insert(Ident::new(model_name_str), &mini_model);

        let scope = crate::model::ScopeStage0 {
            models: &models,
            dimensions: dim_context,
            model_name: model_name_str,
        };
        crate::model::lower_variable(&scope, &parsed.variable)
    };

    // Check for errors introduced during AST lowering (e.g.,
    // MismatchedDimensions from expr2/expr3 lowering). These are stored
    // in the lowered variable's errors field but not in the parsed
    // variable's errors, so we check them separately.
    if let Some(errors) = lowered.equation_errors()
        && !errors.is_empty()
    {
        let mut fatal_diags: Vec<Diagnostic> = Vec::new();
        for err in &errors {
            fatal_diags.push(Diagnostic {
                model: model.name(db).clone(),
                variable: Some(var.ident(db).clone()),
                error: DiagnosticError::Equation(err.clone()),
                severity: DiagnosticSeverity::Error,
            });
        }
        return LoweredVarFragment::Fatal {
            unit_diags,
            fatal_diags,
        };
    }

    // Build minimal metadata: only {self} + deps
    let var_ident_canonical: Ident<Canonical> = Ident::new(&var_ident);
    let var_size = variable_size(db, var, project);

    // Arena for sub-model stub variables allocated by build_submodel_metadata
    let arena = bumpalo::Bump::new();

    // Assign sequential offsets for the minimal context
    let mut mini_metadata: HashMap<Ident<Canonical>, crate::compiler::VariableMetadata<'_>> =
        HashMap::new();
    let mut mini_offset = if is_root {
        crate::vm::IMPLICIT_VAR_COUNT
    } else {
        0
    };

    // Add implicit vars if root
    if is_root {
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

    // Add self
    mini_metadata.insert(
        var_ident_canonical.clone(),
        crate::compiler::VariableMetadata {
            offset: mini_offset,
            size: var_size,
            var: &lowered,
        },
    );
    mini_offset += var_size;

    // Walk dependencies + implicit modules (single shared walk). An
    // unknown dependency is fatal here, exactly as before.
    let VarDepCollection {
        dep_variables,
        extra_submodels,
        implicit_module_vars,
        implicit_submodels,
        unknown_dependency,
        ..
    } = collect_var_dependencies(db, var, model, project, is_root, module_input_names);

    if let Some(diag) = unknown_dependency {
        return LoweredVarFragment::Fatal {
            unit_diags,
            fatal_diags: vec![diag],
        };
    }

    // Add dep metadata referencing the stored dep_variables
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

    // Build the all_metadata map (model_name -> var_name -> metadata)
    let mut all_metadata: HashMap<
        Ident<Canonical>,
        HashMap<Ident<Canonical>, crate::compiler::VariableMetadata<'_>>,
    > = HashMap::new();
    all_metadata.insert(model_name_ident.clone(), mini_metadata);

    // Populate sub-model metadata for implicit and explicit module sub-models
    for (_sub_name, sub_model) in implicit_submodels.iter().chain(extra_submodels.iter()) {
        build_submodel_metadata(&arena, db, *sub_model, project, &mut all_metadata);
    }

    // Build the mini VariableLayout for symbolization
    let mini_layout =
        crate::compiler::symbolic::layout_from_metadata(&all_metadata, model_name_ident)
            .unwrap_or_else(|_| VariableLayout::new(HashMap::new(), 0));
    let rmap = ReverseOffsetMap::from_layout(&mini_layout);

    // Build tables for compilation -- propagate errors rather than
    // silently dropping them, which would shift table indices and cause
    // lookups to read the wrong table at runtime.
    let mut tables: HashMap<Ident<Canonical>, Vec<crate::compiler::Table>> = HashMap::new();
    {
        let gf_tables = lowered.tables();
        if !gf_tables.is_empty() {
            let table_results: crate::Result<Vec<crate::compiler::Table>> = gf_tables
                .iter()
                .map(|t| crate::compiler::Table::new(&var_ident, t))
                .collect();
            match table_results {
                Ok(ts) if !ts.is_empty() => {
                    tables.insert(var_ident_canonical.clone(), ts);
                }
                Err(table_err) => {
                    return LoweredVarFragment::Fatal {
                        unit_diags,
                        fatal_diags: vec![Diagnostic {
                            model: model.name(db).clone(),
                            variable: Some(var.ident(db).clone()),
                            error: DiagnosticError::Model(table_err),
                            severity: DiagnosticSeverity::Error,
                        }],
                    };
                }
                _ => {}
            }
        }
    }

    // Also collect tables from dependency variables that have graphical
    // functions. When a variable uses LOOKUP(dep, x), the dep's table
    // data must be in the mini-Module's tables map so the bytecode
    // compiler can emit the correct Lookup opcodes.
    let source_vars = model.variables(db);
    let all_dep_names: BTreeSet<&String> = deps
        .dt_deps
        .iter()
        .chain(deps.initial_deps.iter())
        .collect();
    let mut extra_dep_names: Vec<String> = Vec::new();
    if var.kind(db) == SourceVariableKind::Stock {
        for flow_name in var.inflows(db).iter().chain(var.outflows(db).iter()) {
            let canonical = canonicalize(flow_name).into_owned();
            if !all_dep_names.contains(&canonical) {
                extra_dep_names.push(canonical);
            }
        }
    }
    let all_names: Vec<&String> = all_dep_names
        .iter()
        .copied()
        .chain(extra_dep_names.iter())
        .collect();
    for dep_name in &all_names {
        let effective = dep_name
            .as_str()
            .strip_prefix('\u{00B7}')
            .unwrap_or(dep_name.as_str());
        if effective.contains('\u{00B7}') {
            continue;
        }
        let dep_canonical: Ident<Canonical> = Ident::new(effective);
        if tables.contains_key(&dep_canonical) {
            continue;
        }
        if let Some(dep_sv) = source_vars.get(effective) {
            let dep_tables = extract_tables_from_source_var(db, dep_sv);
            if !dep_tables.is_empty() {
                tables.insert(dep_canonical, dep_tables);
            }
        }
    }

    let is_module = var.kind(db) == SourceVariableKind::Module;

    // For module variables, populate sub-model metadata so the compiler
    // can generate correct CallModule bytecodes.
    if is_module {
        let sub_model_name = var.model_name(db);
        let sub_canonical = canonicalize(sub_model_name);
        if let Some(sub_model) = project_models.get(sub_canonical.as_ref()) {
            build_submodel_metadata(&arena, db, *sub_model, project, &mut all_metadata);
        }
    }

    // Build Var for each phase this variable participates in
    let core = crate::compiler::ContextCore {
        dimensions: converted_dims,
        dimensions_ctx: dim_context,
        model_name: model_name_ident,
        metadata: &all_metadata,
        module_models,
        inputs,
    };

    let build_var = |is_initial: bool| {
        crate::compiler::Var::new(
            &crate::compiler::Context::new(core, &var_ident_canonical, is_initial),
            &lowered,
        )
    };

    // Build the per-phase Var values. `build_var(false)` is shared by the
    // flow and stock phases; the original called it separately for each,
    // but `Var::new` is deterministic so one call reused is identical.
    let initial = build_var(true);
    let noninitial = build_var(false);

    // Project the owned (offset, size) view out of the metadata map. The
    // map borrows `lowered` and the arena and must stay internal; this
    // projection reads only the owned offset/size pair, never the
    // borrowed variable, so it crosses back to the caller freely.
    let offsets: VarOffsets = all_metadata
        .iter()
        .map(|(k, v)| {
            (
                k.clone(),
                v.iter()
                    .map(|(vk, vm)| (vk.clone(), (vm.offset, vm.size)))
                    .collect(),
            )
        })
        .collect();

    LoweredVarFragment::Lowered {
        unit_diags,
        per_phase_lowered: PerPhaseLowered {
            initial,
            noninitial,
        },
        tables,
        offsets,
        rmap,
        mini_offset,
    }
}
