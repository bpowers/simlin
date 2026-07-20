// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Tests for the `PerElement` ROW PINNING -- specifically the three pin-only
//! descents the ceteris-paribus wrap runs where it deliberately stops walking: a
//! pre-existing `PREVIOUS`/`INIT` call, the GH #517 whole-frozen reducer, and a
//! `LOOKUP` table argument.
//!
//! Split out of `ltm_augment_tests.rs` for the per-file line cap, and mounted as a
//! child of its `tests` module, so `use super::*` resolves every helper that file
//! already has in scope (`PinFixture`, `make_named_dimension`, `deps_set`,
//! `build_wrap_test_occurrences`, ...).
//!
//! Each descent has exactly ONE test that fails when that descent is deleted --
//! the probe matrix is in the branch's commit messages. That property is the point
//! of the file: the regression these commits fix existed because two of the three
//! descents were probed and the third was not, and deleting the un-probed one left
//! the entire 5000-test corpus green.
//!
//! The pinning has TWO obligations and the tests are organized around them:
//! a fragment must COMPILE (no dimension-name subscript may survive into a scalar
//! equation), and it must stay CETERIS PARIBUS (no unattributed live read). The
//! table-argument descent is the only one whose enclosing node does not discharge
//! the second by itself, which is where both defects on this branch landed.

use super::*;

/// The per-element row pinning must descend into a subscript RANGE's endpoints,
/// not just its plain expression indices.
///
/// A source reference can hide in a range bound under another variable's
/// subscript (`other[pop[region]:3]`). The IR walker records an occurrence there
/// (`IndexExpr2::Range` pushes children 0 and 1) and the sibling lowering
/// `substitute_reducers_in_expr0` descends both endpoints -- but the pinning
/// once matched only `IndexExpr0::Expr` and passed a `Range` through untouched.
/// The recorded occurrence was then left un-pinned: its dimension-name subscript
/// survived into the scalar per-element equation, which either fails to compile
/// (a `PREVIOUS`-of-dim-name capture helper) or reads the wrong element.
///
/// Exercised through [`pin_only_source_refs`], the descent the wrap runs under a
/// subtree it froze -- which is where a range bound under a frozen other-dep
/// lands.
#[test]
fn per_element_pin_descends_into_range_endpoints() {
    use crate::dimensions::DimensionsContext;
    use crate::ltm_agg::AxisRead;

    let region = make_named_dimension("region", &["nyc", "boston"]);
    let from_dims = vec![region.clone()];
    let source_dim_elements = vec![vec!["nyc".to_string(), "boston".to_string()]];
    let source_dim_names = vec!["region".to_string()];
    let target_iterated_dims = vec!["region".to_string()];
    let dim_ctx = DimensionsContext::from(
        [datamodel::Dimension::named(
            "region".to_string(),
            vec!["nyc".to_string(), "boston".to_string()],
        )]
        .as_slice(),
    );
    let iter_ctx = IteratedDimCtx {
        source_dim_names: &source_dim_names,
        target_iterated_dims: &target_iterated_dims,
        dep_dims: None,
    };
    let from = Ident::<Canonical>::new("pop");
    let site_axes = vec![AxisRead::Iterated {
        dim: "region".to_string(),
        source_dim: "region".to_string(),
    }];
    let row_parts_bare = vec!["boston".to_string()];
    let mut target_elem_by_dim = HashMap::new();
    target_elem_by_dim.insert("region".to_string(), ("boston".to_string(), 1usize));

    let ctx = super::post_transform::PerElementRefCtx {
        from: &from,
        site_axes: &site_axes,
        row_parts_bare: &row_parts_bare,
        from_dims: &from_dims,
        target_elem_by_dim: &target_elem_by_dim,
        dim_ctx: &dim_ctx,
    };

    // `pop[region]` sits in the LOWER bound of a range index of `other`.
    let ast = Expr0::new("other[pop[region]:3]", LexerType::Equation)
        .expect("fixture parses")
        .expect("fixture is non-empty");
    // The occurrence stream the pinning reads: `pop[region]` is recorded at the
    // range's lower-bound child of `other`'s first index.
    let deps = deps_set(&["other"]);
    let occurrences = build_wrap_test_occurrences(
        &ast,
        &from,
        &deps,
        &source_dim_elements,
        Some(&iter_ctx),
        Some(&dim_ctx),
    );
    let slot_occurrences = SlotOccurrences::new(&occurrences);
    let occ = slot_occurrences.for_slot(0);
    assert!(
        !occ.is_empty(),
        "the fixture must record the range-bound occurrence, or the pin has \
         nothing to read and this test passes vacuously"
    );
    let mut unlowerable = false;
    let lowered =
        super::post_transform::pin_only_source_refs(ast, &ctx, &occ, &[], &mut unlowerable, true);
    assert!(
        !unlowerable,
        "the range-bound occurrence IS recorded, so the pin must lower it rather than \
         reporting it unlowerable"
    );
    let text = print_eqn(&lowered);

    assert!(
        !text.contains("pop[region]"),
        "the source reference inside the range bound must be pinned, not left \
         with its dimension-name subscript; got: {text}"
    );
    assert!(
        text.contains("boston"),
        "the range-bound occurrence must be pinned to this instantiation's row; \
         got: {text}"
    );
}

