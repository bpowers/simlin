// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Module/simulation assembly: turning per-variable symbolic fragments into
//! a concrete `CompiledModule`/`CompiledSimulation`.
//!
//! Holds the table extraction helper (`variable_tables`), the
//! two owners of module input wiring (`build_module_inputs` and
//! `module_input_set`, the only places a wiring is derived from `(src, dst)`
//! reference strings), the per-variable emission tail
//! (`compile_phase_to_per_var_bytecodes` and the `VarFragmentResult` value),
//! the production element-graph source `var_phase_symbolic_fragment_prod`,
//! the resolved recurrence-SCC interleaver (`segment_member_by_element` /
//! `combine_scc_fragment`), the salsa-tracked `assemble_module` (fragment
//! collection, per-program emission order, the one `FragmentMerger` per
//! module) and `assemble_simulation`, and module-instance enumeration
//! (`enumerate_module_instances`). The results-offset map is
//! `layout::flattened_offsets`, beside the layout it flattens.

use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap, HashSet};

use super::*;
use crate::common::{Canonical, Ident};
use crate::compiler::symbolic::Phase;

/// The `compiler::Table`s a source variable's graphical function declares,
/// for the tables map of every fragment that calls it through `LOOKUP`.
///
/// Salsa-tracked so a fragment depends on its dependency's TABLES rather than
/// on the equation those tables are keyed by: the per-element tables of an
/// `Arrayed` equation are read off the equation's element list, so an
/// untracked read would make every caller of a variable recompile on an edit
/// to that variable's equation text. Tracked, the value backdates when the
/// tables are unchanged and an equation-only edit recompiles the edited
/// variable alone (`db::lowered_variable_tests`).
#[salsa::tracked(returns(ref))]
pub(crate) fn variable_tables(
    db: &dyn Db,
    var: SourceVariable,
    project: SourceProject,
) -> Vec<crate::compiler::Table> {
    let source_var = &var;
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
                    && let Some(var_table) = crate::variable::parse_table(Some(gf)).ok().flatten()
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
        Some(gf) => crate::variable::parse_table(Some(gf))
            .ok()
            .flatten()
            .and_then(|vt| crate::compiler::Table::new(ident, &vt).ok())
            .into_iter()
            .collect(),
        None => vec![],
    }
}

/// The namespace prefix of module instance `instance`'s ports: a reference
/// `dst` of `{instance}·{port}` wires `port`, and a `src` under the prefix is
/// internal to the instance.
pub(crate) fn module_input_prefix(instance: &str) -> String {
    format!("{instance}\u{00B7}")
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

/// The set of ports a module instance's wiring binds: each `dst` of `refs`
/// with the instance's prefix (`module_input_prefix`) stripped to the bare
/// sub-model variable name; a `dst` outside the instance's namespace binds
/// nothing. This is the instance's identity at assembly -- the same
/// `stdlib⁚smth1` model compiles to a distinct module per distinct port set,
/// because `isModuleInput(port)` selects the live branch from it.
pub(crate) fn module_input_set<S1: AsRef<str>, S2: AsRef<str>>(
    module_var_prefix: &str,
    refs: impl Iterator<Item = (S1, S2)>,
) -> BTreeSet<Ident<Canonical>> {
    refs.filter_map(|(_src, dst)| {
        let dst_canonical = canonicalize(dst.as_ref());
        let bare = dst_canonical.strip_prefix(module_var_prefix)?;
        Some(Ident::new(bare))
    })
    .collect()
}

/// Result of per-variable compilation: symbolic bytecodes for each phase.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct VarFragmentResult {
    pub fragment: crate::compiler::symbolic::CompiledVarFragment,
    /// The compiler-local half of the flow phase's run-invariance verdict
    /// (GH #712): whether the lowered flow expression is invariant given
    /// that every name it reads is. `None` when the variable has no flow
    /// phase (not in the flows runlist) or when the noninitial lowering
    /// failed. `model_flows_invariant` pairs it with the variable's
    /// `DepRef`s, so it costs no re-lowering and carries no second
    /// dependency set.
    pub flow_locally_invariant: Option<bool>,
}

/// The compiler-local half of run invariance for a variable's flow phase
/// (`compiler::invariance::exprs_are_locally_invariant`): `None` when
/// `flow_var` is an `Err` (noninitial lowering failed) or the expression list
/// is empty.
pub(crate) fn flow_is_locally_invariant(
    flow_var: &Result<crate::compiler::Var, crate::common::Error>,
) -> Option<bool> {
    let flow_var = flow_var.as_ref().ok()?;
    if flow_var.ast.is_empty() {
        return None;
    }
    Some(crate::compiler::invariance::exprs_are_locally_invariant(
        &flow_var.ast,
    ))
}

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

