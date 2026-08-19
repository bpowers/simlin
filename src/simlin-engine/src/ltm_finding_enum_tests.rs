// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Union-graph enumeration/retention tests, split out of `ltm_finding_tests.rs`
//! to keep that file under the 6000-line per-file lint cap. Mounted as a
//! sibling `#[path]` module of `ltm_finding.rs` (like `ltm_finding_tests.rs`
//! itself), so `super::*` reaches `ltm_finding`'s items directly. A handful
//! of fixture helpers defined in `ltm_finding_tests.rs` are shared back here,
//! re-exported `pub(super)` from that file.

use super::tests::{solo_group_loop, synthetic_results};
use super::*;

/// Helper to build stock list from names (duplicated from
/// `ltm_finding_tests.rs`'s identically-named helper -- trivial enough that a
/// cross-module import is not worth the coupling).
fn stock_list(names: &[&str]) -> Vec<Ident<Canonical>> {
    names.iter().map(|n| Ident::new(n)).collect()
}

fn make_found_loop_with_scores(
    var_pairs: &[(&str, &str)],
    stocks: &[&str],
    polarity: LoopPolarity,
    avg_abs_score: f64,
    scores: Vec<(f64, f64)>,
) -> FoundLoop {
    let links: Vec<Link> = var_pairs
        .iter()
        .map(|(from, to)| Link {
            from: Ident::new(from),
            to: Ident::new(to),
            polarity: crate::ltm::LinkPolarity::Positive,
        })
        .collect();
    FoundLoop {
        loop_info: Loop {
            id: String::new(),
            links,
            stocks: stocks.iter().map(|s| Ident::new(s)).collect(),
            polarity,
            dimensions: vec![],
            slot_links: vec![],
        },
        scores,
        avg_abs_score,
        rel_scores: vec![],
        partition: None,
        polarity_confidence: 1.0,
    }
}

/// Create a CyclePartitions where all given stocks are in a single partition.
fn single_partition(stocks: &[&str]) -> CyclePartitions {
    let stock_idents: Vec<Ident<Canonical>> = stocks.iter().map(|s| Ident::new(s)).collect();
    let stock_partition: HashMap<Ident<Canonical>, usize> =
        stock_idents.iter().map(|s| (s.clone(), 0)).collect();
    CyclePartitions {
        partitions: vec![stock_idents],
        stock_partition,
    }
}

// ===========================================================================
// Union-graph circuit enumeration (the primary candidate generator; design:
// docs/design-plans/2026-08-17-ltm-discovery-exact.md).
// ===========================================================================

/// Build a scalar stock/flow/aux datamodel project for enumeration tests.
fn enum_test_project(vars: Vec<crate::datamodel::Variable>) -> crate::datamodel::Project {
    use crate::datamodel;
    datamodel::Project {
        name: "enum_test".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 5.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vars,
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    }
}

pub(super) fn enum_stock(
    ident: &str,
    eqn: &str,
    inflows: &[&str],
    outflows: &[&str],
) -> crate::datamodel::Variable {
    use crate::datamodel;
    datamodel::Variable::Stock(datamodel::Stock {
        ident: ident.to_string(),
        equation: datamodel::Equation::Scalar(eqn.to_string()),
        documentation: String::new(),
        units: None,
        inflows: inflows.iter().map(|s| s.to_string()).collect(),
        outflows: outflows.iter().map(|s| s.to_string()).collect(),
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    })
}

pub(super) fn enum_flow(ident: &str, eqn: &str) -> crate::datamodel::Variable {
    use crate::datamodel;
    datamodel::Variable::Flow(datamodel::Flow {
        ident: ident.to_string(),
        equation: datamodel::Equation::Scalar(eqn.to_string()),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    })
}

fn enum_aux(ident: &str, eqn: &str) -> crate::datamodel::Variable {
    use crate::datamodel;
    datamodel::Variable::Aux(datamodel::Aux {
        ident: ident.to_string(),
        equation: datamodel::Equation::Scalar(eqn.to_string()),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    })
}

/// Compile + LTM-simulate a datamodel project and run discovery with the
/// given candidate generator. Returns the result plus the raw Results (so
/// tests can mutate score series and re-discover).
pub(super) fn discover_project(
    project: &crate::datamodel::Project,
    candidate_gen: CandidateGen,
) -> DiscoveryResult {
    let (results, ctx) = ltm_simulate(project);
    discover_with(&results, &ctx, candidate_gen)
}

/// The compiled context discovery needs alongside the Results.
struct EnumTestCtx {
    causal_graph: CausalGraph,
    stocks: Vec<Ident<Canonical>>,
    ltm_vars: Vec<LtmSyntheticVar>,
    dims: Vec<datamodel::Dimension>,
    expansion: LinkExpansionContext,
}

fn ltm_simulate(project: &crate::datamodel::Project) -> (Results, EnumTestCtx) {
    use salsa::Setter;
    let mut db = crate::db::SimlinDb::default();
    let sync = crate::db::sync_from_datamodel_incremental(&mut db, project, None);
    let sp = sync.project;
    sp.set_ltm_enabled(&mut db).to(true);
    sp.set_ltm_discovery_mode(&mut db).to(true);
    let source_model = *sp.models(&db).get("main").unwrap();
    let compiled = crate::db::compile_project_incremental(&db, sp, "main").unwrap();
    let mut vm = crate::vm::Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();
    let element_edges = crate::db::model_element_causal_edges(&db, source_model, sp);
    let causal_graph = crate::db::causal_graph_from_element_edges(element_edges);
    let stocks: Vec<Ident<Canonical>> = element_edges
        .stocks
        .iter()
        .map(|s| Ident::new(s.as_str()))
        .collect();
    let ltm = crate::db::model_ltm_variables(&db, source_model, sp);
    let dm_dims = crate::db::project_datamodel_dims(&db, sp);
    let expansion = crate::analysis::build_link_expansion_context(&db, source_model, sp);
    (
        results,
        EnumTestCtx {
            causal_graph,
            stocks,
            ltm_vars: ltm.vars.clone(),
            dims: dm_dims.clone(),
            expansion,
        },
    )
}

fn discover_with(
    results: &Results,
    ctx: &EnumTestCtx,
    candidate_gen: CandidateGen,
) -> DiscoveryResult {
    discover_loops_with_candidate_gen(
        results,
        &ctx.causal_graph,
        &ctx.stocks,
        &ctx.ltm_vars,
        &ctx.dims,
        &ctx.expansion,
        &SubModelOutputPorts::new(),
        None,
        candidate_gen,
    )
    .unwrap()
}

/// AC2.1: on the classic logistic model both candidate generators reach every
/// loop, and when they do their answers are IDENTICAL -- same loop set, same
/// per-step scores, and same relative scores.
///
/// The relative-score equality is the load-bearing part and it holds only
/// because this model's discovered set IS its universe: the enumeration path
/// normalizes against full-universe denominators while the fallback path
/// normalizes against the loops it found, so any loop the fallback missed
/// would show up as a relative-score difference rather than as a missing loop.
#[test]
fn enumeration_and_fallback_agree_on_a_simple_model() {
    let project = enum_test_project(vec![
        enum_stock("population", "100", &["births"], &["deaths"]),
        enum_flow("births", "population * 0.1"),
        enum_flow("deaths", "population * population * 0.0001"),
    ]);
    let auto = discover_project(&project, CandidateGen::Auto);
    let fallback = discover_project(
        &project,
        CandidateGen::FallbackOnly(FallbackConfig::DEFAULT),
    );

    assert!(auto.enumeration_complete, "tiny model must enumerate fully");
    assert!(
        !fallback.enumeration_complete,
        "a pinned fallback never claims the enumeration's completeness"
    );

    // Identical loop sets, keyed by each loop's sorted link-from set.
    let key = |r: &DiscoveryResult| -> Vec<Vec<String>> {
        let mut keys: Vec<Vec<String>> = r
            .loops
            .iter()
            .map(|l| {
                let mut nodes: Vec<String> = l
                    .loop_info
                    .links
                    .iter()
                    .map(|k| k.from.as_str().to_string())
                    .collect();
                nodes.sort();
                nodes
            })
            .collect();
        keys.sort();
        keys
    };
    assert_eq!(
        key(&auto),
        key(&fallback),
        "loop sets must match across generators"
    );
    assert_eq!(auto.loops.len(), 2);

    // Scores agree loop-for-loop (match by id: both paths assign
    // content-derived ids, so equal loop sets get equal ids).
    for al in &auto.loops {
        let dl = fallback
            .loops
            .iter()
            .find(|l| l.loop_info.id == al.loop_info.id)
            .expect("matching loop id");
        assert_eq!(
            al.scores, dl.scores,
            "loop {} scores differ",
            al.loop_info.id
        );
        assert_eq!(
            al.rel_scores, dl.rel_scores,
            "loop {} rel scores differ (full universe == discovered set here)",
            al.loop_info.id
        );
    }
}

/// A one-variable PREVIOUS self-reference (the `SAMPLE IF TRUE(...)`-latch
/// shape: C-LEARN carries 49 ever-active self-edges of this kind) is NOT a
/// feedback loop: a self-edge can never be part of an elementary cycle of
/// length >= 2, and both LTM surfaces agree that one variable referencing
/// itself is not feedback (`ltm::indexed`'s `circuit.len() > 1` contract,
/// `CausalGraph::order_variable_cycle`'s `vars.len() < 2` rejection). Neither
/// generator reports it; both report only the population growth loop.
#[test]
fn a_previous_self_latch_is_not_reported_as_a_loop() {
    let project = enum_test_project(vec![
        enum_stock("population", "100", &["births"], &[]),
        enum_flow("births", "population * 0.1"),
        enum_aux(
            "smoothed",
            "PREVIOUS(smoothed, 100) * 0.9 + population * 0.1",
        ),
    ]);
    let auto = discover_project(&project, CandidateGen::Auto);
    let fallback = discover_project(
        &project,
        CandidateGen::FallbackOnly(FallbackConfig::DEFAULT),
    );

    assert!(auto.enumeration_complete);
    let node_sets = |r: &DiscoveryResult| -> Vec<Vec<String>> {
        r.loops
            .iter()
            .map(|l| {
                let mut nodes: Vec<String> = l
                    .loop_info
                    .links
                    .iter()
                    .map(|k| k.from.as_str().to_string())
                    .collect();
                nodes.sort();
                nodes
            })
            .collect()
    };
    assert_eq!(
        node_sets(&auto),
        vec![vec!["births".to_string(), "population".to_string()]],
        "only the population growth loop is a feedback loop here"
    );
    assert_eq!(
        node_sets(&auto),
        node_sets(&fallback),
        "both generators agree a self-latch is not a loop"
    );
    // AC1.1: no reported loop is a single link, under either generator.
    for r in [&auto, &fallback] {
        assert!(r.loops.iter().all(|l| l.loop_info.links.len() >= 2));
    }
}

/// Self-filtering: a cycle whose links are never simultaneously nonzero has
/// loop score exactly 0 at every step, and neither generator may report it --
/// pinned by overwriting one loop's two link-score series with disjoint
/// activity windows post-simulation.
#[test]
fn staggered_activity_cycle_is_not_reported_by_either_generator() {
    let project = enum_test_project(vec![
        enum_stock("population", "100", &["births"], &["deaths"]),
        enum_flow("births", "population * 0.1"),
        enum_flow("deaths", "population * population * 0.0001"),
    ]);
    let (mut results, ctx) = ltm_simulate(&project);

    // Overwrite the births-loop link scores with disjoint activity:
    // population->births active only at steps 1-2, births->population only
    // at steps 3+. The deaths loop is left untouched.
    let score_offset = |results: &Results, from: &str, to: &str| -> usize {
        let name = format!("$\u{205A}ltm\u{205A}link_score\u{205A}{from}\u{2192}{to}");
        *results
            .offsets
            .get(&Ident::<Canonical>::new(&name))
            .unwrap_or_else(|| panic!("missing link score {name}"))
    };
    let p_to_b = score_offset(&results, "population", "births");
    let b_to_p = score_offset(&results, "births", "population");
    let (step_size, step_count) = (results.step_size, results.step_count);
    for step in 1..step_count {
        let (pb, bp) = if step <= 2 { (1.0, 0.0) } else { (0.0, 1.0) };
        results.data[step * step_size + p_to_b] = pb;
        results.data[step * step_size + b_to_p] = bp;
    }

    let auto = discover_with(&results, &ctx, CandidateGen::Auto);
    let fallback = discover_with(
        &results,
        &ctx,
        CandidateGen::FallbackOnly(FallbackConfig::DEFAULT),
    );
    assert!(auto.enumeration_complete);
    for r in [&auto, &fallback] {
        assert_eq!(
            r.loops.len(),
            1,
            "only the deaths loop is ever simultaneously active"
        );
        assert!(
            r.loops[0]
                .loop_info
                .links
                .iter()
                .any(|k| k.from.as_str() == "deaths"),
            "the surviving loop is the deaths loop"
        );
    }
}

/// A tripped enumeration budget falls back to the shortest-path sweep under
/// the DEFAULT weight: the result is exactly what pinning the fallback gives,
/// with `enumeration_complete == false`.
#[test]
fn enumeration_budget_trip_falls_back_to_the_shortest_path_sweep() {
    let project = enum_test_project(vec![
        enum_stock("population", "100", &["births"], &["deaths"]),
        enum_flow("births", "population * 0.1"),
        enum_flow("deaths", "population * population * 0.0001"),
    ]);
    let pinned = discover_project(
        &project,
        CandidateGen::FallbackOnly(FallbackConfig::DEFAULT),
    );

    let _guard = EnumBudgetGuard::new(1, u64::MAX, u64::MAX);
    let auto = discover_project(&project, CandidateGen::Auto);
    assert!(
        !auto.enumeration_complete,
        "a 1-circuit budget cannot complete a 2-loop model"
    );
    assert_eq!(auto.loops.len(), pinned.loops.len());
    for (a, d) in auto.loops.iter().zip(pinned.loops.iter()) {
        assert_eq!(a.loop_info.id, d.loop_info.id);
        assert_eq!(a.scores, d.scores);
    }
    // The enumerator ran but did not COMPLETE, so there is no universe to
    // report even though the generator that was asked for was the exact one:
    // the partial circuit list is discarded, not published as a universe.
    assert_eq!(
        auto.universe_loops, None,
        "an abandoned enumeration publishes no universe count"
    );
}

