// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The value-level LTM gate: what every LTM synthetic variable's slots are
//! WORTH, step by step, on a fixture built to reproduce the ways an arm-level
//! change silently zeroes a score.
//!
//! Nothing else covers this. `clearn_residual_exactness` never enables LTM at
//! all; `clearn_ltm_var_count_guardrail` pins the emitted variable count and the
//! slot width, and neither of those moves when an arm's VALUE is rewritten. A
//! change that rewrote 149 C-LEARN LTM slots to zero passed every named C-LEARN
//! gate (GH #977). The characterization goldens are text, so they catch an arm
//! whose spelling changes and say nothing about an arm whose spelling is right
//! and whose value is not.
//!
//! Two halves, and the split is about run time rather than about coverage: this
//! file is the sub-second default-suite half, and
//! `simulate_ltm::clearn_ltm_slot_maxima_digest` is the `#[ignore]`d C-LEARN
//! half (~25 s release, well past the debug-build 3-minute cap in
//! `docs/dev/rust.md`).
//!
//! **A golden alone would not do this job**, and the reason is the standing
//! constraint in the root `CLAUDE.md`: a golden that pins an artifact is blind
//! to that artifact being stably absent, and a careless re-capture blesses a
//! vanished value. So every mechanism below carries a NAMED assertion that does
//! not depend on the golden's contents, and the golden's job is to catch
//! everything nobody thought to name.

use super::*;
use crate::datamodel;
use crate::test_common::TestProject;

/// The three ways a per-element link-score arm can be wrong about whether it is
/// a structural zero, in one model.
///
/// The target `growth[Region]` is a per-element (`Ast::Arrayed`) flow with no
/// EXCEPT default, so `ZeroSlotPolicy::OmitStructuralZero` is live for it and
/// each arm's fate is decided independently:
///
/// * `nyc` reads the link source `pop[nyc]` AND carries `TIME`. For the
///   `pop[nyc]` link this arm is live on both counts; for the OTHER links it is
///   the load-bearing row -- every occurrence of their source is frozen, and
///   the arm must still be materialized because `TIME` advances. This is the
///   mechanism that makes the naive "the source stayed frozen" collapse unsound
///   (5,035 of C-LEARN's 9,514 no-live-source arms are blocked solely by a live
///   `time()`; GH #1016). If a future relaxation drops it, this arm goes to zero
///   and the assertion below reds.
/// * `boston` reads `alt[a1]` -- a source in a dimension DISJOINT from the
///   target's, subscripted by a bare element name. This is the ACCESS SHAPE in
///   which GH #977's 322 unwrapped-bare-variable arms arise (raw
///   `[developing_b_countries]` against canonical
///   `[aggregated_regions.developing_b_countries]`), and what this row pins is
///   that such an arm is scored LIVE rather than claimed as a structural zero.
///
///   Be precise about what that is NOT: `alt[a1]` is the link's own source
///   here, so the occurrence match and the emitted tree agree about it, and
///   this fixture does not exhibit the raw-vs-canonical MISMATCH itself -- the
///   state where the shape match records no live reference while the wrap
///   leaves the source unwrapped. Sizing that defect is separate work; until it
///   is characterized there is no fixture that reproduces it, and claiming one
///   here would be the more expensive error than having none.
/// * `la` reads only the constant `base`. For every link into `growth` its
///   partial is provably `PREVIOUS(growth)`, so the slot is genuinely omitted
///   and must be EXACTLY `+0.0` -- the promise the omission makes.
///
/// `alt` is deliberately wired back through `pop_total = SUM(pop[*])` so that
/// `alt -> growth -> pop -> pop_total -> alt` is a genuine feedback loop.
/// Without that the exhaustive path emits no `alt[a1] -> growth` score at all
/// and every assertion below would fail on a missing variable rather than on a
/// wrong value -- which is how the first draft of this fixture was caught.
fn ltm_value_gate_project() -> datamodel::Project {
    TestProject::new("ltm_value_gate")
        .with_sim_time(0.0, 5.0, 1.0)
        .named_dimension("Region", &["nyc", "boston", "la"])
        .named_dimension("Alt", &["a1", "a2"])
        .aux("base", "2", None)
        .aux("pop_total", "SUM(pop[*])", None)
        .array_aux("alt[Alt]", "pop_total * 0.05 + TIME * 0.1")
        .array_flow_with_ranges(
            "growth[Region]",
            vec![
                ("nyc", "pop[nyc] * 0.01 + TIME * 0.002"),
                ("boston", "alt[a1] * 0.02"),
                ("la", "base * 0.03"),
            ],
        )
        .array_stock("pop[Region]", "10", &["growth"], &[], None)
        .build_datamodel()
}