/// Compile one phase's lowered `Vec<Expr>` for a single variable into a
/// layout-independent `PerVarBytecodes`.
///
/// **This is the single fragment emission entry point** (GH #964's "explicit,
/// implicit, and LTM variables share one fragment emission implementation").
/// Five call sites reach it: `compile_var_fragment` (explicit variables),
/// `compile_implicit_var_fragment` (SMOOTH/DELAY/TREND helpers),
/// `var_phase_symbolic_fragment_prod` (the element-cycle SCC graph builder,
/// which must reuse the *exact* production path rather than a re-derivation),
/// and both LTM emitters in `db/ltm/compile.rs`. Every one of them lowers a
/// `compiler::fragment::FragmentInput` with `lower_fragment` and emits under
/// that input's `emit_ctx`.
///
/// `base` is the phase-INVARIANT half of the emission context
/// (`FragmentInput::emit_ctx`), built once per variable by the caller: its
/// `runlist_flows`/`temp_sizes` are ignored (this function fills both in per
/// phase), and everything else -- the fragment's `var_sizes` and `tables`, the
/// project-global `dimensions`, and the module-input set -- is borrowed for
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

    // Extract temp sizes from expressions. The table is indexed by id and
    // sized by the number of distinct ids, which is only right for dense ids;
    // lowering guarantees density (every id the fragment's `TempAllocator`
    // issues and keeps is written, and `Var::new` debug-asserts it), so a gap
    // here is a defect upstream and is refused rather than dropped.
    let mut temp_sizes_map: HashMap<u32, usize> = HashMap::new();
    for expr in exprs {
        crate::compiler::extract_temp_sizes_pub(expr, &mut temp_sizes_map);
    }
    let mut temp_sizes: Vec<usize> = vec![0; temp_sizes_map.len()];
    for (id, size) in &temp_sizes_map {
        let slot = temp_sizes.get_mut(*id as usize).ok_or_else(|| {
            format!(
                "temp ids are not dense: temp {id} is written but the fragment defines only {} temps",
                temp_sizes_map.len()
            )
        })?;
        *slot = *size;
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
/// the exact production lowering + emission path (the variable's
/// `FragmentInput` constructor + `lower_fragment` +
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
/// The input is built by the same constructor `compile_var_fragment` uses,
/// under the default no-module-input wiring `build_var_info(.., &[])` uses:
/// `SccPhase::Dt` selects the non-initial lowering, `SccPhase::Initial` the
/// initial one.
///
/// A synthetic helper (`$\u{205A}` prefix, absent from `model.variables`)
/// that lands in a recurrence SCC is **parent-sourced**: its symbolic
/// `PerVarBytecodes` is the parent variable's NAMED implicit helper built
/// through the implicit constructor (`implicit_fragment_input`, the same
/// chain `compile_implicit_var_fragment` runs), so the element-graph builder
/// consumes it exactly like a real member (element-cycle Phase 3 Task 2 /
/// AC3.1, pinned by `synthetic_helper_symbolic_fragment_is_parent_sourced`).
///
/// **Loud-safe contract (the load-bearing invariant -- formalized here).**
/// This accessor returns `None` -- *never* panics, `expect`s, or `unwrap`s
/// on a sourcing failure -- on EVERY way a node fails to be
/// element-sourced:
/// - no `SourceVariable` AND not a parent-sourceable synthetic helper
///   (absent from `model_implicit_var_info`, or the helper's input or phase
///   failed to build): `None` (the loud-safe signal -- AC3.2);
/// - `ExplicitFragment::Fatal` (the variable did not lower at all):
///   explicit `return None`;
/// - the requested phase's lowering errored (`.ok()?`);
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
    // `#[cfg(test)]` only: an active `UnsourceableVarsGuard` forces this
    // node to take the loud-safe `None` arm, so the AC3.2 regression test
    // can exercise the genuinely-unsourceable in-SCC path through the
    // PRODUCTION `model_dependency_graph` chain (an organic orphan that is
    // neither in `source_vars` nor resolvable via `model_implicit_var_info`
    // is hard to construct deterministically; this is the reliable
    // trigger). It returns the SAME `None` a real no-`SourceVariable`
    // node returns, so the test observes the real loud-safe behavior, not
    // a shim. No effect in non-test builds.
    //
    // It sits OUTSIDE the memo deliberately. Inside the tracked body its
    // verdict would be cached against a key the guard is not part of, so a
    // guard toggled between two calls on one `db` would be ignored by the
    // second -- silently, and in the direction that makes the AC3.2 test pass
    // for the wrong reason. Short-circuiting here keeps the override exactly
    // as immediate as it was when this whole function was a plain call.
    #[cfg(test)]
    if crate::db::dep_graph::var_is_forced_unsourceable(var_name) {
        return None;
    }

    var_phase_symbolic_fragment_memo(db, model, project, var_name.to_string(), phase).clone()
}

/// The memoized body of [`var_phase_symbolic_fragment_prod`].
///
/// Salsa-tracked because this is the engine's own per-variable lowering plus
/// codegen -- the same work `compile_var_fragment` does, under the
/// no-module-input wiring -- run once per SCC member per phase by the cycle
/// gate's element-order probe, and it was a plain function. Instrumented on
/// C-LEARN the probe called it **135 times per cold compile for 57 distinct
/// `(variable, phase)` keys**: the dt refinement verifies BOTH phases as a
/// precondition and the init refinement then re-derives the init order, so a
/// 2.4x duplication was structural rather than incidental. It is ~16% of a
/// cold compile, and the whole of it recurs on every recompile of the same
/// unchanged model.
///
/// The key is `(model, project, var_name, phase)` -- the arguments the body
/// already varied over. `var_name` is a `String` rather than a `&str` because
/// a salsa key must be owned; the wrapper above does that one allocation on
/// the caller's behalf and clones the memo out, which is what keeps every
/// existing call site's ownership unchanged. Both are trivial next to the
/// lowering they replace.
#[salsa::tracked(returns(ref))]
fn var_phase_symbolic_fragment_memo(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
    var_name: String,
    phase: SccPhase,
) -> Option<crate::compiler::symbolic::PerVarBytecodes> {
    use crate::compiler::fragment::lower_fragment;
    use crate::db::fragment_compile::implicit_fragment_input;
    use crate::db::var_fragment::{ExplicitFragment, explicit_fragment_input};

    let var_name = var_name.as_str();
    let source_vars = model.variables(db);
    let is_initial = matches!(phase, SccPhase::Initial);
    // No `SourceVariable` (a synthetic INIT/PREVIOUS/SMOOTH/macro-expansion
    // helper, `$\u{205A}` prefix, absent from `model.variables`): before
    // the loud-safe `None`, attempt parent-`implicit_vars` sourcing
    // (element-cycle Phase 3 Task 2 / AC3.1). A synthetic helper that
    // lands in a recurrence SCC has no `SourceVariable` but DOES resolve
    // in `model_implicit_var_info`; its symbolic `PerVarBytecodes` is the
    // parent variable's named implicit helper built through the SAME
    // constructor the production per-variable assembly uses
    // (`implicit_fragment_input` -- the exact `parent → the parse's helper of
    // that name → parse_var → lower_variable` chain
    // `compile_implicit_var_fragment` runs), so the element-graph builder
    // consumes it exactly like a real member (same layout-independent
    // `SymVarRef` form). The element-cycle SCC identification uses the
    // default no-module-input wiring, so source the helper with
    // `module_input_names = &[]` (matching the real-var arm below; the
    // symbolic fragment is role-independent, so there is no `is_root`
    // selector). Genuinely unsourceable (absent from
    // `model_implicit_var_info` too, or the input or phase failed to build)
    // ⇒ `None`, the loud-safe signal (see the rustdoc's loud-safe contract):
    // the SCC stays unresolved and `CircularDependency` is kept -- no panic,
    // no silent miscompile (AC3.2). The cycle-gate probe wants only the
    // fragment; failures stay silent here (the production assembly path
    // attributes them, GH #1000).
    let Some(sv) = source_vars.get(var_name) else {
        let canonical_name = canonicalize(var_name).into_owned();
        let info = model_implicit_var_info(db, model, project);
        let meta = info.get(&canonical_name)?;
        let input = implicit_fragment_input(db, meta, model, project, &[]).ok()?;
        let var = lower_fragment(&input, is_initial).ok()?;
        return compile_phase_to_per_var_bytecodes(&input.emit_ctx(), &var.ast);
    };

    // The variable did not lower at all => `None` (loud-safe).
    let ExplicitFragment {
        input: Some(input), ..
    } = explicit_fragment_input(db, *sv, model, project, &[])
    else {
        return None;
    };
    // The phase's lowering errored => cannot source its production lowered
    // exprs => `None` (loud-safe).
    let var = lower_fragment(&input, is_initial).ok()?;
    compile_phase_to_per_var_bytecodes(&input.emit_ctx(), &var.ast)
}