/// The shared `(Region, Age)` context for the `PerElement` pin-only-descent
/// tests: source `pop[Region, Age]` (`region = [nyc, boston]`,
/// `age = [young, old]`), target `growth[Region]` instantiated at
/// `region\u{B7}boston`, emitting site `pop[Region, young]` (one `Iterated` axis,
/// one `Pinned`).
///
/// The descents differ only in the EQUATION they walk, so the context lives here
/// once. Spelled out per test it is ~60 lines of identical scaffolding, which is
/// exactly where the one interesting line gets lost.
struct PinFixture {
    from: Ident<Canonical>,
    from_dims: Vec<crate::dimensions::Dimension>,
    source_dim_elements: Vec<Vec<String>>,
    source_dim_names: Vec<String>,
    target_iterated_dims: Vec<String>,
    dim_ctx: crate::dimensions::DimensionsContext,
    site_axes: Vec<crate::ltm_agg::AxisRead>,
    row_parts_bare: Vec<String>,
    target_elem_by_dim: HashMap<String, (String, usize)>,
}

impl PinFixture {
    /// `extra_dims` are project dimensions BEYOND the source's own two: a test
    /// needs one to reach the "index names another dimension" arm, and the
    /// source's `from_dims` deliberately stay `[region, age]` so such an index
    /// is genuinely un-resolvable by name.
    fn new(extra_dims: Vec<datamodel::Dimension>) -> Self {
        use crate::ltm_agg::AxisRead;
        let mut project_dims = vec![
            datamodel::Dimension::named(
                "region".to_string(),
                vec!["nyc".to_string(), "boston".to_string()],
            ),
            datamodel::Dimension::named(
                "age".to_string(),
                vec!["young".to_string(), "old".to_string()],
            ),
        ];
        project_dims.extend(extra_dims);
        let mut target_elem_by_dim = HashMap::new();
        target_elem_by_dim.insert("region".to_string(), ("boston".to_string(), 1usize));
        PinFixture {
            from: Ident::<Canonical>::new("pop"),
            from_dims: vec![
                make_named_dimension("region", &["nyc", "boston"]),
                make_named_dimension("age", &["young", "old"]),
            ],
            source_dim_elements: vec![
                vec!["nyc".to_string(), "boston".to_string()],
                vec!["young".to_string(), "old".to_string()],
            ],
            source_dim_names: vec!["region".to_string(), "age".to_string()],
            target_iterated_dims: vec!["region".to_string()],
            dim_ctx: crate::dimensions::DimensionsContext::from(project_dims.as_slice()),
            site_axes: vec![
                AxisRead::Iterated {
                    dim: "region".to_string(),
                    source_dim: "region".to_string(),
                },
                AxisRead::Pinned("young".to_string()),
            ],
            row_parts_bare: vec!["boston".to_string(), "young".to_string()],
            target_elem_by_dim,
        }
    }

    fn iter_ctx(&self) -> IteratedDimCtx<'_> {
        IteratedDimCtx {
            source_dim_names: &self.source_dim_names,
            target_iterated_dims: &self.target_iterated_dims,
            // The `PerElement` live shape suppresses the GH #526 other-dep
            // collapse entirely, so the verdict's `dep_dims` are never consulted.
            dep_dims: None,
        }
    }

    /// Parse `eqn` and build the occurrence stream `db::ltm_ir` would record for
    /// it -- the builder mirrors the IR, `LOOKUP` table-argument skip included,
    /// so the streams these tests read are the production ones.
    fn parse(
        &self,
        eqn: &str,
        deps: &[&str],
    ) -> (
        Expr0,
        HashSet<Ident<Canonical>>,
        Vec<crate::db::ltm_ir::OccurrenceSite>,
    ) {
        let deps = deps_set(deps);
        let ast = Expr0::new(eqn, LexerType::Equation)
            .expect("fixture parses")
            .expect("fixture is non-empty");
        let occurrences = build_wrap_test_occurrences(
            &ast,
            &self.from,
            &deps,
            &self.source_dim_elements,
            Some(&self.iter_ctx()),
            Some(&self.dim_ctx),
        );
        (ast, deps, occurrences)
    }

    /// How many occurrences of the SOURCE the stream records -- every test's
    /// non-vacuity check, since a stream that recorded nothing would let a
    /// do-nothing pin pass.
    fn source_occurrences(occurrences: &[crate::db::ltm_ir::OccurrenceSite]) -> usize {
        occurrences
            .iter()
            .filter(
                |o| matches!(&o.reference, crate::db::ltm_ir::OccurrenceRef::Variable(v) if v == "pop"),
            )
            .count()
    }

    fn generate(
        &self,
        ast: &Expr0,
        deps: &HashSet<Ident<Canonical>>,
        occ: &OccurrenceLookup<'_>,
    ) -> Result<String, PartialEquationError> {
        generate_per_element_link_equation(
            "pop",
            "growth",
            &self.site_axes,
            &self.row_parts_bare,
            "region\u{B7}boston",
            ast,
            deps,
            // Nothing to element-pin by name: the source's pinning is the wrap's job.
            &HashSet::new(),
            &self.from_dims,
            &self.target_elem_by_dim,
            &self.target_iterated_dims,
            &self.dim_ctx,
            None,
            occ,
        )
    }
}