/// The logistic fixture every counter test below shares: one stock with a
/// reinforcing births loop and a balancing deaths loop, so the universe is
/// exactly two elementary cycles and both carry real mass.
fn two_loop_logistic_project() -> crate::datamodel::Project {
    enum_test_project(vec![
        enum_stock("population", "100", &["births"], &["deaths"]),
        enum_flow("births", "population * 0.1"),
        enum_flow("deaths", "population * population * 0.0001"),
    ])
}

/// AC6.1: an exact run reports the size of the candidate universe it
/// enumerated and how many of those loops passed retention, alongside the
/// loops themselves. On a model where nothing is dropped anywhere the three
/// counts agree -- which is what makes the fixtures below, where they
/// deliberately DISAGREE, readable.
#[test]
fn an_exact_run_reports_its_universe_and_retained_counts() {
    let auto = discover_project(&two_loop_logistic_project(), CandidateGen::Auto);

    assert!(auto.enumeration_complete);
    assert_eq!(
        auto.universe_loops,
        Some(2),
        "the universe is exactly the births and deaths cycles"
    );
    assert_eq!(auto.retained_loops, 2, "both loops carry real mass");
    assert_eq!(
        auto.loops.len(),
        auto.retained_loops,
        "an uncapped run reports every retained loop"
    );
}

/// `retained_loops` counts retention survivors BEFORE the `MAX_LOOPS` cap, so
/// a caller can tell "this model has two loops and you are being shown one"
/// apart from "this model has one loop". `loops.len()` alone cannot say that,
/// and neither can `universe_loops` (which counts candidates, most of which a
/// real model's retention drops).
#[test]
fn a_capped_run_reports_the_pre_cap_retained_count() {
    let project = two_loop_logistic_project();
    let _guard = MaxLoopsGuard::new(1);
    let auto = discover_project(&project, CandidateGen::Auto);

    assert!(auto.enumeration_complete);
    assert_eq!(auto.universe_loops, Some(2));
    assert_eq!(auto.retained_loops, 2, "both loops pass retention");
    assert_eq!(auto.loops.len(), 1, "the cap reports only one of them");
}

/// The fallback is a declared SAMPLE, so it has no universe to report:
/// `universe_loops` is `None` there while `retained_loops` still counts the
/// sample's retention survivors.
///
/// Three of the four arms `universe_loops` can take are pinned by name: the
/// completed enumeration (`Some(count)`, above), the abandoned one (`None`,
/// in `enumeration_budget_trip_falls_back_to_the_shortest_path_sweep`), and
/// the pinned fallback (here). The fourth -- the degenerate early returns,
/// where discovery bails before either generator runs -- is
/// `a_model_with_no_links_reports_an_empty_universe`.
#[test]
fn the_fallback_reports_no_universe() {
    let fallback = discover_project(
        &two_loop_logistic_project(),
        CandidateGen::FallbackOnly(FallbackConfig::DEFAULT),
    );

    assert!(!fallback.enumeration_complete);
    assert_eq!(
        fallback.universe_loops, None,
        "a sample has no universe to report"
    );
    assert!(!fallback.loops.is_empty());
    assert_eq!(fallback.retained_loops, fallback.loops.len());
}

/// A model with no causal links at all: discovery bails before either
/// generator runs. The universe is EMPTY rather than unknown -- the arm that
/// keeps `universe_loops.is_some()` equal to `enumeration_complete` on every
/// path, so a caller can read the pair as one statement.
#[test]
fn a_model_with_no_links_reports_an_empty_universe() {
    let project = enum_test_project(vec![enum_aux("constant", "42")]);
    let auto = discover_project(&project, CandidateGen::Auto);

    assert!(auto.loops.is_empty());
    assert!(auto.enumeration_complete);
    assert_eq!(auto.universe_loops, Some(0));
    assert_eq!(auto.retained_loops, 0);
}

/// AC2.4's diamond -- two parallel paths sharing an entry stock and an exit
/// aux -- is the shape one shortest-path tree structurally cannot express: it
/// holds ONE route per node, so of two parallel routes to the shared exit only
/// the cheaper is a tree path, and the stock's single in-edge closes exactly
/// one cycle.
///
/// Closing on EVERY edge rather than only the seed's in-edges is what recovers
/// the sibling: the edge `y -> z` is closed by the forward tree path to `y`
/// and the reverse tree path from `z`, neither of which needs `y` to lie on
/// the forward route to `z`. Both generators therefore report both arms here.
/// The per-family mechanism -- and that the cheap in-edge family still finds
/// exactly one arm -- is pinned in `ltm_finding_fallback_tests.rs`'s
/// `every_edge_closures_recover_both_arms_of_a_diamond`.
#[test]
fn a_diamond_is_enumerated_whole_and_recovered_by_the_fallback() {
    let project = enum_test_project(vec![
        enum_stock("s", "100", &["f"], &[]),
        enum_aux("x", "s * 0.5"),
        enum_aux("y", "s * 0.4"),
        enum_aux("z", "x + y"),
        enum_flow("f", "z * 0.1"),
    ]);
    let node_sets = |r: &DiscoveryResult| -> Vec<Vec<String>> {
        let mut sets: Vec<Vec<String>> = r
            .loops
            .iter()
            .map(|l| {
                let mut nodes: Vec<String> = l
                    .loop_info
                    .links
                    .iter()
                    .map(|k| k.from.as_str().to_string())
                    .collect();
                nodes.sort();
                nodes
            })
            .collect();
        sets.sort();
        sets
    };

    let auto = discover_project(&project, CandidateGen::Auto);
    assert!(auto.enumeration_complete);
    assert_eq!(
        node_sets(&auto),
        vec![
            vec![
                "f".to_string(),
                "s".to_string(),
                "x".to_string(),
                "z".to_string()
            ],
            vec![
                "f".to_string(),
                "s".to_string(),
                "y".to_string(),
                "z".to_string()
            ],
        ],
        "the enumerator finds both arms of the diamond"
    );

    let fallback = discover_project(
        &project,
        CandidateGen::FallbackOnly(FallbackConfig::DEFAULT),
    );
    assert!(!fallback.enumeration_complete);
    assert_eq!(
        node_sets(&fallback),
        node_sets(&auto),
        "closing on every edge recovers the arm no single tree expresses"
    );
}

/// AC1.3: a feedback loop with NO stock -- two auxes reading each other one
/// step back -- is a real loop and enumeration reports it, in a Solo
/// normalization group, ranked after every competing loop.
///
/// The fallback reaches it too, because its seed policy adds one node per
/// non-trivial component holding no stock. A purely stock-seeded search cannot
/// -- there is no stock on the cycle to start from -- which is what that
/// policy exists to close, and this fixture pins both arms. The per-policy
/// seed SETS are pinned in `ltm_finding_fallback_tests.rs`'s
/// `each_seed_policy_selects_the_nodes_it_names`.
#[test]
fn a_stockless_two_node_cycle_is_found_by_both_generators_and_ranks_last() {
    let project = enum_test_project(vec![
        enum_stock("population", "100", &["births"], &["deaths"]),
        enum_flow("births", "population * 0.1"),
        enum_flow("deaths", "population * population * 0.0001"),
        // State held in a PREVIOUS lag rather than in a stock: `lag_a` and
        // `lag_b` each read the other's previous value, so the cycle is real
        // and scorable while touching no stock at all.
        enum_aux("lag_a", "PREVIOUS(lag_b, 0) * 0.5 + population * 0.01"),
        enum_aux("lag_b", "PREVIOUS(lag_a, 0) * 0.5 + 1"),
    ]);
    let node_sets = |r: &DiscoveryResult| -> Vec<Vec<String>> {
        r.loops
            .iter()
            .map(|l| {
                let mut nodes: Vec<String> = l
                    .loop_info
                    .links
                    .iter()
                    .map(|k| k.from.as_str().to_string())
                    .collect();
                nodes.sort();
                nodes
            })
            .collect()
    };

    let auto = discover_project(&project, CandidateGen::Auto);
    assert!(auto.enumeration_complete);
    let stockless = vec!["lag_a".to_string(), "lag_b".to_string()];
    let auto_sets = node_sets(&auto);
    assert!(
        auto_sets.contains(&stockless),
        "the stockless lag cycle must be reported; got {auto_sets:?}"
    );

    // Solo: no stock resolves to a cycle partition, so it is its own
    // denominator and carries no partition index -- and the competitive-first
    // ranking puts it after the two population loops, which really do compete.
    let position = auto_sets
        .iter()
        .position(|s| *s == stockless)
        .expect("just asserted present");
    assert_eq!(
        auto.loops[position].partition, None,
        "a loop touching no stock resolves to no parent-level partition"
    );
    assert_eq!(
        position,
        auto_sets.len() - 1,
        "the Solo loop ranks after every competing loop; got {auto_sets:?}"
    );

    let fallback = discover_project(
        &project,
        CandidateGen::FallbackOnly(FallbackConfig::DEFAULT),
    );
    assert!(
        node_sets(&fallback).contains(&stockless),
        "the default seed policy adds a representative of each stockless \
         component, so the fallback reaches this cycle; got {:?}",
        node_sets(&fallback)
    );

    // The stock-seeded arm is what cannot: same fixture, one knob moved, so
    // what the wider policy buys is stated rather than assumed.
    let stock_seeded = discover_project(
        &project,
        CandidateGen::FallbackOnly(FallbackConfig {
            seeds: FallbackSeeds::Stocks,
            ..FallbackConfig::DEFAULT
        }),
    );
    assert!(
        !node_sets(&stock_seeded).contains(&stockless),
        "a cycle through no stock is unreachable from stock seeds"
    );
}

/// A model with NO parent-level stocks at all (not merely a stockless loop
/// inside a model that has stocks elsewhere, the sibling test above) is not
/// an empty universe: `a` and `b` are both auxes, and the `PREVIOUS` lag
/// between them IS state, so the a<->b cycle is a real feedback loop that
/// must be discovered under both generators. Before the fix,
/// `discover_loops_with_deadlines`'s `stocks.is_empty()` early return
/// declared this universe empty by construction, without ever asking either
/// generator.
#[test]
fn a_model_with_no_stocks_at_all_still_reports_its_previous_lag_loop() {
    let project = enum_test_project(vec![
        enum_aux("a", "PREVIOUS(b, 1) * 0.5 + 1"),
        enum_aux("b", "PREVIOUS(a, 0) * 0.5"),
    ]);

    let auto = discover_project(&project, CandidateGen::Auto);
    assert!(
        auto.enumeration_complete,
        "the enumerator needs no stock seeds at all"
    );
    assert_eq!(
        auto.universe_loops,
        Some(1),
        "one circuit, no dedup twins, no stitching"
    );
    assert_eq!(
        auto.loops.len(),
        1,
        "the a<->b PREVIOUS loop; got {:?}",
        auto.loops
            .iter()
            .map(|l| l
                .loop_info
                .links
                .iter()
                .map(|k| k.from.as_str())
                .collect::<Vec<_>>())
            .collect::<Vec<_>>()
    );
    assert!(
        auto.loops[0].loop_info.stocks.is_empty(),
        "the loop touches no stock -- there are none in the model"
    );
    assert_eq!(
        auto.loops[0].partition, None,
        "a loop touching no stock resolves to no parent-level partition -- \
         a Solo group, per AC1.3"
    );

    let fallback = discover_project(
        &project,
        CandidateGen::FallbackOnly(FallbackConfig::DEFAULT),
    );
    assert!(
        !fallback.enumeration_complete,
        "FallbackOnly must never claim the enumeration ran"
    );
    assert_eq!(
        fallback.loops.len(),
        1,
        "the default seed policy (StocksAndStocklessSccs) seeds a \
         representative of the stockless SCC directly, with no stock seeds \
         to fall back on at all; got {:?}",
        fallback
            .loops
            .iter()
            .map(|l| l
                .loop_info
                .links
                .iter()
                .map(|k| k.from.as_str())
                .collect::<Vec<_>>())
            .collect::<Vec<_>>()
    );
}