/// One member's symbolic opcode stream, split into the pieces the combined
/// fragment relocates independently.
pub(crate) struct MemberSegments {
    /// Whole `AssignTemp` blocks at the head of the member's code that write
    /// temps nothing else in the member writes: the materializations
    /// `compiler::array_operand` hoists ahead of the element code because two
    /// or more elements read them. The prologue belongs to no element --
    /// folding it into the first one and then reordering the segments leaves
    /// every LATER-ordered element reading a temp nothing has written yet --
    /// so `combine_scc_fragment` emits it once, immediately before the first
    /// of its READERS in `element_order`.
    pub(crate) prologue: Vec<crate::compiler::symbolic::SymbolicOpcode>,
    /// The elements whose slice reads a temp the prologue writes. These are
    /// the elements the prologue has to precede, and the ONLY ones: an element
    /// that reads none of its temps may run before it -- and has to, when the
    /// prologue reads that element. Non-empty whenever `prologue` is, because
    /// a candidate block no element reads is not lifted out at all.
    pub(crate) prologue_readers: BTreeSet<usize>,
    /// The per-element slices, keyed by `element_offset`.
    pub(crate) segments: HashMap<usize, Vec<crate::compiler::symbolic::SymbolicOpcode>>,
}

/// The temp `op` reads, if it reads one: a cell (`LoadTempConst`) or a static
/// view whose base is a temp (`PushStaticView`, resolved through
/// `static_views`). `None` for a view id outside the table, which the caller
/// treats as a malformed fragment.
fn temp_read_by(
    op: &crate::compiler::symbolic::SymbolicOpcode,
    static_views: &[crate::compiler::symbolic::SymbolicStaticView],
) -> Result<Option<u32>, String> {
    use crate::compiler::symbolic::{SymStaticViewBase, SymbolicOpcode};
    Ok(match op {
        SymbolicOpcode::LoadTempConst { temp_id, .. } => Some(*temp_id as u32),
        SymbolicOpcode::PushStaticView { view_id } => {
            let view = static_views
                .get(*view_id as usize)
                .ok_or_else(|| format!("static view {view_id} is outside the fragment's table"))?;
            match view.base {
                SymStaticViewBase::Temp(id) => Some(id),
                SymStaticViewBase::Var(_)
                | SymStaticViewBase::PrevVar(_)
                | SymStaticViewBase::InitialVar(_) => None,
            }
        }
        _ => None,
    })
}

/// The temp `op` writes, if it writes one.
///
/// These are the opcodes `compiler::codegen`'s `AssignTemp` arm emits as a
/// block's result: the `BeginIter` loop form (which fills its temp through
/// `StoreIterElement`) and the array-producing / `LookupArray` forms, which
/// write theirs directly. Exhaustive over the variants that carry a temp id;
/// `LoadTempConst` READS one and is deliberately not here.
fn temp_written_by(
    op: &crate::compiler::symbolic::SymbolicOpcode,
) -> Option<crate::bytecode::TempId> {
    use crate::compiler::symbolic::SymbolicOpcode;
    match op {
        SymbolicOpcode::BeginIter {
            write_temp_id,
            has_write_temp,
        } => has_write_temp.then_some(*write_temp_id),
        SymbolicOpcode::VectorElmMap { write_temp_id, .. }
        | SymbolicOpcode::VectorSortOrder { write_temp_id }
        | SymbolicOpcode::Rank { write_temp_id }
        | SymbolicOpcode::LookupArray { write_temp_id, .. }
        | SymbolicOpcode::AllocateAvailable { write_temp_id }
        | SymbolicOpcode::AllocateByPriority { write_temp_id } => Some(*write_temp_id),
        _ => None,
    }
}

/// One candidate prologue block: a whole `AssignTemp` block at the head of a
/// member's code, ending at `end` (exclusive), writing `temp` -- a temp the
/// member writes exactly once -- and reading the temps in `reads`.
struct LeadingBlock {
    end: usize,
    temp: u32,
    reads: Vec<u32>,
}

/// The leading temp-writing blocks of `body`: the longest prefix before the
/// first per-element write that consists of WHOLE blocks each writing a temp
/// the member writes EXACTLY ONCE, in order.
///
/// A block runs from a temp WRITE (`temp_written_by`) to the first point after
/// it where the view stack and the iteration nesting are both back to zero --
/// which is how `compiler::codegen`'s `AssignTemp` arm ends one, whichever form
/// it took: its rustdoc says "AssignTemp doesn't produce a value on the stack",
/// and it pops every view it pushed. Only such a point ends a block. That is
/// what stops the candidates at the end of the hoisted blocks rather than
/// running on into the first element's code, whose own reads balance the view
/// stack too but write no temp. It is an assumption about a DIFFERENT file's
/// emission shape, which is why it is computed and checked here rather than
/// asserted in prose.
///
/// Writing once separates a temp several elements share from one the elements
/// RECYCLE -- `compiler::array_operand` reissues a recycled id per element, so
/// it is written once per element and must stay inside the element that
/// writes it. It does NOT separate a shared temp from a PRIVATE one that only
/// the first element materializes: the materializer emits the shared blocks
/// first and the first element's own blocks right after them, and a private
/// temp is written once too. Which candidates are prologue is decided by who
/// reads them (`segment_member_by_element`).
fn leading_temp_blocks(
    body: &[crate::compiler::symbolic::SymbolicOpcode],
    first_write: usize,
    static_views: &[crate::compiler::symbolic::SymbolicStaticView],
) -> Result<Vec<LeadingBlock>, String> {
    use crate::compiler::symbolic::SymbolicOpcode;

    let mut total_writes: HashMap<crate::bytecode::TempId, usize> = HashMap::new();
    for op in body {
        if let Some(id) = temp_written_by(op) {
            *total_writes.entry(id).or_insert(0) += 1;
        }
    }

    let mut blocks: Vec<LeadingBlock> = Vec::new();
    let mut view_depth: isize = 0;
    let mut iter_depth: isize = 0;
    // The temp the block in progress writes, once its write has been seen.
    let mut writing: Option<crate::bytecode::TempId> = None;
    let mut reads: Vec<u32> = Vec::new();
    for (i, op) in body.iter().take(first_write).enumerate() {
        match op {
            SymbolicOpcode::PushStaticView { .. } | SymbolicOpcode::PushVarViewDirect { .. } => {
                view_depth += 1
            }
            SymbolicOpcode::PopView {} => view_depth -= 1,
            SymbolicOpcode::BeginIter { .. } => iter_depth += 1,
            SymbolicOpcode::EndIter {} => iter_depth -= 1,
            _ => {}
        }
        if let Some(read) = temp_read_by(op, static_views)? {
            reads.push(read);
        }
        if let Some(id) = temp_written_by(op) {
            writing = Some(id);
        }
        if let Some(id) = writing
            && view_depth == 0
            && iter_depth == 0
        {
            // A recycled id (written again by a later element) ends the
            // candidates: everything from here on is the first element's own.
            if total_writes.get(&id).copied().unwrap_or(0) != 1 {
                break;
            }
            blocks.push(LeadingBlock {
                end: i + 1,
                temp: id as u32,
                reads: std::mem::take(&mut reads),
            });
            writing = None;
        }
    }
    Ok(blocks)
}