/// A `PerElement` source occurrence nested inside a WHOLE-FROZEN array reducer
/// must still be row-pinned.
///
/// `growth[Region] = pop[Region, young] + SUM(other[pop[Region, young], *])` has
/// two occurrences of the emitting site's shape. The first is the live one. The
/// second sits in a subscript INDEX inside the reducer, and
/// `OccurrenceLookup::subtree_has_live_shape` excludes index-nested occurrences
/// (the GH #517 / Fig. 2 Q4 rule), so the reducer carries no live reference and
/// the wrap freezes it WHOLE without descending.
///
/// The wrap therefore has to pin that occurrence through its pin-only descent.
/// Left un-pinned, `pop[region, young]` -- a DIMENSION-name subscript -- survives
/// into a scalar link-score fragment, where it needs a `PREVIOUS`-of-dim-name
/// capture helper that cannot compile: the fragment is dropped, the variable
/// keeps a layout slot with no bytecode, and the score reads a constant 0. That
/// is the silent-zero class this track exists to delete, and no char golden
/// reaches this shape -- deleting the descent leaves the whole corpus green.
#[test]
fn per_element_pin_reaches_inside_a_whole_frozen_reducer() {
    let fx = PinFixture::new(vec![]);
    let (ast, deps, occurrences) = fx.parse(
        "pop[Region, young] + SUM(other[pop[Region, young], *])",
        &["other"],
    );
    // Non-vacuity: the reducer-nested occurrence must actually be recorded, and
    // recorded as index-nested -- that bit is what makes the reducer freeze whole.
    assert!(
        occurrences.iter().any(|o| o.index_nested
            && matches!(&o.reference, crate::db::ltm_ir::OccurrenceRef::Variable(v) if v == "pop")),
        "the fixture must record an index-nested `pop` occurrence, or the reducer \
         would not freeze whole and this test would not exercise the descent"
    );
    let slot_occurrences = SlotOccurrences::new(&occurrences);
    let text = fx
        .generate(&ast, &deps, &slot_occurrences.for_slot(0))
        .expect("the per-element equation is derivable");

    assert!(
        !text.contains("pop[region, young]"),
        "the occurrence inside the whole-frozen reducer kept its DIMENSION-name \
         subscript, which cannot compile in a scalar fragment (a silent zero); \
         got: {text}"
    );
    assert!(
        text.contains("pop[region\u{B7}boston, age\u{B7}young]"),
        "the reducer-nested occurrence must be pinned to this instantiation's row, \
         QUALIFIED so the freeze compiles to a direct LoadPrev; got: {text}"
    );
    assert!(
        text.contains("pop[boston, young]"),
        "the LIVE occurrence must keep the bare row spelling; got: {text}"
    );
}