/// AC2.2: the budget SPLIT is what keeps a spent enumeration from spending the
/// whole budget. With the enumeration deadline already past, discovery still
/// runs the fallback and returns real loops; and when the fallback's own
/// deadline then expires mid-sweep, the loops found before it are kept and
/// `truncated` says the report is partial.
///
/// Driven through the deadline + clock seam rather than a `Duration`, so
/// "expired during enumeration, alive during the fallback, then expired part
/// way through the sweep" is a stated fact rather than a race.
#[test]
fn an_expired_enumeration_deadline_still_yields_the_fallbacks_loops() {
    let project = enum_test_project(vec![
        enum_stock("population", "100", &["births"], &["deaths"]),
        enum_flow("births", "population * 0.1"),
        enum_flow("deaths", "population * population * 0.0001"),
    ]);
    let (results, ctx) = ltm_simulate(&project);
    let discover = |deadlines: Deadlines, clock: &mut dyn Clock| {
        discover_loops_with_deadlines(
            &results,
            &ctx.causal_graph,
            &ctx.stocks,
            &ctx.ltm_vars,
            &ctx.dims,
            &ctx.expansion,
            &SubModelOutputPorts::new(),
            deadlines,
            CandidateGen::Auto,
            clock,
        )
        .unwrap()
    };

    // Arm 1: the enumeration deadline is spent, the fallback has all the time
    // it needs. Discovery is a sample, but a COMPLETE one over every step.
    let mut clock = ScriptedClock::new(usize::MAX);
    let spent = clock.deadline() - Duration::from_secs(3600);
    let found = discover(
        Deadlines {
            enumeration: Some(spent),
            fallback: None,
        },
        &mut clock,
    );
    assert!(
        !found.enumeration_complete,
        "the enumeration was abandoned before it could complete"
    );
    assert!(
        !found.truncated,
        "an unbudgeted fallback covers every saved step"
    );
    assert_eq!(
        found.loops.len(),
        2,
        "the fallback still recovers both population loops"
    );

    // Arm 2: the fallback then expires part way. Everything it had already
    // closed survives -- partial results, not nothing -- and `truncated` says
    // so.
    //
    // Read schedule: 1 abandons `ActivityGraph::build`; 2 is step 1's
    // top-of-step check, and step 1 costs nothing more because its link scores
    // are startup-degenerate (`PREVIOUS` has no prior value yet), leaving the
    // stock on no cycle and its search skipped without a read; 3 and 4 are
    // step 2's top-of-step and pre-search checks, and step 2 is where the
    // loops are found. So read 5 -- step 3's top-of-step check -- is the first
    // expiry that leaves work already done.
    let mut clock = ScriptedClock::new(5);
    let live = clock.deadline();
    let spent = live - Duration::from_secs(3600);
    let found = discover(
        Deadlines {
            enumeration: Some(spent),
            fallback: Some(live),
        },
        &mut clock,
    );
    assert_eq!(
        clock.reads, 5,
        "the read schedule this test is calibrated to"
    );
    assert!(!found.enumeration_complete);
    assert!(found.truncated, "the fallback ran out of time mid-sweep");
    assert!(
        !found.loops.is_empty(),
        "a fallback that processed at least one step reports the loops it \
         closed there, not nothing"
    );
}

/// External (full-universe) denominators shrink relative scores relative to
/// the discovered-set-only denominators: rank_and_filter must use them for
/// Partition groups when provided.
#[test]
fn external_totals_are_used_for_partition_denominators() {
    let stock: Ident<Canonical> = Ident::new("population");
    let partitions = CyclePartitions {
        partitions: vec![vec![stock.clone()]],
        stock_partition: [(stock.clone(), 0usize)].into_iter().collect(),
    };
    let make_loop = || FoundLoop {
        loop_info: Loop {
            id: String::new(),
            links: vec![Link {
                from: Ident::new("population"),
                to: Ident::new("births"),
                polarity: LinkPolarity::Positive,
            }],
            stocks: vec![stock.clone()],
            polarity: LoopPolarity::Reinforcing,
            dimensions: vec![],
            slot_links: vec![],
        },
        scores: vec![(0.0, 1.0), (1.0, 1.0)],
        avg_abs_score: 1.0,
        rel_scores: Vec::new(),
        partition: None,
        polarity_confidence: 1.0,
    };

    // Without external totals: the loop is alone, denominator == its own
    // series, rel == 1.0.
    let mut loops = vec![make_loop()];
    rank_and_filter(&mut loops, &partitions, None);
    assert_eq!(loops[0].rel_scores, vec![1.0, 1.0]);

    // With universe totals carrying the full universe's mass (say 4.0 per
    // step, three unreported sibling loops' worth), rel == 0.25.
    let universe = UniverseStats {
        totals: [(0usize, vec![4.0, 4.0])].into_iter().collect(),
        loop_counts: [(0usize, 4usize)].into_iter().collect(),
    };
    let mut loops = vec![make_loop()];
    rank_and_filter(&mut loops, &partitions, Some(&universe));
    assert_eq!(loops[0].rel_scores, vec![0.25, 0.25]);
}

/// Hand-built `Results` for the enumerator / retention unit tests: one result
/// slot per link offset (so `offset == slot`), `step_count` saved steps, values
/// supplied row-major as `data[step * n_offsets + offset]`.
///
/// Step 0 is whatever the caller left there, and every fixture below leaves it
/// zero: every link score's `TIME = INITIAL_TIME` guard arm emits the literal
/// constant `0` there (`ltm_augment::link_score_guard_form_with_numerator`),
/// so it carries no signal in a real run -- confirmed on World3 and C-LEARN,
/// where every union edge's step-0 value is exactly 0.
pub(super) fn enum_results(n_offsets: usize, step_count: usize, data: Vec<f64>) -> Results {
    assert_eq!(
        data.len(),
        n_offsets * step_count,
        "fixture data is row-major"
    );
    Results {
        offsets: HashMap::new(),
        data: data.into_boxed_slice(),
        step_size: n_offsets,
        step_count,
        specs: crate::results::Specs {
            start: 0.0,
            stop: (step_count - 1) as f64,
            dt: 1.0,
            save_step: 1.0,
            method: crate::results::Method::Euler,
            n_chunks: step_count,
        },
        is_vensim: false,
    }
}

/// The sorted node-name sets of the retention survivors, for readable
/// assertions about which circuits were kept.
pub(super) fn survivor_node_sets(
    outcome: &super::enum_gen::RetentionOutcome,
    candidates: &super::enum_gen::EnumeratedCandidates,
    activity: &super::enum_gen::ActivityGraph,
    search: &IndexedSearch,
) -> Vec<Vec<String>> {
    outcome
        .survivors
        .iter()
        .map(|&ci| {
            let mut names: Vec<String> = activity
                .circuit_nodes(candidates.circuit(ci))
                .iter()
                .map(|&n| search.idents[n as usize].as_str().to_string())
                .collect();
            names.sort();
            names
        })
        .collect()
}

/// Direct unit coverage of the retention pass's arms: a loop below
/// MIN_CONTRIBUTION of its partition's total at every step is dropped; a
/// module-traversing loop is scored on its REPORTED (override) series through
/// the same bound/confirm gate as every other circuit and banks that series'
/// mass into the totals, rather than being kept unconditionally; a Solo
/// (no-stock) loop is kept iff ever active. The below-threshold module arm
/// (a module circuit dropped because its OWN override series is small) is
/// `a_module_circuit_below_threshold_via_its_override_series_is_dropped_by_retention`.
#[test]
fn retention_pass_drops_below_threshold_keeps_module_and_solo() {
    // Hand-built results: 2 steps (step 0 unused), edges laid out flat.
    //   big loop   a<->b: |product| = 1.0 at step 1
    //   tiny loop  a<->c: |product| = 1e-8 at step 1 (below 0.1% of total)
    //   solo loop  d<->e: no stock, active at step 1
    //   dead loop  f<->g: no stock, never active -- wait: never-active edges
    //     are pruned from the union graph, so it cannot be enumerated;
    //     instead make it active but let its own product be 0 via one zero
    //     edge... a zero edge is inactive too. The "Solo never active" arm is
    //     unreachable through enumeration (activity pruning guarantees >= 1
    //     active step), so it is not constructed here; the arm exists for
    //     defense in depth.
    let link_offsets: Vec<LinkOffset> = vec![
        ((Ident::new("a"), Ident::new("b")), 0),
        ((Ident::new("b"), Ident::new("a")), 1),
        ((Ident::new("a"), Ident::new("c")), 2),
        ((Ident::new("c"), Ident::new("a")), 3),
        ((Ident::new("d"), Ident::new("e")), 4),
        ((Ident::new("e"), Ident::new("d")), 5),
    ];
    let n_offsets = 6;
    let step_count = 2;
    let mut data = vec![0.0f64; n_offsets * step_count];
    // step 1 values:
    data[n_offsets] = 1.0; // a->b
    data[n_offsets + 1] = 1.0; // b->a
    data[n_offsets + 2] = 1e-4; // a->c
    data[n_offsets + 3] = 1e-4; // c->a  (product 1e-8)
    data[n_offsets + 4] = 0.5; // d->e
    data[n_offsets + 5] = 0.5; // e->d
    let results = enum_results(n_offsets, step_count, data);
    let stocks = stock_list(&["a"]);
    let search = IndexedSearch::build(&link_offsets, &stocks);
    let activity = super::enum_gen::ActivityGraph::build(&search, &results, None, &mut SystemClock)
        .expect("an unbudgeted build never abandons");
    let candidates = super::enum_gen::enumerate_active_circuits(&activity, None, &mut SystemClock);
    assert!(candidates.complete);
    assert_eq!(candidates.len(), 3, "three 2-cycles");

    // Node metadata: `a` is a stock in partition 0; no modules.
    let stock_partition: Vec<Option<usize>> = search
        .idents
        .iter()
        .map(|id| (id.as_str() == "a").then_some(0))
        .collect();
    let no_modules = vec![false; search.idents.len()];
    let no_agg_nodes = vec![false; search.idents.len()];
    let outcome = super::enum_gen::retain_circuits(
        &candidates,
        &activity,
        &stock_partition,
        &no_modules,
        &no_agg_nodes,
        &mut |_, _, _| None,
        None,
        &mut SystemClock,
    )
    .unwrap();
    let survivor_nodes = survivor_node_sets(&outcome, &candidates, &activity, &search);
    assert!(
        survivor_nodes.contains(&vec!["a".to_string(), "b".to_string()]),
        "the dominant loop survives"
    );
    assert!(
        survivor_nodes.contains(&vec!["d".to_string(), "e".to_string()]),
        "the Solo loop survives (rel score is 1 by construction)"
    );
    assert!(
        !survivor_nodes.contains(&vec!["a".to_string(), "c".to_string()]),
        "the 1e-8-vs-1.0 loop peaks far below MIN_CONTRIBUTION and is dropped"
    );
    // Totals carry BOTH partition loops' mass (the dropped one included).
    let totals = &outcome.partition_totals[&0];
    assert!((totals[1] - (1.0 + 1e-8)).abs() < 1e-12);

    // Same fixture with `c` marked as a module node, and an override series
    // for the a->c row (the edge whose target is the module) well ABOVE its
    // raw 1e-4 value (a real per-exit-port pathway score can be any
    // magnitude, unrelated to the composite the raw row carries). Only that
    // ONE row is replaced -- the circuit's OTHER row (c->a, whose target is
    // not a module) keeps contributing its own raw value, exactly like any
    // other link in the loop -- so the circuit's score is
    // `override(a->c) * raw(c->a)`, not the override series alone. Picking
    // `1e4` as the override, against the fixture's raw `c->a = 1e-4`, makes
    // that product exactly `1.0`: well above MIN_CONTRIBUTION, and clean to
    // assert against.
    let modules: Vec<bool> = search.idents.iter().map(|id| id.as_str() == "c").collect();
    let c_id = search
        .idents
        .iter()
        .position(|id| id.as_str() == "c")
        .expect("c is a graph node") as u32;
    let override_series = vec![0.0, 1e4]; // step 0 unused; step 1: 1e4 * raw(c->a) = 1e4 * 1e-4 = 1.0
    let no_agg_nodes = vec![false; search.idents.len()];
    let outcome = super::enum_gen::retain_circuits(
        &candidates,
        &activity,
        &stock_partition,
        &modules,
        &no_agg_nodes,
        &mut |_from, module, _next| {
            (module == c_id).then(|| (Rc::new(override_series.clone()), (0, override_series.len())))
        },
        None,
        &mut SystemClock,
    )
    .unwrap();
    let survivor_nodes = survivor_node_sets(&outcome, &candidates, &activity, &search);
    assert!(
        survivor_nodes.contains(&vec!["a".to_string(), "c".to_string()]),
        "the module circuit's OVERRIDE-substituted score (1.0) clears \
         MIN_CONTRIBUTION of the partition total, even though its raw \
         product (1e-8) would not"
    );
    assert!(
        survivor_nodes.contains(&vec!["a".to_string(), "b".to_string()])
            && survivor_nodes.contains(&vec!["d".to_string(), "e".to_string()]),
        "the module arm does not disturb the non-module and Solo survivors"
    );
    let totals = &outcome.partition_totals[&0];
    assert!(
        (totals[1] - (1.0 + 1.0)).abs() < 1e-9,
        "the partition total banks the module circuit's OVERRIDE-substituted \
         mass (1e4 * raw(c->a) = 1.0), not its raw composite product (1e-8): \
         got {}",
        totals[1]
    );
}

/// A module-traversing circuit is not kept unconditionally any more: when its
/// OWN override series is below `MIN_CONTRIBUTION` of the partition's total,
/// retention drops it exactly as it would a non-module circuit -- this arm
/// was unreachable before the override-aware scoring landed (every module
/// circuit survived regardless of magnitude).
#[test]
fn a_module_circuit_below_threshold_via_its_override_series_is_dropped_by_retention() {
    let link_offsets: Vec<LinkOffset> = vec![
        ((Ident::new("a"), Ident::new("b")), 0),
        ((Ident::new("b"), Ident::new("a")), 1),
        ((Ident::new("a"), Ident::new("c")), 2),
        ((Ident::new("c"), Ident::new("a")), 3),
    ];
    let n_offsets = 4;
    let step_count = 2;
    let mut data = vec![0.0f64; n_offsets * step_count];
    data[n_offsets] = 1.0; // a->b
    data[n_offsets + 1] = 1.0; // b->a
    // a<->c's own RAW row is deliberately large (were it scored raw, it
    // would clear the threshold) -- the point of this fixture is that its
    // OVERRIDE, not its raw row, decides retention.
    data[n_offsets + 2] = 1.0; // a->c
    data[n_offsets + 3] = 1.0; // c->a
    let results = enum_results(n_offsets, step_count, data);
    let stocks = stock_list(&["a"]);
    let search = IndexedSearch::build(&link_offsets, &stocks);
    let activity = super::enum_gen::ActivityGraph::build(&search, &results, None, &mut SystemClock)
        .expect("an unbudgeted build never abandons");
    let candidates = super::enum_gen::enumerate_active_circuits(&activity, None, &mut SystemClock);
    assert!(candidates.complete);
    assert_eq!(candidates.len(), 2, "the two 2-cycles");

    let stock_partition: Vec<Option<usize>> = search
        .idents
        .iter()
        .map(|id| (id.as_str() == "a").then_some(0))
        .collect();
    let modules: Vec<bool> = search.idents.iter().map(|id| id.as_str() == "c").collect();
    let c_id = search
        .idents
        .iter()
        .position(|id| id.as_str() == "c")
        .expect("c is a graph node") as u32;
    // The override is tiny relative to the a<->b loop's mass, so its share
    // never clears MIN_CONTRIBUTION even though the raw row (1.0 * 1.0) would.
    let override_series = vec![0.0, 1e-8];
    let no_agg_nodes = vec![false; search.idents.len()];
    let outcome = super::enum_gen::retain_circuits(
        &candidates,
        &activity,
        &stock_partition,
        &modules,
        &no_agg_nodes,
        &mut |_from, module, _next| {
            (module == c_id).then(|| (Rc::new(override_series.clone()), (0, override_series.len())))
        },
        None,
        &mut SystemClock,
    )
    .unwrap();
    let survivor_nodes = survivor_node_sets(&outcome, &candidates, &activity, &search);
    assert!(
        survivor_nodes.contains(&vec!["a".to_string(), "b".to_string()]),
        "the dominant loop survives"
    );
    assert!(
        !survivor_nodes.contains(&vec!["a".to_string(), "c".to_string()]),
        "the module circuit's override share (1e-8 / ~1.00000001) is far \
         below MIN_CONTRIBUTION and it is DROPPED -- before override-aware \
         scoring, a module circuit was kept unconditionally regardless of \
         magnitude"
    );
    let totals = &outcome.partition_totals[&0];
    assert!(
        (totals[1] - (1.0 + 1e-8)).abs() < 1e-12,
        "the partition total banks the module circuit's OVERRIDE mass \
         (1e-8), not its raw composite product (1.0): got {}",
        totals[1]
    );
}

