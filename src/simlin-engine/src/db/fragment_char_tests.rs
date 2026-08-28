// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Characterization pins for the **per-variable fragment compiler** -- the
//! salsa-cached unit of production compilation (`db::compile_var_fragment` and
//! its implicit/LTM siblings).
//!
//! These were written while that compiler still reached its layout-independent
//! result the long way round -- private per-fragment offsets, a stand-in
//! one-variable `compiler::Module`, and a `symbolize_*` pass to undo both --
//! and they are the gate GH #964's deletion of that round trip was measured
//! against. **All 12 goldens came through it byte-identical, with no
//! regeneration**, which is the strongest available evidence that the shorter
//! route produces the same value rather than a plausible one. The
//! integration corpus (`tests/integration/simulate.rs`) pins *numeric* results
//! of whole models, which is necessary but far too coarse here: it cannot see a
//! fragment change shape, a fragment stop being emitted (an `Option::None`
//! arm), a resource id get renumbered, or a fragment stop being deterministic.
//!
//! Three independent assertions per fixture, none of which subsumes another:
//!
//! 1. **A golden** of every fragment's rendered symbolic form -- opcode stream
//!    with `SymVarRef { name, element_offset }` operands (since GH #964 that is
//!    `compiler::VarRef`, the type lowering itself emits, not the output of a
//!    conversion pass), literal pool,
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
//! header comment for what that measurement found and for the two documented
//! reasons a module-instantiating add is still saturated.
//!
//! **That half is now the load-bearing one.** The value-equality assertions were
//! written when a fragment COULD have absorbed a layout dependency and still
//! looked right; after GH #964 a fragment cannot carry an offset of its own
//! model at all, and consulting one is a compile error, so the type subsumes
//! most of what those assertions were watching for. The execution counts are
//! what can still catch a regression -- a fragment newly depending on something
//! model-wide moves them and nothing else does. Do not loosen them.
//!
//! One property this suite established before that change is worth keeping on
//! the record, because the deletion rested on it: the goldens were byte-
//! identical under a reordering of the (now deleted) private dependency walk,
//! which was direct evidence that a fragment really was independent of the
//! offsets it was being handed.

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
    /// Post-simulation discovery: emits a link score per causal edge
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
/// Nine `SymbolicOpcode` variants carry a `SymVarRef` -- the operands the
/// GH #964 round trip exists to recover -- and the fixtures reach all nine.
/// There were eleven until C1b deleted the two that codegen could not emit:
/// `PushVarView` (never constructed in `compiler/codegen` at all -- only
/// `PushVarViewDirect` is) and a plain `AssignNext` (a stock update's last
/// operation is always the `Op2 Add` of `curr + net * dt`, so codegen now
/// emits the fused `BinOpAssignNext` directly and refuses any other shape).
/// Exhaustive coverage here is therefore a property of the suite, not a
/// coincidence: every reference-bearing opcode family the compiler can emit
/// appears in a golden.
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
        SymbolicOpcode::LookupDirect {
            base_gf,
            table_count,
            elem,
            mode,
        } => format!(
            "LookupDirect base_gf={base_gf} table_count={table_count} elem={elem} mode={mode:?}"
        ),
        SymbolicOpcode::SetCond {} => "SetCond".to_string(),
        SymbolicOpcode::If {} => "If".to_string(),
        SymbolicOpcode::Ret => "Ret".to_string(),
        SymbolicOpcode::LoadModuleInput { input } => format!("LoadModuleInput input={input}"),
        SymbolicOpcode::EvalModule { id, n_inputs } => {
            format!("EvalModule module={id} n_inputs={n_inputs}")
        }
        SymbolicOpcode::AssignCurr { var } => format!("AssignCurr {}", render_var_ref(var)),
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
        SymbolicOpcode::PushStaticView { view_id } => format!("PushStaticView view={view_id}"),
        SymbolicOpcode::PushVarViewDirect { var, dim_list_id } => format!(
            "PushVarViewDirect {} dim_list={dim_list_id}",
            render_var_ref(var)
        ),
        SymbolicOpcode::ViewSubscriptDynamic { dim_idx } => {
            format!("ViewSubscriptDynamic dim={dim_idx}")
        }
        SymbolicOpcode::ViewRangeDynamic { dim_idx } => format!("ViewRangeDynamic dim={dim_idx}"),
        SymbolicOpcode::PopView {} => "PopView".to_string(),
        SymbolicOpcode::LoadTempConst { temp_id, index } => {
            format!("LoadTempConst temp={temp_id} index={index}")
        }
        SymbolicOpcode::BeginIter {
            write_temp_id,
            has_write_temp,
        } => format!("BeginIter write_temp={write_temp_id} has_write_temp={has_write_temp}"),
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
    }
}