/// A `PerElement` source occurrence inside a PRE-EXISTING `PREVIOUS`/`INIT` call
/// must still be row-pinned.
///
/// `growth[Region] = pop[Region, young] + PREVIOUS(pop[Region, young]) +
/// INIT(pop[Region, old])` -- the wrap deliberately does not descend into an
/// already-lagged or already-frozen call (wrapping its contents again would read
/// two steps back and force a nested-PREVIOUS helper chain), so the row pinning
/// has to reach in through the pin-only descent. Left un-pinned,
/// `pop[region, young]` -- a DIMENSION-name subscript -- survives into a scalar
/// link-score fragment, which cannot compile: the fragment is dropped, the
/// variable keeps a layout slot with no bytecode, and the score reads a constant
/// 0.
///
/// This descent was the ONE of the three that no test and no char golden reached:
/// deleting it left all 5274 lib tests and all 634 integration tests green. Both
/// call names ride the same arm, so the fixture exercises `PREVIOUS` and `INIT`
/// together.
#[test]
fn per_element_pin_reaches_inside_a_pre_existing_previous_or_init() {
    let fx = PinFixture::new(vec![]);
    let (ast, deps, occurrences) = fx.parse(
        "pop[Region, young] + PREVIOUS(pop[Region, young]) + INIT(pop[Region, old])",
        &[],
    );
    // Non-vacuity: all three references are recorded, so the descent has real
    // occurrences to pin rather than silently doing nothing.
    assert_eq!(
        PinFixture::source_occurrences(&occurrences),
        3,
        "the fixture must record all three `pop` occurrences: {occurrences:?}"
    );
    let slot_occurrences = SlotOccurrences::new(&occurrences);
    let text = fx
        .generate(&ast, &deps, &slot_occurrences.for_slot(0))
        .expect("the per-element equation is derivable");

    assert!(
        !text.contains("pop[region, young]") && !text.contains("pop[region, old]"),
        "an occurrence inside a pre-existing PREVIOUS/INIT kept its DIMENSION-name \
         subscript, which cannot compile in a scalar fragment (a silent zero); \
         got: {text}"
    );
    assert!(
        text.contains("previous(pop[region\u{B7}boston, age\u{B7}young])"),
        "the PREVIOUS-nested occurrence must be pinned to this instantiation's row, \
         QUALIFIED so the lagged read compiles to a direct LoadPrev; got: {text}"
    );
    assert!(
        text.contains("init(pop[region\u{B7}boston, age\u{B7}old])"),
        "the INIT-nested occurrence rides the same arm and must be pinned to ITS OWN \
         row for this target element; got: {text}"
    );
    assert!(
        text.contains("pop[boston, young]"),
        "the LIVE occurrence must keep the bare row spelling; got: {text}"
    );
}

/// A `PerElement` source reference inside a `LOOKUP` **table** argument is
/// row-pinned STRUCTURALLY, and the partial is emitted.
///
/// `db::ltm_ir` records no occurrence under a table argument
/// (`BuiltinContents::LookupTable(_) => {}` -- "a graphical-function table
/// reference is static data, not a causal edge"). That is right about
/// ATTRIBUTION -- the reference is not a causal edge and earns no score -- but the
/// pin it needs is about COMPILABILITY: left un-pinned, `pop[region, old]` keeps
/// its DIMENSION-name subscript, which cannot resolve in a scalar link-score
/// fragment. The fragment is dropped, the variable keeps a layout slot with no
/// bytecode, and the score reads a constant 0.
///
/// Both facts hold at once, so `pin_dimension_name_indices` discharges the
/// lowering by NAME (the source's own declared dims) without recording an
/// occurrence or classifying a shape -- the same thing `pin_bare_source_ref` does
/// for a bare `Var`. This is the shape that regressed when the row pinning moved
/// into the wrap (`391bc3c1`): the previous pass over the wrapped tree
/// re-classified with the Expr0 classifier, which needs no occurrence, so it
/// pinned the table argument regardless.
///
/// The interim fix declined the whole partial LOUDLY instead, which threw away
/// more than the un-scoreable reference: this fixture's FIRST term is a perfectly
/// good scoreable site, and declining dropped every per-element score of the
/// `pop → growth` edge with it. So the assertion here is the pinned form AND the
/// live row's survival. No char golden reaches this shape (none has a `LOOKUP`
/// inside a partial at all), so nothing else catches it.
#[test]
fn per_element_pin_lowers_structurally_inside_a_lookup_table_arg() {
    let fx = PinFixture::new(vec![]);
    // The table argument reads a DIFFERENT element than the live site does
    // (`old` vs `young`), so a pin that merely echoed the site's own row would
    // pass vacuously. This is codex's exact shape: a target reading the source
    // both normally and in a table argument.
    let (ast, deps, occurrences) = fx.parse(
        "pop[Region, young] + LOOKUP(pop[Region, old], input)",
        &["input"],
    );
    // Non-vacuity: the LIVE occurrence outside the LOOKUP is recorded (so the
    // stream is not simply empty), while the table-argument one is not.
    assert_eq!(
        PinFixture::source_occurrences(&occurrences),
        1,
        "exactly the non-table occurrence is recorded: {occurrences:?}"
    );
    let slot_occurrences = SlotOccurrences::new(&occurrences);
    let text = fx
        .generate(&ast, &deps, &slot_occurrences.for_slot(0))
        .expect(
            "the table-argument reference is lowerable structurally, so the partial must \
             be emitted -- declining it also drops the live site's own score",
        );

    assert!(
        !text.contains("pop[region,"),
        "the table-argument reference kept its DIMENSION-name subscript, which \
         cannot resolve in a scalar fragment (a silent zero); got: {text}"
    );
    assert!(
        text.contains("pop[region\u{B7}boston, age\u{B7}old]"),
        "the table-argument reference must be pinned to THIS target element's own \
         coordinate for the iterated axis, with its literal element selector \
         qualified by the axis that declares it; got: {text}"
    );
    assert!(
        text.contains("lookup(pop[region\u{B7}boston, age\u{B7}old]"),
        "the pin must land inside the LOOKUP's table argument, which the wrap holds \
         verbatim; got: {text}"
    );
    assert!(
        text.contains("pop[boston, young]"),
        "the LIVE occurrence outside the LOOKUP must keep its bare row spelling -- \
         its score is exactly what the interim loud decline threw away; got: {text}"
    );
    assert!(
        !text.contains("previous(pop[region\u{B7}boston, age\u{B7}old]"),
        "a table argument is static data, never a value read, so the wrap must not \
         freeze it (a PREVIOUS of a table has no value slot); got: {text}"
    );
}