/// Retention's totals and survivors are exactly the hand-computed products,
/// on a fixture that carries every value class the scoring pass distinguishes:
/// an ordinary finite loop, a loop with a zero link and a NaN link, and a loop
/// too small to retain.
///
/// The expected totals are written as the same left-to-right accumulation the
/// pass performs and compared for EXACT equality, so this pins bit-identity
/// rather than approximate agreement -- the contiguous score rows and the
/// activity-window restriction must not perturb a single ULP relative to
/// multiplying the raw slab entries in traversal order.
#[test]
fn retention_totals_and_survivors_match_hand_computed_products() {
    // a is the only stock, so all three 2-cycles share partition 0.
    //   A = a<->b: 2 * 3 = 6 at steps 1..3
    //   B = a<->c: a->c is 1 at step 1, exactly 0 at step 2, NaN at step 3,
    //              so B is active only at step 1, where it scores 1 * 4 = 4
    //   C = a<->d: 1e-4 * 1e-4 = 1e-8 at steps 1..3 -- never 0.1% of the total
    let link_offsets: Vec<LinkOffset> = vec![
        ((Ident::new("a"), Ident::new("b")), 0),
        ((Ident::new("b"), Ident::new("a")), 1),
        ((Ident::new("a"), Ident::new("c")), 2),
        ((Ident::new("c"), Ident::new("a")), 3),
        ((Ident::new("a"), Ident::new("d")), 4),
        ((Ident::new("d"), Ident::new("a")), 5),
    ];
    let n_offsets = 6;
    let step_count = 4;
    let mut data = vec![0.0f64; n_offsets * step_count];
    for step in 1..step_count {
        let base = step * n_offsets;
        data[base] = 2.0; // a->b
        data[base + 1] = 3.0; // b->a
        data[base + 2] = match step {
            1 => 1.0,
            2 => 0.0,
            _ => f64::NAN,
        }; // a->c
        data[base + 3] = 4.0; // c->a
        data[base + 4] = 1e-4; // a->d
        data[base + 5] = 1e-4; // d->a
    }
    let results = enum_results(n_offsets, step_count, data);
    let search = IndexedSearch::build(&link_offsets, &stock_list(&["a"]));
    let activity = super::enum_gen::ActivityGraph::build(&search, &results, None, &mut SystemClock)
        .expect("an unbudgeted build never abandons");
    let candidates = super::enum_gen::enumerate_active_circuits(&activity, None, &mut SystemClock);
    assert!(candidates.complete);
    assert_eq!(candidates.len(), 3);

    let stock_partition: Vec<Option<usize>> = search
        .idents
        .iter()
        .map(|id| (id.as_str() == "a").then_some(0))
        .collect();
    let no_modules = vec![false; search.idents.len()];
    let no_agg_nodes = vec![false; search.idents.len()];
    let outcome = super::enum_gen::retain_circuits(
        &candidates,
        &activity,
        &stock_partition,
        &no_modules,
        &no_agg_nodes,
        &mut |_, _, _| None,
        None,
        &mut SystemClock,
    )
    .unwrap();

    // Circuits are emitted min-root-first in adjacency order, so partition 0's
    // totals accumulate A, then B, then C.
    let totals = &outcome.partition_totals[&0];
    assert_eq!(totals[0], 0.0, "step 0 carries no signal");
    assert_eq!(totals[1], ((0.0f64 + 6.0) + 4.0) + 1e-8);
    assert_eq!(
        totals[2],
        (0.0f64 + 6.0) + 1e-8,
        "B's zero link adds nothing"
    );
    assert_eq!(totals[3], (0.0f64 + 6.0) + 1e-8, "B's NaN step is excluded");

    let mut survivors = survivor_node_sets(&outcome, &candidates, &activity, &search);
    survivors.sort();
    assert_eq!(
        survivors,
        vec![
            vec!["a".to_string(), "b".to_string()],
            vec!["a".to_string(), "c".to_string()],
        ],
        "A (0.6 of the total) and B (0.4 at step 1) are retained; C peaks at \
         1e-9 of the total and is not"
    );
}

/// Retention's pass-1 admission test is a BOUND, not the answer: it divides a
/// circuit's mass by the partition total accumulated SO FAR, which for an
/// early circuit is barely more than its own mass. That bound is deliberately
/// loose in the safe direction (it never drops a circuit the exact test would
/// keep), and the confirm step against the final totals is what turns it into
/// the retention decision.
///
/// This fixture puts the negligible circuit FIRST, so its bound is 1.0 -- the
/// most permissive value there is -- while its true share is 1e-8. A retention
/// pass that trusted the bound would keep it.
#[test]
fn retention_confirms_a_circuit_whose_running_bound_overstates_its_share() {
    // `link_offsets` here is sorted by `(from, to)` name -- the shape
    // `parse_link_offsets` actually produces (it sorts before returning, GH
    // #310 review) -- rather than hand-ordered to suit the fixture: the tiny
    // loop's partner is named `b` and the dominant loop's `c` so that sorted
    // order and "tiny loop first" coincide, exactly as they would for any
    // production-derived `link_offsets`. `a`'s out-edges are then walked in
    // that same order, so the tiny a<->b loop is emitted before the dominant
    // a<->c one.
    let link_offsets: Vec<LinkOffset> = vec![
        ((Ident::new("a"), Ident::new("b")), 0),
        ((Ident::new("a"), Ident::new("c")), 1),
        ((Ident::new("b"), Ident::new("a")), 2),
        ((Ident::new("c"), Ident::new("a")), 3),
    ];
    let n_offsets = 4;
    let step_count = 2;
    let mut data = vec![0.0f64; n_offsets * step_count];
    data[n_offsets] = 1e-4; // a->b
    data[n_offsets + 1] = 1.0; // a->c
    data[n_offsets + 2] = 1e-4; // b->a  (a<->b product 1e-8)
    data[n_offsets + 3] = 1.0; // c->a  (a<->c product 1.0)
    let results = enum_results(n_offsets, step_count, data);
    let search = IndexedSearch::build(&link_offsets, &stock_list(&["a"]));
    let activity = super::enum_gen::ActivityGraph::build(&search, &results, None, &mut SystemClock)
        .expect("an unbudgeted build never abandons");
    let candidates = super::enum_gen::enumerate_active_circuits(&activity, None, &mut SystemClock);
    assert!(candidates.complete);
    assert_eq!(
        activity
            .circuit_nodes(candidates.circuit(0))
            .iter()
            .map(|&n| search.idents[n as usize].as_str().to_string())
            .collect::<Vec<_>>(),
        vec!["a".to_string(), "b".to_string()],
        "the fixture only bites if the negligible circuit is emitted first"
    );

    let stock_partition: Vec<Option<usize>> = search
        .idents
        .iter()
        .map(|id| (id.as_str() == "a").then_some(0))
        .collect();
    let no_modules = vec![false; search.idents.len()];
    let no_agg_nodes = vec![false; search.idents.len()];
    let outcome = super::enum_gen::retain_circuits(
        &candidates,
        &activity,
        &stock_partition,
        &no_modules,
        &no_agg_nodes,
        &mut |_, _, _| None,
        None,
        &mut SystemClock,
    )
    .unwrap();
    assert_eq!(
        survivor_node_sets(&outcome, &candidates, &activity, &search),
        vec![vec!["a".to_string(), "c".to_string()]],
        "the confirm step drops the circuit its running bound admitted"
    );
}

/// Defense in depth for the `head & !1u64` step-0 mask and `active_window`'s
/// `lo == 0` case (see the `bits` field doc on `ActivityGraph`): production
/// never sets bit 0 (every link score's `TIME = INITIAL_TIME` guard arm is
/// the literal `0` there), so this fixture makes an edge genuinely active AT
/// step 0 too -- a shape no real run produces, but one the enumerator does
/// not assume away. Correctness requires two things simultaneously: the mask
/// must still require activity at some step >= 1 for the circuit to be
/// emitted at all (bit-0-only activity is not a scorable loop), and once the
/// circuit IS emitted, `active_window` must start its window at step 0
/// rather than silently excluding a genuinely active first step from
/// retention's totals.
#[test]
fn a_circuit_active_at_step_0_too_is_windowed_from_step_0() {
    let link_offsets: Vec<LinkOffset> = vec![
        ((Ident::new("a"), Ident::new("b")), 0),
        ((Ident::new("b"), Ident::new("a")), 1),
    ];
    let n_offsets = 2;
    let step_count = 2;
    // Both edges active at BOTH steps, unlike a real run's step 0.
    let data = vec![
        2.0, 3.0, // step 0: a->b = 2, b->a = 3
        4.0, 5.0, // step 1: a->b = 4, b->a = 5
    ];
    let results = enum_results(n_offsets, step_count, data);
    let search = IndexedSearch::build(&link_offsets, &stock_list(&["a"]));
    let activity = super::enum_gen::ActivityGraph::build(&search, &results, None, &mut SystemClock)
        .expect("an unbudgeted build never abandons");
    let candidates = super::enum_gen::enumerate_active_circuits(&activity, None, &mut SystemClock);
    assert!(candidates.complete);
    assert_eq!(candidates.len(), 1, "the a<->b 2-cycle");

    let stock_partition: Vec<Option<usize>> = search
        .idents
        .iter()
        .map(|id| (id.as_str() == "a").then_some(0))
        .collect();
    let no_modules = vec![false; search.idents.len()];
    let no_agg_nodes = vec![false; search.idents.len()];
    let outcome = super::enum_gen::retain_circuits(
        &candidates,
        &activity,
        &stock_partition,
        &no_modules,
        &no_agg_nodes,
        &mut |_, _, _| None,
        None,
        &mut SystemClock,
    )
    .unwrap();
    assert_eq!(outcome.survivors, vec![0], "the only circuit, ever active");
    assert_eq!(
        outcome.partition_totals[&0],
        vec![6.0, 20.0],
        "step 0's mass (2*3=6) must reach the partition total alongside \
         step 1's (4*5=20), proving `active_window` started at step 0 \
         (`lo == 0`) rather than at step 1"
    );
}

/// The universe circuit count per partition is over ALL enumerated circuits,
/// retention non-survivors included -- it describes how much company a loop
/// has in its partition, which is a fact about the model rather than about
/// what survived a threshold.
#[test]
fn retention_counts_the_universe_circuits_per_partition() {
    // Two partition-0 circuits (one of them far below the retention floor) and
    // one Solo circuit, which belongs to no partition and is counted in none.
    let link_offsets: Vec<LinkOffset> = vec![
        ((Ident::new("a"), Ident::new("b")), 0),
        ((Ident::new("b"), Ident::new("a")), 1),
        ((Ident::new("a"), Ident::new("c")), 2),
        ((Ident::new("c"), Ident::new("a")), 3),
        ((Ident::new("d"), Ident::new("e")), 4),
        ((Ident::new("e"), Ident::new("d")), 5),
    ];
    let n_offsets = 6;
    let step_count = 2;
    let mut data = vec![0.0f64; n_offsets * step_count];
    data[n_offsets] = 1.0;
    data[n_offsets + 1] = 1.0;
    data[n_offsets + 2] = 1e-4;
    data[n_offsets + 3] = 1e-4;
    data[n_offsets + 4] = 0.5;
    data[n_offsets + 5] = 0.5;
    let results = enum_results(n_offsets, step_count, data);
    let search = IndexedSearch::build(&link_offsets, &stock_list(&["a"]));
    let activity = super::enum_gen::ActivityGraph::build(&search, &results, None, &mut SystemClock)
        .expect("an unbudgeted build never abandons");
    let candidates = super::enum_gen::enumerate_active_circuits(&activity, None, &mut SystemClock);
    let stock_partition: Vec<Option<usize>> = search
        .idents
        .iter()
        .map(|id| (id.as_str() == "a").then_some(0))
        .collect();
    let no_modules = vec![false; search.idents.len()];
    let no_agg_nodes = vec![false; search.idents.len()];
    let outcome = super::enum_gen::retain_circuits(
        &candidates,
        &activity,
        &stock_partition,
        &no_modules,
        &no_agg_nodes,
        &mut |_, _, _| None,
        None,
        &mut SystemClock,
    )
    .unwrap();
    assert_eq!(
        outcome.survivors.len(),
        2,
        "one partition loop plus the Solo"
    );
    assert_eq!(
        outcome.partition_circuit_counts,
        [(0usize, 2usize)].into_iter().collect::<HashMap<_, _>>(),
        "both partition-0 circuits are counted, the dropped one included; the \
         Solo circuit belongs to no partition"
    );
}