/// Segment one member's symbolic opcode stream into a prologue and per-element
/// slices, keyed by `element_offset`.
///
/// A per-element slice for element `e` is the run of opcodes up to and
/// including the **write** opcode whose `var.name == member` and
/// `var.element_offset == e` (`AssignCurr | AssignConstCurr |
/// BinOpAssignCurr`), starting after the prologue for the first one. This is
/// the *exact* segmentation `crate::db::dep_graph::symbolic_phase_element_order`
/// builds the SCC element graph from (GH #575) -- the verdict and the combined
/// fragment MUST agree on both boundaries or `element_order` would reference a
/// slice the combiner cannot reproduce, so they share this function rather than
/// a documented contract.
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
/// - a backward jump whose target lies in an EARLIER segment, or in the
///   prologue. Segments are emitted in `element_order`, not in their original
///   order, and a jump offset is relative, so a jump that escaped its own
///   segment would land on whatever opcode happened to sit that far back after
///   the interleave -- a silent miscompile with no bad id and no bad reference
///   to notice. Codegen cannot currently produce one (a `BeginIter` loop writes
///   a TEMP through `StoreIterElement`, and a member's per-element `AssignCurr`
///   is emitted after `EndIter`, so a loop is always wholly inside one
///   segment), which is exactly why it is worth checking rather than asserting
///   in prose: this is an assumption about a DIFFERENT file's emission shape,
///   and nothing else would notice it changing.
///
/// Consumed by `combine_scc_fragment`, which `assemble_module` invokes
/// for every resolved recurrence SCC (the dt flows program and the
/// synthetic-ident init `SymbolicCompiledInitial` path), and by the element
/// graph that orders them.
pub(crate) fn segment_member_by_element(
    member: &str,
    code: &[crate::compiler::symbolic::SymbolicOpcode],
    static_views: &[crate::compiler::symbolic::SymbolicStaticView],
) -> Result<MemberSegments, String> {
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
    // not a per-element current-value write of this member), does not.
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

    let Some(first_write) = body.iter().position(|op| write_element(op).is_some()) else {
        return Err(format!(
            "SCC member `{member}` has no per-element write \
             opcode; not element-sourceable for the combined \
             fragment"
        ));
    };
    let first_elem = write_element(&body[first_write])
        .expect("first_write indexes a per-element write by construction");
    let blocks = leading_temp_blocks(body, first_write, static_views)?;
    let prefix_end = blocks.last().map_or(0, |b| b.end);

    // Which elements read each temp, over the code after the candidate
    // prefix: a segment reading a temp, whether as a cell or as a view, and
    // the opcodes trailing the final write belonging to the last element.
    let mut element_readers: HashMap<u32, BTreeSet<usize>> = HashMap::new();
    let mut segment_reads: BTreeSet<u32> = BTreeSet::new();
    let mut scanned_elem: Option<usize> = None;
    for op in &body[prefix_end..] {
        if let Some(id) = temp_read_by(op, static_views)? {
            segment_reads.insert(id);
        }
        if let Some(elem) = write_element(op) {
            for id in std::mem::take(&mut segment_reads) {
                element_readers.entry(id).or_default().insert(elem);
            }
            scanned_elem = Some(elem);
        }
    }
    if let Some(elem) = scanned_elem {
        for id in segment_reads {
            element_readers.entry(id).or_default().insert(elem);
        }
    }

    // A candidate block is prologue when two or more elements read its temp,
    // or when a later prologue block does (a shared body materialized from a
    // shared operand: the operand is read by that body alone). Decided in
    // reverse so the later block's verdict is known, and a block that is NOT
    // prologue belongs to the first written element, whose own reads of the
    // shared temps it carries count as that element's. The prologue is the
    // qualifying PREFIX: the materializer emits every shared block ahead of
    // the first element's own, so a qualifying block behind a private one is
    // an emission shape this segmentation does not cover, and it is refused
    // rather than left inside an element other readers may precede.
    let mut prologue: Vec<bool> = vec![false; blocks.len()];
    for i in (0..blocks.len()).rev() {
        let temp = blocks[i].temp;
        let mut readers: BTreeSet<usize> = element_readers.get(&temp).cloned().unwrap_or_default();
        let mut read_by_later_prologue = false;
        for (later, block) in blocks.iter().enumerate().skip(i + 1) {
            if block.reads.contains(&temp) {
                if prologue[later] {
                    read_by_later_prologue = true;
                } else {
                    readers.insert(first_elem);
                }
            }
        }
        prologue[i] = readers.len() >= 2 || read_by_later_prologue;
    }
    let prologue_blocks = prologue.iter().position(|is| !is).unwrap_or(blocks.len());
    if prologue[prologue_blocks..].iter().any(|is| *is) {
        return Err(format!(
            "SCC member `{member}` has a temp several elements read behind one \
             only its first element reads; not element-sourceable for the \
             combined fragment"
        ));
    }
    let prologue_len = if prologue_blocks == 0 {
        0
    } else {
        blocks[prologue_blocks - 1].end
    };
    let mut prologue_readers: BTreeSet<usize> = BTreeSet::new();
    for block in &blocks[..prologue_blocks] {
        if let Some(readers) = element_readers.get(&block.temp) {
            prologue_readers.extend(readers.iter().copied());
        }
        if blocks[prologue_blocks..]
            .iter()
            .any(|later| later.reads.contains(&block.temp))
        {
            prologue_readers.insert(first_elem);
        }
    }

    // Jump containment. `lower_bound[pc]` is the first index of the segment
    // `pc` ends up in: segments start just after the previous per-element
    // write (or after the prologue for the first), except that opcodes
    // trailing the FINAL write are appended to the last segment rather than
    // starting a new one (see below), so their bound is that segment's start.
    // A prologue opcode's bound is 0: the prologue is relocated as a unit.
    let mut lower_bound: Vec<usize> = Vec::with_capacity(body.len());
    let mut start = 0usize;
    for (pc, op) in body.iter().enumerate() {
        if pc == prologue_len {
            start = prologue_len;
        }
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

    let prologue: Vec<SymbolicOpcode> = body[..prologue_len].to_vec();
    let mut segments: HashMap<usize, Vec<SymbolicOpcode>> = HashMap::new();
    let mut current: Vec<SymbolicOpcode> = Vec::new();
    let mut last_written_elem: Option<usize> = None;

    for op in &body[prologue_len..] {
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

    Ok(MemberSegments {
        prologue,
        prologue_readers,
        segments,
    })
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
/// `FragmentMerger::concatenate`.** Resources are MEMBER-scoped, not
/// element-scoped: each member's fragment is absorbed into the SCC's
/// `FragmentMerger` exactly ONCE (in `element_order`'s member
/// first-encounter order, so the offset assignment is deterministic),
/// yielding that member's resource base offsets and merging its
/// side-channels (literals, GFs, modules, views, temps, dim-lists) the
/// same way the module's merger merges a fragment. Every segment of
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
/// `assemble_module` invokes this for every resolved recurrence SCC
/// (`combine_resolved_sccs`): `program_fragments` then replaces the members'
/// per-variable fragments with the combined one at the first member's runlist
/// slot (the dt fragment in the flows, the init fragment as one
/// synthetic-ident `SymbolicCompiledInitial`).
pub(crate) fn combine_scc_fragment(
    scc: &ResolvedScc,
    member_fragments: &HashMap<Ident<Canonical>, crate::compiler::symbolic::PerVarBytecodes>,
) -> Result<crate::compiler::symbolic::PerVarBytecodes, String> {
    use crate::compiler::symbolic::{
        FragmentMerger, FragmentResourceOffsets, SymbolicOpcode, TempStrategy, renumber_opcode,
    };

    // Absorb each member ONCE, in `element_order`'s member first-encounter
    // order, so per-member resource offsets are assigned deterministically
    // (the interleave is a pure reordering => byte-stable output, AC2.3).
    // The combined fragment is itself a fragment the module's merger absorbs
    // at assembly, so it is built in an isolated resource namespace -- its own
    // merger -- exactly as a per-variable fragment is. The members' segments
    // are interleaved, so their temps must not share slots (`Sum`, M5).
    let mut merger = FragmentMerger::new(TempStrategy::Sum);
    let mut absorbed: HashMap<Ident<Canonical>, FragmentResourceOffsets> = HashMap::new();
    // Per-member, per-element renumbered segments. Keyed by the same
    // `(member, element)` identity `element_order` carries.
    let mut renumbered_segments: HashMap<(Ident<Canonical>, usize), Vec<SymbolicOpcode>> =
        HashMap::new();
    // Per-member prologues, keyed by the `element_order` index they are emitted
    // immediately before: the member's first READER of the prologue's temps.
    // Two members' first readers are two different entries, so the keys never
    // collide.
    let mut prologue_before: HashMap<usize, Vec<SymbolicOpcode>> = HashMap::new();

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
        // per-fragment prologue the module's merger runs.
        let (off, gf_remap) = merger.absorb(frag)?;
        absorbed.insert(member.clone(), off);

        // Segment the member's symbolic code into its prologue and its
        // per-element slices (the same function the Task 4 verdict builder
        // orders them with, so the two cannot disagree about a boundary),
        // then renumber every opcode by THIS member's offsets and GF remap.
        let seg =
            segment_member_by_element(member.as_str(), &frag.symbolic.code, &frag.static_views)?;
        let renumber_all = |ops: &[SymbolicOpcode]| -> Result<Vec<SymbolicOpcode>, String> {
            ops.iter()
                .map(|op| {
                    renumber_opcode(
                        op,
                        off.lit_offset,
                        &gf_remap,
                        off.mod_offset,
                        off.view_offset,
                        off.temp_offset,
                        off.dl_offset,
                    )
                })
                .collect()
        };

        // The prologue is emitted once, immediately before the first of its
        // readers in `element_order`, so every CURRENT-value read it makes of
        // an in-SCC element has to be evaluated before that point. The element
        // graph adds exactly that constraint (`symbolic_phase_element_order`
        // wires a prologue read into every reader of the prologue), so this
        // never fires on an order that graph produced -- it is here because
        // the two live in different files and a disagreement between them is
        // precisely the class of bug that produces a well-formed program
        // reading a temp nothing has written.
        if !seg.prologue.is_empty() {
            let first_reader = scc
                .element_order
                .iter()
                .position(|(m, e)| m == member && seg.prologue_readers.contains(e))
                .ok_or_else(|| {
                    format!(
                        "SCC member `{}` has a prologue but none of its readers is in element_order; keeping CircularDependency",
                        member.as_str()
                    )
                })?;
            let mut reads: BTreeSet<(Ident<Canonical>, usize)> = BTreeSet::new();
            for op in &seg.prologue {
                if !crate::db::dep_graph::ordering_reads(
                    op,
                    &frag.static_views,
                    &scc.phase,
                    &mut reads,
                ) {
                    return Err(format!(
                        "SCC member `{}` has a prologue opcode referencing a static view outside its fragment; keeping CircularDependency",
                        member.as_str()
                    ));
                }
            }
            for (name, elem) in &reads {
                if !scc.members.contains(name) {
                    continue;
                }
                let read_at = scc
                    .element_order
                    .iter()
                    .position(|(m, e)| m == name && e == elem);
                if read_at.is_none_or(|at| at >= first_reader) {
                    return Err(format!(
                        "SCC member `{}`'s prologue reads `{}`[{}], which element_order does not evaluate before the prologue's first reader; keeping CircularDependency",
                        member.as_str(),
                        name.as_str(),
                        elem
                    ));
                }
            }
            prologue_before.insert(first_reader, renumber_all(&seg.prologue)?);
        }

        for (elem, ops) in seg.segments {
            renumbered_segments.insert((member.clone(), elem), renumber_all(&ops)?);
        }
    }

    // Emit the renumbered segments in `element_order`. Every entry must
    // map to exactly one segment (a missing one is loud-safe). Each
    // segment is consumed exactly once: a duplicate `(member, element)` in
    // `element_order` (which the Task 4 builder cannot produce -- nodes
    // are unique) would try to reuse a removed segment and fail loud-safe.
    let mut combined_code: Vec<SymbolicOpcode> = Vec::new();
    for (index, (member, elem)) in scc.element_order.iter().enumerate() {
        // A member's prologue precedes the first of its readers, wherever the
        // order puts that reader.
        if let Some(prologue) = prologue_before.remove(&index) {
            combined_code.extend(prologue);
        }
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

/// The fragments a module assembles, keyed by variable, plus the order its
/// LTM fragments follow the runlists in.
struct ModuleFragments<'db> {
    by_name: HashMap<String, Cow<'db, VarFragmentResult>>,
    /// The LTM fragments in emission order: synthetic variables in generation
    /// order, then implicit helpers in name order. They follow the runlist in
    /// every program, each contributing the phases it has bytecode for. LTM
    /// variables have no ordering constraint against the scheduled variables
    /// because `PREVIOUS` reads the previous step's committed values.
    ltm_tail: Vec<String>,
}

/// Compile every fragment of the module -- explicit variables, implicit
/// (stdlib) helpers and, under LTM, the synthetic variables and their implicit
/// helpers -- into one map, in layout order.
///
/// An LTM fragment whose variable references resolve outside this module's
/// layout is dropped: a sub-model's LTM equation can name an implicit stdlib
/// instance that exists only under the root's qualified name
/// (`$⁚var⁚0⁚smth1`), and the root generates its own scores for it, so the
/// sub-model's copy would be a duplicate. The LTM selection logic (the
/// salsa-cached `(from, to)` path vs. direct compilation) lives in
/// `compile_ltm_fragment_for`, so `model_ltm_fragment_diagnostics` reports
/// exactly the compile failures this pass drops.
fn collect_fragments<'db>(
    db: &'db dyn Db,
    model: SourceModel,
    project: SourceProject,
    module_inputs: ModuleInputSet<'db>,
    layout: &crate::compiler::symbolic::VariableLayout,
) -> ModuleFragments<'db> {
    use crate::compiler::symbolic::fragment_vars_in_layout;

    let mut by_name: HashMap<String, Cow<'db, VarFragmentResult>> = HashMap::new();
    for (name, svar) in model.variables(db).iter() {
        if let Some(result) = compile_var_fragment(db, *svar, model, project, module_inputs) {
            by_name.insert(name.clone(), Cow::Borrowed(result));
        }
    }
    for name in model_implicit_var_info(db, model, project).keys() {
        if let Some(result) =
            compile_implicit_var_fragment(db, model, project, name.clone(), module_inputs)
        {
            by_name.insert(name.clone(), Cow::Borrowed(result));
        }
    }

    let mut ltm_tail: Vec<String> = Vec::new();
    if project.ltm_enabled(db) {
        // GH #486's non-Euler rejection is NOT enforced per module here: the
        // integration method that actually runs is a single, main-model-
        // governed property of the whole assembled simulation, so it is
        // resolved once, against the main-governed method, in
        // `assemble_simulation` (`ltm_non_euler_guard`).
        let ltm_vars = model_ltm_variables(db, model, project);
        for (index, ltm_var) in ltm_vars.vars.iter().enumerate() {
            let name = canonicalize(&ltm_var.name).into_owned();
            if let Some(result) = compile_ltm_fragment_for(db, model, project, index, ltm_var)
                && fragment_vars_in_layout(&result.fragment, layout)
            {
                by_name.insert(name.clone(), Cow::Borrowed(result));
                ltm_tail.push(name);
            }
        }
        // Each implicit helper rides on its `LtmImplicitVarMeta`, so no parent
        // equation is re-parsed here.
        let ltm_implicit = model_ltm_implicit_var_info(db, model, project);
        let mut names: Vec<&String> = ltm_implicit.keys().collect();
        names.sort_unstable();
        // The plain lowering helpers take the input names as `&[String]`.
        let module_input_names = module_inputs.names(db);
        for name in names {
            if by_name.contains_key(name) {
                continue;
            }
            if let Some(result) = compile_ltm_implicit_var_fragment(
                db,
                &ltm_implicit[name],
                model,
                project,
                module_input_names,
                None,
            ) && fragment_vars_in_layout(&result.fragment, layout)
            {
                by_name.insert(name.clone(), Cow::Owned(result));
                ltm_tail.push(name.clone());
            }
        }
    }
    ModuleFragments { by_name, ltm_tail }
}

/// The combined fragments of a module's resolved recurrence SCCs.
///
/// A multi-member (or single-variable) recurrence SCC whose induced element
/// graph the cycle gate proved acyclic (`ModelDepGraphResult::resolved_sccs`)
/// is lowered as ONE combined `PerVarBytecodes` whose per-element writes follow
/// the SCC's verified `element_order` (`combine_scc_fragment`), instead of the
/// members' individual one-contiguous-block-per-variable fragments -- the
/// latter cannot express the required cross-member per-element interleaving.
/// Each member's symbolic fragment is sourced via the EXACT production
/// emission path (`var_phase_symbolic_fragment_prod`, never a re-derivation),
/// so every write keeps its original `SymVarRef { name, element_offset }`;
/// `resolve_module` therefore maps each write to the same model slot the
/// acyclic layout assigns and the results-offset map is unchanged (AC2.3).
///
/// Two combined fragments per SCC: the DT one (only for a `Dt`-phase SCC -- an
/// `Initial`-phase SCC is stock-backed and stocks are not flow variables) and
/// the INIT one, built for EVERY resolved SCC, because a `Dt`-phase aux SCC's
/// members carry the SAME recurrence in their init equations and the initials
/// runlist groups both phases contiguously (see the `build_scc_grouping(false)`
/// runlist comment). The SCC's `element_order` (dt order for a `Dt` SCC) is
/// valid for the init interleave because a same-equation aux's init and dt
/// element graphs are structurally identical; if they ever diverge (a member's
/// init fragment cannot be segmented to match `element_order`)
/// `combine_scc_fragment` returns a loud-safe `Err` and assembly fails with an
/// Assembly diagnostic rather than miscompiling.
struct SccFragments {
    dt: Vec<Option<crate::compiler::symbolic::PerVarBytecodes>>,
    init: Vec<crate::compiler::symbolic::PerVarBytecodes>,
    /// The synthetic ident the init fragment is filed under
    /// (`$⁚scc⁚init⁚{n}`). `resolve_module` / `eval_initials` consume
    /// `compiled_initials` positionally (ident-agnostic; offsets re-derived
    /// from the bytecode's `AssignCurr` operands), so one
    /// `SymbolicCompiledInitial` may write every member's init slots.
    init_ident: Vec<String>,
    /// Member -> SCC index. A member is in at most one SCC (the SCCs are
    /// pairwise disjoint -- see `scc_map_from_resolved`), so this is
    /// well-defined.
    of_member: HashMap<String, usize>,
}

/// Build every resolved SCC's combined fragments. Loud-safe: an unsourceable
/// member (`var_phase_symbolic_fragment_prod` returned `None`) or a
/// `combine_scc_fragment` error is an `Err` that aborts assembly; a combined
/// fragment is NEVER silently dropped or partially injected.
fn combine_resolved_sccs(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
    sccs: &[ResolvedScc],
) -> Result<SccFragments, String> {
    let combine = |scc: &ResolvedScc, phase: SccPhase| {
        let mut member_fragments = HashMap::with_capacity(scc.members.len());
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
                    "resolved recurrence SCC member `{}` has no sourceable symbolic \
                             fragment for its phase; cannot build the combined per-element \
                             fragment",
                    member.as_str()
                )
            })?;
            member_fragments.insert(member.clone(), frag);
        }
        combine_scc_fragment(scc, &member_fragments)
    };

    let mut out = SccFragments {
        dt: Vec::with_capacity(sccs.len()),
        init: Vec::with_capacity(sccs.len()),
        init_ident: Vec::with_capacity(sccs.len()),
        of_member: HashMap::new(),
    };
    for (idx, scc) in sccs.iter().enumerate() {
        out.dt.push(if scc.phase == SccPhase::Dt {
            Some(combine(scc, SccPhase::Dt)?)
        } else {
            None
        });
        out.init.push(combine(scc, SccPhase::Initial)?);
        out.init_ident
            .push(format!("$\u{205A}scc\u{205A}init\u{205A}{idx}"));
        for member in &scc.members {
            out.of_member.insert(member.as_str().to_string(), idx);
        }
    }
    Ok(out)
}