/// The loud net BEHIND the structural pin: a BARE table argument whose index the
/// rule cannot discharge statically is declined, and the whole partial abandoned.
///
/// The rule resolves exactly three index forms -- the axis's own dimension name,
/// an element that axis declares, and a numeric literal. Everything else is
/// declined here, for one of two DIFFERENT reasons, and the difference matters
/// because only one of them is unconditional:
///
/// - a RUNTIME read (`pop[Region, idx]`, `pop[Region, pop[Region, old]]`) COMPILES
///   fine; the problem is ceteris paribus. A BARE `LOOKUP` table argument is not
///   inside a freeze -- the wrap holds it verbatim, since a `PREVIOUS` of a table
///   has no value slot -- so the index stays LIVE, and
///   `codegen::extract_table_info` evaluates it to select the table element. The
///   `young` partial would then vary with the current-step value of `old`,
///   misattributing one row's influence to another. Pinning these purely because
///   it made the fragment COMPILE is what a reviewer caught on this branch. Inside
///   a freeze the same index is already lagged and is KEPT -- see
///   [`per_element_pin_keeps_a_runtime_table_index_inside_a_freeze`];
/// - the rule having NO ANSWER (`pop[State, old]` on a source declared over
///   `[Region, Age]`) is a compilability verdict and is loud everywhere: resolving
///   it would need to decide that `State` reads `Region` through a positional
///   mapping rather than being a mismatched or transposed axis, and that per-axis
///   CLASSIFICATION is `db::ltm_ir`'s -- which recorded nothing here.
///
/// Together with [`per_element_pin_lowers_structurally_inside_a_lookup_table_arg`]
/// and the frozen twin, this is the whole contract: statically-resolvable table
/// arguments are pinned and scored, already-lagged runtime ones are pinned as far
/// as they go, and the rest stays loud instead of emitting either an un-pinned
/// dimension-name subscript (a dropped fragment reading 0) or a plausible wrong
/// score.
#[test]
fn per_element_pin_declines_every_non_static_bare_table_arg_index() {
    // Each case is `(label, equation, deps, extra project dims)`. `state` is a
    // real dimension positionally parallel to `region` -- the shape whose
    // resolution would need the mapping classifier.
    let cases: [(&str, &str, &[&str], Vec<datamodel::Dimension>); 3] = [
        (
            "a variable index -- the simplest runtime element read",
            "pop[Region, young] + LOOKUP(pop[Region, idx], input)",
            &["input", "idx"],
            vec![],
        ),
        (
            "a nested source read selecting the element at runtime",
            "pop[Region, young] + LOOKUP(pop[Region, pop[Region, old]], input)",
            &["input"],
            vec![],
        ),
        (
            "another dimension's name, which only the IR can classify",
            "pop[Region, young] + LOOKUP(pop[State, old], input)",
            &["input"],
            vec![datamodel::Dimension::named(
                "state".to_string(),
                vec!["ny".to_string(), "ma".to_string()],
            )],
        ),
    ];

    for (label, eqn, deps, extra_dims) in cases {
        let fx = PinFixture::new(extra_dims);
        let (ast, deps, occurrences) = fx.parse(eqn, deps);
        // Non-vacuity: the table argument is the reference WITHOUT an occurrence
        // (the IR skips it whole), so the decline must come from the structural
        // rule rather than from the IR.
        assert_eq!(
            PinFixture::source_occurrences(&occurrences),
            1,
            "{label}: only the live occurrence outside the LOOKUP is recorded: \
             {occurrences:?}"
        );
        let slot_occurrences = SlotOccurrences::new(&occurrences);
        let err = fx.generate(&ast, &deps, &slot_occurrences.for_slot(0));
        assert!(
            matches!(
                err,
                Err(PartialEquationError {
                    kind: PartialEquationErrorKind::UnfreezablePartial,
                    ..
                })
            ),
            "{label}: must be a loud Err, never an Ok equation carrying an \
             un-pinned subscript or an unattributed live read: {err:?}"
        );
    }
}