/// AC4.1's zero-mass sibling: a circuit's EDGES can be individually active
/// (nonzero at every step) while its PRODUCT underflows to exactly 0 at
/// every one of them, banking no mass anywhere. Such a circuit must not
/// inflate its partition's universe count -- `loop_counts` means "how many
/// loops' mass is in this total", and a circuit that banked none of it is
/// not one of them -- so the OTHER loop in the partition is left effectively
/// alone in its own denominator (mean relative score exactly `+-1`, ranked
/// as if it were Solo) rather than spuriously competing against a total it
/// alone built.
#[test]
fn a_circuit_whose_product_always_underflows_to_zero_does_not_inflate_the_universe_count() {
    // X = a<->b: 1e-200 * 1e-200 underflows to exactly 0.0 at every step --
    //     each edge is individually active (nonzero, finite), so X is
    //     enumerated, but its product never banks any mass.
    // Y = a<->c: an ordinary finite loop, the partition's only real member.
    let link_offsets: Vec<LinkOffset> = vec![
        ((Ident::new("a"), Ident::new("b")), 0),
        ((Ident::new("b"), Ident::new("a")), 1),
        ((Ident::new("a"), Ident::new("c")), 2),
        ((Ident::new("c"), Ident::new("a")), 3),
    ];
    let n_offsets = 4;
    let step_count = 2;
    let mut data = vec![0.0f64; n_offsets * step_count];
    data[n_offsets] = 1e-200;
    data[n_offsets + 1] = 1e-200;
    data[n_offsets + 2] = 2.0;
    data[n_offsets + 3] = 3.0;
    assert_eq!(
        1e-200f64 * 1e-200f64,
        0.0,
        "the fixture only bites if this underflows exactly to zero"
    );
    let results = enum_results(n_offsets, step_count, data);
    let search = IndexedSearch::build(&link_offsets, &stock_list(&["a"]));
    let activity = super::enum_gen::ActivityGraph::build(&search, &results, None, &mut SystemClock)
        .expect("an unbudgeted build never abandons");
    let candidates = super::enum_gen::enumerate_active_circuits(&activity, None, &mut SystemClock);
    assert!(candidates.complete);
    assert_eq!(
        candidates.len(),
        2,
        "both 2-cycles, each individually active"
    );

    let stock_partition: Vec<Option<usize>> = search
        .idents
        .iter()
        .map(|id| (id.as_str() == "a").then_some(0))
        .collect();
    let no_modules = vec![false; search.idents.len()];
    let no_agg_nodes = vec![false; search.idents.len()];
    let outcome = super::enum_gen::retain_circuits(
        &candidates,
        &activity,
        &stock_partition,
        &no_modules,
        &no_agg_nodes,
        &mut |_, _, _| None,
        None,
        &mut SystemClock,
    )
    .unwrap();

    assert_eq!(
        outcome.partition_circuit_counts,
        [(0usize, 1usize)].into_iter().collect::<HashMap<_, _>>(),
        "the zero-product circuit must not be counted toward the universe -- \
         only the one loop that actually banks mass is"
    );
    let survivors = survivor_node_sets(&outcome, &candidates, &activity, &search);
    assert_eq!(
        survivors,
        vec![vec!["a".to_string(), "c".to_string()]],
        "the zero-product circuit can satisfy no threshold either, so it is \
         never a survivor -- unaffected by the count fix, checked for good \
         measure"
    );
    assert_eq!(
        outcome.distinct_circuits, 1,
        "the distinct-loop count behind universe_loops is the mass-bearing \
         count, so the zero-product circuit is not in it either"
    );

    // The same rule for a Solo circuit (no stock on it): its own series is its
    // denominator, and a zero product at every step means it carries nothing
    // -- it is scored, found massless, and neither survives nor counts.
    let solo_partition: Vec<Option<usize>> = vec![None; search.idents.len()];
    let solo_outcome = super::enum_gen::retain_circuits(
        &candidates,
        &activity,
        &solo_partition,
        &no_modules,
        &no_agg_nodes,
        &mut |_, _, _| None,
        None,
        &mut SystemClock,
    )
    .unwrap();
    assert_eq!(
        survivor_node_sets(&solo_outcome, &candidates, &activity, &search),
        vec![vec!["a".to_string(), "c".to_string()]],
        "a Solo circuit whose product is zero at every step is dropped by \
         scoring, not kept for being enumerated"
    );
    assert_eq!(solo_outcome.distinct_circuits, 1);

    // The ranking consequence: with a universe count of 1, `rank_and_filter`
    // must classify Y as NOT competing (mean_rel exactly 1.0, ranked as if
    // Solo) even though it sits in a resolved `NormGroup::Partition` rather
    // than `NormGroup::Solo` -- exactly what the OLD count of 2 would have
    // wrongly classified as competing.
    let mut loops = vec![
        make_found_loop_with_scores(
            &[("a", "c"), ("c", "a")],
            &["a"],
            LoopPolarity::Reinforcing,
            6.0,
            vec![(0.0, 0.0), (1.0, 6.0)],
        ),
        solo_group_loop("s"),
    ];
    let partitions = single_partition(&["a"]);
    let universe = UniverseStats {
        totals: outcome.partition_totals,
        loop_counts: outcome.partition_circuit_counts,
    };
    rank_and_filter(&mut loops, &partitions, Some(&universe));
    let y = loops
        .iter()
        .find(|l| l.avg_abs_score == 6.0)
        .expect("Y must survive retention");
    assert!(
        y.rel_scores
            .iter()
            .all(|&r| r == 0.0 || (r.abs() - 1.0).abs() < 1e-9),
        "Y's relative score must read exactly +-1, matching a Solo loop's \
         signature, since the universe count of 1 makes it uncontested: {:?}",
        y.rel_scores
    );
}

/// AC4.1: a circuit whose running product overflows to `Inf` and then meets a
/// `0` link has a NaN score at that step with no NaN link anywhere.
///
/// Testing the LINKS for NaN -- which is what a short-circuiting product does
/// -- calls that step a number, and `Inf * 0` then enters the partition total
/// as `NaN.abs()`, poisoning the denominator for every sibling loop at that
/// step for the rest of the run. Testing the finished PRODUCT is what makes
/// the step behave like any other NaN: no mass, no retention, and the
/// partition's other loops keep a finite relative score there.
#[test]
fn an_inf_times_zero_product_is_excluded_from_totals_and_retention() {
    // X = a->b->c->a, whose first two links multiply to +Inf, and whose third
    //     is exactly 0 at step 2 (so X is active at steps 1 and 3, and step 2
    //     falls inside its activity window).
    // Y = a<->d, a finite sibling in the same partition.
    let link_offsets: Vec<LinkOffset> = vec![
        ((Ident::new("a"), Ident::new("b")), 0),
        ((Ident::new("b"), Ident::new("c")), 1),
        ((Ident::new("c"), Ident::new("a")), 2),
        ((Ident::new("a"), Ident::new("d")), 3),
        ((Ident::new("d"), Ident::new("a")), 4),
    ];
    let n_offsets = 5;
    let step_count = 4;
    let mut data = vec![0.0f64; n_offsets * step_count];
    for step in 1..step_count {
        let base = step * n_offsets;
        data[base] = 1e300; // a->b
        data[base + 1] = 1e300; // b->c  (product overflows to +Inf)
        data[base + 2] = if step == 2 { 0.0 } else { 1.0 }; // c->a
        data[base + 3] = 5.0; // a->d
        data[base + 4] = 1.0; // d->a
    }
    let results = enum_results(n_offsets, step_count, data);
    let search = IndexedSearch::build(&link_offsets, &stock_list(&["a"]));
    let activity = super::enum_gen::ActivityGraph::build(&search, &results, None, &mut SystemClock)
        .expect("an unbudgeted build never abandons");
    let candidates = super::enum_gen::enumerate_active_circuits(&activity, None, &mut SystemClock);
    assert!(candidates.complete);
    assert_eq!(candidates.len(), 2, "the triangle and the 2-cycle");

    let stock_partition: Vec<Option<usize>> = search
        .idents
        .iter()
        .map(|id| (id.as_str() == "a").then_some(0))
        .collect();
    let no_modules = vec![false; search.idents.len()];
    let no_agg_nodes = vec![false; search.idents.len()];
    let outcome = super::enum_gen::retain_circuits(
        &candidates,
        &activity,
        &stock_partition,
        &no_modules,
        &no_agg_nodes,
        &mut |_, _, _| None,
        None,
        &mut SystemClock,
    )
    .unwrap();

    let totals = &outcome.partition_totals[&0];
    assert_eq!(
        totals[2], 5.0,
        "the Inf*0 step contributes nothing, so the total is Y's mass alone"
    );
    assert!(
        totals[1].is_infinite() && totals[3].is_infinite(),
        "an Inf score is real divergent signal and stays in the total"
    );
    // Y's relative score at the poisoned step is finite and well-defined.
    assert_eq!(5.0 / totals[2], 1.0);

    let survivors = survivor_node_sets(&outcome, &candidates, &activity, &search);
    assert_eq!(
        survivors,
        vec![vec!["a".to_string(), "d".to_string()]],
        "X cannot satisfy retention: NaN at step 2, and Inf/Inf = NaN at 1 and 3"
    );
}

/// `subtract_reported_mass_from_totals`' Inf convention: at a dominance
/// inflection the running total is `Inf` (kept, like any other real
/// divergent signal), and if the DROPPED duplicate's own score is ALSO `Inf`
/// there, a plain subtraction would compute `Inf - Inf == NaN`, poisoning
/// the total for every sibling loop at that step for the rest of the run --
/// exactly the failure class
/// `an_inf_times_zero_product_is_excluded_from_totals_and_retention` pins
/// for pass 1. The fix must skip the subtraction at such a step and leave
/// the total `Inf`, not merely avoid crashing.
#[test]
fn subtract_reported_mass_from_totals_skips_an_infinite_step_to_avoid_inf_minus_inf() {
    let partitions = single_partition(&["a"]);
    let mut totals: HashMap<usize, Vec<f64>> = HashMap::new();
    totals.insert(0, vec![10.0, f64::INFINITY]);
    // The dropped duplicate representative: finite at step 0, Inf at step 1
    // (the same dominance inflection the total is already Inf at).
    let dropped = make_found_loop_with_scores(
        &[("a", "b"), ("b", "a")],
        &["a"],
        LoopPolarity::Reinforcing,
        0.0,
        vec![(0.0, 4.0), (1.0, f64::INFINITY)],
    );
    subtract_reported_mass_from_totals(&dropped, &partitions, &mut totals);
    let series = &totals[&0];
    assert_eq!(
        series[0], 6.0,
        "a finite step subtracts normally: 10 - 4 = 6"
    );
    assert!(
        series[1].is_infinite() && series[1] > 0.0,
        "an Inf total must stay Inf, never become NaN, when the dropped \
         twin's own score is also Inf there: got {}",
        series[1]
    );
}

/// The canonical rotations of a set of node-id paths, as name lists -- the
/// directed-cycle identity both generators dedup on, rendered readably.
fn canonical_name_cycles(search: &IndexedSearch, paths: &[Vec<u32>]) -> Vec<Vec<String>> {
    let mut out: Vec<Vec<String>> = paths
        .iter()
        .map(|path| {
            let names: Vec<String> = path
                .iter()
                .map(|&n| search.idents[n as usize].as_str().to_string())
                .collect();
            crate::ltm::canonical_rotation(&names)
        })
        .collect();
    out.sort();
    out
}

/// Issue #308 at the enumeration level: two directed 3-cycles over the same
/// node set in OPPOSITE directions are different loops -- different polarities,
/// different scores -- and the canonical-rotation identity keeps them apart.
///
/// The fallback's arm of the same claim is
/// `fallback::tests::opposite_direction_three_cycles_are_both_kept`; it needs a
/// per-step weight flip to surface both, since one Dijkstra tree expresses one
/// path per node, while the enumerator emits both from one graph.
#[test]
fn the_enumerator_keeps_opposite_direction_three_cycles_distinct() {
    // Every ordered pair over {a, b, c}: three 2-cycles and both 3-cycles.
    let link_offsets: Vec<LinkOffset> = vec![
        ((Ident::new("a"), Ident::new("b")), 0),
        ((Ident::new("a"), Ident::new("c")), 1),
        ((Ident::new("b"), Ident::new("a")), 2),
        ((Ident::new("b"), Ident::new("c")), 3),
        ((Ident::new("c"), Ident::new("a")), 4),
        ((Ident::new("c"), Ident::new("b")), 5),
    ];
    let n_offsets = 6;
    let step_count = 2;
    let mut data = vec![0.0f64; n_offsets * step_count];
    for slot in data.iter_mut().skip(n_offsets) {
        *slot = 1.0;
    }
    let results = enum_results(n_offsets, step_count, data);
    let search = IndexedSearch::build(&link_offsets, &stock_list(&["a"]));
    let activity = super::enum_gen::ActivityGraph::build(&search, &results, None, &mut SystemClock)
        .expect("an unbudgeted build never abandons");
    let candidates = super::enum_gen::enumerate_active_circuits(&activity, None, &mut SystemClock);
    assert!(candidates.complete);

    let paths: Vec<Vec<u32>> = (0..candidates.len())
        .map(|ci| activity.circuit_nodes(candidates.circuit(ci)))
        .collect();
    let cycles = canonical_name_cycles(&search, &paths);
    assert_eq!(
        cycles,
        vec![
            vec!["a".to_string(), "b".to_string()],
            vec!["a".to_string(), "b".to_string(), "c".to_string()],
            vec!["a".to_string(), "c".to_string()],
            vec!["a".to_string(), "c".to_string(), "b".to_string()],
            vec!["b".to_string(), "c".to_string()],
        ],
        "both directed 3-cycles survive alongside the three 2-cycles"
    );
}