/// One fragment in a program's emission order.
struct Emitted<'a> {
    /// The initial's ident (`SymbolicCompiledInitial::ident`); unused by the
    /// other two programs.
    ident: &'a str,
    /// Whether the flow fragment is run-invariant (GH #712). Only an ordinary
    /// scheduled flow variable can be; an SCC's combined fragment reads
    /// co-member current values within the step, and every LTM fragment reads
    /// `PREVIOUS`, so those are dynamic.
    invariant: bool,
    bc: &'a crate::compiler::symbolic::PerVarBytecodes,
}

/// The fragments of one program, in emission order.
///
/// The runlist comes first. A member of a resolved recurrence SCC is replaced
/// by the SCC's combined fragment for the phase, injected once at the first
/// member and skipped at the rest: the members are a contiguous, byte-stable
/// block at the SCC's topological slot, so the combined fragment lands in the
/// correct relative position (the runlist itself is salsa-owned and never
/// mutated). A module input's value is written by the parent
/// (`EvalModule`/`LoadModuleInput`): in the initials and flows its copy
/// fragment (`LoadModuleInput -> AssignCurr`) is emitted when it has one and
/// nothing is missed when it does not, and in the stocks it is never emitted.
/// Any other scheduled variable without bytecode for the phase is reported in
/// `missing`. Then the LTM tail, each fragment contributing the phases it has
/// bytecode for.
fn program_fragments<'a>(
    phase: Phase,
    runlist: &'a [String],
    fragments: &'a ModuleFragments<'_>,
    sccs: &'a SccFragments,
    is_module_input: impl Fn(&str) -> bool,
    flows_invariant: &BTreeSet<String>,
    missing: &mut Vec<String>,
) -> Vec<Emitted<'a>> {
    let mut out: Vec<Emitted<'a>> = Vec::new();
    let mut injected: HashSet<usize> = HashSet::new();
    for name in runlist {
        if let Some(&scc) = sccs.of_member.get(name.as_str()) {
            let combined = match phase {
                Phase::Initials => Some(&sccs.init[scc]),
                Phase::Flows => sccs.dt[scc].as_ref(),
                // A stock-backed (`Initial`-phase) SCC's members update their
                // stocks through their own fragments.
                Phase::Stocks => None,
            };
            if let Some(bc) = combined {
                if injected.insert(scc) {
                    out.push(Emitted {
                        ident: &sccs.init_ident[scc],
                        invariant: false,
                        bc,
                    });
                }
                continue;
            }
        }
        if phase == Phase::Stocks && is_module_input(name) {
            continue;
        }
        match fragments
            .by_name
            .get(name)
            .and_then(|f| f.fragment.phase(phase))
        {
            Some(bc) => out.push(Emitted {
                ident: name,
                invariant: phase == Phase::Flows && flows_invariant.contains(name),
                bc,
            }),
            None if is_module_input(name) => {}
            None => missing.push(name.clone()),
        }
    }
    for name in &fragments.ltm_tail {
        if let Some(bc) = fragments
            .by_name
            .get(name)
            .and_then(|f| f.fragment.phase(phase))
        {
            out.push(Emitted {
                ident: name,
                invariant: false,
                bc,
            });
        }
    }
    out
}