/// The mirror of the decline: a runtime table index that is ALREADY inside a
/// freeze is kept, and the score survives.
///
/// `pop[Region, young] + PREVIOUS(LOOKUP(pop[Region, idx], input))` reaches the
/// very same no-occurrence table argument -- the IR skips a table argument wherever
/// it appears -- but here the enclosing `PREVIOUS` lags everything inside it, index
/// reads included, so `idx` is already ceteris-paribus and the partial is sound.
/// Refusing it (which an earlier revision of this branch did, because the refusal
/// keyed on "is a table argument" rather than "is not inside a freeze") drops a
/// perfectly good per-element score, and with it every loop score through the edge
/// -- the same over-decline that motivated this whole fix.
///
/// So the freeze context is threaded in, and `Region` is still pinned: the pin is
/// what makes the fragment compile, and it is orthogonal to the freeze.
#[test]
fn per_element_pin_keeps_a_runtime_table_index_inside_a_freeze() {
    for (label, eqn) in [
        (
            "inside a pre-existing PREVIOUS",
            "pop[Region, young] + PREVIOUS(LOOKUP(pop[Region, idx], input))",
        ),
        (
            "inside a whole-frozen reducer",
            "pop[Region, young] + SUM(other[LOOKUP(pop[Region, idx], input)])",
        ),
    ] {
        let fx = PinFixture::new(vec![]);
        let (ast, deps, occurrences) = fx.parse(eqn, &["input", "idx", "other"]);
        assert_eq!(
            PinFixture::source_occurrences(&occurrences),
            1,
            "{label}: the table argument is un-recorded, so the rule is what decides: \
             {occurrences:?}"
        );
        let slot_occurrences = SlotOccurrences::new(&occurrences);
        let text = fx
            .generate(&ast, &deps, &slot_occurrences.for_slot(0))
            .unwrap_or_else(|e| {
                panic!(
                    "{label}: the index is already lagged by the enclosing freeze, so \
                     the partial is ceteris-paribus and must be emitted: {e:?}"
                )
            });
        assert!(
            text.contains("pop[region\u{B7}boston, idx]"),
            "{label}: the iterated axis must still be pinned (that is what makes the \
             fragment compile) and the already-lagged runtime index left alone; \
             got: {text}"
        );
        assert!(
            text.contains("pop[boston, young]"),
            "{label}: the LIVE occurrence must keep its bare row spelling; got: {text}"
        );
    }

    // The other half of the split: a freeze answers the CETERIS-PARIBUS question,
    // never the COMPILABILITY one. `pop[State, old]` on a source declared over
    // `[Region, Age]` still has no spelling this rule can produce, so a
    // dimension-name subscript would survive into the scalar fragment and fail to
    // compile -- inside a freeze exactly as outside one. It must stay loud.
    let fx = PinFixture::new(vec![datamodel::Dimension::named(
        "state".to_string(),
        vec!["ny".to_string(), "ma".to_string()],
    )]);
    let (ast, deps, occurrences) = fx.parse(
        "pop[Region, young] + PREVIOUS(LOOKUP(pop[State, old], input))",
        &["input"],
    );
    assert_eq!(
        PinFixture::source_occurrences(&occurrences),
        1,
        "the table argument is un-recorded here too: {occurrences:?}"
    );
    let slot_occurrences = SlotOccurrences::new(&occurrences);
    let err = fx.generate(&ast, &deps, &slot_occurrences.for_slot(0));
    assert!(
        matches!(
            err,
            Err(PartialEquationError {
                kind: PartialEquationErrorKind::UnfreezablePartial,
                ..
            })
        ),
        "an index no pin can spell must stay loud even inside a freeze -- a freeze \
         cannot make a dimension-name subscript compile: {err:?}"
    );
}