/// One LTM synthetic variable's per-slot, per-step series.
struct LtmSlotSeries {
    name: String,
    /// Slot index within the variable (0 for a scalar).
    slot: usize,
    values: Vec<f64>,
}

/// Simulate `project` with LTM on and return every LTM synthetic variable's
/// slots, name-sorted then slot-ordered.
///
/// Widths come from each variable's own `dimensions` via the project's
/// dimension context rather than from a hand-written table, so a variable that
/// changes shape is read at its real width instead of being silently truncated.
fn ltm_slot_series(project: &datamodel::Project) -> Vec<LtmSlotSeries> {
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, project);
    use salsa::Setter;
    sync.project.set_ltm_enabled(&mut db).to(true);
    // Re-sync so every downstream query sees the flag (mirrors the other
    // db-level LTM fixtures in this crate).
    let sync = sync_from_datamodel(&db, project);
    sync.project.set_ltm_enabled(&mut db).to(true);

    let ltm = crate::db::model_ltm_variables(&db, sync.models["main"].source, sync.project);
    let dim_ctx = crate::db::project_dimensions_context(&db, sync.project);

    let compiled = crate::db::compile_project_incremental(&db, sync.project, "main")
        .expect("the value-gate fixture must compile with LTM enabled");
    let offsets = compiled.offsets.clone();
    let mut vm = crate::vm::Vm::new(compiled).expect("vm");
    vm.run_to_end().expect("run");
    let results = vm.into_results();

    let mut out: Vec<LtmSlotSeries> = Vec::new();
    let mut vars: Vec<&crate::db::LtmSyntheticVar> = ltm.vars.iter().collect();
    vars.sort_by(|a, b| a.name.cmp(&b.name));
    for var in vars {
        let Some(&base) = offsets.get(&crate::common::Ident::new(&var.name)) else {
            // A variable with no layout slot is a real defect, but it is
            // `model_ltm_fragment_diagnostics`' to report; this gate is about
            // values, so record it loudly rather than skipping it silently.
            panic!("LTM variable {} has no result offset", var.name);
        };
        let width: usize = var
            .dimensions
            .iter()
            .map(|d| {
                let canonical = crate::common::CanonicalDimensionName::from_raw(d);
                dim_ctx.get(&canonical).map(|dim| dim.len()).unwrap_or(1)
            })
            .product::<usize>()
            .max(1);
        for slot in 0..width {
            let off = base + slot;
            out.push(LtmSlotSeries {
                name: var.name.clone(),
                slot,
                values: (0..results.step_count)
                    .map(|s| results.data[s * results.step_size + off])
                    .collect(),
            });
        }
    }
    out
}

/// Render the slab as a stable text table. `{:.12e}` keeps the sign of zero
/// (`-0.000000000000e0`), which matters here: an omitted slot is `+0.0` and a
/// materialized trivial arm ending in `* SIGN(dx)` can be `-0.0`, and the two
/// must stay distinguishable in the pin.
fn render_slab(series: &[LtmSlotSeries]) -> String {
    let mut out = String::new();
    for s in series {
        out.push_str(&format!("{}[{}]", s.name, s.slot));
        for v in &s.values {
            out.push_str(&format!(" {:.12e}", v));
        }
        out.push('\n');
    }
    out
}

fn assert_value_golden(name: &str, actual: &str) {
    let path = format!(
        "{}/src/db/ltm_value_golden/{name}.txt",
        env!("CARGO_MANIFEST_DIR")
    );
    if std::env::var("UPDATE_LTM_VALUE_GOLDEN").is_ok() {
        let dir = format!("{}/src/db/ltm_value_golden", env!("CARGO_MANIFEST_DIR"));
        std::fs::create_dir_all(&dir).expect("create golden dir");
        std::fs::write(&path, actual).expect("write golden");
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!("missing golden {path}: {e}; run once with UPDATE_LTM_VALUE_GOLDEN=1 to capture")
    });
    if actual != expected {
        eprintln!("\n===== LTM VALUE GOLDEN MISMATCH ({name}): actual below =====");
        eprintln!("{actual}");
        eprintln!("===== end actual (expected in {path}) =====\n");
    }
    assert_eq!(actual, &expected, "LTM value golden mismatch for {name}");
}