/// A loop whose links are simultaneously nonzero at exactly ONE saved step is
/// found -- by both generators -- at that step.
///
/// This is the complement of the activity rule: an inactive link means the
/// loop's score is exactly 0 there, so a generator that only ever looked at
/// one step would miss the loop, and one that ignored activity would report
/// loops that score 0 everywhere. Loops whose links are only ever active at
/// DIFFERENT steps stay undiscoverable to both (GH #699), which
/// `staggered_activity_cycle_is_not_reported_by_either_generator` pins.
#[test]
fn a_cycle_active_at_only_one_step_is_found_by_both_generators() {
    // a <-> b, with a->b inactive at step 1 and active at step 2.
    let link_offsets: Vec<LinkOffset> = vec![
        ((Ident::new("a"), Ident::new("b")), 0),
        ((Ident::new("b"), Ident::new("a")), 1),
    ];
    let n_offsets = 2;
    let step_count = 3;
    let mut data = vec![0.0f64; n_offsets * step_count];
    data[n_offsets] = 0.0; // a->b at step 1: inactive
    data[n_offsets + 1] = 10.0; // b->a at step 1
    data[2 * n_offsets] = 0.5; // a->b at step 2: active
    data[2 * n_offsets + 1] = 10.0; // b->a at step 2
    let results = enum_results(n_offsets, step_count, data);
    let search = IndexedSearch::build(&link_offsets, &stock_list(&["a"]));

    let activity = super::enum_gen::ActivityGraph::build(&search, &results, None, &mut SystemClock)
        .expect("an unbudgeted build never abandons");
    let candidates = super::enum_gen::enumerate_active_circuits(&activity, None, &mut SystemClock);
    assert!(candidates.complete);
    let enumerated: Vec<Vec<u32>> = (0..candidates.len())
        .map(|ci| activity.circuit_nodes(candidates.circuit(ci)))
        .collect();
    assert_eq!(
        canonical_name_cycles(&search, &enumerated),
        vec![vec!["a".to_string(), "b".to_string()]],
        "the enumerator's activity AND is nonempty at step 2, so the cycle is \
         emitted"
    );

    let outcome = super::fallback::sweep(
        &search,
        &results,
        FallbackConfig::DEFAULT,
        None,
        &mut SystemClock,
    );
    assert!(!outcome.truncated);
    assert_eq!(
        canonical_name_cycles(&search, &outcome.paths),
        vec![vec!["a".to_string(), "b".to_string()]],
        "the fallback closes the cycle at the step where both links are active"
    );
}

/// Enumerator unit: min-root canonical emission (each cycle exactly once,
/// path starting at its minimum node id) and self-edge exclusion.
#[test]
fn enumerator_emits_each_active_cycle_exactly_once() {
    let (activity, _search) = two_triangles_and_a_self_edge();
    let candidates = super::enum_gen::enumerate_active_circuits(&activity, None, &mut SystemClock);
    assert!(candidates.complete);
    assert_eq!(
        candidates.len(),
        2,
        "two triangles; the active z->z self-edge yields no circuit"
    );
    for i in 0..candidates.len() {
        let nodes = activity.circuit_nodes(candidates.circuit(i));
        assert_eq!(nodes.len(), 3, "both circuits are triangles");
        let min = nodes.iter().min().unwrap();
        assert_eq!(nodes[0], *min, "canonical min-root rotation");
    }
}

/// Two disjoint triangles (a->b->c->a, u->v->w->u) plus an active self-edge
/// z->z, all active at step 1. Six union edges, two circuits, six edge rows.
fn two_triangles_and_a_self_edge() -> (super::enum_gen::ActivityGraph, IndexedSearch) {
    let (search, results) = two_triangles_search_and_results();
    let activity = super::enum_gen::ActivityGraph::build(&search, &results, None, &mut SystemClock)
        .expect("an unbudgeted build never abandons");
    (activity, search)
}

/// The topology and recorded series behind [`two_triangles_and_a_self_edge`],
/// before the activity graph is built -- what the deadline tests below need,
/// since the build is itself one of the phases under test.
fn two_triangles_search_and_results() -> (IndexedSearch, Results) {
    let link_offsets: Vec<LinkOffset> = vec![
        ((Ident::new("a"), Ident::new("b")), 0),
        ((Ident::new("b"), Ident::new("c")), 1),
        ((Ident::new("c"), Ident::new("a")), 2),
        ((Ident::new("u"), Ident::new("v")), 3),
        ((Ident::new("v"), Ident::new("w")), 4),
        ((Ident::new("w"), Ident::new("u")), 5),
        ((Ident::new("z"), Ident::new("z")), 6),
    ];
    let n_offsets = 7;
    let step_count = 2;
    let mut data = vec![0.0f64; n_offsets * step_count];
    for slot in data.iter_mut().skip(n_offsets).take(n_offsets) {
        *slot = 1.0;
    }
    let results = enum_results(n_offsets, step_count, data);
    let search = IndexedSearch::build(&link_offsets, &stock_list(&["a"]));
    (search, results)
}

/// Every enumeration budget reports an incomplete enumeration when it trips:
/// the circuit count, the edge-visit count, and -- AC3.3, the memory bound --
/// the total emitted edge rows. The fourth stop condition, an expired
/// wall-clock deadline, is the arm
/// `enumerate_active_circuits_abandons_an_already_expired_deadline` below
/// covers.
#[test]
fn each_enumeration_budget_arm_reports_incomplete() {
    let (activity, _search) = two_triangles_and_a_self_edge();
    assert!(
        super::enum_gen::enumerate_active_circuits(&activity, None, &mut SystemClock).complete,
        "the fixture completes when nothing is overridden"
    );

    for (arm, circuits, visits, edge_rows) in [
        ("circuit budget", 1usize, u64::MAX, u64::MAX),
        ("visit budget", usize::MAX, 1u64, u64::MAX),
        ("edge-row budget", usize::MAX, u64::MAX, 1u64),
    ] {
        let _guard = EnumBudgetGuard::new(circuits, visits, edge_rows);
        let truncated =
            super::enum_gen::enumerate_active_circuits(&activity, None, &mut SystemClock);
        assert!(
            !truncated.complete,
            "a tripped {arm} must report an incomplete enumeration"
        );
    }
}

/// The union graph's own storage (one score row plus one activity bitset per
/// active edge) is bounded by a byte budget of its own: the circuit and
/// edge-row budgets bound the enumeration, not this copy, and on a many-edge,
/// many-saved-step model it would duplicate a large part of the results slab
/// before a single circuit is considered. Tripping it abandons the build --
/// the same `None` an expired deadline yields -- so discovery takes the
/// fallback, which reads the slab in place.
#[test]
fn a_union_graph_over_its_storage_budget_abandons_the_build() {
    let (search, results) = two_triangles_search_and_results();
    assert!(
        super::enum_gen::ActivityGraph::build(&search, &results, None, &mut SystemClock).is_some(),
        "the fixture builds when nothing is overridden"
    );
    let _guard = super::enum_gen::ActivityGraphBytesGuard::new(1);
    assert!(
        super::enum_gen::ActivityGraph::build(&search, &results, None, &mut SystemClock).is_none(),
        "one byte cannot hold a single edge's row, so the build must abandon"
    );
}

// --- Budget split (AC4.4) -------------------------------------------------

/// The enumeration path gets a documented fraction of the caller's budget and
/// the fallback gets the caller's own expiry -- not a second slice of what is
/// left, which would cap discovery below the budget the caller asked for.
#[test]
fn a_budget_splits_into_an_enumeration_share_and_the_callers_expiry() {
    let started = Instant::now();
    let limit = Duration::from_millis(400);
    let deadlines = super::split_budget(started, limit);
    assert_eq!(
        deadlines.enumeration,
        Some(started + limit.mul_f64(super::ENUM_BUDGET_FRACTION))
    );
    assert_eq!(deadlines.fallback, Some(started + limit));
}

/// A zero budget is still a budget: both phases are already expired, so
/// discovery returns immediately rather than treating "no time" as "no limit".
#[test]
fn a_zero_budget_expires_both_phases_at_the_start_instant() {
    let started = Instant::now();
    let deadlines = super::split_budget(started, Duration::ZERO);
    assert_eq!(deadlines.enumeration, Some(started));
    assert_eq!(deadlines.fallback, Some(started));
}

// --- Per-phase deadline awareness (AC2.2) ---------------------------------
//
// Discovery's enumeration path has three phases, and a caller's wall-clock
// budget has to bind in each of them: a budget that only the LAST phase
// honoured would be spent entirely inside the first two on the model that
// needs it most. Each phase gets one arm here for an already-expired deadline
// and one for the unbudgeted case, where reading the clock at all is the
// defect (a per-value clock read would swamp the work it guards). The fourth
// phase, the fallback sweep, owns the same pair in
// `ltm_finding_fallback_tests.rs`.

/// The stock-partition / module-node metadata `retain_circuits` takes, for the
/// two-triangle fixture: `a` is the only stock and there are no modules.
fn two_triangle_retention_metadata(
    search: &IndexedSearch,
) -> (Vec<Option<usize>>, Vec<bool>, Vec<bool>) {
    let stock_partition: Vec<Option<usize>> = search
        .idents
        .iter()
        .map(|id| (id.as_str() == "a").then_some(0))
        .collect();
    let no_modules = vec![false; search.idents.len()];
    let no_agg_nodes = vec![false; search.idents.len()];
    (stock_partition, no_modules, no_agg_nodes)
}

#[test]
fn activity_graph_build_abandons_an_already_expired_deadline() {
    let (search, results) = two_triangles_search_and_results();
    let mut clock = ScriptedClock::new(1);
    let deadline = clock.deadline();
    assert!(
        super::enum_gen::ActivityGraph::build(&search, &results, Some(deadline), &mut clock)
            .is_none(),
        "a caller whose budget is already spent must not copy the score slab \
         it is about to discard"
    );
    assert_eq!(
        clock.reads, 1,
        "the check runs before the first edge is copied"
    );
}

#[test]
fn activity_graph_build_never_reads_the_clock_when_unbudgeted() {
    let (search, results) = two_triangles_search_and_results();
    let mut clock = ScriptedClock::new(1);
    assert!(
        super::enum_gen::ActivityGraph::build(&search, &results, None, &mut clock).is_some(),
        "an unbudgeted build always completes"
    );
    assert_eq!(clock.reads, 0);
}

/// A chain of edges whose score series is longer than one deadline-check
/// interval: the shape of a small model saved at very many steps, which is
/// what the libsimlin and pysimlin wall-clock budget fixtures are (200,001
/// saved steps over a handful of links). Every value is active, so the build
/// does its full per-step work on every edge.
fn long_series_search_and_results(
    edges: &[(&str, &str)],
    step_count: usize,
) -> (IndexedSearch, Results) {
    let link_offsets: Vec<LinkOffset> = edges
        .iter()
        .enumerate()
        .map(|(i, (from, to))| ((Ident::new(from), Ident::new(to)), i))
        .collect();
    let n_offsets = edges.len();
    let data = vec![1.0f64; n_offsets * step_count];
    let results = enum_results(n_offsets, step_count, data);
    let search = IndexedSearch::build(&link_offsets, &stock_list(&["a"]));
    (search, results)
}

/// The deadline check has to fire INSIDE one edge's series, not only between
/// edges. A model saved at many steps over few links makes a single edge's
/// copy milliseconds of work, so an edge-granular check would spend a
/// millisecond-scale budget entirely inside the build and hand the fallback a
/// deadline that has already passed (AC4.4).
#[test]
fn activity_graph_build_checks_the_deadline_inside_one_edges_series() {
    let step_count = super::enum_gen::ACTIVITY_BUILD_DEADLINE_CHECK_VALUES + 4096;
    let (search, results) = long_series_search_and_results(&[("a", "b")], step_count);
    // Read 1 is the pre-scan check (not expired); read 2 is the first interval
    // boundary, which lands one check interval into this single edge.
    let mut clock = ScriptedClock::new(2);
    let deadline = clock.deadline();
    assert!(
        super::enum_gen::ActivityGraph::build(&search, &results, Some(deadline), &mut clock)
            .is_none(),
        "a deadline expiring part way through one long edge must abandon the build"
    );
    assert_eq!(
        clock.reads, 2,
        "the interval boundary is inside the edge, not at the next edge"
    );
}

/// One check interval is the same number of VALUES whichever way the graph is
/// shaped -- many edges over few steps, or few edges over many steps -- which
/// is the claim `ACTIVITY_BUILD_DEADLINE_CHECK_VALUES` makes.
#[test]
fn activity_graph_build_spends_one_check_interval_per_block_of_values() {
    let check_values = super::enum_gen::ACTIVITY_BUILD_DEADLINE_CHECK_VALUES;
    let step_count = check_values + 4096;
    let (search, results) = long_series_search_and_results(&[("a", "b"), ("b", "a")], step_count);
    let mut clock = ScriptedClock::new(usize::MAX);
    let deadline = clock.deadline();
    assert!(
        super::enum_gen::ActivityGraph::build(&search, &results, Some(deadline), &mut clock)
            .is_some(),
        "a live deadline lets the build finish"
    );
    // One read before any value is copied, then one per whole interval of
    // values copied after it -- so the count depends on the total values and
    // not on how they are split across edges.
    let values = 2 * step_count;
    assert_eq!(
        clock.reads,
        values.div_ceil(check_values),
        "the check interval must be counted in values, not in edges"
    );
}

#[test]
fn enumerate_active_circuits_abandons_an_already_expired_deadline() {
    // The enumerator amortizes its clock reads over `DEADLINE_CHECK_INTERVAL`
    // edge visits, which a two-triangle fixture never reaches; the override
    // makes every visit a check so the arm is exercised on a tiny graph
    // (docs/dev/rust.md#test-time-budgets). With `visit_interval == 1` every
    // visit is BOTH the first visit and an interval multiple, so this pins
    // the same first-visit catch as the override-free test below -- kept
    // because it additionally exercises `visit_interval == 1` as a boundary
    // value of the periodic mask itself.
    let _guard = super::enum_gen::EnumDeadlineVisitIntervalGuard::new(1);
    let (activity, _search) = two_triangles_and_a_self_edge();
    let mut clock = ScriptedClock::new(1);
    let deadline = clock.deadline();
    let candidates =
        super::enum_gen::enumerate_active_circuits(&activity, Some(deadline), &mut clock);
    assert!(
        !candidates.complete,
        "an expired deadline mid-search reports an incomplete enumeration, so \
         the caller discards the partial set and falls back"
    );
    assert_eq!(clock.reads, 1, "the first visit is a check");
}

