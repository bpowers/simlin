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

    let target_dims = vec![make_named_dimension("region", &["nyc", "boston"])];
    let target_elements = vec!["boston".to_string()];
    let ctx = super::post_transform::PerElementRefCtx {
        from: &from,
        site_axes: &site_axes,
        row_parts_bare: &row_parts_bare,
        from_dims: &from_dims,
        target_dims: &target_dims,
        target_elements: &target_elements,
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
        super::post_transform::pin_only_source_refs(ast, &ctx, &occ, &[], &mut unlowerable);
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

/// The full ENUMERATION of [`super::post_transform::dep_element_pins`]: how a
/// dep's DECLARED dimensions relate to the target element, and what pin each
/// relation yields (GH #974).
///
/// This is the projection behind every bare-reference pin -- the rule
/// `subscript_idents_at_element` consumes and the rule `pin_bare_source_ref`
/// reuses for the live source -- so the rows are derived from the two
/// enumerations it composes, not from examples:
///
/// - per AXIS, [`dep_axis_elements`](super::post_transform) has three outcomes:
///   the target ITERATES the dep's own dimension (identity), the target
///   iterates a dimension with a usable CORRESPONDENCE to it (which of the two
///   the spelling picks -- GH #997), or neither (declined). The declined arm is
///   reached by an unrelated dimension, and is a row;
/// - per DEP, the axis outcomes combine into three: every axis resolved
///   (`complete`, the only kind that may subscript a bare reference), some
///   resolved (present but incomplete -- it may still substitute a
///   dimension-name index), none resolved (absent from the table entirely).
///
/// The order rows exist because the pin is spelled in the DEP's declaration
/// order, not the target's: `flip[Age,Region]` under a `growth[Region,Age]`
/// target is the shape whose pre-fix full-target-tuple pin COMPILED and read
/// the wrong element.
///
/// One arm is deliberately unrowed: `per_element_row_for_target` can also
/// decline an axis whose correspondence has no entry at the target element's
/// index. That is a mid-edit dimension inconsistency (a mapping whose two
/// dimensions differ in size), it declines exactly like the rows here, and no
/// well-formed project reaches it.
#[test]
fn dep_element_pins_projection_enumeration() {
    use crate::dimensions::DimensionsContext;

    // `state` is POSITIONALLY mapped to `region`, so a `State` axis reads the
    // region coordinate's positional partner; `other` is unrelated to both.
    let build_ctx = |element_map: Vec<(String, String)>| {
        let mut state = datamodel::Dimension::named(
            "state".to_string(),
            vec!["west".to_string(), "east".to_string()],
        );
        state.mappings = vec![datamodel::DimensionMapping {
            target: "region".to_string(),
            element_map,
        }];
        // `dblx`/`dbly` each map to BOTH target axes, so a per-axis search that
        // tracks no `used` set hands them the same one (P2-2).
        let dbl = |name: &str, elems: Vec<String>| {
            let mut d = datamodel::Dimension::named(name.to_string(), elems);
            d.mappings = vec![
                datamodel::DimensionMapping {
                    target: "region".to_string(),
                    element_map: vec![],
                },
                datamodel::DimensionMapping {
                    target: "age".to_string(),
                    element_map: vec![],
                },
            ];
            d
        };
        DimensionsContext::from(
            [
                datamodel::Dimension::named(
                    "region".to_string(),
                    vec!["nyc".to_string(), "boston".to_string()],
                ),
                datamodel::Dimension::named(
                    "age".to_string(),
                    vec!["young".to_string(), "old".to_string()],
                ),
                state,
                dbl("dblx", vec!["x1".to_string(), "x2".to_string()]),
                dbl("dbly", vec!["y1".to_string(), "y2".to_string()]),
                datamodel::Dimension::named(
                    "other".to_string(),
                    vec!["o1".to_string(), "o2".to_string()],
                ),
            ]
            .as_slice(),
        )
    };
    let region = make_named_dimension("region", &["nyc", "boston"]);
    let age = make_named_dimension("age", &["young", "old"]);
    let state = make_named_dimension("state", &["west", "east"]);
    let other = make_named_dimension("other", &["o1", "o2"]);
    let dblx = make_named_dimension("dblx", &["x1", "x2"]);
    let dbly = make_named_dimension("dbly", &["y1", "y2"]);

    // Target `growth[Region,Age]` at element `(boston, old)` -- both
    // coordinates are the SECOND element of their dimension, which is what
    // makes the positional correspondence observable (`state·east`).
    let target_dims = vec![region.clone(), age.clone()];
    let target_elements = vec!["boston".to_string(), "young".to_string()];

    let pinnable: Vec<(Ident<Canonical>, Vec<crate::dimensions::Dimension>)> = vec![
        // identity, in the target's own order
        (Ident::new("same"), vec![region.clone(), age.clone()]),
        // identity, REORDERED -- the silent-wrong-element row
        (Ident::new("flip"), vec![age.clone(), region.clone()]),
        // a strict SUBSET -- the arity row
        (Ident::new("sub"), vec![age.clone()]),
        // a positionally MAPPED axis beside an identity one
        (Ident::new("mapped"), vec![state.clone(), age.clone()]),
        // one axis resolves, one does not -> incomplete
        (Ident::new("partial"), vec![region.clone(), other.clone()]),
        // nothing resolves -> absent
        (Ident::new("unrelated"), vec![other.clone()]),
        // P2-2: BOTH axes can map to BOTH target axes. The allocation must be
        // one-to-one and in declaration order, matching the compiler.
        (Ident::new("doubly"), vec![dblx.clone(), dbly.clone()]),
    ];

    let axes_of = |pins: &HashMap<Ident<Canonical>, crate::ltm_augment::DepElementPin>,
                   name: &str|
     -> Option<(Vec<(String, String)>, bool)> {
        pins.get(&Ident::<Canonical>::new(name))
            .map(|p| (p.axes.clone(), p.bare_row.is_some()))
    };
    let axis = |dim: &str, elem: &str| (dim.to_string(), elem.to_string());

    let positional = build_ctx(vec![]);
    let pins = super::post_transform::dep_element_pins(
        &pinnable,
        &target_dims,
        &target_elements,
        &positional,
    );

    assert_eq!(
        axes_of(&pins, "same"),
        Some((
            vec![
                axis("region", "region\u{B7}boston"),
                axis("age", "age\u{B7}young")
            ],
            true
        )),
        "a dep declaring the target's own dimensions in the target's order pins \
         to the target element"
    );
    assert_eq!(
        axes_of(&pins, "flip"),
        Some((
            vec![
                axis("age", "age\u{B7}young"),
                axis("region", "region\u{B7}boston")
            ],
            true
        )),
        "a REORDERED dep must be pinned in ITS OWN declaration order -- the \
         target's tuple would compile and read the transposed element"
    );
    assert_eq!(
        axes_of(&pins, "sub"),
        Some((vec![axis("age", "age\u{B7}young")], true)),
        "a subset-dims dep must be pinned over its own single axis, not the \
         target's full tuple"
    );
    assert_eq!(
        axes_of(&pins, "mapped"),
        Some((
            vec![
                axis("state", "state\u{B7}east"),
                axis("age", "age\u{B7}young")
            ],
            true
        )),
        "a positionally-mapped axis reads the corresponding element of its own \
         dimension (`boston` is Region's second, so State's second is `east`)"
    );
    assert_eq!(
        axes_of(&pins, "partial"),
        Some((vec![axis("region", "region\u{B7}boston")], false)),
        "a dep with one unprojectable axis stays in the table (its dimension-name \
         indices are still substitutable) but is NOT complete"
    );
    assert_eq!(
        axes_of(&pins, "unrelated"),
        None,
        "a dep no axis of which projects has nothing to rewrite and must be absent"
    );
    // P2-2: each target axis is consumed once, so the second dep axis gets the
    // second target axis rather than re-claiming the first. `boston` is Region's
    // second element and `old` is Age's, so a one-to-one allocation reads each
    // mapped dimension's SECOND element.
    assert_eq!(
        axes_of(&pins, "doubly"),
        Some((
            vec![axis("dblx", "dblx\u{B7}x2"), axis("dbly", "dbly\u{B7}y1")],
            true
        )),
        "two dep axes that can each map to either target axis must be allocated \
         ONE-TO-ONE in declaration order, as the compiler allocates them; an \
         independent per-axis search gives both the first target axis"
    );

    // P2-1: a target that REPEATS a dimension. The two axes are different reads,
    // and the simulation gives a bare dep the FIRST -- measured by
    // `repeated_target_dimension_reads_the_first_axis`. A map keyed by dimension
    // name cannot express this at all; the positional projection can.
    let repeated_dims = vec![region.clone(), region.clone()];
    let repeated_elements = vec!["nyc".to_string(), "boston".to_string()];
    let repeated_pins = super::post_transform::dep_element_pins(
        &[(Ident::new("w"), vec![region.clone()])],
        &repeated_dims,
        &repeated_elements,
        &positional,
    );
    assert_eq!(
        repeated_pins
            .get(&Ident::<Canonical>::new("w"))
            .map(|p| (p.axes.clone(), p.bare_row.is_some())),
        Some((vec![axis("region", "region\u{B7}nyc")], true)),
        "a subset dep under a repeated-dimension target reads the FIRST axis; a \
         name-keyed map keeps only the last and would say `boston`"
    );

    // An EXPLICIT element map is where the pin's TWO rows part (GH #997). This
    // block asserted a single declining row until then, on the reasoning that
    // execution "resolves positionally and ignores the map" -- true of a BARE
    // reference and false of a `mapped[State, ...]` subscript, and one row could
    // not say both. The map below sends `west` to `boston`, the REVERSE of the
    // positional diagonal (`boston` is Region's second, so positionally it reads
    // State's second, `east`), so the two rows disagree on every element and
    // neither assertion can pass by accident.
    let element_mapped = build_ctx(vec![
        ("west".to_string(), "boston".to_string()),
        ("east".to_string(), "nyc".to_string()),
    ]);
    let pins = super::post_transform::dep_element_pins(
        &pinnable,
        &target_dims,
        &target_elements,
        &element_mapped,
    );
    let mapped_pin = pins
        .get(&Ident::<Canonical>::new("mapped"))
        .expect("the mapped dep projects on both rows");
    assert_eq!(
        mapped_pin.axes,
        vec![
            axis("state", "state\u{B7}west"),
            axis("age", "age\u{B7}young")
        ],
        "a `mapped[State, Age]` subscript FOLLOWS the declared element map: the \
         index survives to `IndexOp::ActiveDimRef` and `build_view_from_ops` \
         resolves it name-first, then through the map"
    );
    assert_eq!(
        mapped_pin.bare_row,
        Some(vec![
            "state\u{B7}east".to_string(),
            "age\u{B7}young".to_string()
        ]),
        "a BARE `mapped` reference is rewritten into the iterated spelling by \
         pass 0 and read by ORDINAL, so it reads State's second element -- the \
         other one"
    );

    // The same dep under a POSITIONAL mapping: the two rows coincide, which is
    // why one row sufficed before GH #997 and why nothing about the shipped
    // positional cases moves.
    let positional_pins = super::post_transform::dep_element_pins(
        &pinnable,
        &target_dims,
        &target_elements,
        &positional,
    );
    let positional_mapped = positional_pins
        .get(&Ident::<Canonical>::new("mapped"))
        .expect("the mapped dep projects");
    assert_eq!(
        positional_mapped.bare_row,
        Some(
            positional_mapped
                .axes
                .iter()
                .map(|(_, elem)| elem.clone())
                .collect::<Vec<_>>()
        )
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
    /// The QUALIFIED target element this instantiation emits for -- `region·boston`
    /// for [`PinFixture::new`], `state·ma` for [`PinFixture::mapped`].
    target_element: String,
    /// The target equation's dimensions in AXIS order, and this instantiation's
    /// element as a positional tuple over them. Every fixture here iterates one
    /// dimension, so these are the one-element twins of `target_iterated_dims`
    /// and `target_elem_by_dim` -- carried separately because a repeated
    /// dimension makes the name-keyed pair unrepresentable, which is what the
    /// pin projection now refuses to depend on.
    target_dims: Vec<crate::dimensions::Dimension>,
    target_elements: Vec<String>,
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
            target_element: "region\u{B7}boston".to_string(),
            target_dims: vec![make_named_dimension("region", &["nyc", "boston"])],
            target_elements: vec!["boston".to_string()],
        }
    }

    /// The MAPPED twin of [`PinFixture::new`]: the same source `pop[Region, Age]`,
    /// but the target iterates `State` (`[ny, ma]`) instead of `Region`, with a
    /// declared `State`/`Region` dimension mapping, instantiated at `state·ma`.
    /// Under the POSITIONAL correspondence that reads source row
    /// `[boston, young]` -- `ma` is `State`'s second element and `boston` is
    /// `Region`'s -- so a `State`-named index of a `Region` axis is spellable even
    /// though the two names differ.
    ///
    /// `declare_on_state` picks the DECLARATION DIRECTION: `true` declares the
    /// mapping on `State` toward `Region`, `false` on `Region` toward `State`.
    /// Both correspondences honor both directions (GH #757), so both must pin.
    /// A non-empty `element_map` makes the mapping an EXPLICIT element map, which
    /// changes nothing for the ITERATED spelling these fixtures use: execution
    /// folds that index to an ordinal and never reads the map (GH #997), so the
    /// pin is the positional element either way.
    fn mapped(declare_on_state: bool, element_map: Vec<(String, String)>) -> Self {
        use crate::ltm_agg::AxisRead;
        let mut state = datamodel::Dimension::named(
            "state".to_string(),
            vec!["ny".to_string(), "ma".to_string()],
        );
        let mut region = datamodel::Dimension::named(
            "region".to_string(),
            vec!["nyc".to_string(), "boston".to_string()],
        );
        let mapping = |target: &str, element_map: Vec<(String, String)>| {
            vec![datamodel::DimensionMapping {
                target: target.to_string(),
                element_map,
            }]
        };
        if declare_on_state {
            state.mappings = mapping("region", element_map);
        } else {
            region.mappings = mapping(
                "state",
                element_map
                    .into_iter()
                    .map(|(a, b)| (b, a))
                    .collect::<Vec<_>>(),
            );
        }
        let project_dims = vec![
            region,
            datamodel::Dimension::named(
                "age".to_string(),
                vec!["young".to_string(), "old".to_string()],
            ),
            state,
        ];
        let mut target_elem_by_dim = HashMap::new();
        target_elem_by_dim.insert("state".to_string(), ("ma".to_string(), 1usize));
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
            target_iterated_dims: vec!["state".to_string()],
            dim_ctx: crate::dimensions::DimensionsContext::from(project_dims.as_slice()),
            site_axes: vec![
                AxisRead::Iterated {
                    dim: "state".to_string(),
                    source_dim: "region".to_string(),
                },
                AxisRead::Pinned("young".to_string()),
            ],
            row_parts_bare: vec!["boston".to_string(), "young".to_string()],
            target_elem_by_dim,
            target_element: "state\u{B7}ma".to_string(),
            target_dims: vec![make_named_dimension("state", &["ny", "ma"])],
            target_elements: vec!["ma".to_string()],
        }
    }

    /// The NAME-COLLISION twin of [`PinFixture::new`]: the source's second axis is
    /// `Bucket = [region, old]`, so its element `region` is spelled exactly like the
    /// `Region` DIMENSION the target iterates. Source `pop[Region, Bucket]`, target
    /// `growth[Region]` at `region·boston`, emitting site `pop[Region, old]`.
    ///
    /// XMILE lets an element name collide with a dimension name, and
    /// `compiler::subscript`'s `normalize_subscripts3` resolves the collision by
    /// looking the index up in the AXIS's own elements FIRST (`get_element_index`,
    /// "takes priority") and only then as a dimension name. So `pop[Region, region]`
    /// reads `Bucket`'s `region` element -- not an iteration over `Region` -- and a
    /// pin that reads it the other way either drops the edge or spells a row the
    /// simulation never reads.
    fn colliding_element() -> Self {
        use crate::ltm_agg::AxisRead;
        let project_dims = vec![
            datamodel::Dimension::named(
                "region".to_string(),
                vec!["nyc".to_string(), "boston".to_string()],
            ),
            datamodel::Dimension::named(
                "bucket".to_string(),
                vec!["region".to_string(), "old".to_string()],
            ),
        ];
        let mut target_elem_by_dim = HashMap::new();
        target_elem_by_dim.insert("region".to_string(), ("boston".to_string(), 1usize));
        PinFixture {
            from: Ident::<Canonical>::new("pop"),
            from_dims: vec![
                make_named_dimension("region", &["nyc", "boston"]),
                make_named_dimension("bucket", &["region", "old"]),
            ],
            source_dim_elements: vec![
                vec!["nyc".to_string(), "boston".to_string()],
                vec!["region".to_string(), "old".to_string()],
            ],
            source_dim_names: vec!["region".to_string(), "bucket".to_string()],
            target_iterated_dims: vec!["region".to_string()],
            dim_ctx: crate::dimensions::DimensionsContext::from(project_dims.as_slice()),
            site_axes: vec![
                AxisRead::Iterated {
                    dim: "region".to_string(),
                    source_dim: "region".to_string(),
                },
                AxisRead::Pinned("old".to_string()),
            ],
            row_parts_bare: vec!["boston".to_string(), "old".to_string()],
            target_elem_by_dim,
            target_element: "region\u{B7}boston".to_string(),
            target_dims: vec![make_named_dimension("region", &["nyc", "boston"])],
            target_elements: vec!["boston".to_string()],
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
            &self.target_element,
            ast,
            deps,
            // Nothing to element-pin by name: the source's pinning is the wrap's job.
            &HashMap::new(),
            &self.from_dims,
            &self.target_elem_by_dim,
            &self.target_dims,
            &self.target_elements,
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

/// GH #984, both halves at once: a `LOOKUP` table argument's HEAD stays bare and
/// its runtime INDEX gets frozen.
///
/// The two halves pull against each other, which is why the issue asks for one
/// test that pins both. The head must NOT be wrapped: a graphical-function table
/// has no value slot, so `lookup(PREVIOUS(table), ...)` cannot compile and the
/// whole fragment silently zeroes -- the WRLD3 failure mode. But the index IS a
/// value read: `codegen::extract_table_info`'s `Expr::Subscript` arm builds the
/// element offset out of the live index `Expr`s, so a live `idx` made EVERY
/// partial of that target vary with `idx`'s current-step movement, attributing it
/// to whichever source the partial isolates. Holding the whole ARGUMENT verbatim
/// bought the first at the price of the second.
///
/// The rows are the three shapes an index read can take -- a bare variable, a
/// compound expression around one, and a nested source subscript -- so the
/// freeze is shown to reach inside a compound index (`PREVIOUS(idx) + 1`, not
/// `PREVIOUS(idx + 1)`) rather than only replacing whole indices. The static
/// selectors, the unspellable ones, and the frozen-context column are the
/// sibling [`per_element_pin_index_verdict_enumeration`]'s job.
///
/// Before the fix each of these was a loud DECLINE
/// (`WrapOutcome::missing_occurrence` -> a warned skip), which threw away every
/// per-element score on the edge including the live site's own. Freezing keeps
/// the score AND makes it ceteris-paribus.
///
/// **`idx` is deliberately NOT in the declared dep set**, and that is the whole
/// difference between a fixture and an oracle here. Production derives the wrap's
/// dep set from `variable::identifier_set`, whose `BuiltinContents::LookupTable`
/// arm does not walk the table expression, so an index variable referenced only
/// inside a table argument is NOT a dependency -- asserted below on the fixture's
/// own equation rather than assumed. Declaring `idx` a dep would make the freeze
/// fire through the ordinary other-dep path and the test would pass on an input
/// production cannot produce, which is exactly how the first version of this fix
/// shipped without firing at all.
#[test]
fn per_element_pin_freezes_a_runtime_table_arg_index() {
    // `(label, table argument, the expected frozen spelling)`.
    let cases: [(&str, &str, &str); 3] = [
        (
            "a variable index -- the simplest runtime element read",
            "pop[Region, idx]",
            "lookup(pop[region\u{B7}boston, PREVIOUS(idx, idx)]",
        ),
        (
            "a compound index: the freeze reaches the read, not the whole expression",
            "pop[Region, idx + 1]",
            "lookup(pop[region\u{B7}boston, PREVIOUS(idx, idx) + 1]",
        ),
        (
            "a nested source read selecting the element at runtime",
            "pop[Region, pop[Region, old]]",
            "lookup(pop[region\u{B7}boston, \
             PREVIOUS(pop[region\u{B7}boston, age\u{B7}old], \
             pop[region\u{B7}boston, age\u{B7}old])]",
        ),
    ];

    for (label, table_arg, expected) in cases {
        let fx = PinFixture::new(vec![]);
        let eqn = format!("pop[Region, young] + LOOKUP({table_arg}, input)");
        // The dep list is the production one for this equation: `input` is a
        // dependency and `idx` is not. Checked against the extractor itself, so
        // the fixture cannot drift from what production supplies.
        assert!(
            !crate::variable::identifier_set(&crate::variable::scalar_ast(&eqn), &[], None)
                .contains(&Ident::<Canonical>::new("idx")),
            "{label}: production's dep extractor must NOT report a table-only \
             index as a dependency, or this fixture is testing the ordinary \
             other-dep path instead of the table-index freeze"
        );
        let (ast, deps, occurrences) = fx.parse(&eqn, &["input"]);
        // Non-vacuity: the table argument is the reference WITHOUT an occurrence
        // (the IR skips it whole), so the freeze comes from the wrap's own index
        // pass rather than from the IR's classification.
        assert_eq!(
            PinFixture::source_occurrences(&occurrences),
            1,
            "{label}: only the live occurrence outside the LOOKUP is recorded: \
             {occurrences:?}"
        );
        let slot_occurrences = SlotOccurrences::new(&occurrences);
        let text = fx
            .generate(&ast, &deps, &slot_occurrences.for_slot(0))
            .unwrap_or_else(|e| {
                panic!("{label}: the partial must be emitted, not declined: {e:?}")
            });
        assert!(
            text.contains(expected),
            "{label}: expected the frozen index {expected:?}; got: {text}"
        );
        assert!(
            !text.contains("lookup(PREVIOUS("),
            "{label}: the table HEAD must stay bare -- a PREVIOUS of a table has no \
             value slot; got: {text}"
        );
        assert!(
            text.contains("pop[boston, young]"),
            "{label}: the LIVE occurrence must keep its bare row spelling -- the \
             pre-fix decline threw its score away too; got: {text}"
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
        // `idx` is not declared: production does not classify a table-only index
        // as a dependency (see `per_element_pin_freezes_a_runtime_table_arg_index`),
        // and this test's point is that the enclosing freeze -- not the dep set --
        // is what makes the index ceteris-paribus here.
        let (ast, deps, occurrences) = fx.parse(eqn, &["input", "other"]);
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
/// from an UNFROZEN table argument, whose index is a live read and so is frozen
/// rather than merely pinned -- see
/// [`per_element_pin_freezes_a_runtime_table_arg_index`], and
/// [`per_element_pin_keeps_a_runtime_table_index_inside_a_freeze`] for the same
/// argument inside a freeze, where this test's reasoning applies instead.
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

/// One row of a pin-index verdict enumeration: a label, the source subscript as
/// written in the `LOOKUP` table argument, and the expected outcome in each freeze
/// context. `Some(spelling)` means the partial is EMITTED and its text contains
/// that spelling; `None` means it is DECLINED (`UnfreezablePartial` -> warned
/// skip).
type PinIndexCell<'a> = (&'a str, &'a str, Option<&'a str>, Option<&'a str>);

/// Run one enumeration table against `fx`, both freeze columns per row.
///
/// `live_ref` is the source reference the emitting site holds LIVE, spelled at
/// `fx`'s own shape -- it is what makes the equation a `PerElement` target at all,
/// and it is deliberately not the cell under test. The cell under test is the
/// table argument, which the IR records NOTHING for.
///
/// The declared dep list is `["input"]` and NOT `["input", "idx"]`, matching what
/// `variable::identifier_set` reports for these equations: a variable appearing
/// only as a table-argument subscript index is not a dependency, so a row that
/// declared it would exercise the ordinary other-dep freeze instead of the
/// table-index one. `per_element_pin_freezes_a_runtime_table_arg_index` asserts
/// that against the extractor itself.
fn assert_pin_index_verdicts(fx: &PinFixture, live_ref: &str, cases: &[PinIndexCell<'_>]) {
    // A DIMENSION-name subscript surviving into a scalar fragment is the silent
    // zero this whole rule exists to prevent, so every cell forbids one on any
    // dimension in play -- the source's own axes and the target's iterated dims.
    let forbidden: Vec<String> = fx
        .source_dim_names
        .iter()
        .chain(fx.target_iterated_dims.iter())
        .map(|d| format!("pop[{d},"))
        .collect();
    for (label, subscript, expect_bare, expect_frozen) in cases {
        for (frozen, expected) in [(false, expect_bare), (true, expect_frozen)] {
            let ctx = if frozen { "inside a freeze" } else { "bare" };
            let eqn = if frozen {
                format!("{live_ref} + PREVIOUS(LOOKUP({subscript}, input))")
            } else {
                format!("{live_ref} + LOOKUP({subscript}, input)")
            };
            let (ast, deps, occurrences) = fx.parse(&eqn, &["input"]);
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
                    for bad in &forbidden {
                        assert!(
                            !text.contains(bad.as_str()),
                            "{label} ({ctx}): no DIMENSION-name subscript ({bad:?}) may \
                             survive into a scalar fragment; got: {text}"
                        );
                    }
                    // The emitting site must actually be held LIVE at its bare row,
                    // or the cell is not a `PerElement` instantiation at all and the
                    // table argument's verdict was reached in the wrong context.
                    assert!(
                        text.contains(&format!("pop[{}]", fx.row_parts_bare.join(", "))),
                        "{label} ({ctx}): the live occurrence must keep the bare row \
                         spelling, or this fixture is not a PerElement instantiation; \
                         got: {text}"
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

/// The full ENUMERATION: every index kind the rule can meet, in both freeze
/// contexts, with a stated verdict for each cell.
///
/// Every previous finding in this machinery -- the `391bc3c1` regression and the
/// four review findings on this branch -- was a CELL, not a logic error: one shape
/// landing in the wrong bucket. Each was fixed after someone supplied the
/// counterexample. This test exists so the space is stated rather than sampled:
/// the rule sorts an index into exactly one of
/// [`super::post_transform::IndexVerdict`]'s three outcomes -- `Pinned`, `Keep`,
/// `Unspellable` -- purely on the index's spelling, and the WRAP separately
/// decides whether a runtime read needs freezing. Two columns, eighteen rows, no
/// gaps.
///
/// "No gaps" is a checkable claim, not a hope: `IndexExpr0` has exactly five
/// variants and every one has a row -- `Wildcard`, `StarRange`, `Range`,
/// `DimPosition`, and `Expr` (which fans out into the `Const` / `Var` / compound
/// rows, and the `Var` rows into every way a name can or cannot resolve). What the
/// table does NOT vary is subscript ARITY beyond the over-arity row, and that is
/// deliberate: `codegen::extract_table_info` accepts only a table argument
/// selecting exactly ONE element (a resolved `Var`, a `StaticSubscript` of
/// `view.size() == 1`, or a `Subscript` whose every index is
/// `SubscriptIndex::Single`), so an UNDER-arity subscript is `BadTable` for the same
/// reason the range/wildcard/star-range rows are unreachable. Rowing it would state
/// a verdict about a shape no compilable model produces.
///
/// The table came back all-green once while still being wrong twice, which is
/// worth stating plainly: a mutation probe proves a table CONSTRAINS the code, and
/// nothing more. Whether each cell asserts the RIGHT verdict is a review question,
/// and whether the rows cover the space is a completeness question. Both defects
/// were of the second kind -- `@N` and a mapped dimension name were simply not
/// rows -- so a row added here is worth more than an assertion added to an
/// existing one. The MAPPED axis needs a different target dimension than the
/// source's, so its rows live in the sibling
/// [`per_element_pin_mapped_axis_verdict_enumeration`] over
/// [`PinFixture::mapped`]; everything resolvable against the source's own axis
/// names is here.
///
/// Reading the table: see [`PinIndexCell`]. The unfrozen column is a bare `LOOKUP`
/// table argument; the frozen column is the same argument inside a pre-existing
/// `PREVIOUS`. Only the runtime-read rows differ between columns, and since
/// GH #984 the difference is a FREEZE rather than a refusal: a bare table
/// argument's index is wrapped in `PREVIOUS` by
/// `ltm_augment::freeze_lookup_table_indices`, while the same index inside a
/// pre-existing freeze is already lagged and is left alone. Every static row is
/// identical in both columns, and every unspellable row is loud in both.
///
/// Three rows are marked UNREACHABLE and assert current behavior rather than a
/// derived requirement, because a compilable model cannot produce them: codegen's
/// `extract_table_info` rejects a range, wildcard or star-range table index
/// outright (`BadTable`, "range subscripts not supported in lookup tables"), so
/// the TARGET's own equation would fail to compile long before its link scores are
/// generated. If one ever became reachable the failure would surface as a
/// fragment-compile Warning rather than a warned skip -- loud either way, through
/// a different channel. Left as-is deliberately rather than guessed into
/// `Unspellable`, since no reachable shape distinguishes the two choices.
///
/// One further row (the numeric literal) is about the RULE's verdict, not about
/// downstream compilability: a numeric index into a NAMED dimension is not
/// resolvable by the compiler, but that is a property of the target's own
/// equation, which carries the same index. The rule's job is only to leave a
/// static selector alone, and that is what the cell pins.
#[test]
fn per_element_pin_index_verdict_enumeration() {
    let cases: [PinIndexCell<'_>; 18] = [
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
        (
            // `@N` reached the catch-all and was scored a RUNTIME read, so a bare
            // table argument declined and every link score on the edge was dropped
            // -- even though `compiler::context`'s subscript lowering resolves
            // `DimPosition` to a concrete element offset in scalar context. It
            // selects a FIXED element and reads nothing at the current step, so it
            // is static in both senses the rule cares about, and needs no pin: the
            // compiler that owns position syntax resolves it.
            "an `@N` position index",
            "pop[Region, @2]",
            Some("pop[region\u{B7}boston, @2]"),
            Some("pop[region\u{B7}boston, @2]"),
        ),
        (
            // The same defect in a different spelling: the catch-all never looked
            // INSIDE a compound index, so arithmetic over literals scored a runtime
            // read though it is exactly as static as the bare `1` two rows up.
            "constant arithmetic over literals",
            "pop[Region, 1 + 1]",
            Some("pop[region\u{B7}boston, 1 + 1]"),
            Some("pop[region\u{B7}boston, 1 + 1]"),
        ),
        (
            // A 0-arity BUILTIN index used to be loud in the bare column, because
            // the rule could not tell `TIME` (varies every step) from `PI` (does
            // not) inside an `Expr0::App` without a fourth copy of the builtin
            // classification `builtins`/`compiler::invariance` own -- so it
            // declined conservatively. It no longer has to decide: the rule keeps
            // the index either way, and the wrap's index pass leaves a 0-arity
            // builtin live exactly as it does everywhere else in a partial (TIME
            // is not a dep being isolated, and the guard form reads it live too).
            "a 0-arity builtin index",
            "pop[Region, TIME]",
            Some("pop[region\u{B7}boston, time()]"),
            Some("pop[region\u{B7}boston, time()]"),
        ),
        // --- unspellable: a COMPILABILITY verdict, so loud in BOTH contexts ----
        (
            "the source's own dim at a position the target does not project",
            "pop[Region, Age]",
            None,
            None,
        ),
        (
            // The target iterates `Region`, not `State`, so nothing says WHICH
            // `State` element this instantiation reads. Contrast the mapped table's
            // rows, where the target DOES iterate `State`: there the shared
            // correspondence names the element and the same spelling pins.
            "another dimension's name the target does not iterate",
            "pop[State, young]",
            None,
            None,
        ),
        (
            // A SUBDIMENSION name gets no special treatment: it is a dimension the
            // target does not iterate, so it names no coordinate. (`bigcity` is a
            // proper subdimension of the `region` axis by element containment.)
            // Neither does the shared classifier treat one specially -- only its
            // `StarRange` arm resolves a subdimension (`*:Sub`, GH #766), and a
            // plain `Var` index naming one declines there too, so a target
            // ITERATING a subdimension of the source axis cannot produce a
            // `PerElement` site at all and never reaches this rule.
            "a subdimension name of the source's axis",
            "pop[BigCity, young]",
            None,
            None,
        ),
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
        // --- runtime reads: kept by the rule, FROZEN by the wrap when bare -----
        // The two columns differ by exactly one `PREVIOUS`, which IS the GH #984
        // fix: a bare table argument's index read is lagged, and the same read
        // inside a pre-existing freeze is left alone rather than double-lagged.
        (
            "an undeclared bare name -- a variable selecting the element",
            "pop[Region, idx]",
            Some("pop[region\u{B7}boston, PREVIOUS(idx, idx)]"),
            Some("pop[region\u{B7}boston, idx]"),
        ),
        (
            "an arithmetic index expression",
            "pop[Region, idx + 1]",
            Some("pop[region\u{B7}boston, PREVIOUS(idx, idx) + 1]"),
            Some("pop[region\u{B7}boston, idx + 1]"),
        ),
        (
            "a nested source subscript selecting the element",
            "pop[Region, pop[Region, young]]",
            Some(
                "pop[region\u{B7}boston, \
                 PREVIOUS(pop[region\u{B7}boston, age\u{B7}young], \
                 pop[region\u{B7}boston, age\u{B7}young])]",
            ),
            Some("pop[region\u{B7}boston, pop[region\u{B7}boston, age\u{B7}young]]"),
        ),
        // --- UNREACHABLE from a compilable model -- see fn docs ---------------
        (
            "a range index (UNREACHABLE: codegen rejects it as a table index)",
            "pop[Region, 1:2]",
            Some("pop[region\u{B7}boston,"),
            Some("pop[region\u{B7}boston,"),
        ),
        (
            "a wildcard index (UNREACHABLE: codegen rejects it as a table index)",
            "pop[Region, *]",
            Some("pop[region\u{B7}boston,"),
            Some("pop[region\u{B7}boston,"),
        ),
        (
            "a star-range index (UNREACHABLE: codegen rejects it as a table index)",
            "pop[Region, *:Age]",
            Some("pop[region\u{B7}boston,"),
            Some("pop[region\u{B7}boston,"),
        ),
    ];

    // `state` and `bigcity` ride along in every cell so the two rows that name
    // another dimension have real dimensions to name; they change no other row,
    // since the rule resolves an index against the source's OWN axis at that
    // position and neither is one of the target's iterated dims.
    let fx = PinFixture::new(vec![
        datamodel::Dimension::named(
            "state".to_string(),
            vec!["ny".to_string(), "ma".to_string()],
        ),
        datamodel::Dimension::named("bigcity".to_string(), vec!["nyc".to_string()]),
    ]);
    assert!(
        fx.dim_ctx.is_subdimension_of(
            &crate::common::CanonicalDimensionName::from_raw("bigcity"),
            &crate::common::CanonicalDimensionName::from_raw("region"),
        ),
        "the subdimension row needs `bigcity` to genuinely be a subdimension of the \
         source's `region` axis, or it duplicates the unrelated-dimension row"
    );

    assert_pin_index_verdicts(&fx, "pop[Region, young]", &cases);
}

/// The NAME-COLLISION half of the enumeration: an index name that is BOTH an element
/// of the source's axis at that position AND a dimension the target iterates.
///
/// XMILE permits the collision, and `compiler::subscript`'s `normalize_subscripts3`
/// breaks it toward the ELEMENT: it looks the index up in the axis's own
/// `indexed_elements` first ("takes priority") and only falls back to the
/// dimension-name `ActiveDimRef` reading. The pin has to break it the same way or it
/// contradicts the equation's actual meaning -- and both wrong outcomes are bad in
/// the two directions this branch cares about. Reading `region` as an iteration over
/// `Region` finds no `Region`/`Bucket` mapping and declines, dropping every score on
/// a valid edge (the silent-zero direction); and if such a mapping DID exist it would
/// pin the correspondence's element instead of the literal one, which is a compilable
/// CONFIDENTLY WRONG row -- the outcome worse than none.
///
/// The control row beside it is the same axis's OTHER element, which no dimension is
/// named after: it must resolve identically, so a fix that merely special-cased the
/// colliding name would not pass both.
#[test]
fn per_element_pin_colliding_element_name_verdict_enumeration() {
    let fx = PinFixture::colliding_element();
    assert!(
        fx.dim_ctx.is_dimension_name("region"),
        "the collision needs `region` to be a real DIMENSION name as well as an \
         element of the `bucket` axis, or this test proves nothing"
    );

    let cases: [PinIndexCell<'_>; 2] = [
        (
            "an axis element whose name is also an iterated dimension",
            "pop[Region, region]",
            Some("pop[region\u{B7}boston, bucket\u{B7}region]"),
            Some("pop[region\u{B7}boston, bucket\u{B7}region]"),
        ),
        (
            "the control: the same axis's non-colliding element",
            "pop[Region, old]",
            Some("pop[region\u{B7}boston, bucket\u{B7}old]"),
            Some("pop[region\u{B7}boston, bucket\u{B7}old]"),
        ),
    ];
    assert_pin_index_verdicts(&fx, "pop[Region, old]", &cases);
}

/// The MAPPED half of the enumeration: the same rule, asked about an index naming a
/// dimension the target ITERATES that is not the source axis's own name.
///
/// This is the second review finding, and it is the case a name comparison cannot
/// decide. `growth[State] = pop[State, young] + LOOKUP(pop[State, old], input)` over
/// a source declared `pop[Region, Age]` is a perfectly good model when a positional
/// `State`/`Region` mapping exists: the executed simulation reads `Region`'s element
/// at the same position as the `State` element being computed. The rule used to
/// judge `State` unspellable purely because the name differed from `Region`, which
/// dropped the whole `pop -> growth` score edge.
///
/// The verdicts are not this rule's opinion. Each row is whatever
/// `DimensionsContext::positional_correspondence` says, reached through
/// [`super::post_transform::per_element_row_for_target`] -- the SAME derivation the
/// occurrence-driven pin and the link-score NAME use, and the same one
/// `ltm_agg::classify_axis_access` gates its `Iterated` arm on (through
/// `iterated_axis_slot_elements`, the correspondence's preimage inversion). So the
/// rows below double as an agreement statement: a mapped pair this rule pins is
/// exactly a mapped pair the classifier calls `Iterated`, in both declaration
/// directions.
///
/// The correspondence is the POSITIONAL one because every index here spells a
/// dimension the target ITERATES, which `compiler::subscript` binds to an ordinal
/// (GH #997). An element-mapped pair therefore pins too -- and pins to the
/// ORDINAL's element, not the map's.
///
/// Both columns are the SAME for every row here, and that is the point: these are
/// `Pinned` and `Unspellable` verdicts, neither of which the freeze context can
/// change. A mapped index is a static selector once the correspondence names its
/// element, and unspellable when it does not.
#[test]
fn per_element_pin_mapped_axis_verdict_enumeration() {
    // `ma` is `State`'s second element, so the positional correspondence reads
    // `Region`'s second -- `boston`. A row that pins must say so.
    let positional: [PinIndexCell<'_>; 2] = [
        (
            "a mapped dimension name (the iterated axis)",
            "pop[State, old]",
            Some("pop[region\u{B7}boston, age\u{B7}old]"),
            Some("pop[region\u{B7}boston, age\u{B7}old]"),
        ),
        (
            // The mapped axis composes with the ordinary literal-element arm rather
            // than replacing it: only the axis whose index names an iterated dim
            // goes through the correspondence.
            "a mapped dimension name beside a literal element",
            "pop[State, age\u{B7}old]",
            Some("pop[region\u{B7}boston, age\u{B7}old]"),
            Some("pop[region\u{B7}boston, age\u{B7}old]"),
        ),
    ];
    // GH #997: an element-mapped pair pins POSITIONALLY, so its expected
    // spelling is the same as the positional rows'. The map below is the reverse
    // permutation (ma -> nyc), so `region\u{B7}nyc` is what a map-following pin
    // would produce and its absence is asserted separately below.
    let element_mapped: [PinIndexCell<'_>; 1] = [(
        "an element-mapped pair (the ordinal wins)",
        "pop[State, old]",
        Some("pop[region\u{B7}boston, age\u{B7}old]"),
        Some("pop[region\u{B7}boston, age\u{B7}old]"),
    )];

    // Declaration direction must not matter: `positional_correspondence` honors a
    // mapping declared on either dimension (GH #757), so a reverse-declared
    // positional pair pins identically. A forward-only gate here would silently drop
    // half the mapped models.
    for declare_on_state in [true, false] {
        let fx = PinFixture::mapped(declare_on_state, vec![]);
        assert_pin_index_verdicts(&fx, "pop[State, young]", &positional);

        // An EXPLICIT element map does not change the verdict, because this
        // spelling never reads the map: `pop[State, old]` names the dimension the
        // equation ITERATES, and `mapped_reference_semantics_tests`' `Permuted`
        // row measures that spelling folding to an ordinal against the VM. This
        // block asserted a loud decline until GH #997, on the (correct, but
        // spelling-specific) reasoning that FOLLOWING the map would spell a read
        // the simulation never performs -- the fix is to describe the ordinal,
        // not to describe nothing.
        let fx = PinFixture::mapped(
            declare_on_state,
            vec![
                ("ny".to_string(), "boston".to_string()),
                ("ma".to_string(), "nyc".to_string()),
            ],
        );
        assert_pin_index_verdicts(&fx, "pop[State, young]", &element_mapped);

        // GH #997, the OTHER spelling on the SAME fixture: an index naming the
        // source's own `Region` -- a dimension this target does NOT iterate --
        // pins through the ELEMENT MAP, so it reads `nyc` where the
        // iterated-dimension spelling above reads `boston`. One fixture, two
        // indices, two rules; that is the whole of #997 in one assertion.
        assert_pin_index_verdicts(
            &fx,
            "pop[State, young]",
            &[(
                "the source's own dimension name (the map-following spelling)",
                "pop[Region, old]",
                Some("pop[region\u{B7}nyc, age\u{B7}old]"),
                Some("pop[region\u{B7}nyc, age\u{B7}old]"),
            )],
        );

        // The discriminator: the declared map sends `ma` to `nyc`, so a
        // map-following pin would spell `region\u{B7}nyc`. It must not appear.
        let (ast, deps, occurrences) = fx.parse(
            "pop[State, young] + LOOKUP(pop[State, old], input)",
            &["input"],
        );
        let slot_occurrences = SlotOccurrences::new(&occurrences);
        let text = fx
            .generate(&ast, &deps, &slot_occurrences.for_slot(0))
            .expect("the element-mapped pair pins positionally");
        assert!(
            !text.contains("region\u{B7}nyc"),
            "the element map's own element must not be pinned for an \
             iterated-dimension index; got: {text}"
        );
    }
}