/// A fragment's opcode count without its trailing `Ret`: what `concatenate`
/// contributes per fragment, and therefore the unit of the run-invariant
/// prefix boundary (M6).
fn ret_stripped_len(frag: &crate::compiler::symbolic::PerVarBytecodes) -> usize {
    let code = &frag.symbolic.code;
    if code.last() == Some(&crate::compiler::symbolic::SymbolicOpcode::Ret) {
        code.len() - 1
    } else {
        code.len()
    }
}

/// The module's dimension table and name pool (`ByteCodeContext::dimensions` /
/// `names`) from the project's converted dimensions.
fn dimension_metadata(
    converted_dims: &[crate::dimensions::Dimension],
) -> (Vec<String>, Vec<crate::bytecode::DimensionInfo>) {
    let mut names: Vec<String> = Vec::new();
    let mut infos: Vec<crate::bytecode::DimensionInfo> = Vec::new();
    let intern = |names: &mut Vec<String>, name: &str| -> crate::bytecode::NameId {
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
                let name_id = intern(&mut names, dim_name.as_str());
                infos.push(crate::bytecode::DimensionInfo::indexed(
                    name_id,
                    *size as u16,
                ));
            }
            crate::dimensions::Dimension::Named(dim_name, named_dim) => {
                let name_id = intern(&mut names, dim_name.as_str());
                let element_name_ids: smallvec::SmallVec<[crate::bytecode::NameId; 8]> = named_dim
                    .elements
                    .iter()
                    .map(|elem| intern(&mut names, elem.as_str()))
                    .collect();
                infos.push(crate::bytecode::DimensionInfo::named(
                    name_id,
                    element_name_ids,
                ));
            }
        }
    }
    (names, infos)
}

