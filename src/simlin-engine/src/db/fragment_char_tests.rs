// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Characterization pins for the **per-variable fragment compiler** -- the
//! salsa-cached unit of production compilation (`db::compile_var_fragment` and
//! its implicit/LTM siblings), whose emission half currently reaches its
//! layout-independent result through a mini-layout / stand-in
//! `compiler::Module` / `symbolize_*` round trip (GH #964).
//!
//! This suite is the gate the round-trip deletion is measured against. The
//! integration corpus (`tests/integration/simulate.rs`) pins *numeric* results
//! of whole models, which is necessary but far too coarse here: it cannot see a
//! fragment change shape, a fragment stop being emitted (an `Option::None`
//! arm), a resource id get renumbered, or a fragment stop being deterministic.
//!
//! Three independent assertions per fixture, none of which subsumes another:
//!
//! 1. **A golden** of every fragment's rendered symbolic form -- opcode stream
//!    with `SymVarRef { name, element_offset }` operands, literal pool,
//!    graphical functions, module declarations, static views, temp sizes and
//!    dim lists -- plus the model's `compute_layout` body layout and the VM's
//!    result table. Regenerate with `UPDATE_FRAGMENT_GOLDEN=1`, but only after
//!    a reviewer has adjudicated the change.
//! 2. **A declared phase map** ([`FixtureExpect::phases`]), a *required*
//!    argument listing every rendered (variable, phase) pair and whether it
//!    carries a fragment. This is the half a golden cannot provide: a golden
//!    that pins an artifact is structurally blind to that artifact being
//!    *stably absent* (the hard-won lesson recorded on
//!    `ltm_char_tests::FragmentExpectation`), and `UPDATE_FRAGMENT_GOLDEN=1`
//!    would happily re-capture a fixture whose fragments all vanished. The
//!    phase map is hand-written and never regenerated, so it reds.
//! 3. **Hand-computed VM spot checks** ([`FixtureExpect::spot_checks`]), also
//!    required and never regenerated. The rendered result table in the golden
//!    shows a reviewer *what* moved; the spot checks are what stays meaningful
//!    when a later commit legitimately changes bytecode shape.
//!
//! A fourth, universal assertion runs over every fragment of every fixture:
//! `temp_sizes` must be ordered by temp id. That is a determinism invariant,
//! not a formatting preference -- see `db::assemble::temp_sizes_by_id` -- and
//! `multi_temp_fragment_is_byte_identical_across_fresh_databases` sharpens it
//! from a coin flip into a reliable detector.
//!
//! The second half of the file measures INCREMENTALITY, which no golden can
//! see: which fragment-compiler bodies actually re-execute after an edit, using
//! the `#[cfg(test)]` execution records in `db::fragment_compile` rather than
//! memo pointer equality. Read `layout_only_edits_and_fragment_cache_reuse`'s
//! header comment for what that measurement found -- GH #964's "layout-only
//! project edits continue to reuse unchanged salsa-cached fragments" criterion
//! does not hold today, and these tests pin the baseline so the stage that
//! deletes the round trip can be measured against it.
//!
//! One property this suite establishes as a side effect is worth naming,
//! because stage 3 depends on it: the goldens are byte-identical under a
//! reordering of the mini-layout's dependency walk. That is direct evidence
//! that a fragment really is independent of the private offsets the round trip
//! hands out -- the premise the whole deletion rests on.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use super::*;
use crate::compiler::symbolic::{
    PerVarBytecodes, SymStaticViewBase, SymVarRef, SymbolicModuleDecl, SymbolicOpcode,
    SymbolicStaticView,
};
use crate::datamodel;
use crate::test_common::TestProject;

// ---------------------------------------------------------------------------
// Fixture declaration
// ---------------------------------------------------------------------------

/// A compilation phase of one variable. The three phases are exactly the three
/// `Option<PerVarBytecodes>` fields of `symbolic::CompiledVarFragment`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Phase {
    Initial,
    Flow,
    Stock,
}

impl Phase {
    const ALL: [Phase; 3] = [Phase::Initial, Phase::Flow, Phase::Stock];

    fn label(self) -> &'static str {
        match self {
            Phase::Initial => "initial",
            Phase::Flow => "flow",
            Phase::Stock => "stock",
        }
    }
}

/// Which emitter produced a fragment. Rendered into the golden so a variable
/// silently migrating between emitters (e.g. an explicit variable becoming an
/// implicit helper) is visible in the diff.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FragmentKind {
    Explicit,
    Implicit,
    Ltm,
    LtmImplicit,
}

impl FragmentKind {
    fn label(self) -> &'static str {
        match self {
            FragmentKind::Explicit => "explicit",
            FragmentKind::Implicit => "implicit-helper",
            FragmentKind::Ltm => "ltm-synthetic",
            FragmentKind::LtmImplicit => "ltm-implicit-helper",
        }
    }
}

/// One rendered variable: its key (`model::variable`), which emitter produced
/// it, and its per-phase fragments.
struct RenderedVar {
    key: String,
    kind: FragmentKind,
    initial: Option<PerVarBytecodes>,
    flow: Option<PerVarBytecodes>,
    stock: Option<PerVarBytecodes>,
}

impl RenderedVar {
    fn phase(&self, phase: Phase) -> Option<&PerVarBytecodes> {
        match phase {
            Phase::Initial => self.initial.as_ref(),
            Phase::Flow => self.flow.as_ref(),
            Phase::Stock => self.stock.as_ref(),
        }
    }

    /// The canonical `initial+flow+stock` spelling of the phases that carry a
    /// fragment, or `none`. This is the exact string a fixture declares.
    fn phase_spelling(&self) -> String {
        let present: Vec<&str> = Phase::ALL
            .iter()
            .filter(|p| self.phase(**p).is_some())
            .map(|p| p.label())
            .collect();
        if present.is_empty() {
            "none".to_string()
        } else {
            present.join("+")
        }
    }
}

/// Everything a fixture must state that the golden cannot state for it.
struct FixtureExpect {
    /// The models to render, each with the module-input set to compile its
    /// variables under. `&[]` is the input-agnostic set the diagnostic pass and
    /// the element-graph probe use; a sub-model additionally deserves its real
    /// instance input set, since that is a *different* salsa cache entry.
    models: &'static [(&'static str, &'static [&'static str])],
    /// EXHAUSTIVE `model::variable -> phases` map over every variable the
    /// fixture renders, where `phases` is `none` or a `+`-joined subset of
    /// `initial`, `flow`, `stock` in that order.
    ///
    /// Required, and never regenerated by `UPDATE_FRAGMENT_GOLDEN=1`. A
    /// variable that stops being emitted, a phase that stops compiling, and a
    /// variable that appears out of nowhere each fail here even if the golden
    /// was re-captured.
    phases: &'static [(&'static str, &'static str)],
    /// Why the absences in `phases` are the right absences. A missing fragment
    /// with no stated reason is indistinguishable from a bug that was captured
    /// into a golden.
    why: &'static str,
    /// Hand-computed `(step, saved variable, value)` VM checks. Required and
    /// non-empty: this is the runtime consequence, the assertion that survives
    /// a legitimate bytecode-shape change.
    spot_checks: &'static [(usize, &'static str, f64)],
    /// Whether -- and in which loop-enumeration mode -- to enable LTM, so the
    /// LTM synthetic vars and their implicit helpers are generated and
    /// rendered. The two modes emit DIFFERENT synthetic variables from
    /// different emitter arms, so both are worth a fixture.
    ltm: FixtureLtm,
    /// Assert the model resolves exactly one recurrence SCC and render its
    /// combined fragment (the `combine_scc_fragment` interleave that
    /// `assemble_module` injects) into the golden.
    expect_one_resolved_scc: bool,
}

impl FixtureExpect {
    /// The common case: one `main` model, no module inputs, no LTM, no SCC.
    const fn plain(
        phases: &'static [(&'static str, &'static str)],
        why: &'static str,
        spot_checks: &'static [(usize, &'static str, f64)],
    ) -> Self {
        FixtureExpect {
            models: &[("main", &[])],
            phases,
            why,
            spot_checks,
            ltm: FixtureLtm::Off,
            expect_one_resolved_scc: false,
        }
    }
}

/// How a fixture configures LTM.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FixtureLtm {
    Off,
    /// Johnson enumeration of every elementary circuit: emits one link score
    /// per loop edge plus a `$⁚ltm⁚loop_score⁚{id}` per circuit.
    Exhaustive,
    /// The strongest-path heuristic: emits a link score per causal edge
    /// (including edges no circuit traverses) and no loop-score variables.
    Discovery,
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Shortest round-trip `f64` spelling (`1.0`, `0.5`, `NaN`, `inf`). Every
/// fixture's arithmetic is exact IEEE-754 `+ - * /` on small values, so this is
/// stable across platforms; deliberately NOT a rounded format, which would hide
/// a real numeric change.
fn f(v: f64) -> String {
    format!("{v:?}")
}

fn render_var_ref(v: &SymVarRef) -> String {
    format!("{}@{}", v.name, v.element_offset)
}

/// The four implicit globals live at fixed absolute slots and never go through
/// a variable layout, so `LoadGlobalVar` keeps a raw offset. Name it so the
/// golden reads as a program.
fn global_var_name(off: u16) -> &'static str {
    match off as usize {
        crate::vm::TIME_OFF => "time",
        crate::vm::DT_OFF => "dt",
        crate::vm::INITIAL_TIME_OFF => "initial_time",
        crate::vm::FINAL_TIME_OFF => "final_time",
        _ => "<not-an-implicit-global>",
    }
}