/// The pin-only descent must reach a source reference nested inside a source
/// subscript's own INDEX EXPRESSION, not just inside a range bound -- inside a
/// FREEZE, where pinning is the whole job.
///
/// `PREVIOUS(pop[Region, pop[Region, young]])` reads a dynamically-selected
/// element of the source. The outer occurrence's second axis is not describable
/// per axis (it is an expression, not a coordinate), so that index reaches the
/// descent's index closure, and the INNER `pop[Region, young]` is a recorded
/// occurrence sitting in it.
///
/// The closure descended a `Range`'s two endpoints but passed a plain `Expr` index
/// through untouched -- while its own comment claimed it descended "exactly as the
/// other-variable arm above does", and that arm handles both. The nested reference
/// therefore kept its DIMENSION-name subscript, in a scalar fragment that cannot
/// compile, with nothing set loud either.
///
/// Everything here is already lagged by the enclosing `PREVIOUS`, index reads
/// included, so pinning is sufficient and NO further freeze is wanted (a nested
/// `PREVIOUS` would read two steps back). That is exactly what distinguishes this
/// from the table-argument case, which is not inside a freeze and is therefore
/// declined -- see
/// [`per_element_pin_declines_a_table_arg_with_a_runtime_index`].
#[test]
fn per_element_pin_descends_into_a_source_subscript_index_expression() {
    let fx = PinFixture::new(vec![]);
    let (ast, deps, occurrences) = fx.parse(
        "pop[Region, young] + PREVIOUS(pop[Region, pop[Region, young]])",
        &[],
    );
    // Non-vacuity: the index-nested occurrence must be recorded, or the closure
    // would have nothing to pin and this test would pass on an empty walk.
    assert!(
        occurrences.iter().any(|o| o.index_nested
            && matches!(&o.reference, crate::db::ltm_ir::OccurrenceRef::Variable(v) if v == "pop")),
        "the fixture must record an index-nested `pop` occurrence: {occurrences:?}"
    );
    let slot_occurrences = SlotOccurrences::new(&occurrences);
    let text = fx
        .generate(&ast, &deps, &slot_occurrences.for_slot(0))
        .expect("the per-element equation is derivable");

    assert!(
        !text.contains("pop[region,"),
        "a source reference nested in an INDEX EXPRESSION kept its DIMENSION-name \
         subscript, which cannot resolve in a scalar fragment (a silent zero); \
         got: {text}"
    );
    assert!(
        text.contains("pop[region\u{B7}boston, pop[region\u{B7}boston, age\u{B7}young]]"),
        "both the outer frozen read and the reference inside its index must be \
         pinned to this target element's row, QUALIFIED; got: {text}"
    );
    assert!(
        !text.contains("previous(pop[region\u{B7}boston, age\u{B7}young])"),
        "the enclosing PREVIOUS already lags the index read, so the descent must \
         NOT add a second freeze (that would read two steps back); got: {text}"
    );
    assert!(
        text.contains("pop[boston, young]"),
        "the LIVE occurrence must keep the bare row spelling; got: {text}"
    );
}