fn render_static_view(idx: usize, sv: &SymbolicStaticView) -> String {
    let base = match &sv.base {
        SymStaticViewBase::Var(v) => render_var_ref(v),
        SymStaticViewBase::PrevVar(v) => format!("prev({})", render_var_ref(v)),
        SymStaticViewBase::InitialVar(v) => format!("initial({})", render_var_ref(v)),
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
        if let Some(result) =
            compile_implicit_var_fragment(db, model, project, name.clone(), inputs)
        {
            push(&mut out, FragmentKind::Implicit, result);
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
                &owned_inputs,
                None,
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
// pop). The post-resolution materializer gives `order` and `ranked`'s
// array-producing builtins temp storage immediately before their readers, so
// this fixture pins `temp_sizes`, the temp-carrying opcodes and (via the
// universal ordering assertion) their determinism.
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
// and renumbers (`FragmentMerger::absorb_gf`, #582), so every fragment's gf
// block is pinned here.
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
                ("main::$⁚init_expr⁚0⁚arg0", "initial"),
                ("main::$⁚prev_expr⁚0⁚arg0", "flow"),
                ("main::init_direct", "initial+flow"),
                ("main::init_expr", "initial+flow"),
                ("main::prev_direct", "flow"),
                ("main::prev_expr", "flow"),
                ("main::x", "initial+flow"),
            ],
            "`INIT(...)` reads the frozen initial-values buffer, so its capture \
             helper is evaluated once in the initials phase and omitted from \
             flows. `PREVIOUS` reads the prior step's committed values, so its \
             capture helper is refreshed in flows and omitted from initials. \
             The two synthesized `arg0` helpers therefore occupy disjoint \
             phases; `x` and the INIT consumers still run in initials and \
             flows because their own equations are needed in both.",
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
                ("main::$⁚$⁚ltm⁚link_score⁚growth→level⁚0⁚arg0", "flow"),
                ("main::$⁚$⁚ltm⁚link_score⁚growth→level⁚1⁚arg0", "flow"),
                ("main::$⁚$⁚ltm⁚link_score⁚growth→level⁚2⁚arg0", "flow"),
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
                  score synthesizes PREVIOUS capture helpers. Those helpers \
                  refresh the next committed snapshot in flows; PREVIOUS's \
                  fallback supplies the first step, so they need no initial \
                  fragment.",
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
                ("main::$⁚$⁚ltm⁚link_score⁚growth→level⁚0⁚arg0", "flow"),
                ("main::$⁚$⁚ltm⁚link_score⁚growth→level⁚1⁚arg0", "flow"),
                ("main::$⁚$⁚ltm⁚link_score⁚growth→level⁚2⁚arg0", "flow"),
                ("main::$⁚ltm⁚link_score⁚growth→level", "flow"),
                ("main::$⁚ltm⁚link_score⁚level→growth", "flow"),
                ("main::$⁚ltm⁚link_score⁚rate→growth", "flow"),
                ("main::growth", "flow"),
                ("main::level", "initial+stock"),
                ("main::rate", "flow"),
            ],
            why: "Discovery mode scores every CAUSAL edge, so `rate→growth` \
                  appears here even though no circuit traverses it, and no \
                  loop-score variable is emitted at all (discovery finds \
                  and ranks loops after the run instead of enumerating \
                  circuits at compile time). \
                  Every score is a scalar aux, hence flow-only. Only the \
                  `growth→level` score -- the stock-update edge, whose \
                  ceteris-paribus numerator re-integrates the stock -- \
                  synthesizes PREVIOUS capture helpers. Those helpers refresh \
                  the next committed snapshot in flows; PREVIOUS's fallback \
                  supplies the first step, so they need no initial fragment.",
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
// The criterion held only as a VALUE property until C1b: the fragments were
// identical across a layout edit, but every one of them was recompiled to
// rediscover that. Two coarse salsa edges caused it, and both were inside the
// round trip GH #964 is deleting:
//
//   1. `compile_var_fragment` read the whole `ModelDepGraphResult`
//      (`model_dependency_graph`) but used only three runlist-membership bits
//      of it. Any variable added to the model changes the runlists, so the
//      value changed and every dependent re-executed. Narrowed by the
//      `var_runlist_membership` projection, which returns those three bits and
//      backdates.
//   2. the explicit fragment constructor read the whole
//      `SourceModel::variables` map field to resolve dependency names, so any
//      change to the model's variable SET invalidated every fragment through
//      that edge too. Narrowed by the `model_variable_by_name` firewall query,
//      so a fragment depends on the dependencies it actually looks up.
//
// Fixing edge 1 alone was provably unmeasurable while edge 2 stood, which is
// why they landed together. A THIRD edge (`model_implicit_var_info`, narrowed
// by `model_implicit_var_by_name`) is exercised only by a variable that
// synthesizes an implicit helper, so it has its own test below rather than a
// row here.
//
// **These counts are the gate C1c is measured against.** They cover a plain
// aux here, PREVIOUS/INIT helpers below, and module-instantiating helpers in
// `module_helper_add_reparses_only_the_added_variable`. A rewrite that
// reintroduces a model-wide dependency reds on body executions, not merely on
// equal output values.
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

fn implicit_execs(execs: &[(FragmentExecKind, String)]) -> Vec<&str> {
    execs
        .iter()
        .filter(|(kind, _)| *kind == FragmentExecKind::Implicit)
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
    // Both checks are kept, and the reason is NOT that one is weaker. Since
    // C1b narrowed the two coarse edges the counts above are tight, and they
    // are in fact the more sensitive of the two: a fragment cannot change
    // value without its body re-running, so any layout dependency strong
    // enough to move the value has already moved a count. What value equality
    // adds is INDEPENDENCE from the counting apparatus -- it holds whatever
    // C1c does to the query structure, the recorder's `FragmentExecKind`s, or
    // which compiler owns which variable, and it holds whether or not the
    // compile re-ran. A rewrite is free to change the counts for a legitimate
    // reason; it is never free to change these values.
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
        vec!["added"],
        "adding an unrelated variable must compile ONLY the new variable: no \
         existing fragment's inputs changed, so none may re-execute"
    );
    assert_probe_unchanged(&db, &state3, &probe_before, "an unrelated ADD");

    // Edit 2: DELETE an unrelated variable (`other`). Nothing referenced it, so
    // no surviving fragment has a changed input -- not even a re-execution that
    // would backdate.
    let deleted = cache_probe_project(&[("added", "5")], false);
    let (state4, del_execs) = resync_and_assemble(&mut db, &deleted, Some(&state3));
    assert_eq!(
        explicit_execs(&del_execs),
        Vec::<&str>::new(),
        "deleting an unrelated variable must re-execute no fragment at all"
    );
    assert_probe_unchanged(&db, &state4, &probe_before, "an unrelated DELETE");

    // Edit 3: RENAME an unrelated variable (`added` -> `renamed`). A rename is
    // a delete plus an add, so exactly the new name compiles.
    let renamed = cache_probe_project(&[("renamed", "5")], false);
    let (state5, rename_execs) = resync_and_assemble(&mut db, &renamed, Some(&state4));
    assert_eq!(
        explicit_execs(&rename_execs),
        vec!["renamed"],
        "renaming an unrelated variable must compile only the renamed variable"
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

/// `main` reads `sub·output` across a module boundary. `producer_extra` adds a
/// variable to the SUB-model that sorts before `output` (so `output`'s offset
/// inside `producer` moves); `unrelated_extra` adds one to a model nothing
/// instantiates.
fn submodel_layout_project(producer_extra: bool, unrelated_extra: bool) -> datamodel::Project {
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

    let mut producer_vars = vec![aux("input", "0", true), aux("output", "input * 10", false)];
    if producer_extra {
        // Sorts before both, so `compute_layout`'s alphabetical body layout
        // pushes `output` from slot 1 to slot 2.
        producer_vars.insert(0, aux("aaa", "1", false));
    }

    let mut unrelated_vars = vec![aux("u1", "1", false)];
    if unrelated_extra {
        unrelated_vars.push(aux("u2", "2", false));
    }

    datamodel::Project {
        name: "frag_submodel_layout".to_string(),
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
                ],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
                macro_spec: None,
            },
            datamodel::Model {
                name: "producer".to_string(),
                sim_specs: None,
                variables: producer_vars,
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
                macro_spec: None,
            },
            datamodel::Model {
                name: "unrelated".to_string(),
                sim_specs: None,
                variables: unrelated_vars,
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

/// Every `element_offset` a fragment's opcodes carry for references to
/// `module_var`, across all three phases, in opcode order.
fn cross_module_element_offsets(
    fragment: &crate::compiler::symbolic::CompiledVarFragment,
    module_var: &str,
) -> Vec<usize> {
    let phases = [
        fragment.initial_bytecodes.as_ref(),
        fragment.flow_bytecodes.as_ref(),
        fragment.stock_bytecodes.as_ref(),
    ];
    let mut offsets = Vec::new();
    for bc in phases.into_iter().flatten() {
        for op in &bc.symbolic.code {
            let var = match op {
                SymbolicOpcode::LoadVar { var }
                | SymbolicOpcode::SymLoadPrev { var }
                | SymbolicOpcode::SymLoadInitial { var }
                | SymbolicOpcode::LoadSubscript { var }
                | SymbolicOpcode::AssignCurr { var }
                | SymbolicOpcode::AssignConstCurr { var, .. }
                | SymbolicOpcode::BinOpAssignCurr { var, .. }
                | SymbolicOpcode::BinOpAssignNext { var, .. }
                | SymbolicOpcode::PushVarViewDirect { var, .. } => var,
                _ => continue,
            };
            if var.name.as_str() == module_var {
                offsets.push(var.element_offset);
            }
        }
    }
    offsets
}

/// The ONE layout channel that still reaches into a fragment -- and the only
/// place the value-equality assertions above still have teeth of their own.
///
/// Since GH #964 a fragment carries no offset of its own model at all, and
/// consulting one is impossible (a `FragmentInput` carries no offset of the
/// model being compiled; only a module dependency's sub-model shape has
/// slots), so the TYPE now subsumes most of what those
/// assertions were watching for. One channel survives, in the other direction:
/// a cross-module reference `sub·output` lowers to
/// `VarRef { name: sub, element_offset: <output's offset INSIDE producer> }`,
/// because the parent's layout has a single entry spanning the whole instance
/// and none for `sub·output`. `Context::resolve` reads that offset out of the
/// module dependency's `ModelShape` (`db::model_shape`, which is
/// `compute_layout(producer)` keyed by variable name), already fixed before any
/// parent fragment compiles.
///
/// So the parent's cached fragment is a function of the SUB-model's layout,
/// and both directions are load-bearing:
///
/// * growing `producer` ahead of `output` MUST change the parent's fragment,
///   and MUST re-execute it. A cache hit here would serve a stale
///   `element_offset` and the parent would read the wrong slot of the instance
///   -- a silent wrong number, not a failure to compile.
/// * growing a model nothing instantiates must NOT change it.
///
/// Neither direction was pinned before. The behavior is pre-existing and
/// unchanged by GH #964; what changed is that it is now the only layout input
/// a fragment has, which is what makes it worth its own test.
#[test]
fn parent_fragment_tracks_the_sub_models_layout_and_nothing_else() {
    let base = submodel_layout_project(false, false);
    let grown_submodel = submodel_layout_project(true, false);
    let grown_unrelated = submodel_layout_project(false, true);

    let mut db = SimlinDb::default();
    let state1 = sync_from_datamodel_incremental(&mut db, &base, None);
    assemble_simulation(&db, state1.to_sync_result().project, "main".to_string())
        .expect("the sub-model layout fixture must assemble");
    let before = probe_fragment(&db, &state1, "main", "usesub");

    // Precondition: the fixture really does carry a cross-module reference, so
    // a shape change that stopped emitting one fails here rather than making
    // the assertions below vacuously true.
    assert_eq!(
        cross_module_element_offsets(&before, "sub"),
        vec![1],
        "`usesub` must read `sub` at `output`'s offset inside `producer` \
         (input@0, output@1)"
    );

    // Direction 1: the sub-model's layout shifts under the parent.
    let (state2, execs) = resync_and_assemble(&mut db, &grown_submodel, Some(&state1));
    let after_submodel = probe_fragment(&db, &state2, "main", "usesub");
    assert_eq!(
        cross_module_element_offsets(&after_submodel, "sub"),
        vec![2],
        "adding `aaa` to `producer` moves `output` to slot 2, so the parent's \
         cross-module reference must follow it"
    );
    assert_ne!(
        before, after_submodel,
        "the parent's fragment must change when the sub-model's layout shifts \
         beneath it"
    );
    // Exactly three fragments re-execute, and the set is the point: `usesub`
    // MUST be in it (a cache hit would serve the stale element offset), `sub`
    // must be too (its `SymbolicModuleDecl` and `EvalModule` describe an
    // instance whose size changed), and `src` -- an ordinary aux in `main` --
    // must not, or the sub-model edit has become a whole-project invalidation.
    // `aaa` is the new sub-model variable itself.
    assert_eq!(
        explicit_execs(&execs),
        vec!["aaa", "sub", "usesub"],
        "growing the sub-model must re-execute the new variable, the module \
         variable, and the parent's cross-module reader -- and nothing else"
    );

    // ...and back: shrinking the sub-model again restores the identical
    // fragment, so the dependence is on the layout rather than on the edit.
    let state3 = sync_from_datamodel_incremental(&mut db, &base, Some(&state2));
    assert_eq!(
        before,
        probe_fragment(&db, &state3, "main", "usesub"),
        "removing the sub-model variable must restore the identical parent \
         fragment"
    );

    // Direction 2: a model nothing instantiates grows. `main` and `producer`
    // are untouched, so the parent's fragment must be bit-identical AND must
    // not recompile.
    //
    // The execution set is measured, not assumed: exactly `["sub"]`, stable
    // across repeated runs. `sub` re-executes because it is the edited
    // variable. `usesub`, the cross-module READER this test is about, is not in
    // the set: its fragment is a function of `producer`'s layout, and
    // `producer` did not move.
    //
    // Value equality alone could not make that claim. It cannot distinguish
    // "correctly cached" from "recompiled and happened to agree", which is
    // precisely the distinction direction 1 turns on.
    let (state4, unrelated_execs) = resync_and_assemble(&mut db, &grown_unrelated, Some(&state3));
    assert_eq!(
        explicit_execs(&unrelated_execs),
        vec!["sub"],
        "growing a model nothing instantiates must not recompile the parent's \
         cross-module reader"
    );
    assert_eq!(
        before,
        probe_fragment(&db, &state4, "main", "usesub"),
        "adding a variable to a model nothing instantiates must not change the \
         parent's fragment"
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

    // ...but the blast radius is one hop wide, not zero: `explicit_fragment_input`
    // builds its dependency-granular mini `LoweringModel` by PARSING each
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
         LoweringModel re-parses `k`; it must not reach any further"
    );
}

/// The cache granularity of the OTHER two fragment compilers, measured rather
/// than assumed.
///
/// `compile_implicit_var_fragment` is a salsa query keyed on the helper's own
/// canonical name, so an implicit (SMOOTH/DELAY/TREND/PREVIOUS/INIT) helper
/// recompiles only when something it reads changes -- NOT merely because
/// assembly re-ran, which an equation edit to any variable in the model
/// causes. `compile_ltm_var_fragment` is likewise tracked, per `(from, to)`
/// link.
///
/// The implicit assertion below is the whole reason the query is keyed the way
/// it is. While it was a plain function every helper in the model recompiled on
/// every assembly: this fixture recompiled both of `smoothed`'s helpers when
/// `unrelated` was edited, and on C-LEARN it was 651 helper compiles per cold
/// assembly and ~28% of the cost of a warm single-equation edit. A change that
/// reverts the query to a plain function reds here on the count rather than
/// merely running slower.
///
/// Pinned also because stage 3 of GH #964 routes all three emitters through one
/// implementation: the explicit path must not silently acquire the implicit
/// path's granularity, or vice versa.
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
    let (state3, execs) = resync_and_assemble(&mut db, &edited, Some(&state2));
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
        Vec::<&str>::new(),
        "no implicit helper recompiles on an edit to a variable none of them \
         reads: `compile_implicit_var_fragment` has its own cache entry per \
         helper, so its granularity is the helper's, not `assemble_module`'s"
    );

    // The complement, so the assertion above cannot pass by the query having
    // become unreachable: editing a variable a helper DOES read must still
    // recompile it.
    //
    // Only ONE of the two helpers reads `src`, and which one is a property of
    // `builtins_visitor`'s synthesis rather than of this cache: an argument
    // that is already a bare `Var` is passed through by name and gets no helper
    // at all, so `SMTH1(src, 2)` synthesizes `⁚arg1` for the literal `2` and
    // wires `src` straight into the `⁚smth1` module instance. The granularity
    // is therefore per HELPER, not per parent variable -- editing `src` leaves
    // the constant-capture helper's fragment cached.
    let src_edited = project_with("5", "2");
    let (_state4, src_execs) = resync_and_assemble(&mut db, &src_edited, Some(&state3));
    let implicit_after_src: Vec<&str> = src_execs
        .iter()
        .filter(|(kind, _)| *kind == FragmentExecKind::Implicit)
        .map(|(_, name)| name.as_str())
        .collect();
    assert_eq!(
        implicit_after_src,
        vec!["smoothed#$\u{205A}smoothed\u{205A}0\u{205A}smth1"],
        "editing `src` must still recompile the helper that reads it (a query \
         that never re-executed would be a cache bug, not a cache win), and \
         must NOT recompile `\u{205A}arg1`, which captures the literal `2`"
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

/// The third coarse edge (`model_implicit_var_info`), pinned for both a plain
/// implicit helper and one that instantiates a module.
///
/// `layout_only_edits_and_fragment_cache_reuse` above adds a plain aux, which
/// synthesizes no implicit variable at all and so never touches this edge. A
/// variable whose equation calls `PREVIOUS` or `INIT` DOES synthesize one, and
/// before the `model_implicit_var_by_name` projection every fragment in the
/// model re-executed for it -- `explicit_fragment_input` read the whole
/// implicit-var map to answer a per-name question.
///
/// A module-instantiating helper must be tight too. Its five
/// `stdlib⁚smth1` template variables compile for the first time, while the
/// unchanged variables of the edited model retain their per-variable parse and
/// fragment memos. `module_helper_add_reparses_only_the_added_variable` isolates
/// that boundary with salsa execution counts over the same production path.
#[test]
fn implicit_helper_add_is_tight_for_plain_and_module_helpers() {
    use crate::db::exec_probe::ProbedDb;

    // `probe` reads `k`; the added variable is independent of both, so this is
    // a layout-only edit from `probe`'s point of view in every case.
    let project_with = |extra: Option<(&str, &str)>| {
        let mut tp = TestProject::new("frag_cache_implicit_edge")
            .with_sim_time(0.0, 2.0, 1.0)
            .scalar_aux("k", "3")
            .scalar_aux("probe", "k * 2");
        if let Some((name, eqn)) = extra {
            tp = tp.scalar_aux(name, eqn);
        }
        tp.build_datamodel()
    };

    // ── A PREVIOUS helper: narrowed by `model_implicit_var_by_name`.
    let mut db = SimlinDb::default();
    let base = project_with(None);
    let state1 = sync_from_datamodel_incremental(&mut db, &base, None);
    assemble_simulation(&db, state1.to_sync_result().project, "main".to_string())
        .expect("priming assemble");

    let (state2, control) = resync_and_assemble(&mut db, &base, Some(&state1));
    assert_eq!(
        control,
        Vec::new(),
        "control: re-syncing the identical project must re-execute nothing"
    );

    // The argument must be an EXPRESSION, not a bare variable. A direct scalar
    // `PREVIOUS(k, 0)` compiles straight to the `LoadPrev` opcode and
    // synthesizes NO implicit variable at all, so it never touches this edge --
    // it passes whether or not the projection exists, which makes it a
    // vacuous fixture. (A mutation probe reintroducing the whole-map read is
    // how that was caught: it stayed green on the bare-variable form and reds
    // on this one.) `PREVIOUS(k * 2, 0)` is rewritten through a synthesized
    // scalar helper aux, which is the shape that changes
    // `model_implicit_var_info` without instantiating any module.
    let with_prev = project_with(Some(("lagged", "PREVIOUS(k * 2, 0)")));
    let (_state3, prev_execs) = resync_and_assemble(&mut db, &with_prev, Some(&state2));
    assert_eq!(
        explicit_execs(&prev_execs),
        vec!["lagged"],
        "adding a PREVIOUS-helper-bearing variable must compile only the new \
         variable: the implicit-var map changed, but no existing fragment asks \
         it about a name whose answer moved"
    );

    // ── An SMTH1 helper: the template is new work; `k` and `probe` are not.
    let mut db2 = ProbedDb::new();
    let base2 = project_with(None);
    let s1 = sync_from_datamodel_incremental(db2.db_mut(), &base2, None);
    assemble_simulation(db2.db(), s1.to_sync_result().project, "main".to_string())
        .expect("priming assemble");

    db2.reset();
    let s2 = sync_from_datamodel_incremental(db2.db_mut(), &base2, Some(&s1));
    reset_fragment_executions();
    assemble_simulation(db2.db(), s2.to_sync_result().project, "main".to_string())
        .expect("control assemble");
    let control2 = fragment_executions();
    assert_eq!(
        control2,
        Vec::new(),
        "control: re-syncing the identical project must re-execute nothing"
    );
    assert!(
        db2.counts().is_empty(),
        "control: re-syncing the identical project must re-execute no tracked query"
    );

    let with_smth = project_with(Some(("smoothed", "SMTH1(k, 2)")));
    db2.reset();
    let _s3 = sync_from_datamodel_incremental(db2.db_mut(), &with_smth, Some(&s2));
    reset_fragment_executions();
    assemble_simulation(db2.db(), _s3.to_sync_result().project, "main".to_string())
        .expect("SMTH1 assemble");
    let smth_execs = fragment_executions();
    // `delay_time`/`flow`/`initial_value`/`input`/`output` are the spliced
    // `stdlib⁚smth1` template's own variables, compiling for the first time.
    // The new module seeds initials, so its initial-dependency closure pulls in
    // parent-side source `k`: `k` legitimately gains initial bytecode while
    // unrelated `probe` remains cached.
    assert_eq!(
        explicit_execs(&smth_execs),
        vec![
            "delay_time",
            "flow",
            "initial_value",
            "input",
            "k",
            "output",
            "smoothed"
        ],
        "adding a module-instantiating helper must compile the new parent and \
         the previously-unused stdlib template, add initial bytecode to `k`, \
         and reuse the unrelated `probe` fragment"
    );
    assert_eq!(
        implicit_execs(&smth_execs),
        vec![
            "smoothed#$\u{205A}smoothed\u{205A}0\u{205A}arg1",
            "smoothed#$\u{205A}smoothed\u{205A}0\u{205A}smth1"
        ],
        "the first module instance must compile exactly its two new helpers"
    );
    assert_eq!(
        db2.counts().get("parse_source_variable").copied(),
        Some((6, 6)),
        "the assembly must parse the new `smoothed` source and the five first-use \
         stdlib sources exactly once each; `k`'s legitimate phase-membership \
         rebuild must reuse its parsed equation, and `probe` remains cached"
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

/// Adding a second module helper reparses only that variable, measured through
/// salsa execution counts over every tracked query (`db::exec_probe`), not just
/// the fragment compilers.
///
/// * **`project.models` changing is not a cause.** Both scenarios below assert
///   the map is IDENTICAL across the edit, and that `stdlib⁚smth1` is already
///   in it before either edit runs. `db::sync` splices every stdlib model on
///   every sync (`sync_from_datamodel_incremental`) and calls `set_models` only
///   on a changed map, so instantiating a SMOOTH adds no model: the template
///   was there from the first sync of a project that never mentions it.
/// * Scenario A adds a plain aux to a model that already holds a module
///   instance. Scenario B adds a second module instance. Both edits execute one
///   parse and one explicit fragment: parse identity is the variable and its
///   project-global context, never a model-wide module-name set.
#[test]
fn module_helper_add_reparses_only_the_added_variable() {
    use crate::db::exec_probe::ProbedDb;

    // `probe` exercises D3's model-local bare-element classifier;
    // `smoothed` reads `k` through a SMTH1 instance. The variable each
    // scenario adds is independent of all four existing sources.
    let project_with = |extra: Option<(&str, &str)>| {
        let mut tp = TestProject::new("frag_cache_module_ident_edge")
            .with_sim_time(0.0, 2.0, 1.0)
            .named_dimension("d", &["e1", "e2"])
            .array_with_ranges("vals[d]", vec![("e1", "10"), ("e2", "20")])
            .scalar_aux("k", "3")
            .scalar_aux("probe", "PREVIOUS(vals[e2], 0)")
            .scalar_aux("smoothed", "SMTH1(k, 2)");
        if let Some((name, eqn)) = extra {
            tp = tp.scalar_aux(name, eqn);
        }
        tp.build_datamodel()
    };

    // Returns (explicit and implicit fragment bodies that ran, whole-query
    // execution table, whether the project's model map moved) for one edit off
    // the primed base.
    let measure = |extra: Option<(&str, &str)>| {
        let mut probed = ProbedDb::new();
        let base = project_with(None);
        let state1 = sync_from_datamodel_incremental(probed.db_mut(), &base, None);
        assemble_simulation(
            probed.db(),
            state1.to_sync_result().project,
            "main".to_string(),
        )
        .expect("priming assemble");

        // Control: an identical re-sync changes no input, so nothing at all may
        // re-execute. Without it, any count below could be measuring the
        // re-sync rather than the edit.
        probed.reset();
        let (state2, control) = resync_and_assemble(probed.db_mut(), &base, Some(&state1));
        assert_eq!(
            control,
            Vec::new(),
            "control: re-syncing the identical project must re-execute no \
             fragment compiler"
        );
        assert!(
            probed.counts().is_empty(),
            "control: re-syncing the identical project must re-execute no \
             TRACKED QUERY at all; got {:?}",
            probed.counts()
        );

        let models_before = state2.to_sync_result().project.models(probed.db()).clone();
        assert!(
            models_before.contains_key("stdlib\u{205A}smth1"),
            "the stdlib template must already be spliced BEFORE the edit, or \
             this fixture cannot tell a splice apart from an ident-set change"
        );

        probed.reset();
        let (state3, execs) =
            resync_and_assemble(probed.db_mut(), &project_with(extra), Some(&state2));
        let models_after = state3.to_sync_result().project.models(probed.db()).clone();
        (
            explicit_execs(&execs)
                .into_iter()
                .map(str::to_string)
                .collect::<Vec<_>>(),
            implicit_execs(&execs)
                .into_iter()
                .map(str::to_string)
                .collect::<Vec<_>>(),
            probed.counts(),
            models_before == models_after,
        )
    };

    // ── Scenario A: a plain aux, added to a model that already instantiates a
    // module. Tight. The spliced stdlib template and the existing instance do
    // not, by themselves, saturate anything.
    let (a_fragments, a_helpers, a_counts, a_models_unchanged) = measure(Some(("other", "1")));
    assert!(
        a_models_unchanged,
        "adding a plain aux must not move `project.models`"
    );
    assert_eq!(
        a_fragments,
        vec!["other"],
        "with the module instance and its stdlib template already compiled, \
         adding an unrelated aux must compile only that aux"
    );
    assert_eq!(
        a_helpers,
        Vec::<String>::new(),
        "a plain auxiliary must not create an implicit helper"
    );
    assert_eq!(
        a_counts.get("parse_source_variable").map(|(runs, _)| *runs),
        Some(1),
        "...and must parse only that aux: parse identity contains no model-wide \
         variable-name set, so every unchanged source keeps its cache key"
    );

    // ── Scenario B: a SECOND module instance in the same model. The module
    // map does not move and the stdlib template does not recompile. Only the
    // added variable parses and compiles.
    let (b_fragments, b_helpers, b_counts, b_models_unchanged) =
        measure(Some(("smoothed2", "SMTH1(k, 3)")));
    assert!(
        b_models_unchanged,
        "adding a second module instance must not move `project.models` \
         either: the stdlib models are spliced on every sync, so the map \
         already held `stdlib\u{205A}smth1`"
    );
    assert_eq!(
        b_fragments,
        vec!["smoothed2"],
        "a second module instance must compile only its new parent; the \
         existing parent and independent variables are unchanged"
    );
    assert_eq!(
        b_helpers,
        vec![
            "smoothed2#$\u{205A}smoothed2\u{205A}0\u{205A}arg1",
            "smoothed2#$\u{205A}smoothed2\u{205A}0\u{205A}smth1"
        ],
        "a second module instance must compile only its own two new helpers"
    );
    assert_eq!(
        b_counts.get("parse_source_variable").map(|(runs, _)| *runs),
        Some(1),
        "only the added variable may parse"
    );
    assert_eq!(
        b_counts.get("model_module_map"),
        None,
        "the whole-model module map the Phase 7 investigation named as a third \
         cause no longer exists (Phase 3), so it cannot be one"
    );
}

/// Changing which child output a parent reads during initialization changes
/// the initial-phase membership of exactly the old and new child outputs. The
/// project-level requirement walk may re-execute, but its per-module seed
/// projection must backdate every unrelated child fragment.
#[test]
fn qualified_initial_seed_edit_recompiles_only_changed_memberships() {
    use crate::testutils::{sim_specs_with_units, x_aux, x_model, x_module, x_project};

    let project_with = |output: &str| {
        let child = x_model(
            "child",
            vec![
                x_aux("out_a", "10", None),
                x_aux("out_b", "20", None),
                x_aux("unrelated", "30", None),
            ],
        );
        let main = x_model(
            "main",
            vec![
                x_module("child", &[], None),
                x_aux("frozen", &format!("INIT(child.{output})"), None),
            ],
        );
        x_project(sim_specs_with_units("month"), &[main, child])
    };

    let mut db = SimlinDb::default();
    let base = project_with("out_a");
    let state1 = sync_from_datamodel_incremental(&mut db, &base, None);
    assemble_simulation(&db, state1.to_sync_result().project, "main".to_string())
        .expect("priming assemble");

    let edited = project_with("out_b");
    let (_state2, executions) = resync_and_assemble(&mut db, &edited, Some(&state1));
    assert_eq!(
        explicit_execs(&executions),
        ["frozen", "out_a", "out_b"],
        "the edited reader and the two variables whose initials membership \
         changed are the complete recompilation set; unrelated stays cached"
    );
    assert_eq!(implicit_execs(&executions), Vec::<&str>::new());
}