/// Render one symbolic opcode as a single assembly-style line.
///
/// The match is deliberately EXHAUSTIVE with no `_` arm: a new
/// `SymbolicOpcode` variant must be given a rendering here before it can
/// appear in a golden, so no reference-bearing opcode family can silently
/// enter the goldens as unprintable noise.
///
/// Eleven `SymbolicOpcode` variants carry a `SymVarRef` -- the operands the
/// GH #964 round trip exists to recover -- and the fixtures reach nine of
/// them. The two they do not appear to be unreachable from the compiler, not
/// merely unexercised: `PushVarView` is never constructed by `compiler/codegen`
/// at all (only `PushVarViewDirect` is), and a plain `AssignNext` is always
/// consumed by `ByteCodeBuilder`'s `Op2 + AssignNext -> BinOpAssignNext`
/// peephole, because a stock update's last operation is always the `Op2 Add`
/// of `curr + net * dt`. Both are still handled by the VM, the symbolizer, the
/// resolver and the wasm backend, so they are dead weight rather than a gap in
/// this suite.
fn render_opcode(op: &SymbolicOpcode, literals: &[f64]) -> String {
    let lit = |id: u16| match literals.get(id as usize) {
        Some(v) => format!("#{id} (={})", f(*v)),
        None => format!("#{id} (=<out-of-pool>)"),
    };
    match op {
        SymbolicOpcode::Op2 { op } => format!("Op2 {op:?}"),
        SymbolicOpcode::Not {} => "Not".to_string(),
        SymbolicOpcode::LoadConstant { id } => format!("LoadConstant {}", lit(*id)),
        SymbolicOpcode::LoadVar { var } => format!("LoadVar {}", render_var_ref(var)),
        SymbolicOpcode::SymLoadPrev { var } => format!("LoadPrev {}", render_var_ref(var)),
        SymbolicOpcode::SymLoadInitial { var } => format!("LoadInitial {}", render_var_ref(var)),
        SymbolicOpcode::LoadGlobalVar { off } => {
            format!("LoadGlobalVar off={off} ({})", global_var_name(*off))
        }
        SymbolicOpcode::PushSubscriptIndex { bounds } => {
            format!("PushSubscriptIndex bounds={bounds}")
        }
        SymbolicOpcode::LoadSubscript { var } => format!("LoadSubscript {}", render_var_ref(var)),
        SymbolicOpcode::SetCond {} => "SetCond".to_string(),
        SymbolicOpcode::If {} => "If".to_string(),
        SymbolicOpcode::Ret => "Ret".to_string(),
        SymbolicOpcode::LoadModuleInput { input } => format!("LoadModuleInput input={input}"),
        SymbolicOpcode::EvalModule { id, n_inputs } => {
            format!("EvalModule module={id} n_inputs={n_inputs}")
        }
        SymbolicOpcode::AssignCurr { var } => format!("AssignCurr {}", render_var_ref(var)),
        SymbolicOpcode::AssignNext { var } => format!("AssignNext {}", render_var_ref(var)),
        SymbolicOpcode::Apply { func } => format!("Apply {func:?}"),
        SymbolicOpcode::Lookup {
            base_gf,
            table_count,
            mode,
        } => format!("Lookup base_gf={base_gf} table_count={table_count} mode={mode:?}"),
        SymbolicOpcode::AssignConstCurr { var, literal_id } => format!(
            "AssignConstCurr {} {}",
            render_var_ref(var),
            lit(*literal_id)
        ),
        SymbolicOpcode::BinOpAssignCurr { op, var } => {
            format!("BinOpAssignCurr {op:?} {}", render_var_ref(var))
        }
        SymbolicOpcode::BinOpAssignNext { op, var } => {
            format!("BinOpAssignNext {op:?} {}", render_var_ref(var))
        }
        SymbolicOpcode::PushVarView { var, dim_list_id } => {
            format!("PushVarView {} dim_list={dim_list_id}", render_var_ref(var))
        }
        SymbolicOpcode::PushTempView {
            temp_id,
            dim_list_id,
        } => format!("PushTempView temp={temp_id} dim_list={dim_list_id}"),
        SymbolicOpcode::PushStaticView { view_id } => format!("PushStaticView view={view_id}"),
        SymbolicOpcode::PushVarViewDirect { var, dim_list_id } => format!(
            "PushVarViewDirect {} dim_list={dim_list_id}",
            render_var_ref(var)
        ),
        SymbolicOpcode::ViewSubscriptConst { dim_idx, index } => {
            format!("ViewSubscriptConst dim={dim_idx} index={index}")
        }
        SymbolicOpcode::ViewSubscriptDynamic { dim_idx } => {
            format!("ViewSubscriptDynamic dim={dim_idx}")
        }
        SymbolicOpcode::ViewRange {
            dim_idx,
            start,
            end,
        } => format!("ViewRange dim={dim_idx} start={start} end={end}"),
        SymbolicOpcode::ViewRangeDynamic { dim_idx } => format!("ViewRangeDynamic dim={dim_idx}"),
        SymbolicOpcode::ViewStarRange {
            dim_idx,
            subdim_relation_id,
        } => format!("ViewStarRange dim={dim_idx} subdim_relation={subdim_relation_id}"),
        SymbolicOpcode::ViewWildcard { dim_idx } => format!("ViewWildcard dim={dim_idx}"),
        SymbolicOpcode::ViewTranspose {} => "ViewTranspose".to_string(),
        SymbolicOpcode::PopView {} => "PopView".to_string(),
        SymbolicOpcode::DupView {} => "DupView".to_string(),
        SymbolicOpcode::LoadTempConst { temp_id, index } => {
            format!("LoadTempConst temp={temp_id} index={index}")
        }
        SymbolicOpcode::LoadTempDynamic { temp_id } => format!("LoadTempDynamic temp={temp_id}"),
        SymbolicOpcode::BeginIter {
            write_temp_id,
            has_write_temp,
        } => format!("BeginIter write_temp={write_temp_id} has_write_temp={has_write_temp}"),
        SymbolicOpcode::LoadIterElement {} => "LoadIterElement".to_string(),
        SymbolicOpcode::LoadIterTempElement { temp_id } => {
            format!("LoadIterTempElement temp={temp_id}")
        }
        SymbolicOpcode::LoadIterViewTop {} => "LoadIterViewTop".to_string(),
        SymbolicOpcode::LoadIterViewAt { offset } => format!("LoadIterViewAt offset={offset}"),
        SymbolicOpcode::StoreIterElement {} => "StoreIterElement".to_string(),
        SymbolicOpcode::NextIterOrJump { jump_back } => {
            format!("NextIterOrJump jump_back={jump_back}")
        }
        SymbolicOpcode::EndIter {} => "EndIter".to_string(),
        SymbolicOpcode::ArraySum {} => "ArraySum".to_string(),
        SymbolicOpcode::ArrayMax {} => "ArrayMax".to_string(),
        SymbolicOpcode::ArrayMin {} => "ArrayMin".to_string(),
        SymbolicOpcode::ArrayMean {} => "ArrayMean".to_string(),
        SymbolicOpcode::ArrayStddev {} => "ArrayStddev".to_string(),
        SymbolicOpcode::ArraySize {} => "ArraySize".to_string(),
        SymbolicOpcode::VectorSelect {} => "VectorSelect".to_string(),
        SymbolicOpcode::VectorElmMap {
            write_temp_id,
            full_source_len,
        } => format!("VectorElmMap write_temp={write_temp_id} full_source_len={full_source_len}"),
        SymbolicOpcode::VectorSortOrder { write_temp_id } => {
            format!("VectorSortOrder write_temp={write_temp_id}")
        }
        SymbolicOpcode::Rank { write_temp_id } => format!("Rank write_temp={write_temp_id}"),
        SymbolicOpcode::LookupArray {
            base_gf,
            table_count,
            mode,
            write_temp_id,
        } => format!(
            "LookupArray base_gf={base_gf} table_count={table_count} mode={mode:?} \
             write_temp={write_temp_id}"
        ),
        SymbolicOpcode::AllocateAvailable { write_temp_id } => {
            format!("AllocateAvailable write_temp={write_temp_id}")
        }
        SymbolicOpcode::AllocateByPriority { write_temp_id } => {
            format!("AllocateByPriority write_temp={write_temp_id}")
        }
        SymbolicOpcode::BeginBroadcastIter {
            n_sources,
            dest_temp_id,
        } => format!("BeginBroadcastIter n_sources={n_sources} dest_temp={dest_temp_id}"),
        SymbolicOpcode::LoadBroadcastElement { source_idx } => {
            format!("LoadBroadcastElement source={source_idx}")
        }
        SymbolicOpcode::StoreBroadcastElement {} => "StoreBroadcastElement".to_string(),
        SymbolicOpcode::NextBroadcastOrJump { jump_back } => {
            format!("NextBroadcastOrJump jump_back={jump_back}")
        }
        SymbolicOpcode::EndBroadcastIter {} => "EndBroadcastIter".to_string(),
    }
}

fn render_static_view(idx: usize, sv: &SymbolicStaticView) -> String {
    let base = match &sv.base {
        SymStaticViewBase::Var(v) => render_var_ref(v),
        SymStaticViewBase::Temp(id) => format!("temp{id}"),
    };
    let sparse: Vec<String> = sv
        .sparse
        .iter()
        .map(|m| format!("dim{}->{:?}", m.dim_index, m.parent_offsets.as_slice()))
        .collect();
    format!(
        "      view[{idx}]: base={base} dims={:?} strides={:?} offset={} dim_ids={:?} sparse=[{}]",
        sv.dims.as_slice(),
        sv.strides.as_slice(),
        sv.offset,
        sv.dim_ids.as_slice(),
        sparse.join(", ")
    )
}

fn render_module_decl(idx: usize, md: &SymbolicModuleDecl) -> String {
    let inputs: Vec<&str> = md.input_set.iter().map(|i| i.as_str()).collect();
    format!(
        "      decl[{idx}]: model={} inputs=[{}] var={}",
        md.model_name.as_str(),
        inputs.join(", "),
        render_var_ref(&md.var)
    )
}

/// Render one phase's `PerVarBytecodes` in full. Every field of the struct is
/// covered; a field added to `PerVarBytecodes` without a rendering here would
/// be invisible to the goldens, so keep this in sync with the struct.
fn render_fragment(bc: &PerVarBytecodes) -> String {
    let mut out = String::new();
    let literals: Vec<String> = bc.symbolic.literals.iter().map(|v| f(*v)).collect();
    out.push_str(&format!("    literals: [{}]\n", literals.join(", ")));
    let temps: Vec<String> = bc
        .temp_sizes
        .iter()
        .map(|(id, size)| format!("temp{id}:{size}"))
        .collect();
    out.push_str(&format!("    temp_sizes: [{}]\n", temps.join(", ")));
    let dim_lists: Vec<String> = bc.dim_lists.iter().map(|dl| format!("{dl:?}")).collect();
    out.push_str(&format!("    dim_lists: [{}]\n", dim_lists.join(", ")));
    if bc.graphical_functions.is_empty() {
        out.push_str("    graphical_functions: []\n");
    } else {
        out.push_str("    graphical_functions:\n");
        for (i, gf) in bc.graphical_functions.iter().enumerate() {
            let pts: Vec<String> = gf
                .iter()
                .map(|(x, y)| format!("({}, {})", f(*x), f(*y)))
                .collect();
            out.push_str(&format!("      gf[{i}]: {}\n", pts.join(" ")));
        }
    }
    if bc.module_decls.is_empty() {
        out.push_str("    module_decls: []\n");
    } else {
        out.push_str("    module_decls:\n");
        for (i, md) in bc.module_decls.iter().enumerate() {
            out.push_str(&render_module_decl(i, md));
            out.push('\n');
        }
    }
    if bc.static_views.is_empty() {
        out.push_str("    static_views: []\n");
    } else {
        out.push_str("    static_views:\n");
        for (i, sv) in bc.static_views.iter().enumerate() {
            out.push_str(&render_static_view(i, sv));
            out.push('\n');
        }
    }
    out.push_str("    code:\n");
    for (pc, op) in bc.symbolic.code.iter().enumerate() {
        out.push_str(&format!(
            "      {pc:04}  {}\n",
            render_opcode(op, &bc.symbolic.literals)
        ));
    }
    out
}

// ---------------------------------------------------------------------------
// Fragment collection (mirroring `assemble_module`'s enumeration)
// ---------------------------------------------------------------------------

/// Collect every fragment `assemble_module` would compile for `model_name`
/// under `module_input_names`, in the same four passes assembly runs: explicit
/// source variables, implicit (SMOOTH/DELAY/TREND/PREVIOUS/INIT) helpers, and
/// -- when LTM is on -- LTM synthetic variables and their implicit helpers.
///
/// Output is sorted by key so the golden is order-stable regardless of the
/// `HashMap` iteration order of any of the underlying maps.
///
/// One deliberate divergence from assembly: `assemble_module` additionally
/// drops an LTM fragment whose symbolic references do not resolve in the
/// model's layout (`fragment_vars_in_layout`, the sub-model stdlib-instance
/// case). That filter is a property of the layout, not of the fragment
/// compiler, and it never fires on these single-model fixtures, so rendering
/// pre-filter keeps the goldens a pin on what the EMITTERS produce.
fn collect_model_fragments(
    db: &SimlinDb,
    project: SourceProject,
    model_name: &str,
    module_input_names: &[&str],
) -> Vec<RenderedVar> {
    let model = *project
        .models(db)
        .get(model_name)
        .unwrap_or_else(|| panic!("fixture declares model `{model_name}`, which does not exist"));
    let owned_inputs: Vec<String> = module_input_names.iter().map(|s| s.to_string()).collect();
    let inputs = ModuleInputSet::from_names(db, &owned_inputs);
    let dep_graph = model_dependency_graph(db, model, project, inputs);

    let mut out: Vec<RenderedVar> = Vec::new();
    let push = |out: &mut Vec<RenderedVar>, kind: FragmentKind, result: &VarFragmentResult| {
        out.push(RenderedVar {
            key: format!("{model_name}::{}", result.fragment.ident),
            kind,
            initial: result.fragment.initial_bytecodes.clone(),
            flow: result.fragment.flow_bytecodes.clone(),
            stock: result.fragment.stock_bytecodes.clone(),
        });
    };

    let source_vars = model.variables(db);
    let mut explicit_names: Vec<&String> = source_vars.keys().collect();
    explicit_names.sort();
    for name in explicit_names {
        if let Some(result) = compile_var_fragment(db, source_vars[name], model, project, inputs) {
            push(&mut out, FragmentKind::Explicit, result);
        }
    }

    let implicit_info = model_implicit_var_info(db, model, project);
    let mut implicit_names: Vec<&String> = implicit_info.keys().collect();
    implicit_names.sort();
    for name in implicit_names {
        if let Some(result) = compile_implicit_var_fragment(
            db,
            &implicit_info[name],
            model,
            project,
            dep_graph,
            &owned_inputs,
        ) {
            push(&mut out, FragmentKind::Implicit, &result);
        }
    }

    if project.ltm_enabled(db) {
        let ltm_vars = model_ltm_variables(db, model, project);
        let mut sorted: Vec<&LtmSyntheticVar> = ltm_vars.vars.iter().collect();
        sorted.sort_by(|a, b| a.name.cmp(&b.name));
        for ltm_var in sorted {
            if let Some(result) = compile_ltm_synthetic_fragment(db, ltm_var, model, project) {
                push(&mut out, FragmentKind::Ltm, &result);
            }
        }
        let ltm_implicit = model_ltm_implicit_var_info(db, model, project);
        let mut ltm_implicit_names: Vec<&String> = ltm_implicit.keys().collect();
        ltm_implicit_names.sort();
        for name in ltm_implicit_names {
            if let Some(result) = compile_ltm_implicit_var_fragment(
                db,
                &ltm_implicit[name],
                model,
                project,
                dep_graph,
                &owned_inputs,
            ) {
                push(&mut out, FragmentKind::LtmImplicit, &result);
            }
        }
    }

    out.sort_by(|a, b| a.key.cmp(&b.key));
    out
}