/// Assemble a complete CompiledModule from per-variable fragments.
///
/// Salsa-tracked: the per-module assembly (fragment collection, SCC
/// combined-fragment build, the merge, resolve) is memoized so an unchanged
/// module (same `model`/`project`/`is_root`/`module_inputs`) is a pure cache
/// hit -- no re-merge, no re-resolve. The success payload rides behind an
/// `Arc` so salsa's clone-out on each cache-hit read is a single refcount bump
/// rather than a deep bytecode clone.
///
/// `module_inputs` is an interned `ModuleInputSet` (the sorted canonical input
/// names). The empty set is the no-inputs case and, being a single interned
/// id, shares one cache entry across all no-input callers.
#[salsa::tracked(returns(clone))]
pub fn assemble_module<'db>(
    db: &'db dyn Db,
    model: SourceModel,
    project: SourceProject,
    is_root: bool,
    module_inputs: ModuleInputSet<'db>,
) -> Result<std::sync::Arc<crate::bytecode::CompiledModule>, String> {
    use crate::compiler::symbolic::{
        FragmentMerger, SymbolicCompiledInitial, SymbolicCompiledModule, TempStrategy,
        resolve_module,
    };

    let dep_graph = model_dependency_graph(db, model, project, module_inputs);
    if dep_graph.has_cycle() {
        return Err(format!(
            "model '{}' has circular dependencies",
            model.name(db)
        ));
    }
    // `compute_layout` returns the role-independent *body* layout (offsets
    // from 0). The root module relocates it by `IMPLICIT_VAR_COUNT` to
    // reserve the implicit-global slots; every fragment `SymVarRef` and
    // module-decl `off` is resolved against this final layout, so the root
    // shift lands once here and the submodule path uses the body layout
    // verbatim (the parent relocates a submodule via its module-decl `off`,
    // which already comes from the parent's shifted layout).
    let body_layout = compute_layout(db, model, project);
    let root_layout;
    let layout: &crate::compiler::symbolic::VariableLayout = if is_root {
        root_layout = body_layout.root_shifted();
        &root_layout
    } else {
        body_layout
    };
    // Fail fast (before compiling thousands of fragments) when the layout
    // exceeds the bytecode's u16-addressable slot range. `resolve_var_ref` has
    // a defense-in-depth checked cast, but by then the expensive per-variable
    // compilation has already run; checking here surfaces one clear error
    // immediately. See `check_layout_addressable` for why a silent overflow
    // corrupts every result.
    crate::compiler::symbolic::check_layout_addressable(layout.n_slots, model.name(db))?;

    let fragments = collect_fragments(db, model, project, module_inputs, layout);
    let sccs = combine_resolved_sccs(db, model, project, &dep_graph.resolved_sccs)?;

    // The `is_module_input` predicate, reconstructed from the interned set --
    // the exact inverse of the input set's key derivation.
    let canonical_inputs = module_inputs.canonical_input_set(db);
    let is_module_input = |name: &str| canonical_inputs.contains(&*canonicalize(name));
    // `model_flows_invariant` guards internally (empty for a non-root module);
    // call it unconditionally so the check lives in one place.
    let flows_invariant = model_flows_invariant(db, model, project, is_root, module_inputs);

    let mut missing: Vec<String> = Vec::new();
    let initials = program_fragments(
        Phase::Initials,
        &dep_graph.runlist_initials,
        &fragments,
        &sccs,
        is_module_input,
        &flows_invariant,
        &mut missing,
    );
    let flows = program_fragments(
        Phase::Flows,
        &dep_graph.runlist_flows,
        &fragments,
        &sccs,
        is_module_input,
        &flows_invariant,
        &mut missing,
    );
    let stocks = program_fragments(
        Phase::Stocks,
        &dep_graph.runlist_stocks,
        &fragments,
        &sccs,
        is_module_input,
        &flows_invariant,
        &mut missing,
    );
    if !missing.is_empty() {
        return Err(format!(
            "failed to compile fragments for variables: {}",
            missing.join(", ")
        ));
    }

    // ── Run-invariant flow partition (GH #712) ─────────────────────────
    //
    // Stably partition the flow fragments so every run-invariant fragment
    // precedes every dynamic one, preserving each group's original relative
    // (topological) order. This is a valid topological order: the invariant
    // subgraph is closed under its dependencies (an invariant var cannot
    // depend on a dynamic var, by construction), so no reader is moved ahead
    // of a dependency. The split boundary is the invariant prefix's opcode
    // length in the concatenated flow program.
    //
    // `FragmentMerger::concatenate` is opcode-count-preserving (M6): it strips
    // each fragment's single trailing `Ret` and copies every remaining opcode
    // 1:1, appending one terminal `Ret`. So the prefix opcode length is exactly
    // the sum of the invariant fragments' Ret-stripped code lengths.
    // `resolve_module` (symbolic -> concrete) is likewise 1:1 and does no
    // fusion, so this count is the boundary in the final `compiled_flows.code`.
    // The boundary is fusion-proof at `Vm::new` (no `fuse_three_address` window
    // crosses a fragment boundary -- every fragment ends in an `Assign*`, and
    // no window starts with or uses an `Assign*` as a combiner); see the design
    // note `docs/design-plans/2026-06-04-time-invariant-hoisting.md`.
    let (invariant_flows, dynamic_flows): (Vec<&Emitted<'_>>, Vec<&Emitted<'_>>) =
        flows.iter().partition(|e| e.invariant);
    let flows_invariant_opcode_len: usize =
        invariant_flows.iter().map(|e| ret_stripped_len(e.bc)).sum();
    let flow_frags: Vec<&crate::compiler::symbolic::PerVarBytecodes> = invariant_flows
        .iter()
        .chain(dynamic_flows.iter())
        .map(|e| e.bc)
        .collect();
    let stock_frags: Vec<&crate::compiler::symbolic::PerVarBytecodes> =
        stocks.iter().map(|e| e.bc).collect();

    // One merger builds every program of the module, in the order initials,
    // flows, stocks, so every program's module / view / temp / dim-list /
    // graphical-function ids index the one table set it finishes with (M8);
    // each initial is a program of its own because `eval_initials` runs them
    // one at a time, and each program keeps its own literal pool. Fragments
    // are emitted as contiguous runs, so temps recycle (`Recycle`, M5).
    let mut merger = FragmentMerger::new(TempStrategy::Recycle);
    let compiled_initials = initials
        .iter()
        .map(|e| {
            Ok(SymbolicCompiledInitial {
                ident: Ident::new(e.ident),
                bytecode: merger.standalone_program(e.bc)?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let compiled_flows = merger.concatenate(&flow_frags)?;
    let compiled_stocks = merger.concatenate(&stock_frags)?;
    let tables = merger.into_side_channels();

    // Read the project-global converted dims from the salsa-cached query
    // instead of rebuilding them here.
    let (names, dimensions) = dimension_metadata(project_converted_dimensions(db, project));

    let sym_module = SymbolicCompiledModule {
        ident: Ident::new(model.name(db)),
        compiled_initials,
        compiled_flows,
        compiled_stocks,
        graphical_functions: tables.graphical_functions,
        module_decls: tables.module_decls,
        static_views: tables.static_views,
        dimensions,
        names,
        temp_offsets: tables.temp_offsets,
        temp_total_size: tables.temp_total_size,
        dim_lists: tables.dim_lists,
        flows_invariant_opcode_len,
    };

    // Resolve symbolic -> concrete offsets. The CompiledModule stays a pure,
    // salsa-cached artifact; the 3-address fusion (R2) is applied later, at
    // Vm::new, to the execution copy of the bytecode. The success payload is
    // wrapped in an `Arc` so salsa's clone-out is a refcount bump (the inner
    // bytecode is large).
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
/// `Arc` so clone-out is a refcount bump rather than a deep clone of the
/// modules/offsets maps.
#[salsa::tracked(returns(clone))]
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

    // The results-offset map: the root layout, flattened through every module
    // instance, so a name reads the slot its fragment writes.
    let root_model = project_models[main_model_canonical.as_ref()];
    let offsets = flattened_offsets(db, project, root_model);

    Ok(std::sync::Arc::new(CompiledSimulation::new(
        compiled_modules,
        specs,
        root_key,
        offsets,
    )))
}

type ModuleInstanceMap = HashMap<Ident<Canonical>, BTreeSet<BTreeSet<Ident<Canonical>>>>;

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

/// Enumerate all module instances in a project, starting from the main model.
/// Returns a map from model name to the set of distinct input sets that model
/// is instantiated with.
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

        let inputs = module_input_set(
            &module_input_prefix(var_name),
            source_var
                .module_refs(db)
                .iter()
                .map(|mr| (&mr.src, &mr.dst)),
        );

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
        let parsed = parse_source_variable(db, meta.parent_source_var, project);
        let inputs: BTreeSet<Ident<Canonical>> =
            if let Some(dm_module) = meta.find_in(parsed).and_then(|iv| iv.module()) {
                module_input_set(
                    &module_input_prefix(name),
                    dm_module.references.iter().map(|mr| (&mr.src, &mr.dst)),
                )
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

                let inputs: BTreeSet<Ident<Canonical>> =
                    if let Some(dm_module) = im_meta.variable.module() {
                        module_input_set(
                            &module_input_prefix(im_name),
                            dm_module.references.iter().map(|mr| (&mr.src, &mr.dst)),
                        )
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