/// The full ENUMERATION: every index kind the rule can meet, in both freeze
/// contexts, with a stated verdict for each cell.
///
/// Every previous finding in this machinery -- the `391bc3c1` regression and both
/// review findings on this branch -- was a CELL, not a logic error: one shape
/// landing in the wrong bucket. Each was fixed after someone supplied the
/// counterexample. This test exists so the space is stated rather than sampled:
/// the rule sorts an index into exactly one of
/// [`super::post_transform::IndexVerdict`]'s four outcomes, and the outcome
/// depends on the index's spelling and (for `RuntimeRead` alone) on whether the
/// subtree is already frozen. Two columns, thirteen rows, no gaps.
///
/// Reading the table: `Some(spelling)` means the partial is EMITTED and its text
/// contains that spelling; `None` means the partial is DECLINED
/// (`UnfreezablePartial` -> warned skip). The unfrozen column is a bare `LOOKUP`
/// table argument; the frozen column is the same argument inside a pre-existing
/// `PREVIOUS`. Only the runtime-read rows differ between columns -- that is the
/// whole content of the compilability-vs-ceteris-paribus split.
///
/// Two rows are marked UNREACHABLE and assert current behavior rather than a
/// derived requirement, because a compilable model cannot produce them: codegen's
/// `extract_table_info` rejects a range or wildcard table index outright
/// (`BadTable`, "range subscripts not supported in lookup tables"), so the
/// TARGET's own equation would fail to compile long before its link scores are
/// generated. Their frozen cells are therefore honestly uncertain: the rule
/// currently accepts them, and if they ever became reachable the failure would
/// surface as a fragment-compile Warning rather than a warned skip -- loud either
/// way, through a different channel. Left as-is deliberately rather than guessed
/// into `Unspellable`, since no reachable shape distinguishes the two choices.
///
/// One further row (the numeric literal) is about the RULE's verdict, not about
/// downstream compilability: a numeric index into a NAMED dimension is not
/// resolvable by the compiler, but that is a property of the target's own
/// equation, which carries the same index. The rule's job is only to leave a
/// static selector alone, and that is what the cell pins.
#[test]
fn per_element_pin_index_verdict_enumeration() {
    // (label, the source subscript as written in the table argument,
    //  expected outside a freeze, expected inside a freeze)
    let cases: [(&str, &str, Option<&str>, Option<&str>); 13] = [
        // --- static selectors: pinned, and identical in both contexts ----------
        (
            "the axis's own dimension name",
            "pop[Region, young]",
            Some("pop[region\u{B7}boston, age\u{B7}young]"),
            Some("pop[region\u{B7}boston, age\u{B7}young]"),
        ),
        (
            "an element that axis declares",
            "pop[Region, old]",
            Some("pop[region\u{B7}boston, age\u{B7}old]"),
            Some("pop[region\u{B7}boston, age\u{B7}old]"),
        ),
        (
            "an already dim\u{B7}elem-qualified element",
            "pop[Region, age\u{B7}old]",
            Some("pop[region\u{B7}boston, age\u{B7}old]"),
            Some("pop[region\u{B7}boston, age\u{B7}old]"),
        ),
        (
            "a numeric literal (rule verdict only -- see fn docs)",
            "pop[Region, 1]",
            Some("pop[region\u{B7}boston, 1]"),
            Some("pop[region\u{B7}boston, 1]"),
        ),
        // --- unspellable: a COMPILABILITY verdict, so loud in BOTH contexts ----
        (
            "the source's own dim at a position the target does not project",
            "pop[Region, Age]",
            None,
            None,
        ),
        ("another dimension's name", "pop[State, young]", None, None),
        (
            "the source's own dims TRANSPOSED",
            "pop[Age, young]",
            None,
            None,
        ),
        (
            "an over-arity index no axis owns",
            "pop[Region, young, young]",
            None,
            None,
        ),
        // --- runtime reads: a CETERIS-PARIBUS verdict, so context decides ------
        (
            "an undeclared bare name -- a variable selecting the element",
            "pop[Region, idx]",
            None,
            Some("pop[region\u{B7}boston, idx]"),
        ),
        (
            "an arithmetic index expression",
            "pop[Region, idx + 1]",
            None,
            Some("pop[region\u{B7}boston, idx + 1]"),
        ),
        (
            "a nested source subscript selecting the element",
            "pop[Region, pop[Region, young]]",
            None,
            Some("pop[region\u{B7}boston, pop[region\u{B7}boston, age\u{B7}young]]"),
        ),
        // --- UNREACHABLE from a compilable model -- see fn docs ---------------
        (
            "a range index (UNREACHABLE: codegen rejects it as a table index)",
            "pop[Region, 1:2]",
            None,
            Some("pop[region\u{B7}boston,"),
        ),
        (
            "a wildcard index (UNREACHABLE: codegen rejects it as a table index)",
            "pop[Region, *]",
            None,
            Some("pop[region\u{B7}boston,"),
        ),
    ];

    // `state` rides along in every cell so the "another dimension's name" row has
    // a real dimension to name; it changes no other row, since the rule resolves
    // an index against the source's OWN axis at that position.
    let fx = PinFixture::new(vec![datamodel::Dimension::named(
        "state".to_string(),
        vec!["ny".to_string(), "ma".to_string()],
    )]);

    for (label, subscript, expect_bare, expect_frozen) in cases {
        for (frozen, expected) in [(false, expect_bare), (true, expect_frozen)] {
            let ctx = if frozen { "inside a freeze" } else { "bare" };
            let eqn = if frozen {
                format!("pop[Region, young] + PREVIOUS(LOOKUP({subscript}, input))")
            } else {
                format!("pop[Region, young] + LOOKUP({subscript}, input)")
            };
            let (ast, deps, occurrences) = fx.parse(&eqn, &["input", "idx"]);
            // Non-vacuity, every cell: the table argument is the reference the IR
            // records NOTHING for, so the rule is what decides -- not the IR.
            assert_eq!(
                PinFixture::source_occurrences(&occurrences),
                1,
                "{label} ({ctx}): only the live occurrence outside the LOOKUP may be \
                 recorded, or this cell tests the IR rather than the rule: \
                 {occurrences:?}"
            );
            let slot_occurrences = SlotOccurrences::new(&occurrences);
            let got = fx.generate(&ast, &deps, &slot_occurrences.for_slot(0));
            match expected {
                Some(spelling) => {
                    let text = got.unwrap_or_else(|e| {
                        panic!("{label} ({ctx}): expected an emitted partial: {e:?}")
                    });
                    assert!(
                        text.contains(spelling),
                        "{label} ({ctx}): expected {spelling:?} in the partial; got: {text}"
                    );
                    assert!(
                        !text.contains("pop[region,"),
                        "{label} ({ctx}): no DIMENSION-name subscript may survive into a \
                         scalar fragment; got: {text}"
                    );
                }
                None => assert!(
                    matches!(
                        got,
                        Err(PartialEquationError {
                            kind: PartialEquationErrorKind::UnfreezablePartial,
                            ..
                        })
                    ),
                    "{label} ({ctx}): expected a loud UnfreezablePartial decline; got: {got:?}"
                ),
            }
        }
    }
}