/// `compute_layout`'s body layout for `model_name`, rendered as a compact
/// `name: [offset, offset+size)` table.
///
/// Fragments are layout-INDEPENDENT by construction, so this cannot change
/// when a fragment does -- which is precisely why it is worth pinning
/// separately: a layout renumbering is a distinct failure class from a
/// fragment shape change, and only this section can see it.
fn render_layout(db: &SimlinDb, project: SourceProject, model_name: &str) -> String {
    let model = *project.models(db).get(model_name).unwrap();
    let layout = compute_layout(db, model, project);
    let mut names: Vec<String> = model.variables(db).keys().cloned().collect();
    names.extend(model_implicit_var_info(db, model, project).keys().cloned());
    if project.ltm_enabled(db) {
        names.extend(
            model_ltm_variables(db, model, project)
                .vars
                .iter()
                .map(|v| canonicalize(&v.name).into_owned()),
        );
        names.extend(
            model_ltm_implicit_var_info(db, model, project)
                .keys()
                .cloned(),
        );
    }
    names.sort();
    names.dedup();

    let mut out = format!("  n_slots: {}\n", layout.n_slots);
    for name in names {
        match layout.get(&name) {
            Some(entry) => out.push_str(&format!(
                "  {name}: [{}, {})\n",
                entry.offset,
                entry.offset + entry.size
            )),
            None => out.push_str(&format!("  {name}: <no layout slot>\n")),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Runtime consequence
// ---------------------------------------------------------------------------

/// Every saved variable's value at each of `steps`, sorted by name.
fn run_and_sample(
    db: &SimlinDb,
    project: SourceProject,
    steps: &BTreeSet<usize>,
) -> BTreeMap<usize, BTreeMap<String, f64>> {
    let compiled = compile_project_incremental(db, project, "main")
        .expect("fixture must compile through the production incremental path");
    let mut vm = crate::vm::Vm::new(compiled).expect("fixture must build a VM");
    vm.run_to_end().expect("fixture must run to completion");
    let results = vm.into_results();

    let mut sampled: BTreeMap<usize, BTreeMap<String, f64>> = BTreeMap::new();
    for &step in steps {
        assert!(
            step < results.step_count,
            "fixture asks for step {step} but the run saved only {} steps",
            results.step_count
        );
        let row = &mut sampled.entry(step).or_default();
        for (name, &off) in results.offsets.iter() {
            row.insert(
                name.to_string(),
                results.data[step * results.step_size + off],
            );
        }
    }
    sampled
}

fn render_runtime(sampled: &BTreeMap<usize, BTreeMap<String, f64>>) -> String {
    let mut out = String::new();
    for (step, row) in sampled {
        out.push_str(&format!("  step {step}:\n"));
        for (name, value) in row {
            out.push_str(&format!("    {name} = {}\n", f(*value)));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// The characterization entry point
// ---------------------------------------------------------------------------

/// Compare `actual` against `fragment_char_golden/{name}.txt`. Set
/// `UPDATE_FRAGMENT_GOLDEN=1` to (re)capture -- but only after a reviewer has
/// adjudicated the change; a re-captured golden that hides a vanished fragment
/// still fails on [`FixtureExpect::phases`]. A missing golden fails loudly.
#[track_caller]
fn assert_golden(name: &str, actual: &str) {
    let dir = format!("{}/src/db/fragment_char_golden", env!("CARGO_MANIFEST_DIR"));
    let path = format!("{dir}/{name}.txt");
    if std::env::var("UPDATE_FRAGMENT_GOLDEN").is_ok() {
        std::fs::create_dir_all(&dir).expect("create golden dir");
        std::fs::write(&path, actual).expect("write golden");
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!("missing golden {path}: {e}; run once with UPDATE_FRAGMENT_GOLDEN=1 to capture")
    });
    if actual != expected {
        eprintln!("\n===== GOLDEN MISMATCH ({name}): actual below =====");
        eprintln!("{actual}");
        eprintln!("===== end actual (expected in {path}) =====\n");
    }
    assert_eq!(actual, &expected, "golden mismatch for {name}");
}

/// Build the fixture's db exactly as production does, plus the LTM flags when
/// the fixture asks for them.
fn fixture_db(project: &datamodel::Project, ltm: FixtureLtm) -> (SimlinDb, SourceProject) {
    use salsa::Setter;
    let mut db = SimlinDb::default();
    let source_project = sync_from_datamodel(&db, project).project;
    match ltm {
        FixtureLtm::Off => {}
        FixtureLtm::Exhaustive => {
            source_project.set_ltm_enabled(&mut db).to(true);
        }
        FixtureLtm::Discovery => {
            source_project.set_ltm_discovery_mode(&mut db).to(true);
            source_project.set_ltm_enabled(&mut db).to(true);
        }
    }
    (db, source_project)
}

/// `temp_sizes` must be strictly increasing in temp id.
///
/// This is a determinism invariant, not a style rule: `PerVarBytecodes` is a
/// salsa-cached value with a derived `PartialEq` and `Vec` equality is
/// order-sensitive, so a `HashMap`-ordered `temp_sizes` makes two identical
/// compiles of the same fragment compare unequal whenever the per-process hash
/// seed differs -- defeating backdating and making the artifact irreproducible
/// (GH #595's class). Asserted on every fragment of every fixture rather than
/// in one place, because the ordering is established at THREE separate emission
/// sites (`db::assemble` and both copies in `db::ltm::compile`).
#[track_caller]
fn assert_temp_sizes_ordered(key: &str, phase: Phase, bc: &PerVarBytecodes) {
    let ids: Vec<u32> = bc.temp_sizes.iter().map(|(id, _)| *id).collect();
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    assert_eq!(
        ids,
        sorted,
        "{key} ({}): temp_sizes must be ordered by temp id -- an unordered \
         (HashMap-iteration-order) vector makes this salsa-cached fragment \
         compare unequal to an identical recompile, defeating backdating and \
         making the compiled artifact run-to-run nondeterministic",
        phase.label()
    );
}

#[track_caller]
fn assert_fragment_fixture(golden: &str, project: datamodel::Project, expect: FixtureExpect) {
    assert!(
        !expect.spot_checks.is_empty(),
        "fixture `{golden}` must declare at least one hand-computed VM spot \
         check: a fragment golden alone cannot tell a shape change from a \
         behavior change"
    );

    let (db, source_project) = fixture_db(&project, expect.ltm);

    let mut rendered = String::new();
    let mut actual_phases: Vec<(String, String)> = Vec::new();

    for (model_name, module_inputs) in expect.models {
        rendered.push_str(&format!(
            "########## model {model_name} (module inputs: [{}]) ##########\n",
            module_inputs.join(", ")
        ));
        rendered.push_str("== layout ==\n");
        rendered.push_str(&render_layout(&db, source_project, model_name));

        for var in collect_model_fragments(&db, source_project, model_name, module_inputs) {
            rendered.push_str(&format!(
                "== {} [{}] : {} ==\n",
                var.key,
                var.kind.label(),
                var.phase_spelling()
            ));
            for phase in Phase::ALL {
                match var.phase(phase) {
                    None => rendered.push_str(&format!("  {}: <no fragment>\n", phase.label())),
                    Some(bc) => {
                        assert_temp_sizes_ordered(&var.key, phase, bc);
                        rendered.push_str(&format!("  {}:\n", phase.label()));
                        rendered.push_str(&render_fragment(bc));
                    }
                }
            }
            actual_phases.push((var.key.clone(), var.phase_spelling()));
        }
    }

    if expect.expect_one_resolved_scc {
        rendered.push_str(&render_resolved_scc(&db, source_project));
    }

    // The declared phase map: exhaustive, hand-written, never regenerated.
    //
    // Checked BEFORE the model is run, deliberately. A dropped phase usually
    // also makes the model refuse to compile (`NotSimulatable`) or run to a
    // different answer, and if the run went first, every such regression would
    // be reported as a downstream failure naming no phase. Reporting it here
    // names the exact (variable, phase) pair that moved.
    let expected_phases: Vec<(String, String)> = expect
        .phases
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    assert_eq!(
        actual_phases, expected_phases,
        "fixture `{golden}`'s (variable -> compiled phases) map does not match \
         its declaration.\n  declared reason for the absences: {}\n  A phase \
         that became `<no fragment>` is a silently-dropped fragment (the \
         variable keeps its layout slot and reads a constant 0). A phase that \
         appeared, or a variable that appeared/vanished, means the emitter's \
         gating changed. Neither can be fixed by re-running with \
         UPDATE_FRAGMENT_GOLDEN=1.",
        expect.why
    );

    let steps: BTreeSet<usize> = expect.spot_checks.iter().map(|(s, _, _)| *s).collect();
    let sampled = run_and_sample(&db, source_project, &steps);
    rendered.push_str("########## runtime ##########\n");
    rendered.push_str(&render_runtime(&sampled));

    assert_golden(golden, &rendered);

    for (step, name, expected) in expect.spot_checks {
        let row = sampled
            .get(step)
            .unwrap_or_else(|| panic!("fixture `{golden}`: no sampled row for step {step}"));
        let actual = *row.get(*name).unwrap_or_else(|| {
            panic!(
                "fixture `{golden}`: spot check names `{name}`, which is not a saved \
                 variable. Saved: {:?}",
                row.keys().collect::<Vec<_>>()
            )
        });
        assert!(
            (actual - expected).abs() < 1e-9,
            "fixture `{golden}`: step {step} `{name}` = {actual}, hand-computed {expected}"
        );
    }
}

/// Assert the fixture resolves exactly one recurrence SCC and render its
/// combined fragment -- built through the EXACT production path
/// `assemble_module` uses (`var_phase_symbolic_fragment_prod` per member ->
/// `combine_scc_fragment`), so the golden pins the bytecode that is actually
/// injected into the runlist rather than a re-derivation.
fn render_resolved_scc(db: &SimlinDb, project: SourceProject) -> String {
    let model = *project.models(db).get("main").unwrap();
    let dep_graph = model_dependency_graph(db, model, project, ModuleInputSet::empty(db));
    assert!(
        !dep_graph.has_cycle,
        "the SCC fixture's element-acyclic recurrence must survive the cycle gate"
    );
    assert_eq!(
        dep_graph.resolved_sccs.len(),
        1,
        "the SCC fixture must resolve exactly one recurrence SCC"
    );
    let scc = &dep_graph.resolved_sccs[0];

    let mut member_fragments: HashMap<Ident<Canonical>, PerVarBytecodes> = HashMap::new();
    for member in &scc.members {
        let frag = var_phase_symbolic_fragment_prod(
            db,
            model,
            project,
            member.as_str(),
            scc.phase.clone(),
        )
        .unwrap_or_else(|| {
            panic!(
                "SCC member `{}` must be element-sourceable",
                member.as_str()
            )
        });
        member_fragments.insert(member.clone(), frag);
    }
    let combined = combine_scc_fragment(scc, &member_fragments)
        .expect("the resolved SCC must combine into one fragment");
    assert_temp_sizes_ordered("<combined scc>", Phase::Flow, &combined);

    let members: Vec<&str> = scc.members.iter().map(|m| m.as_str()).collect();
    let order: Vec<String> = scc
        .element_order
        .iter()
        .map(|(name, elem)| format!("{}@{elem}", name.as_str()))
        .collect();
    let mut out = String::from("########## resolved recurrence SCC ##########\n");
    out.push_str(&format!("  phase: {:?}\n", scc.phase));
    out.push_str(&format!("  members: [{}]\n", members.join(", ")));
    out.push_str(&format!("  element_order: [{}]\n", order.join(", ")));
    out.push_str("  combined fragment:\n");
    out.push_str(&render_fragment(&combined));
    out
}

// ---------------------------------------------------------------------------
// Fixture 1: scalar aux chain + a stock with an inflow and an outflow.
//
// The baseline shape: a constant, two flows, a stock, and a downstream aux.
// It is what pins the three-phase gating itself -- a stock compiles `initial`
// and `stock` but never `flow`, a non-stock compiles `flow` and (only when a
// stock's initial reaches it) `initial`.
// ---------------------------------------------------------------------------

fn scalar_chain_model() -> datamodel::Project {
    TestProject::new("frag_scalar_chain")
        .with_sim_time(0.0, 2.0, 1.0)
        .aux("k", "2", None)
        .flow("inflow", "k * 3", None)
        .flow("outflow", "level * 0.1", None)
        .stock("level", "10", &["inflow"], &["outflow"], None)
        .aux("derived", "inflow + 1", None)
        .build_datamodel()
}

#[test]
fn char_scalar_chain() {
    assert_fragment_fixture(
        "scalar_chain",
        scalar_chain_model(),
        FixtureExpect::plain(
            &[
                ("main::derived", "flow"),
                ("main::inflow", "flow"),
                ("main::k", "flow"),
                ("main::level", "initial+stock"),
                ("main::outflow", "flow"),
            ],
            "A stock is excluded from the flows runlist (`!is_stock || \
             is_module_input`), so `level` carries `initial` (its `10` init) and \
             `stock` (its dt update) but no flow phase. No non-stock variable is \
             reachable from a stock's INITIAL equation here -- `level`'s init is \
             the literal `10` -- so none of them carries an initial phase.",
            &[
                // k = 2, inflow = 6, outflow = 0.1 * level, derived = 7.
                // t=0: level = 10 -> outflow 1.0, net +5
                // t=1: level = 15 -> outflow 1.5, net +4.5
                // t=2: level = 19.5
                (0, "level", 10.0),
                (0, "outflow", 1.0),
                (0, "derived", 7.0),
                (1, "level", 15.0),
                (1, "outflow", 1.5),
                (2, "level", 19.5),
            ],
        ),
    );
}

// ---------------------------------------------------------------------------
// Fixture 2: the three arrayed equation shapes.
//
//   * `base[region]`     -- `Equation::ApplyToAll` (one equation, all elements)
//   * `perelem[region]`  -- `Equation::Arrayed` (one equation PER element)
//   * `exceptdef[region]` -- `Equation::Arrayed` with an EXCEPT default
//
// These three lower through visibly different paths (A2A iteration, per-element
// unrolling, default application), so the fragment shapes must stay distinct.
// ---------------------------------------------------------------------------

fn arrayed_shapes_model() -> datamodel::Project {
    TestProject::new("frag_arrayed_shapes")
        .with_sim_time(0.0, 1.0, 1.0)
        .named_dimension("region", &["east", "west"])
        .array_aux("base[region]", "10")
        .array_with_ranges(
            "perelem[region]",
            vec![("east", "base[east] * 2"), ("west", "base[west] + 5")],
        )
        .array_with_default_and_overrides("exceptdef[region]", "1", vec![("east", "7")])
        .build_datamodel()
}

#[test]
fn char_arrayed_shapes() {
    assert_fragment_fixture(
        "arrayed_shapes",
        arrayed_shapes_model(),
        FixtureExpect::plain(
            &[
                ("main::base", "flow"),
                ("main::exceptdef", "flow"),
                ("main::perelem", "flow"),
            ],
            "Three auxes, no stocks: nothing is in the initials or stocks \
             runlist, so every variable carries the flow phase alone.",
            &[
                (0, "base[east]", 10.0),
                (0, "base[west]", 10.0),
                // per-element equations: east = 10 * 2, west = 10 + 5
                (0, "perelem[east]", 20.0),
                (0, "perelem[west]", 15.0),
                // EXCEPT default: `east` is overridden to 7, `west` takes the
                // default `1`.
                (0, "exceptdef[east]", 7.0),
                (0, "exceptdef[west]", 1.0),
            ],
        ),
    );
}

// ---------------------------------------------------------------------------
// Fixture 3: static vs dynamic subscripts.
//
// `stat = arr[west]` resolves its index at compile time (a `StaticSubscript` /
// constant view subscript); `dyn = arr[idx]` cannot, because `idx` is a
// variable, so it must emit a runtime index. Both reference `arr` through the
// mini-layout, so both are reference-bearing opcodes the round trip must
// round-trip.
// ---------------------------------------------------------------------------

fn subscript_model() -> datamodel::Project {
    TestProject::new("frag_subscripts")
        .with_sim_time(0.0, 1.0, 1.0)
        .named_dimension("region", &["east", "west", "north"])
        .array_with_ranges(
            "arr[region]",
            vec![("east", "1"), ("west", "2"), ("north", "3")],
        )
        // `idx` is a variable, so `arr[idx]` is a genuine runtime subscript.
        // Driving it off `time` also keeps it from being folded to a constant.
        .scalar_aux("idx", "1 + time")
        .scalar_aux("stat", "arr[west]")
        .scalar_aux("dynamic", "arr[idx]")
        .build_datamodel()
}

#[test]
fn char_subscripts() {
    assert_fragment_fixture(
        "subscripts",
        subscript_model(),
        FixtureExpect::plain(
            &[
                ("main::arr", "flow"),
                ("main::dynamic", "flow"),
                ("main::idx", "flow"),
                ("main::stat", "flow"),
            ],
            "Four auxes, no stocks: flow phase only.",
            &[
                (0, "stat", 2.0),
                // idx = 1 + time; subscripts are 1-based, so t=0 selects
                // `east` (1) and t=1 selects `west` (2).
                (0, "idx", 1.0),
                (0, "dynamic", 1.0),
                (1, "idx", 2.0),
                (1, "dynamic", 2.0),
            ],
        ),
    );
}

// ---------------------------------------------------------------------------
// Fixture 3b: a RUNTIME array view, and an arrayed stock.
//
// The two reference-bearing shapes fixture 3 cannot reach. Everywhere else in
// this suite an array reference resolves at compile time into the static-view
// table (`PushStaticView`), which carries its base as a `SymbolicStaticView`
// rather than as an opcode operand. `VECTOR ELM MAP(arr[idx], ..)` cannot:
// `idx` is a variable, so the base view is pushed at RUNTIME by
// `PushVarViewDirect`, the only view opcode that carries a `SymVarRef`
// operand, and narrowed by `ViewSubscriptDynamic`. The arrayed stock adds
// per-element stock writes.
//
// (`SUM(mat[idx, *])` -- a dynamic index beside a wildcard -- would be the
// obvious way to reach the same opcodes and is NOT usable: the engine rejects
// it with `ArrayReferenceNeedsExplicitSubscripts`, loudly, before compiling.)
//
// Building this fixture surfaced a real bug, fixed in the same change:
// `codegen::full_source_len` had no arm for a DYNAMIC `Expr::Subscript`, so it
// reported a full source extent of 1 and every ELM MAP read outside `[0, 1)`
// returned the out-of-range `:NA:` (NaN) -- which was every read whenever
// `idx` selected anything but the source's first element. `mapped` was all-NaN
// at t=1 here. The golden's `full_source_len=3` operand and the `mapped` rows
// at both steps are the pin; the focused unit test is
// `array_tests::...::elm_map_dynamic_source_subscript_uses_full_variable_extent_vm`.
// The offsets are all zero only to keep this fixture about the VIEW opcodes.
// ---------------------------------------------------------------------------

fn dynamic_view_model() -> datamodel::Project {
    TestProject::new("frag_dynamic_view")
        .with_sim_time(0.0, 1.0, 1.0)
        .named_dimension("region", &["east", "west", "north"])
        .array_with_ranges(
            "arr[region]",
            vec![("east", "1"), ("west", "2"), ("north", "3")],
        )
        // All-zero offsets: see the note above the fixture on why a non-zero
        // offset cannot be used here.
        .array_aux("off1[region]", "0")
        .scalar_aux("idx", "1 + time")
        .array_aux("mapped[region]", "VECTOR ELM MAP(arr[idx], off1[region])")
        .array_stock("acc[region]", "0", &["grow"], &[], None)
        .array_flow("grow[region]", "arr[region]", None)
        .build_datamodel()
}

#[test]
fn char_dynamic_view_and_arrayed_stock() {
    assert_fragment_fixture(
        "dynamic_view",
        dynamic_view_model(),
        FixtureExpect::plain(
            &[
                ("main::acc", "initial+stock"),
                ("main::arr", "flow"),
                ("main::grow", "flow"),
                ("main::idx", "flow"),
                ("main::mapped", "flow"),
                ("main::off1", "flow"),
            ],
            "`acc` is a stock: initial + stock, never flow. Everything else is \
             an aux or a flow, so flow only; no INIT reaches any of them, since \
             `acc`'s initial is the literal `0`.",
            &[
                // VECTOR ELM MAP reads the source at `base + off1[i]`, where
                // `base` is the flat position of `arr[idx]` and `idx = 1 + time`
                // is 1-based. With all-zero offsets every element reads the base:
                // arr = [1, 2, 3], so t=0 selects arr[east] and t=1 arr[west].
                (0, "mapped[east]", 1.0),
                (0, "mapped[west]", 1.0),
                (0, "mapped[north]", 1.0),
                (1, "mapped[east]", 2.0),
                (1, "mapped[north]", 2.0),
                // The arrayed stock accumulates its arrayed inflow per element.
                (0, "acc[north]", 0.0),
                (1, "acc[east]", 1.0),
                (1, "acc[west]", 2.0),
                (1, "acc[north]", 3.0),
            ],
        ),
    );
}

// ---------------------------------------------------------------------------
// Fixture 4: an array reducer plus two array-PRODUCING vector builtins.
//
// `total = SUM(arr[*])` is the reducer family (view push, reduce opcode, view
// pop). `order` and `ranked` are array-producing builtins, which the A2A
// hoisting path pre-computes once into an `AssignTemp` and then reads back
// per element -- so this fixture is what pins `temp_sizes`, the temp-carrying
// opcodes, and (via the universal ordering assertion) their determinism.
// ---------------------------------------------------------------------------

fn array_ops_model() -> datamodel::Project {
    TestProject::new("frag_array_ops")
        .with_sim_time(0.0, 1.0, 1.0)
        .named_dimension("region", &["east", "west", "north"])
        .array_with_ranges(
            "arr[region]",
            vec![("east", "30"), ("west", "10"), ("north", "20")],
        )
        .scalar_aux("total", "SUM(arr[*])")
        .array_aux("order[region]", "VECTOR SORT ORDER(arr[region], 1)")
        .array_aux("ranked[region]", "RANK(arr[region], 1)")
        // `sorted` routes an array-producing builtin's OUTPUT back through
        // another one: ELM MAP reads `arr` at the offsets `order` computed.
        .array_aux("sorted[region]", "VECTOR ELM MAP(arr[east], order[region])")
        // Two array-producing builtins in ONE equation: the only shape that
        // gives a single fragment more than one temp, and therefore the only
        // one where `temp_sizes`' ordering is observable at all.
        .array_aux(
            "combo[region]",
            "VECTOR SORT ORDER(arr[region], 1) + RANK(arr[region], 1)",
        )
        .build_datamodel()
}

#[test]
fn char_array_ops() {
    assert_fragment_fixture(
        "array_ops",
        array_ops_model(),
        FixtureExpect::plain(
            &[
                ("main::arr", "flow"),
                ("main::combo", "flow"),
                ("main::order", "flow"),
                ("main::ranked", "flow"),
                ("main::sorted", "flow"),
                ("main::total", "flow"),
            ],
            "Six auxes, no stocks: flow phase only.",
            &[
                (0, "total", 60.0),
                // VECTOR SORT ORDER ascending is the genuine-Vensim 0-BASED
                // permutation: position i holds the source index of the i-th
                // smallest element. arr = [30, 10, 20] -> [1, 2, 0].
                (0, "order[east]", 1.0),
                (0, "order[west]", 2.0),
                (0, "order[north]", 0.0),
                // RANK is 1-BASED ordinal position: 30 is 3rd, 10 is 1st,
                // 20 is 2nd.
                (0, "ranked[east]", 3.0),
                (0, "ranked[west]", 1.0),
                (0, "ranked[north]", 2.0),
                // VECTOR ELM MAP: result[i] = arr[base + order[i]] with base
                // at `arr[east]` (flat 0), so `sorted` is `arr` ascending.
                (0, "sorted[east]", 10.0),
                (0, "sorted[west]", 20.0),
                (0, "sorted[north]", 30.0),
                // combo = order + ranked, element-wise.
                (0, "combo[east]", 4.0),
                (0, "combo[west]", 3.0),
                (0, "combo[north]", 2.0),
            ],
        ),
    );
}

// ---------------------------------------------------------------------------
// Fixture 5: graphical functions, scalar and per-element arrayed.
//
//   * `curve` is the WITH LOOKUP shape -- a value-bearing aux whose own gf is
//     applied to its input equation, lowering to a scalar `Lookup`.
//   * `g[region]` carries one gf PER ELEMENT; `out` calls each element's own
//     table (scalar `Lookup` at a per-element `base_gf`), and `gtotal` reduces
//     over the whole arrayed-GF result, which routes through the array-
//     producing `LookupArray` opcode into a temp (GH #580).
//
// `graphical_functions` is a fragment-local resource that assembly de-duplicates
// and renumbers (`GfDedup`, #582), so every fragment's gf block is pinned here.
// ---------------------------------------------------------------------------

/// A two-point continuous gf over x in [0, 1] whose y-values are
/// `(base, base + slope)`. Evaluated at `time` with `time` running 0..=1 at
/// dt = 1 it yields exactly `base` then `base + slope`.
fn ramp_gf(base: f64, slope: f64) -> datamodel::GraphicalFunction {
    datamodel::GraphicalFunction {
        kind: datamodel::GraphicalFunctionKind::Continuous,
        x_points: Some(vec![0.0, 1.0]),
        y_points: vec![base, base + slope],
        x_scale: datamodel::GraphicalFunctionScale { min: 0.0, max: 1.0 },
        y_scale: datamodel::GraphicalFunctionScale {
            min: base.min(base + slope),
            max: base.max(base + slope),
        },
    }
}

/// A per-element arrayed-GF holder: one `(element, "time", None, gf)` slot per
/// element, so each element's value is its OWN table evaluated at `time`.
fn arrayed_gf_holder(
    ident: &str,
    dim: &str,
    elems: Vec<(&str, datamodel::GraphicalFunction)>,
) -> datamodel::Variable {
    datamodel::Variable::Aux(datamodel::Aux {
        ident: ident.to_string(),
        equation: datamodel::Equation::Arrayed(
            vec![dim.to_string()],
            elems
                .into_iter()
                .map(|(name, gf)| (name.to_string(), "time".to_string(), None, Some(gf)))
                .collect(),
            None,
            false,
        ),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    })
}

fn graphical_function_model() -> datamodel::Project {
    let mut tp = TestProject::new("frag_gf")
        .with_sim_time(0.0, 1.0, 1.0)
        .named_dimension("region", &["east", "west"])
        .scalar_aux("drive", "time")
        .aux_with_gf("curve", "drive", ramp_gf(5.0, 10.0));
    tp.variables.push(arrayed_gf_holder(
        "g",
        "region",
        vec![("east", ramp_gf(1.0, 1.0)), ("west", ramp_gf(2.0, 2.0))],
    ));
    tp.array_aux("out[region]", "LOOKUP(g[region], time)")
        .scalar_aux("gtotal", "SUM(LOOKUP(g[*], time))")
        .build_datamodel()
}

#[test]
fn char_graphical_functions() {
    assert_fragment_fixture(
        "graphical_functions",
        graphical_function_model(),
        FixtureExpect::plain(
            &[
                ("main::curve", "flow"),
                ("main::drive", "flow"),
                ("main::g", "flow"),
                ("main::gtotal", "flow"),
                ("main::out", "flow"),
            ],
            "Five auxes, no stocks: flow phase only.",
            &[
                // curve's gf is (0,5) -> (1,15) evaluated at drive == time.
                (0, "curve", 5.0),
                (1, "curve", 15.0),
                // per-element tables: east (0,1)->(1,2), west (0,2)->(1,4).
                (0, "g[east]", 1.0),
                (0, "g[west]", 2.0),
                (1, "g[east]", 2.0),
                (1, "g[west]", 4.0),
                (0, "out[east]", 1.0),
                (1, "out[west]", 4.0),
                (0, "gtotal", 3.0),
                (1, "gtotal", 6.0),
            ],
        ),
    );
}

// ---------------------------------------------------------------------------
// Fixture 6: a standalone lookup-only table plus two consumers (#606).
//
// `table` is a *static table*, not a value-bearing variable: it is excluded
// from every runlist and from the saved output, so it must compile to NO
// fragment at all -- which is exactly the class of absence a golden alone is
// blind to and the declared phase map catches. Its data reaches the two
// consumers only through their `LOOKUP(table, x)` call sites, which resolve
// the table by ident rather than by a layout reference.
// ---------------------------------------------------------------------------

fn lookup_only_model() -> datamodel::Project {
    TestProject::new("frag_lookup_only")
        .with_sim_time(0.0, 1.0, 1.0)
        // An empty equation plus a gf IS the lookup-only form.
        .aux_with_gf("table", "", ramp_gf(3.0, 4.0))
        .scalar_aux("at_time", "LOOKUP(table, time)")
        .scalar_aux("at_half", "LOOKUP(table, 0.5)")
        .build_datamodel()
}

#[test]
fn char_lookup_only_table() {
    assert_fragment_fixture(
        "lookup_only_table",
        lookup_only_model(),
        FixtureExpect::plain(
            &[
                ("main::at_half", "flow"),
                ("main::at_time", "flow"),
                ("main::table", "none"),
            ],
            "`table` is a standalone lookup-only holder (#606): a static table, \
             excluded from every runlist and from the saved output. \
             `compile_var_fragment` still returns a fragment for it -- so it \
             appears here -- but with all three phases empty, because the \
             runlist gates reject it in each. `none` is therefore the CONTRACT \
             for this variable, not a dropped fragment: if it ever gains a \
             phase it has started producing a series of its own, and if the \
             row disappears entirely the fragment compiler started returning \
             whole-variable `None` for it (a different, and unattributed, \
             failure path).",
            &[
                // table is (0,3) -> (1,7); at_time samples it at `time`.
                (0, "at_time", 3.0),
                (1, "at_time", 7.0),
                // ...and at_half samples the SAME table at a different, constant
                // argument: two calls are independent.
                (0, "at_half", 5.0),
            ],
        ),
    );
}

// ---------------------------------------------------------------------------
// Fixture 7: an explicit sub-model instance with a wired input, plus a stdlib
// module call.
//
// This is the module-declaration family: `sub` compiles to a fragment carrying
// a `SymbolicModuleDecl` and an `EvalModule`, its input is fed by a
// `LoadModuleInput`, and the sub-model's own variables compile as a SEPARATE
// salsa cache entry per module-input set -- which is why the fixture renders
// `producer` twice, once input-agnostically (the diagnostic pass / element-graph
// probe wiring) and once at its real instance input set `{input}` (assembly's).
//
// `smoothed = SMTH1(src, 2)` additionally synthesizes the implicit
// SMOOTH helper variables, so this fixture is what pins
// `compile_implicit_var_fragment`'s output.
// ---------------------------------------------------------------------------

fn module_model() -> datamodel::Project {
    let aux = |ident: &str, equation: &str, can_be_module_input: bool| {
        datamodel::Variable::Aux(datamodel::Aux {
            ident: ident.to_string(),
            equation: datamodel::Equation::Scalar(equation.to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat {
                can_be_module_input,
                ..datamodel::Compat::default()
            },
        })
    };
    datamodel::Project {
        name: "frag_modules".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 1.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![],
        units: vec![],
        models: vec![
            datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![
                    aux("src", "3", false),
                    datamodel::Variable::Module(datamodel::Module {
                        ident: "sub".to_string(),
                        model_name: "producer".to_string(),
                        documentation: String::new(),
                        units: None,
                        references: vec![datamodel::ModuleReference {
                            src: "src".to_string(),
                            dst: "sub.input".to_string(),
                        }],
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    aux("usesub", "sub.output * 2", false),
                    aux("smoothed", "SMTH1(src, 2)", false),
                ],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
                macro_spec: None,
            },
            datamodel::Model {
                name: "producer".to_string(),
                sim_specs: None,
                variables: vec![aux("input", "0", true), aux("output", "input * 10", false)],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
                macro_spec: None,
            },
        ],
        source: None,
        ai_information: None,
    }
}

#[test]
fn char_modules_and_stdlib_call() {
    assert_fragment_fixture(
        "modules",
        module_model(),
        FixtureExpect {
            // `producer` twice, deliberately. A sub-model compiles a SEPARATE
            // salsa cache entry per module-input set, and the two entries are
            // materially different bytecode: at `&[]` the port variable
            // compiles its own equation, at `{input}` it compiles
            // `LoadModuleInput`. The `&[]` entry is the one the diagnostic
            // pass and the element-graph SCC probe consume -- the surface
            // stages 2 and 3 are most likely to perturb -- and the `{input}`
            // entry is the one assembly consumes.
            models: &[("main", &[]), ("producer", &[]), ("producer", &["input"])],
            phases: &[
                ("main::$⁚smoothed⁚0⁚arg1", "initial+flow"),
                ("main::$⁚smoothed⁚0⁚smth1", "initial+flow+stock"),
                ("main::smoothed", "initial+flow"),
                ("main::src", "initial+flow"),
                ("main::sub", "initial+flow+stock"),
                ("main::usesub", "flow"),
                ("producer::input", "flow"),
                ("producer::output", "flow"),
                ("producer::input", "flow"),
                ("producer::output", "flow"),
            ],
            why: "A module variable is the ONLY shape that compiles all three \
                  phases: it is not a stock, so the `!is_stock` arm of the \
                  flows gate admits it, and it IS a module, so the \
                  `is_stock || is_module` stock gate admits it too -- hence \
                  `initial+flow+stock` for both `sub` and the SMTH1 instance. \
                  The SMOOTH's internal stock puts everything that reaches its \
                  initial equation into the initials runlist as well: `src`, \
                  the synthesized `arg1` delay-time helper, `smoothed` (which \
                  reads the instance's output), and `sub`. `usesub` is not \
                  among them because `producer` holds no stock. \
                  `producer::input` carries a flow fragment at BOTH input sets, \
                  and for the ordinary reason: it is an Aux, so `!is_stock` \
                  already satisfies the flows gate. The `is_module_input` arm \
                  of that gate (`fragment_compile.rs`, \
                  `(!is_stock || is_module_input) && …`) only ever admits a \
                  STOCK-typed input port, which this fixture has none of. What \
                  the input set changes here is the compiled BODY, not the \
                  gate: compare the two `producer::input` renders in the \
                  golden -- `AssignConstCurr` of its own `0` equation at `&[]`, \
                  `LoadModuleInput` at `{input}`.",
            spot_checks: &[
                // src = 3 -> producer.input = 3 -> producer.output = 30 ->
                // usesub = 60. SMTH1's default initial IS its input, and the
                // input never moves, so `smoothed` sits at 3 forever.
                (0, "src", 3.0),
                (0, "sub\u{00B7}output", 30.0),
                (0, "usesub", 60.0),
                (0, "smoothed", 3.0),
                (1, "smoothed", 3.0),
            ],
            ltm: FixtureLtm::Off,
            expect_one_resolved_scc: false,
        },
    );
}

// ---------------------------------------------------------------------------
// Fixture 8: PREVIOUS and INIT, in both of their lowered forms.
//
// A DIRECT scalar argument compiles to the `LoadPrev` / `LoadInitial` opcode;
// an expression argument is first rewritten through a synthesized scalar helper
// aux (`builtins_visitor.rs`), so the opcode reads the HELPER rather than the
// user variable. Both forms are reference-bearing, and the helper form is the
// only place an implicit helper is read by a `SymLoadPrev` / `SymLoadInitial`.
// ---------------------------------------------------------------------------

fn prev_init_model() -> datamodel::Project {
    TestProject::new("frag_prev_init")
        .with_sim_time(0.0, 2.0, 1.0)
        .scalar_aux("x", "time * 2")
        .scalar_aux("prev_direct", "PREVIOUS(x)")
        .scalar_aux("prev_expr", "PREVIOUS(x + 1)")
        .scalar_aux("init_direct", "INIT(x)")
        .scalar_aux("init_expr", "INIT(x + 1)")
        .build_datamodel()
}

#[test]
fn char_prev_and_init() {
    assert_fragment_fixture(
        "prev_init",
        prev_init_model(),
        FixtureExpect::plain(
            &[
                ("main::$⁚init_expr⁚0⁚arg0", "initial+flow"),
                ("main::$⁚prev_expr⁚0⁚arg0", "flow"),
                ("main::init_direct", "initial+flow"),
                ("main::init_expr", "initial+flow"),
                ("main::prev_direct", "flow"),
                ("main::prev_expr", "flow"),
                ("main::x", "initial+flow"),
            ],
            "`INIT(...)` reads the frozen initial-values buffer, so everything \
             an INIT argument reaches must also be evaluated in the initials \
             phase: `x`, the `INIT(x + 1)` capture helper, and the two INIT \
             consumers. `PREVIOUS` reads the PRIOR step's committed values, \
             which the initials phase has not produced, so both PREVIOUS \
             consumers and the `PREVIOUS(x + 1)` capture helper are flow-only \
             -- the asymmetry between the two synthesized `arg0` helpers is \
             the load-bearing detail here.",
            &[
                // x = 2 * time -> 0, 2, 4.
                (0, "x", 0.0),
                (2, "x", 4.0),
                // PREVIOUS(x) is 0 at the initial step (the unary form
                // desugars to PREVIOUS(x, 0)), then the prior step's x.
                (0, "prev_direct", 0.0),
                (1, "prev_direct", 0.0),
                (2, "prev_direct", 2.0),
                // PREVIOUS(x + 1) goes through a capture helper holding x + 1.
                (1, "prev_expr", 1.0),
                (2, "prev_expr", 3.0),
                // INIT freezes at the initial step: x@t0 = 0, (x + 1)@t0 = 1.
                (2, "init_direct", 0.0),
                (2, "init_expr", 1.0),
            ],
        ),
    );
}

// ---------------------------------------------------------------------------
// Fixture 9: a resolved recurrence SCC (the `ref.mdl` shape).
//
// `ce` and `ecc` form a whole-variable 2-cycle whose induced ELEMENT graph is
// acyclic, so `resolve_recurrence_sccs` resolves it and `assemble_module`
// injects ONE combined fragment built by interleaving the members' per-element
// segments. That combined fragment is a second, distinct consumer of the exact
// production per-variable fragment (through `var_phase_symbolic_fragment_prod`),
// so it is rendered into the golden alongside the members it is built from.
// ---------------------------------------------------------------------------

fn recurrence_scc_model() -> datamodel::Project {
    TestProject::new("frag_recurrence_scc")
        .with_sim_time(0.0, 1.0, 1.0)
        .named_dimension("t", &["t1", "t2", "t3"])
        .array_with_ranges(
            "ce[t]",
            vec![("t1", "1"), ("t2", "ecc[t1] + 1"), ("t3", "ecc[t2] + 1")],
        )
        .array_with_ranges(
            "ecc[t]",
            vec![
                ("t1", "ce[t1] + 1"),
                ("t2", "ce[t2] + 1"),
                ("t3", "ce[t3] + 1"),
            ],
        )
        .build_datamodel()
}

#[test]
fn char_resolved_recurrence_scc() {
    assert_fragment_fixture(
        "recurrence_scc",
        recurrence_scc_model(),
        FixtureExpect {
            models: &[("main", &[])],
            phases: &[("main::ce", "flow"), ("main::ecc", "flow")],
            why: "Two arrayed auxes, no stocks: flow phase only. Both members \
                  still compile their OWN per-variable fragment -- the combined \
                  SCC fragment is built from them and injected at assembly, it \
                  does not replace them here.",
            spot_checks: &[
                // The element chain: ce[t1] = 1, ecc[e] = ce[e] + 1,
                // ce[t2] = ecc[t1] + 1, ce[t3] = ecc[t2] + 1.
                (0, "ce[t1]", 1.0),
                (0, "ecc[t1]", 2.0),
                (0, "ce[t2]", 3.0),
                (0, "ecc[t2]", 4.0),
                (0, "ce[t3]", 5.0),
                (0, "ecc[t3]", 6.0),
            ],
            ltm: FixtureLtm::Off,
            expect_one_resolved_scc: true,
        },
    );
}

// ---------------------------------------------------------------------------
// Fixture 10: an LTM-enabled model, in BOTH loop-enumeration modes.
//
// The two `db/ltm/compile.rs` emission sites -- each an inline COPY of the
// shared compile+symbolize tail that `db::assemble` also carries -- are what
// this fixture reaches: `compile_ltm_synthetic_fragment` for the link/loop
// score variables, and `compile_ltm_implicit_var_fragment` for the PREVIOUS
// capture helpers those score equations synthesize.
//
// Both modes are rendered because they emit DIFFERENT synthetic variables from
// different arms: exhaustive enumeration produces one link score per LOOP edge
// plus a `$⁚ltm⁚loop_score⁚{id}` per circuit (the `compile_direct` arm),
// while discovery produces one link score per CAUSAL edge -- including
// `rate→growth`, which no circuit traverses -- and no loop scores. Pinning only
// one would leave the other arm's fragment shape unpinned. The model is
// deliberately the smallest one with a feedback loop, since the
// synthetic-variable count grows fast.
// ---------------------------------------------------------------------------

fn ltm_loop_model() -> datamodel::Project {
    TestProject::new("frag_ltm_loop")
        .with_sim_time(0.0, 2.0, 1.0)
        .aux("rate", "0.1", None)
        .flow("growth", "level * rate", None)
        .stock("level", "10", &["growth"], &[], None)
        .build_datamodel()
}

/// The core simulation is unchanged by LTM: level starts at 10 and
/// growth = 0.1 * level, Euler at dt = 1 -> 10, 11, 12.1.
const LTM_LOOP_SPOT_CHECKS: &[(usize, &str, f64)] = &[
    (0, "level", 10.0),
    (0, "growth", 1.0),
    (1, "level", 11.0),
    (2, "level", 12.1),
];

#[test]
fn char_ltm_fragments_exhaustive() {
    assert_fragment_fixture(
        "ltm_loop_exhaustive",
        ltm_loop_model(),
        FixtureExpect {
            models: &[("main", &[])],
            phases: &[
                (
                    "main::$⁚$⁚ltm⁚link_score⁚growth→level⁚0⁚arg0",
                    "initial+flow",
                ),
                (
                    "main::$⁚$⁚ltm⁚link_score⁚growth→level⁚1⁚arg0",
                    "initial+flow",
                ),
                (
                    "main::$⁚$⁚ltm⁚link_score⁚growth→level⁚2⁚arg0",
                    "initial+flow",
                ),
                ("main::$⁚ltm⁚link_score⁚growth→level", "flow"),
                ("main::$⁚ltm⁚link_score⁚level→growth", "flow"),
                ("main::$⁚ltm⁚loop_score⁚r1", "flow"),
                ("main::growth", "flow"),
                ("main::level", "initial+stock"),
                ("main::rate", "flow"),
            ],
            why: "Exhaustive enumeration finds the single circuit \
                  `level -> growth -> level` and emits a link score for each of \
                  its TWO edges plus one `loop_score⁚r1` for the circuit -- so \
                  `rate→growth`, a causal edge no circuit traverses, gets no \
                  score here (contrast the discovery fixture). Every synthetic \
                  is a scalar aux, hence flow-only. Only the `growth→level` \
                  score synthesizes PREVIOUS capture helpers, and those land in \
                  the initials runlist because a stock's initial equation \
                  reaches them.",
            spot_checks: LTM_LOOP_SPOT_CHECKS,
            ltm: FixtureLtm::Exhaustive,
            expect_one_resolved_scc: false,
        },
    );
}

#[test]
fn char_ltm_fragments_discovery() {
    assert_fragment_fixture(
        "ltm_loop_discovery",
        ltm_loop_model(),
        FixtureExpect {
            models: &[("main", &[])],
            phases: &[
                (
                    "main::$⁚$⁚ltm⁚link_score⁚growth→level⁚0⁚arg0",
                    "initial+flow",
                ),
                (
                    "main::$⁚$⁚ltm⁚link_score⁚growth→level⁚1⁚arg0",
                    "initial+flow",
                ),
                (
                    "main::$⁚$⁚ltm⁚link_score⁚growth→level⁚2⁚arg0",
                    "initial+flow",
                ),
                ("main::$⁚ltm⁚link_score⁚growth→level", "flow"),
                ("main::$⁚ltm⁚link_score⁚level→growth", "flow"),
                ("main::$⁚ltm⁚link_score⁚rate→growth", "flow"),
                ("main::growth", "flow"),
                ("main::level", "initial+stock"),
                ("main::rate", "flow"),
            ],
            why: "Discovery mode scores every CAUSAL edge, so `rate→growth` \
                  appears here even though no circuit traverses it, and no \
                  loop-score variable is emitted at all (discovery ranks \
                  strongest paths per step instead of enumerating circuits). \
                  Every score is a scalar aux, hence flow-only. Only the \
                  `growth→level` score -- the stock-update edge, whose \
                  ceteris-paribus numerator re-integrates the stock -- \
                  synthesizes PREVIOUS capture helpers, and those land in the \
                  initials runlist because a stock's initial equation reaches \
                  them.",
            spot_checks: LTM_LOOP_SPOT_CHECKS,
            ltm: FixtureLtm::Discovery,
            expect_one_resolved_scc: false,
        },
    );
}

// ---------------------------------------------------------------------------
// Salsa cache reuse under layout-only project edits
//
// GH #964's acceptance criteria include "layout-only project edits continue to
// reuse unchanged salsa-cached fragments". Stage 3 of that issue is exactly the
// change that could break it, so it is measured here -- with execution records
// rather than memo pointers, because a fragment is DESIGNED to be
// layout-independent and therefore compares equal across a layout edit whether
// or not the compile re-ran. Salsa backdates on that equality and keeps the
// memo address, so every pointer-based check passes either way.
//
// Each test has a control step -- re-syncing the IDENTICAL project must
// re-execute nothing -- so a count is attributable to the edit that follows it.
//
// WHAT THIS MEASURED: that acceptance criterion does NOT hold today. Adding,
// deleting or renaming an unrelated variable re-executes EVERY per-variable
// fragment compile in the edited model. Two coarse salsa edges cause it, and
// both are inside the round trip GH #964 is deleting, so this is a baseline for
// stage 3 rather than a regression it introduced:
//
//   1. `compile_var_fragment` reads the whole `ModelDepGraphResult`
//      (`model_dependency_graph`) but uses only three runlist-membership bits
//      of it. Any variable added to the model changes the runlists, so the
//      value changes and every dependent re-executes.
//   2. `lower_var_fragment` / `collect_var_dependencies` read the whole
//      `SourceModel::variables` map field to resolve dependency names, so any
//      change to the model's variable SET invalidates every fragment through
//      that edge too.
//
// The fragments produced are identical, so salsa backdates them and nothing
// downstream re-runs -- which is exactly why no pointer-based test ever caught
// this, and why the expensive half (the compile itself) is the part that is
// not being saved. Narrowing edge 1 is a small salsa projection; narrowing
// edge 2 needs a per-name variable lookup query, which is a design change of
// the size stages 2 and 3 are for. These tests pin today's numbers so that
// change can be measured, and so stage 3 cannot make them worse unnoticed.
// ---------------------------------------------------------------------------

/// A flat model: `probe` (the fragment under test) reads `k`; every other
/// variable is independent of both, so adding, deleting or renaming one is a
/// pure layout change from `probe`'s point of view.
fn cache_probe_project(extra: &[(&str, &str)], keep_other: bool) -> datamodel::Project {
    let mut tp = TestProject::new("frag_cache_probe")
        .with_sim_time(0.0, 1.0, 1.0)
        .scalar_aux("k", "3")
        .scalar_aux("probe", "k * 2");
    if keep_other {
        tp = tp.scalar_aux("other", "1");
    }
    for (name, eqn) in extra {
        tp = tp.scalar_aux(name, eqn);
    }
    tp.build_datamodel()
}

/// Re-sync `project` onto `prev` and re-assemble, returning the new sync state
/// and every fragment-compiler body entry the re-assembly caused.
fn resync_and_assemble(
    db: &mut SimlinDb,
    project: &datamodel::Project,
    prev: Option<&PersistentSyncState>,
) -> (PersistentSyncState, Vec<(FragmentExecKind, String)>) {
    let state = sync_from_datamodel_incremental(db, project, prev);
    let source_project = state.to_sync_result().project;
    reset_fragment_executions();
    assemble_simulation(db, source_project, "main".to_string())
        .expect("the cache-probe fixture must assemble");
    (state, fragment_executions())
}

/// One variable's whole `CompiledVarFragment` (all three phases), for the
/// value-equality half of the layout-edit tests.
fn probe_fragment(
    db: &SimlinDb,
    state: &PersistentSyncState,
    model_name: &str,
    var: &str,
) -> crate::compiler::symbolic::CompiledVarFragment {
    let sync = state.to_sync_result();
    let model = sync.models[model_name].source;
    let source_var = sync.models[model_name].variables[var].source;
    compile_var_fragment(
        db,
        source_var,
        model,
        sync.project,
        ModuleInputSet::empty(db),
    )
    .as_ref()
    .unwrap_or_else(|| panic!("`{var}` must compile"))
    .fragment
    .clone()
}

fn explicit_execs(execs: &[(FragmentExecKind, String)]) -> Vec<&str> {
    execs
        .iter()
        .filter(|(kind, _)| *kind == FragmentExecKind::Explicit)
        .map(|(_, name)| name.as_str())
        .collect()
}

#[test]
fn layout_only_edits_and_fragment_cache_reuse() {
    let mut db = SimlinDb::default();

    let base = cache_probe_project(&[], true);
    let state1 = sync_from_datamodel_incremental(&mut db, &base, None);
    assemble_simulation(&db, state1.to_sync_result().project, "main".to_string())
        .expect("priming assemble");
    let probe_before = probe_fragment(&db, &state1, "main", "probe");

    // Control: an identical re-sync changes no input field, so nothing at all
    // may re-execute. Without this, any count below could be measuring the
    // re-sync rather than the edit.
    let (state2, control) = resync_and_assemble(&mut db, &base, Some(&state1));
    assert_eq!(
        control,
        Vec::new(),
        "control: re-syncing the identical project must re-execute no fragment \
         compiler at all"
    );

    // `probe`'s fragment must be VALUE-identical after EVERY edit, not merely
    // at the end. This is the property the execution counts cannot express,
    // and the one that matters most for stage 3.
    //
    // The counts are saturated -- every fragment already re-executes -- so a
    // newly-introduced layout dependency would not move them by one. Value
    // equality catches it: a fragment encoding anything about the model-global
    // layout changes when an unrelated variable is added, deleted or renamed.
    // It is also the stronger property, since it holds whether or not the
    // compile re-ran.
    //
    // Asserted after each edit INDIVIDUALLY and deliberately. Checking only
    // the end state is not enough: the base and the post-rename project happen
    // to hold the same NUMBER of variables here, so a fragment that leaked
    // `n_slots` would come back to its original value and a single end-state
    // comparison would pass. Only the post-add state (one more variable) has a
    // different layout size. A mutation probe that leaked the layout into the
    // literal pool is what surfaced that.
    //
    // `db::incremental_compile_tests::test_ac1_3_ac1_4_fragment_reuse_on_add_remove`
    // already asserts this shape, but only on a two-scalar-variable fixture;
    // `layout_only_edits_preserve_fragment_values_for_rich_shapes` adds the
    // arrayed / module / SCC shapes, where a layout dependency would hide.
    #[track_caller]
    fn assert_probe_unchanged(
        db: &SimlinDb,
        state: &PersistentSyncState,
        before: &crate::compiler::symbolic::CompiledVarFragment,
        after_what: &str,
    ) {
        assert_eq!(
            *before,
            probe_fragment(db, state, "main", "probe"),
            "`probe`'s fragment changed after {after_what} -- it is no longer \
             layout-independent, so assembly's single resolve step is no longer \
             the only place offsets are decided"
        );
    }

    // Edit 1: ADD an unrelated variable. This is the only one of the three
    // that changes the model's variable COUNT relative to the baseline.
    let added = cache_probe_project(&[("added", "5")], true);
    let (state3, add_execs) = resync_and_assemble(&mut db, &added, Some(&state2));
    assert_eq!(
        explicit_execs(&add_execs),
        vec!["added", "k", "other", "probe"],
        "adding an unrelated variable re-executes EVERY fragment in the model, \
         `probe` included"
    );
    assert_probe_unchanged(&db, &state3, &probe_before, "an unrelated ADD");

    // Edit 2: DELETE an unrelated variable (`other`).
    let deleted = cache_probe_project(&[("added", "5")], false);
    let (state4, del_execs) = resync_and_assemble(&mut db, &deleted, Some(&state3));
    assert_eq!(
        explicit_execs(&del_execs),
        vec!["added", "k", "probe"],
        "deleting an unrelated variable re-executes every surviving fragment"
    );
    assert_probe_unchanged(&db, &state4, &probe_before, "an unrelated DELETE");

    // Edit 3: RENAME an unrelated variable (`added` -> `renamed`).
    let renamed = cache_probe_project(&[("renamed", "5")], false);
    let (state5, rename_execs) = resync_and_assemble(&mut db, &renamed, Some(&state4));
    assert_eq!(
        explicit_execs(&rename_execs),
        vec!["k", "probe", "renamed"],
        "renaming an unrelated variable re-executes every fragment"
    );
    assert_probe_unchanged(&db, &state5, &probe_before, "an unrelated RENAME");
}

/// The value half of the layout-edit contract on the shapes most likely to
/// acquire a layout dependency: an arrayed variable (per-element offsets), a
/// module instance (a `SymbolicModuleDecl` carrying a `SymVarRef`), and a
/// resolved recurrence SCC member (whose combined fragment is built from these
/// per-member ones).
///
/// Kept separate from the counting test because it needs richer fixtures, and
/// because it is the assertion that survives stage 3 changing the counts.
#[test]
fn layout_only_edits_preserve_fragment_values_for_rich_shapes() {
    // (fixture label, builder taking the extra unrelated variables, probe var)
    let arrayed = |extra: &[(&str, &str)]| {
        let mut tp = TestProject::new("layout_value_arrayed")
            .with_sim_time(0.0, 1.0, 1.0)
            .named_dimension("region", &["east", "west", "north"])
            .array_with_ranges(
                "arr[region]",
                vec![("east", "1"), ("west", "2"), ("north", "3")],
            )
            .array_aux("probe[region]", "arr[region] * 2");
        for (name, eqn) in extra {
            tp = tp.scalar_aux(name, eqn);
        }
        tp.build_datamodel()
    };
    assert_fragment_value_survives_layout_edits(
        &arrayed(&[]),
        &arrayed(&[("added", "5")]),
        "probe",
    );

    let scc = |extra: &[(&str, &str)]| {
        let mut tp = TestProject::new("layout_value_scc")
            .with_sim_time(0.0, 1.0, 1.0)
            .named_dimension("t", &["t1", "t2", "t3"])
            .array_with_ranges(
                "ce[t]",
                vec![("t1", "1"), ("t2", "ecc[t1] + 1"), ("t3", "ecc[t2] + 1")],
            )
            .array_with_ranges(
                "ecc[t]",
                vec![
                    ("t1", "ce[t1] + 1"),
                    ("t2", "ce[t2] + 1"),
                    ("t3", "ce[t3] + 1"),
                ],
            );
        for (name, eqn) in extra {
            tp = tp.scalar_aux(name, eqn);
        }
        tp.build_datamodel()
    };
    assert_fragment_value_survives_layout_edits(&scc(&[]), &scc(&[("added", "5")]), "ce");

    // The module instance: `sub`'s fragment carries a `SymbolicModuleDecl`
    // whose `var` is a `SymVarRef`, plus an `EvalModule`. Both are the kind of
    // operand a layout dependency would show up in.
    assert_fragment_value_survives_layout_edits(
        &module_layout_project(false),
        &module_layout_project(true),
        "sub",
    );
}

/// `main` holds `src`, a `sub` instance of `producer`, and (when `with_extra`)
/// one unrelated aux -- a pure layout edit from `sub`'s point of view.
fn module_layout_project(with_extra: bool) -> datamodel::Project {
    let aux = |ident: &str, equation: &str, can_be_module_input: bool| {
        datamodel::Variable::Aux(datamodel::Aux {
            ident: ident.to_string(),
            equation: datamodel::Equation::Scalar(equation.to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat {
                can_be_module_input,
                ..datamodel::Compat::default()
            },
        })
    };
    let mut main_vars = vec![
        aux("src", "3", false),
        datamodel::Variable::Module(datamodel::Module {
            ident: "sub".to_string(),
            model_name: "producer".to_string(),
            documentation: String::new(),
            units: None,
            references: vec![datamodel::ModuleReference {
                src: "src".to_string(),
                dst: "sub.input".to_string(),
            }],
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        }),
    ];
    if with_extra {
        main_vars.push(aux("added", "5", false));
    }
    datamodel::Project {
        name: "layout_value_module".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 1.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![],
        units: vec![],
        models: vec![
            datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: main_vars,
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
                macro_spec: None,
            },
            datamodel::Model {
                name: "producer".to_string(),
                sim_specs: None,
                variables: vec![aux("input", "0", true), aux("output", "input * 10", false)],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
                macro_spec: None,
            },
        ],
        source: None,
        ai_information: None,
    }
}

/// Compile `var`'s fragment before and after a layout-only edit (adding an
/// unrelated variable) on the same incremental database, and assert the value
/// is unchanged.
#[track_caller]
fn assert_fragment_value_survives_layout_edits(
    base: &datamodel::Project,
    edited: &datamodel::Project,
    var: &str,
) {
    let mut db = SimlinDb::default();
    let state1 = sync_from_datamodel_incremental(&mut db, base, None);
    let before = probe_fragment(&db, &state1, "main", var);

    let state2 = sync_from_datamodel_incremental(&mut db, edited, Some(&state1));
    let after = probe_fragment(&db, &state2, "main", var);
    assert_eq!(
        before, after,
        "`{var}`'s fragment changed when an unrelated variable was added -- it \
         is not layout-independent"
    );

    // ...and back again: deleting the unrelated variable must restore the
    // identical fragment, not merely a differently-shaped one.
    let state3 = sync_from_datamodel_incremental(&mut db, base, Some(&state2));
    let restored = probe_fragment(&db, &state3, "main", var);
    assert_eq!(
        before, restored,
        "`{var}`'s fragment changed when the unrelated variable was deleted again"
    );
}

#[test]
fn equation_only_edit_recompiles_only_the_edited_fragment() {
    // The contrast case for `layout_only_edits_and_fragment_cache_reuse`: an
    // equation edit that does not change the edited variable's dependency set
    // leaves every other variable's inputs bit-identical, so only the edited
    // fragment recompiles. This is the incrementality the per-variable
    // fragment cache actually delivers today, and it is what makes the
    // layout-edit result above a *narrower* claim than "the cache does not
    // work".
    //
    // `db::fragment_cache_tests::test_compile_var_fragment_caching` asserts the
    // same thing with memo pointer equality, which cannot distinguish "reused"
    // from "recompiled to an equal value"; this is the version with evidence.
    let mut db = SimlinDb::default();

    let base = cache_probe_project(&[("independent", "9")], true);
    let state1 = sync_from_datamodel_incremental(&mut db, &base, None);
    assemble_simulation(&db, state1.to_sync_result().project, "main".to_string())
        .expect("priming assemble");

    let (state2, control) = resync_and_assemble(&mut db, &base, Some(&state1));
    assert_eq!(
        control,
        Vec::new(),
        "control: re-syncing the identical project must re-execute nothing"
    );

    // Same variable set, same dependency set for every variable -- only
    // `independent`'s constant moves, and nothing reads `independent`.
    let edited = cache_probe_project(&[("independent", "11")], true);
    let (state3, execs) = resync_and_assemble(&mut db, &edited, Some(&state2));
    assert_eq!(
        explicit_execs(&execs),
        vec!["independent"],
        "an equation-only edit to a variable nothing reads must recompile ONLY \
         that variable's fragment"
    );

    // ...but the blast radius is one hop wide, not zero: `lower_var_fragment`
    // builds its dependency-granular mini `ModelStage0` by PARSING each
    // dependency, so a consumer's fragment depends on its dependencies'
    // equation text and not merely on their shape. Editing `k`'s constant
    // therefore recompiles `probe` as well.
    //
    // That one hop is intrinsic to the mini-stage design (track-C invariant 2:
    // pointing the mini stage at a whole-project cached stage would make the
    // radius the whole project instead), so it is pinned as the CURRENT
    // contract, not flagged as a defect. If a later stage widens it beyond one
    // hop, this reds.
    let mut k_edited = cache_probe_project(&[("independent", "11")], true);
    assert_eq!(
        k_edited.models[0].variables[0].get_ident(),
        "k",
        "this edit rewrites slot 0 in place; it must be `k`, or the test is \
         silently measuring a different variable"
    );
    k_edited.models[0].variables[0] = datamodel::Variable::Aux(datamodel::Aux {
        ident: "k".to_string(),
        equation: datamodel::Equation::Scalar("4".to_string()),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    });
    let (_state4, k_execs) = resync_and_assemble(&mut db, &k_edited, Some(&state3));
    assert_eq!(
        explicit_execs(&k_execs),
        vec!["k", "probe"],
        "editing `k` recompiles `k` AND its one consumer `probe`, whose mini \
         ModelStage0 re-parses `k`; it must not reach any further"
    );
}

/// The cache granularity of the OTHER two fragment compilers, measured rather
/// than assumed.
///
/// `compile_implicit_var_fragment` is not a salsa query at all -- it is a plain
/// function called from the tracked `assemble_module` -- so every implicit
/// (SMOOTH/DELAY/TREND/PREVIOUS/INIT) helper recompiles whenever assembly
/// re-runs, which an equation edit to ANY variable in the model causes.
/// `compile_ltm_var_fragment` IS tracked, per `(from, to)` link.
///
/// Pinned because stage 3 of GH #964 routes all three emitters through one
/// implementation: if that implementation is a salsa query, these numbers
/// should drop, and if it is a plain function, the explicit path could
/// silently acquire the implicit path's granularity instead.
#[test]
fn implicit_and_ltm_fragment_cache_granularity() {
    use salsa::Setter;

    let project_with = |smoothed_input: &str, unrelated: &str| {
        TestProject::new("frag_cache_implicit")
            .with_sim_time(0.0, 1.0, 1.0)
            .scalar_aux("src", smoothed_input)
            .scalar_aux("smoothed", "SMTH1(src, 2)")
            .scalar_aux("unrelated", unrelated)
            .build_datamodel()
    };

    let mut db = SimlinDb::default();
    let base = project_with("3", "1");
    let state1 = sync_from_datamodel_incremental(&mut db, &base, None);
    assemble_simulation(&db, state1.to_sync_result().project, "main".to_string())
        .expect("priming assemble");

    let (state2, control) = resync_and_assemble(&mut db, &base, Some(&state1));
    assert_eq!(
        control,
        Vec::new(),
        "control: re-syncing the identical project must re-execute nothing"
    );

    // Edit a variable the SMTH1 helper does not read.
    let edited = project_with("3", "2");
    let (_state3, execs) = resync_and_assemble(&mut db, &edited, Some(&state2));
    assert_eq!(
        explicit_execs(&execs),
        vec!["unrelated"],
        "the explicit path is per-variable: only the edited fragment recompiles"
    );
    let implicit: Vec<&str> = execs
        .iter()
        .filter(|(kind, _)| *kind == FragmentExecKind::Implicit)
        .map(|(_, name)| name.as_str())
        .collect();
    assert_eq!(
        implicit,
        vec!["smoothed#0", "smoothed#1"],
        "every implicit helper of the model recompiles on an edit to a variable \
         none of them reads: `compile_implicit_var_fragment` has no cache entry \
         of its own, so its granularity is `assemble_module`'s"
    );

    // The LTM link fragments, on the same shape of edit.
    let mut ltm_db = SimlinDb::default();
    let ltm_base = ltm_loop_model();
    let ltm_state1 = sync_from_datamodel_incremental(&mut ltm_db, &ltm_base, None);
    let ltm_project = ltm_state1.to_sync_result().project;
    ltm_project.set_ltm_enabled(&mut ltm_db).to(true);
    assemble_simulation(&ltm_db, ltm_project, "main".to_string()).expect("priming LTM assemble");

    let (ltm_state2, ltm_control) = resync_and_assemble(&mut ltm_db, &ltm_base, Some(&ltm_state1));
    assert_eq!(
        ltm_control,
        Vec::new(),
        "control: re-syncing the identical LTM project must re-execute nothing"
    );

    // Step A: change a CONSTANT. A link score's equation is derived from the
    // TARGET's equation structure, not from any source's value, so no link
    // score's text moves and no LTM fragment recompiles.
    let rate_edited = TestProject::new("frag_ltm_loop")
        .with_sim_time(0.0, 2.0, 1.0)
        .aux("rate", "0.2", None)
        .flow("growth", "level * rate", None)
        .stock("level", "10", &["growth"], &[], None)
        .build_datamodel();
    let (ltm_state3, rate_execs) =
        resync_and_assemble(&mut ltm_db, &rate_edited, Some(&ltm_state2));
    assert_eq!(
        ltm_execs_of(&rate_execs),
        Vec::<&str>::new(),
        "editing a constant moves no link-score equation, so no LTM link \
         fragment recompiles"
    );

    // Step B: change an equation ON the circuit. At least the link score whose
    // TARGET moved must recompile; `rate\u{2192}growth` is not emitted in
    // exhaustive mode at all, so the whole set is a subset of the circuit's two
    // edges.
    //
    // Only the LOWER BOUND is asserted, because the exact set is
    // NONDETERMINISTIC. Over 40 in-process repetitions of exactly this
    // sequence, `["growth\u{2192}level", "level\u{2192}growth"]` came up 33
    // times and `["level\u{2192}growth"]` 7 times.
    //
    // What that nondeterminism is, established by measurement rather than
    // inferred: `compile_ltm_var_fragment`'s returned VALUE is equal before and
    // after the edit in every repetition, and `link_score_equation_text_shaped`
    // backdates correctly in every repetition -- only whether the body re-runs
    // varies. Salsa backdates the equal value, so no consumer observes a
    // difference and no artifact changes (both `ltm_loop_*` goldens hold across
    // repeated runs). It is wasted recompilation of one link fragment, not a
    // wrong answer. It also reproduces when the query is demanded DIRECTLY,
    // so it is not assembly's map-walk order.
    //
    // What it is NOT: it is not the `dep_idents` ordering (measured across that
    // fix with no effect) and not any value on the shaded query's chain. The
    // residual is which recorded dependency reports "maybe changed" given an
    // unchanged dependency set, which is a salsa dependency-verification
    // question rather than a fragment-compiler one. Asserting the full set
    // here would land a flaky test.
    let growth_edited = TestProject::new("frag_ltm_loop")
        .with_sim_time(0.0, 2.0, 1.0)
        .aux("rate", "0.2", None)
        .flow("growth", "level * rate * 1", None)
        .stock("level", "10", &["growth"], &[], None)
        .build_datamodel();
    let (_ltm_state4, growth_execs) =
        resync_and_assemble(&mut ltm_db, &growth_edited, Some(&ltm_state3));
    let recompiled = ltm_execs_of(&growth_execs);
    assert!(
        recompiled.contains(&"level\u{2192}growth"),
        "editing `growth`'s equation must recompile the link score whose TARGET \
         it is; got {recompiled:?}"
    );
    assert!(
        recompiled
            .iter()
            .all(|edge| *edge == "level\u{2192}growth" || *edge == "growth\u{2192}level"),
        "only the circuit's own two edges have link scores in exhaustive mode, \
         so nothing outside them may recompile; got {recompiled:?}"
    );
}

fn ltm_execs_of(execs: &[(FragmentExecKind, String)]) -> Vec<&str> {
    execs
        .iter()
        .filter(|(kind, _)| *kind == FragmentExecKind::Ltm)
        .map(|(_, name)| name.as_str())
        .collect()
}

/// A fragment carrying more than one temp must compile to a byte-identical
/// `PerVarBytecodes` every time, on independent databases.
///
/// The single-fixture `assert_temp_sizes_ordered` check inside
/// `assert_fragment_fixture` is a 50/50 detector on a two-temp fragment: a
/// two-entry `HashMap` yields its keys in the right order about half the time,
/// and each `HashMap` gets a fresh hash key from the thread-local counter. This
/// test repeats the compile on independent databases so an unordered
/// `temp_sizes` is caught with probability `1 - 2^-N` instead.
///
/// What is at stake is not the rendering: `PerVarBytecodes` is a salsa-cached
/// value with a derived `PartialEq`, so an order flip makes an identical
/// fragment compare unequal, salsa stops backdating, and the compiled artifact
/// stops being reproducible run to run (GH #595's class).
#[test]
fn multi_temp_fragment_is_byte_identical_across_fresh_databases() {
    const REPEATS: usize = 12;

    let dm = array_ops_model();
    let compile_combo = || {
        let db = SimlinDb::default();
        let source_project = sync_from_datamodel(&db, &dm).project;
        let model = *source_project.models(&db).get("main").unwrap();
        let combo = source_project.models(&db)["main"].variables(&db)["combo"];
        compile_var_fragment(
            &db,
            combo,
            model,
            source_project,
            ModuleInputSet::empty(&db),
        )
        .as_ref()
        .expect("`combo` must compile")
        .fragment
        .flow_bytecodes
        .clone()
        .expect("`combo` must have a flow fragment")
    };

    let first = compile_combo();
    assert_eq!(
        first.temp_sizes.len(),
        2,
        "the fixture must keep MORE THAN ONE temp in a single fragment, or this \
         test cannot observe an ordering at all"
    );
    for i in 1..REPEATS {
        let again = compile_combo();
        assert_eq!(
            first, again,
            "compile #{i} of the same variable on a fresh database produced a \
             different fragment; a salsa-cached value that is not a function of \
             its inputs defeats backdating and makes the artifact irreproducible"
        );
    }
}