/// The first-visit arm at the PRODUCTION `DEADLINE_CHECK_INTERVAL` (8192, no
/// override): an already-expired deadline must be caught even though the
/// two-triangle fixture's whole enumeration (6 edge visits) never reaches an
/// interval multiple. Before the first-visit check existed, this deadline
/// went undetected on any graph below the interval -- true of nearly every
/// real model -- and the enumeration ran to completion on a budget that was
/// already spent.
#[test]
fn enumerate_active_circuits_catches_an_already_expired_deadline_at_production_interval() {
    let (activity, _search) = two_triangles_and_a_self_edge();
    let mut clock = ScriptedClock::new(1);
    let deadline = clock.deadline();
    let candidates =
        super::enum_gen::enumerate_active_circuits(&activity, Some(deadline), &mut clock);
    assert!(
        !candidates.complete,
        "an already-expired deadline must be caught on the very first visit, \
         regardless of the graph's total visit count relative to \
         DEADLINE_CHECK_INTERVAL"
    );
    assert_eq!(clock.reads, 1, "the first visit is a check");
}

/// The PERIODIC arm, distinct from the first-visit one: a deadline that is
/// still live at visit 1 but expires before the next interval-multiple visit
/// is caught there, not on the first visit. `expire_at_read == 2` keeps the
/// clock live through the first-visit check (read 1) and expired by the
/// interval check (read 2); `visit_interval == 4` puts that check at visit 4
/// -- the first edge of the SECOND root's (`u`'s) triangle -- so the first
/// triangle (`a, b, c`) is fully enumerated before the cutoff.
#[test]
fn enumerate_active_circuits_catches_a_deadline_that_expires_mid_search() {
    let _guard = super::enum_gen::EnumDeadlineVisitIntervalGuard::new(4);
    let (activity, _search) = two_triangles_and_a_self_edge();
    let mut clock = ScriptedClock::new(2);
    let deadline = clock.deadline();
    let candidates =
        super::enum_gen::enumerate_active_circuits(&activity, Some(deadline), &mut clock);
    assert!(
        !candidates.complete,
        "the deadline expiring mid-search must still report an incomplete \
         enumeration"
    );
    assert_eq!(
        candidates.len(),
        1,
        "the a/b/c triangle completes before the cutoff; u/v/w's does not \
         start"
    );
    assert_eq!(
        clock.reads, 2,
        "one check at visit 1 (not yet expired) and one at visit 4, the next \
         interval multiple (expired)"
    );
}

#[test]
fn enumerate_active_circuits_never_reads_the_clock_when_unbudgeted() {
    let _guard = super::enum_gen::EnumDeadlineVisitIntervalGuard::new(1);
    let (activity, _search) = two_triangles_and_a_self_edge();
    let mut clock = ScriptedClock::new(1);
    let candidates = super::enum_gen::enumerate_active_circuits(&activity, None, &mut clock);
    assert!(candidates.complete);
    assert_eq!(clock.reads, 0);
}

#[test]
fn retain_circuits_abandons_an_already_expired_deadline() {
    let (activity, search) = two_triangles_and_a_self_edge();
    let candidates = super::enum_gen::enumerate_active_circuits(&activity, None, &mut SystemClock);
    let (stock_partition, no_modules, no_agg_nodes) = two_triangle_retention_metadata(&search);
    let mut clock = ScriptedClock::new(1);
    let deadline = clock.deadline();
    assert!(
        super::enum_gen::retain_circuits(
            &candidates,
            &activity,
            &stock_partition,
            &no_modules,
            &no_agg_nodes,
            &mut |_, _, _| None,
            Some(deadline),
            &mut clock,
        )
        .is_none(),
        "retention over a full universe can outlast the budget on its own, so \
         it must be able to abandon and let the fallback run"
    );
    assert_eq!(
        clock.reads, 1,
        "the check runs before the first circuit is scored"
    );
}

#[test]
fn retain_circuits_never_reads_the_clock_when_unbudgeted() {
    let (activity, search) = two_triangles_and_a_self_edge();
    let candidates = super::enum_gen::enumerate_active_circuits(&activity, None, &mut SystemClock);
    let (stock_partition, no_modules, no_agg_nodes) = two_triangle_retention_metadata(&search);
    let mut clock = ScriptedClock::new(1);
    assert!(
        super::enum_gen::retain_circuits(
            &candidates,
            &activity,
            &stock_partition,
            &no_modules,
            &no_agg_nodes,
            &mut |_, _, _| None,
            None,
            &mut clock,
        )
        .is_some(),
        "an unbudgeted retention always completes"
    );
    assert_eq!(clock.reads, 0);
}

/// The WORK-based deadline trigger (`RETENTION_DEADLINE_CHECK_EDGE_STEPS`)
/// catches a deadline expiring mid-pass on a model the CIRCUIT-count trigger
/// alone cannot bound: two circuits never reach `RETENTION_DEADLINE_CHECK_CIRCUITS`
/// (4096) as a multiple after circuit 0, so without the work-based trigger
/// this fixture's second circuit would score unchecked. Each circuit here is
/// "long" in the sense that matters to the check -- `len * window`, not node
/// count -- by being active over MANY saved steps.
#[test]
fn retain_circuits_work_trigger_catches_a_deadline_expiring_mid_pass() {
    // Two independent 2-cycles (a<->b, c<->d), both active across every
    // saved step from 1 on, so each circuit's own scoring work is
    // `len(2) * window(step_count - 1)`.
    let link_offsets: Vec<LinkOffset> = vec![
        ((Ident::new("a"), Ident::new("b")), 0),
        ((Ident::new("b"), Ident::new("a")), 1),
        ((Ident::new("c"), Ident::new("d")), 2),
        ((Ident::new("d"), Ident::new("c")), 3),
    ];
    let n_offsets = 4;
    let step_count = 200;
    let mut data = vec![0.0f64; n_offsets * step_count];
    for step in 1..step_count {
        let base = step * n_offsets;
        data[base] = 1.0; // a->b
        data[base + 1] = 1.0; // b->a
        data[base + 2] = 1.0; // c->d
        data[base + 3] = 1.0; // d->c
    }
    let results = enum_results(n_offsets, step_count, data);
    let stocks = stock_list(&["a", "c"]);
    let search = IndexedSearch::build(&link_offsets, &stocks);
    let activity = super::enum_gen::ActivityGraph::build(&search, &results, None, &mut SystemClock)
        .expect("an unbudgeted build never abandons");
    let candidates = super::enum_gen::enumerate_active_circuits(&activity, None, &mut SystemClock);
    assert!(candidates.complete);
    assert_eq!(candidates.len(), 2, "the two independent 2-cycles");

    let stock_partition: Vec<Option<usize>> = search
        .idents
        .iter()
        .map(|id| match id.as_str() {
            "a" => Some(0),
            "c" => Some(1),
            _ => None,
        })
        .collect();
    let no_modules = vec![false; search.idents.len()];
    let no_agg_nodes = vec![false; search.idents.len()];

    // Each circuit's work is `2 * 199 = 398` edge-steps. An interval of 256
    // sits strictly between one circuit's work and two, so the work-based
    // trigger must fire when the SECOND circuit's check runs (having
    // accumulated the first circuit's 398 edge-steps since the mandatory
    // circuit-0 check) -- well before circuit-count could ever see two
    // circuits as a multiple of `RETENTION_DEADLINE_CHECK_CIRCUITS` (4096).
    let _interval_guard = super::enum_gen::RetentionDeadlineCheckEdgeStepsGuard::new(256);
    // Read 1 (the mandatory circuit-0 check) is not yet expired, so circuit 0
    // scores; read 2 (the work-triggered circuit-1 check) IS expired.
    let mut clock = ScriptedClock::new(2);
    let deadline = clock.deadline();
    assert!(
        super::enum_gen::retain_circuits(
            &candidates,
            &activity,
            &stock_partition,
            &no_modules,
            &no_agg_nodes,
            &mut |_, _, _| None,
            Some(deadline),
            &mut clock,
        )
        .is_none(),
        "the work-based trigger must catch the deadline mid-pass, after \
         circuit 0's edge-steps crossed the interval but before circuit 1 \
         would otherwise have gone unchecked"
    );
    assert_eq!(
        clock.reads, 2,
        "two checks: the mandatory circuit-0 check (not yet expired) and the \
         work-triggered circuit-1 check (expired) -- without the work-based \
         trigger, only the first would ever run on a two-circuit universe"
    );
}

/// The enumerator's two exactness claims -- min-root canonicalization (every
/// cycle emitted exactly once) and the per-root induced-SCC restriction
/// (Johnson's `A_k`, which refuses branches that provably cannot return to the
/// root) -- are properties no single fixture can establish, since both are
/// about circuits that must NOT be missed or duplicated.
///
/// Compare against a brute-force reference on pseudorandom graphs: every
/// elementary cycle, walked with no pruning of any kind from every start node,
/// filtered to those simultaneously active at some saved step. The reference
/// shares no code with the enumerator, so it arbitrates both claims at once.
#[test]
fn enumerator_matches_brute_force_active_cycles_on_synthetic_graphs() {
    // A dense 6-node core plus a sink-only node. No parallel edges: the union
    // graph gives one row per (from, to) pair, which is what
    // `parse_link_offsets` guarantees in production.
    let names = ["a", "b", "c", "d", "e", "f"];
    let mut edge_pairs: Vec<(&str, &str)> = Vec::new();
    for &from in &names {
        for &to in &names {
            edge_pairs.push((from, to)); // self-edges included, and dropped
        }
    }
    edge_pairs.push(("a", "g")); // a node with no outbound edges
    let link_offsets: Vec<LinkOffset> = edge_pairs
        .iter()
        .enumerate()
        .map(|(i, (from, to))| ((Ident::new(from), Ident::new(to)), i))
        .collect();
    let stocks = stock_list(&["a", "c", "e"]);

    for seed in [1u64, 7, 42, 1000, 999_983] {
        let results = synthetic_results(link_offsets.len(), 12, seed);
        let search = IndexedSearch::build(&link_offsets, &stocks);
        let activity =
            super::enum_gen::ActivityGraph::build(&search, &results, None, &mut SystemClock)
                .expect("an unbudgeted build never abandons");
        let candidates =
            super::enum_gen::enumerate_active_circuits(&activity, None, &mut SystemClock);
        assert!(candidates.complete, "seed {seed} must enumerate fully");

        let mut enumerated: Vec<Vec<String>> = (0..candidates.len())
            .map(|ci| {
                let nodes: Vec<String> = activity
                    .circuit_nodes(candidates.circuit(ci))
                    .iter()
                    .map(|&n| search.idents[n as usize].as_str().to_string())
                    .collect();
                crate::ltm::canonical_rotation(&nodes)
            })
            .collect();
        let before_dedup = enumerated.len();
        enumerated.sort();
        enumerated.dedup();
        assert_eq!(
            enumerated.len(),
            before_dedup,
            "seed {seed}: every cycle must be emitted exactly once"
        );

        let mut expected = brute_force_active_cycles(&results, &link_offsets);
        expected.sort();
        assert!(
            !expected.is_empty(),
            "seed {seed} fixture must produce cycles"
        );
        assert_eq!(
            enumerated, expected,
            "seed {seed}: the enumerated set must be the active-cycle universe"
        );
    }
}

/// Every elementary cycle (length >= 2) of the union edge set that is
/// simultaneously active at some saved step in `1..step_count`, derived
/// straight from the raw offsets and walked with no pruning: from every start
/// node, extend every simple path and emit on return to the start.
///
/// Deliberately independent of the enumerator -- no min-root restriction, no
/// SCC restriction, no running activity AND -- so it can arbitrate both.
fn brute_force_active_cycles(results: &Results, link_offsets: &[LinkOffset]) -> Vec<Vec<String>> {
    let step_count = results.step_count;
    let mut nodes: Vec<String> = Vec::new();
    let mut id_of: HashMap<String, usize> = HashMap::new();
    let mut intern = |name: &str, nodes: &mut Vec<String>| -> usize {
        *id_of.entry(name.to_string()).or_insert_with(|| {
            nodes.push(name.to_string());
            nodes.len() - 1
        })
    };

    // Union edges: non-self pairs active at >= 1 step in 1..step_count.
    let mut edges: Vec<(usize, usize, Vec<bool>)> = Vec::new();
    for ((from, to), offset) in link_offsets {
        if from == to {
            continue;
        }
        let mut active = vec![false; step_count];
        let mut any = false;
        for (step, slot) in active.iter_mut().enumerate().skip(1) {
            let value = results.data[step * results.step_size + offset];
            if value != 0.0 && value.is_finite() || value.is_infinite() {
                *slot = true;
                any = true;
            }
        }
        if any {
            let f = intern(from.as_str(), &mut nodes);
            let t = intern(to.as_str(), &mut nodes);
            edges.push((f, t, active));
        }
    }
    let n = nodes.len();
    let mut adj: Vec<Vec<(usize, usize)>> = vec![Vec::new(); n];
    for (i, (from, to, _)) in edges.iter().enumerate() {
        adj[*from].push((*to, i));
    }

    let mut found: Vec<Vec<String>> = Vec::new();
    let mut on_path = vec![false; n];
    let mut path: Vec<usize> = Vec::new();
    let mut used: Vec<usize> = Vec::new();
    for start in 0..n {
        walk_simple_cycles(
            start,
            start,
            &adj,
            &edges,
            step_count,
            &mut on_path,
            &mut path,
            &mut used,
            &mut nodes.clone(),
            &mut found,
        );
    }
    found.sort();
    found.dedup();
    found
}