/// Find one slot's series by variable name substring + slot index.
fn slot<'a>(series: &'a [LtmSlotSeries], name_contains: &str, slot: usize) -> &'a [f64] {
    let hits: Vec<&LtmSlotSeries> = series
        .iter()
        .filter(|s| s.name.contains(name_contains) && s.slot == slot)
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "expected exactly one slot matching {name_contains:?}[{slot}]; got {:?}",
        series
            .iter()
            .map(|s| format!("{}[{}]", s.name, s.slot))
            .collect::<Vec<_>>()
    );
    &hits[0].values
}

#[test]
fn ltm_slot_values_are_pinned_on_the_value_gate_fixture() {
    let series = ltm_slot_series(&ltm_value_gate_project());
    assert!(
        !series.is_empty(),
        "the fixture emitted no LTM slots at all, so this gate would pass vacuously"
    );
    assert_value_golden("value_gate", &render_slab(&series));
}

/// Mechanism 1: an arm with NO live source reference but a live `TIME` must be
/// materialized and must carry a non-zero value.
///
/// The `alt[a1] -> growth` link's `nyc` slot is that arm: `pop[nyc]` and `base`
/// are frozen for this link, `alt[a1]` does not appear in the `nyc` equation at
/// all, and what remains live is `TIME * 0.002`. Under the negative "the
/// source stayed frozen" criterion this slot would be dropped to zero; under
/// the positive predicate `TIME` is `BuiltinReach::Varying`, so the arm stays.
///
/// This assertion does not depend on the golden, which is the point: a careless
/// `UPDATE_LTM_VALUE_GOLDEN=1` re-capture would bless the zeroed slot, and this
/// would still red.
#[test]
fn a_time_bearing_arm_with_no_live_source_is_not_zeroed() {
    let series = ltm_slot_series(&ltm_value_gate_project());
    // Region declaration order: nyc=0, boston=1, la=2.
    let nyc = slot(&series, "link_score\u{205A}alt[a1]\u{2192}growth", 0);
    assert!(
        nyc.iter().any(|v| v.abs() > 1e-12 && v.is_finite()),
        "the TIME-bearing `nyc` arm was zeroed: an arm whose only live content \
         is a time-dependent builtin is NOT a structural zero; got {nyc:?}"
    );
}

/// Mechanism 2: an arm whose source is reached through a bare element name of a
/// DISJOINT dimension must be materialized and non-zero.
///
///
/// The `alt[a1] -> growth` link's `boston` slot is that arm -- `growth[boston]
/// = alt[a1] * 0.02`, the source subscripted by a raw element spelling, in a
/// dimension disjoint from the target's. It is the access shape GH #977's 322
/// unwrapped-bare-variable arms live in, and the guard is that an arm reached
/// this way is scored rather than omitted. See the fixture's rustdoc for what
/// this deliberately does not claim: it does not reproduce the raw-vs-canonical
/// mismatch, only the shape it occurs in.
#[test]
fn a_disjoint_dim_element_subscript_arm_is_not_zeroed() {
    let series = ltm_slot_series(&ltm_value_gate_project());
    let boston = slot(&series, "link_score\u{205A}alt[a1]\u{2192}growth", 1);
    assert!(
        boston.iter().any(|v| v.abs() > 1e-12 && v.is_finite()),
        "the `boston` arm, which reads its source through a bare element \
         subscript of a disjoint dimension, was zeroed; got {boston:?}"
    );
}

/// Mechanism 3, the other direction: a genuinely structural-zero arm must be
/// EXACTLY zero, at every step.
///
/// `growth[la] = base * 0.03` reads no link source and nothing that varies, so
/// every link into `growth` omits that slot and
/// `compiler::expand_arrayed_with_hoisting` lowers it to one
/// `AssignCurr(off, Const(0.0))`. Asserting exact equality rather than a
/// tolerance is what makes this catch the omission claiming a slot it should
/// not have: a near-zero residual would pass a tolerance and is precisely the
/// signal that the arm was NOT provably `PREVIOUS(target)`.
#[test]
fn a_structural_zero_arm_is_exactly_zero() {
    let series = ltm_slot_series(&ltm_value_gate_project());
    for source in ["alt[a1]\u{2192}growth", "pop[nyc]\u{2192}growth"] {
        let la = slot(&series, &format!("link_score\u{205A}{source}"), 2);
        for (step, v) in la.iter().enumerate() {
            assert_eq!(
                *v, 0.0,
                "{source} `la` slot must be an exact structural zero at every \
                 step; step {step} was {v}"
            );
        }
    }
}