/// Recursive helper for [`brute_force_active_cycles`]: extend the simple path
/// ending at `at`, emitting whenever it closes back on `start` with at least
/// two nodes and at least one step where every used edge is active.
#[allow(clippy::too_many_arguments)]
fn walk_simple_cycles(
    start: usize,
    at: usize,
    adj: &[Vec<(usize, usize)>],
    edges: &[(usize, usize, Vec<bool>)],
    step_count: usize,
    on_path: &mut [bool],
    path: &mut Vec<usize>,
    used: &mut Vec<usize>,
    nodes: &mut [String],
    found: &mut Vec<Vec<String>>,
) {
    on_path[at] = true;
    path.push(at);
    for &(to, edge) in &adj[at] {
        if to == start {
            if path.len() >= 2 {
                used.push(edge);
                let active = (1..step_count).any(|t| used.iter().all(|&e| edges[e].2[t]));
                if active {
                    let names: Vec<String> = path.iter().map(|&n| nodes[n].clone()).collect();
                    found.push(crate::ltm::canonical_rotation(&names));
                }
                used.pop();
            }
        } else if !on_path[to] {
            used.push(edge);
            walk_simple_cycles(
                start, to, adj, edges, step_count, on_path, path, used, nodes, found,
            );
            used.pop();
        }
    }
    path.pop();
    on_path[at] = false;
}

/// A module-input edge's activity is read through the composite's NaN shadow:
/// its own slot (the module COMPOSITE) is NaN at the only step the cycle could
/// score, but a per-pathway slot behind it is finite, so the edge is active
/// there, the union graph carries the cycle, the enumerator emits it with the
/// pathway value in its score row, and the fallback closes it too. Without the
/// pathway slots (the control below) the same fixture holds no cycle at all --
/// which is the blind spot `IndexedEdge::value_at` exists to close.
#[test]
fn a_module_input_edge_is_active_where_a_pathway_is_finite_under_a_nan_composite() {
    // Slots: 0 = a->b composite, 1 = b->a, 2 = a->b pathway. Two steps.
    let link_offsets: Vec<LinkOffset> = vec![
        ((Ident::new("a"), Ident::new("b")), 0),
        ((Ident::new("b"), Ident::new("a")), 1),
    ];
    let n_offsets = 3;
    let step_count = 2;
    let mut data = vec![0.0f64; n_offsets * step_count];
    data[n_offsets] = f64::NAN; // a->b composite: shadowed
    data[n_offsets + 1] = 1.0; // b->a
    data[n_offsets + 2] = 0.5; // a->b pathway: finite
    let results = enum_results(n_offsets, step_count, data);
    let stocks = stock_list(&["a"]);

    // Control: composite-only activity sees no cycle.
    let bare = IndexedSearch::build(&link_offsets, &stocks);
    let activity = super::enum_gen::ActivityGraph::build(&bare, &results, None, &mut SystemClock)
        .expect("an unbudgeted build never abandons");
    let candidates = super::enum_gen::enumerate_active_circuits(&activity, None, &mut SystemClock);
    assert!(candidates.complete);
    assert_eq!(candidates.len(), 0, "the NaN composite hides the cycle");

    // With the pathway slot attached the edge is active at step 1.
    let mut search = IndexedSearch::build(&link_offsets, &stocks);
    let a: Ident<Canonical> = Ident::new("a");
    let b: Ident<Canonical> = Ident::new("b");
    search.attach_module_pathways(&mut |from, to| {
        if *from == a && *to == b {
            vec![2]
        } else {
            Vec::new()
        }
    });
    let activity = super::enum_gen::ActivityGraph::build(&search, &results, None, &mut SystemClock)
        .expect("an unbudgeted build never abandons");
    let candidates = super::enum_gen::enumerate_active_circuits(&activity, None, &mut SystemClock);
    assert!(candidates.complete);
    assert_eq!(
        canonical_name_cycles(
            &search,
            &(0..candidates.len())
                .map(|i| activity.circuit_nodes(candidates.circuit(i)))
                .collect::<Vec<_>>()
        ),
        vec![vec!["a".to_string(), "b".to_string()]],
        "the pathway makes the edge, and so the cycle, active"
    );
    // Only ACTIVITY is repaired: the score row keeps the recorded composite
    // (NaN here), so a circuit whose per-exit-port override does not resolve
    // scores NaN at this step -- exactly what materialization reports --
    // rather than banking a pathway value the returned loop never carries.
    let row = activity.edge_row_of(0, 1).expect("a->b is a union edge");
    assert!(activity.score_at(row, 1).is_nan());

    let outcome = super::fallback::sweep(
        &search,
        &results,
        FallbackConfig::DEFAULT,
        None,
        &mut SystemClock,
    );
    assert_eq!(
        canonical_name_cycles(&search, &outcome.paths),
        vec![vec!["a".to_string(), "b".to_string()]],
        "the fallback reads the same repaired activity"
    );
}

/// A Solo loop's relative score is +/-1 by construction, so the ranking can
/// only ever report the strongest `max_loops()` of them; retention keeps
/// exactly those (by mean |reported score|, index order among ties) and drops
/// the rest before anything is materialized -- the difference, on a stockless
/// component with very many mass-bearing cycles, between materializing the
/// cap's worth of loops and all of them. The distinct-loop count still counts
/// every mass-bearing Solo circuit: they are all real loops of the universe.
#[test]
fn retention_keeps_only_the_strongest_max_loops_solo_circuits() {
    // Three disjoint 2-cycles, no stocks anywhere (all Solo), with means
    // 3.0 (a<->b), 1.0 (c<->d), 2.0 (e<->f) over the two saved steps.
    let link_offsets: Vec<LinkOffset> = vec![
        ((Ident::new("a"), Ident::new("b")), 0),
        ((Ident::new("b"), Ident::new("a")), 1),
        ((Ident::new("c"), Ident::new("d")), 2),
        ((Ident::new("d"), Ident::new("c")), 3),
        ((Ident::new("e"), Ident::new("f")), 4),
        ((Ident::new("f"), Ident::new("e")), 5),
    ];
    let n_offsets = 6;
    let step_count = 2;
    let mut data = vec![0.0f64; n_offsets * step_count];
    data[n_offsets] = 6.0; // a->b   (product 6 at step 1; mean over 2 steps = 3)
    data[n_offsets + 1] = 1.0; // b->a
    data[n_offsets + 2] = 2.0; // c->d   (product 2; mean 1)
    data[n_offsets + 3] = 1.0; // d->c
    data[n_offsets + 4] = 4.0; // e->f   (product 4; mean 2)
    data[n_offsets + 5] = 1.0; // f->e
    let results = enum_results(n_offsets, step_count, data);
    let search = IndexedSearch::build(&link_offsets, &stock_list(&[]));
    let activity = super::enum_gen::ActivityGraph::build(&search, &results, None, &mut SystemClock)
        .expect("an unbudgeted build never abandons");
    let candidates = super::enum_gen::enumerate_active_circuits(&activity, None, &mut SystemClock);
    assert!(candidates.complete);
    assert_eq!(candidates.len(), 3);
    let solo: Vec<Option<usize>> = vec![None; search.idents.len()];
    let no_modules = vec![false; search.idents.len()];
    let no_agg = vec![false; search.idents.len()];

    let _guard = MaxLoopsGuard::new(2);
    let outcome = super::enum_gen::retain_circuits(
        &candidates,
        &activity,
        &solo,
        &no_modules,
        &no_agg,
        &mut |_, _, _| None,
        None,
        &mut SystemClock,
    )
    .unwrap();
    assert_eq!(
        survivor_node_sets(&outcome, &candidates, &activity, &search),
        vec![
            vec!["a".to_string(), "b".to_string()],
            vec!["e".to_string(), "f".to_string()],
        ],
        "the two strongest Solo loops survive; the weakest is dropped by the cap"
    );
    assert_eq!(
        outcome.distinct_circuits, 3,
        "every mass-bearing Solo loop is still a member of the universe"
    );
}

/// A partition total is accumulated saturating: two finite masses whose sum
/// overflows f64 leave the total at f64::MAX rather than Inf, so each loop's
/// share stays finite (0.5 here) and both clear retention -- an Inf total would
/// have read every finite share as 0 and dropped a real universe wholesale.
#[test]
fn a_finite_partition_total_saturates_instead_of_overflowing_to_inf() {
    let link_offsets: Vec<LinkOffset> = vec![
        ((Ident::new("a"), Ident::new("b")), 0),
        ((Ident::new("a"), Ident::new("c")), 1),
        ((Ident::new("b"), Ident::new("a")), 2),
        ((Ident::new("c"), Ident::new("a")), 3),
    ];
    let n_offsets = 4;
    let step_count = 2;
    let mut data = vec![0.0f64; n_offsets * step_count];
    data[n_offsets] = 1e308; // a->b
    data[n_offsets + 1] = 1e308; // a->c
    data[n_offsets + 2] = 1.0; // b->a
    data[n_offsets + 3] = 1.0; // c->a
    assert!(
        (1e308f64 + 1e308f64).is_infinite(),
        "the fixture must overflow"
    );
    let results = enum_results(n_offsets, step_count, data);
    let search = IndexedSearch::build(&link_offsets, &stock_list(&["a"]));
    let activity = super::enum_gen::ActivityGraph::build(&search, &results, None, &mut SystemClock)
        .expect("an unbudgeted build never abandons");
    let candidates = super::enum_gen::enumerate_active_circuits(&activity, None, &mut SystemClock);
    assert_eq!(candidates.len(), 2);
    let stock_partition: Vec<Option<usize>> = search
        .idents
        .iter()
        .map(|id| (id.as_str() == "a").then_some(0))
        .collect();
    let no_modules = vec![false; search.idents.len()];
    let no_agg = vec![false; search.idents.len()];
    let outcome = super::enum_gen::retain_circuits(
        &candidates,
        &activity,
        &stock_partition,
        &no_modules,
        &no_agg,
        &mut |_, _, _| None,
        None,
        &mut SystemClock,
    )
    .unwrap();
    assert_eq!(
        outcome.partition_totals[&0][1],
        f64::MAX,
        "saturated, not Inf"
    );
    assert_eq!(outcome.survivors.len(), 2, "both loops keep a finite share");
    assert_eq!(outcome.partition_circuit_counts[&0], 2);
}

/// Retention keeps only the strongest `max_loops()` Solo loops for
/// materialization, but every mass-bearing Solo loop PASSED retention, so the
/// reported `retained_loops` counts them all: a capped stockless report reads
/// `retained_loops > len(loops)`, not as the whole retained set.
#[test]
fn retained_loops_counts_the_solo_survivors_retention_did_not_materialize() {
    let project = enum_test_project(vec![
        enum_aux("a", "PREVIOUS(b, 1) * 0.5 + 1"),
        enum_aux("b", "PREVIOUS(a, 0) * 0.5"),
        enum_aux("c", "PREVIOUS(d, 1) * 0.4 + 1"),
        enum_aux("d", "PREVIOUS(c, 0) * 0.4"),
        enum_aux("e", "PREVIOUS(f, 1) * 0.3 + 1"),
        enum_aux("f", "PREVIOUS(e, 0) * 0.3"),
    ]);
    let _guard = MaxLoopsGuard::new(2);
    let auto = discover_project(&project, CandidateGen::Auto);
    assert!(auto.enumeration_complete);
    assert_eq!(
        auto.universe_loops,
        Some(3),
        "three Solo loops in the universe"
    );
    assert_eq!(auto.loops.len(), 2, "the cap holds two");
    assert_eq!(
        auto.retained_loops, 3,
        "all three passed retention; the cap, not retention, dropped the third"
    );
}

/// The mean |score| statistic is a running mean: two finite values whose SUM
/// overflows still have their representable mean, so twin representatives and
/// Solo ranks are decided on strength rather than on an Inf-vs-Inf tie.
#[test]
fn the_mean_abs_statistic_does_not_overflow_where_the_sum_would() {
    let series = [1e308f64, 1e308, 0.0];
    let nan_mask = [false, false, true];
    assert!((1e308f64 + 1e308f64).is_infinite());
    let mean = super::enum_gen::mean_abs_over_valid(&series, &nan_mask);
    assert_eq!(mean, 1e308);
}

/// An infinite observation makes the mean |score| infinite rather than NaN:
/// folding `Inf` into the running update would produce `Inf - Inf`, and a NaN
/// on both sides of a comparison would hand a twin's representative or a Solo
/// loop's rank to circuit order instead of to the divergent loop.
#[test]
fn the_mean_abs_statistic_stays_infinite_when_an_observation_is_infinite() {
    let series = [1.0f64, f64::INFINITY, 2.0];
    let nan_mask = [false, false, false];
    assert_eq!(
        super::enum_gen::mean_abs_over_valid(&series, &nan_mask),
        f64::INFINITY
    );
    let series2 = [f64::INFINITY, f64::INFINITY];
    assert_eq!(
        super::enum_gen::mean_abs_over_valid(&series2, &[false, false]),
        f64::INFINITY
    );
}

/// A stitched loop is charged against the enumeration's storage bound like
/// any circuit: with no room left, `push_node_path` refuses (pushing nothing)
/// so the caller treats the enumeration as incomplete rather than allocating
/// past the bound.
#[test]
fn a_stitched_loop_over_the_storage_bound_is_refused() {
    let (activity, _search) = two_triangles_and_a_self_edge();
    let mut candidates =
        super::enum_gen::enumerate_active_circuits(&activity, None, &mut SystemClock);
    assert!(candidates.complete);
    let before = candidates.len();
    // The a->b->c triangle as a node path (ids 0,1,2 by interning order).
    let _guard = EnumBudgetGuard::new(usize::MAX, u64::MAX, 1);
    assert!(
        !candidates.push_node_path(&[0, 1, 2], &activity),
        "one row-equivalent of budget cannot hold a three-edge loop"
    );
    assert_eq!(candidates.len(), before, "nothing was pushed");
}
