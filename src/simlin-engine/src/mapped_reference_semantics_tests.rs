// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! What the EXECUTED simulation reads for a cross-dimension arrayed
//! reference, pinned cell by cell against the VM (GH #997, and the
//! execution-side issues #756 / #753 it describes).
//!
//! `DimensionsContext::mapped_element_correspondence` carries a prose
//! account of a fork in executed behavior: which of THREE resolution rules a
//! mapped reference gets -- by ordinal (POSITIONAL), by the element's own
//! NAME in the source dimension, or by the declared ELEMENT MAP -- depends on
//! how the reference is spelled, and the last two are a single name-first
//! path rather than two independent ones. That account was assembled from
//! reading the lowering plus one ground-truth comparison (C-LEARN's
//! `Ref.vdf`, gated by `simulates_clearn`). This module measures it instead,
//! so the rustdoc's rows are checkable and a change to any of them turns a
//! test red rather than silently invalidating the reasoning built on top.
//!
//! The two-rule framing (positional versus element map) is the natural one
//! and it is what an earlier revision of that rustdoc said. It is wrong; see
//! "Three resolution rules, not two" below.
//!
//! # The matrix
//!
//! Every cell is a whole model built through the production path
//! (`TestProject` -> salsa -> VM) with per-element source values that are
//! pairwise distinct, so which source element a target element read is
//! uniquely identified by the number that comes out. Three axes:
//!
//! **Reference spelling** ([`Spelling`], 4 variants). The first three are
//! GH #997's; the fourth ([`Spelling::StockFlow`]) was found by measuring
//! during this work, and the same change that added this module added its
//! bullet to `mapped_element_correspondence`'s rustdoc.
//!
//! **Mapping kind** ([`MappingKind`], 5 variants) plus two no-mapping
//! controls (`no_mapping_*`), which are what distinguishes "resolves
//! positionally" from "there is no mapping machinery involved at all".
//!
//! **Declaration direction** ([`Direction`], 2 variants): which of the two
//! dimensions carries the mapping. Both directions of each fixture encode
//! the SAME element correspondence, so an expectation that holds for one
//! and not the other is a direction effect; [`expected`] is keyed on
//! (kind, spelling) only and `every_cell_of_the_matrix` asserts it for both
//! directions, which is where direction-insensitivity is asserted rather
//! than assumed.
//!
//! 4 x 5 x 2 = 40 mapped cells, all in `every_cell_of_the_matrix`; the two
//! controls contribute 4 cells each in `no_mapping_equal_cardinality` and
//! `no_mapping_unequal_cardinality`. The three axis enumerations are walked
//! through `Spelling::all` / `MappingKind::all` / `Direction::all`, each
//! built from a successor `match` so that adding a variant fails to compile
//! rather than silently escaping the matrix.
//!
//! No cell is collapsed away: the `MappingKind::Positional` row is not
//! discriminating (all three resolution rules agree there by construction)
//! and is kept precisely because that is why the fork went unnoticed --
//! `fixture_discriminates` asserts which rows discriminate and which do not,
//! so "not discriminating" is recorded rather than implied by a missing test.
//!
//! # Three resolution rules, not two
//!
//! The obvious framing is positional-versus-element-map. It is wrong, and
//! [`MappingKind::SharedElementNames`] is the row that shows it: the two
//! map-following spellings actually resolve **name-first**, trying the active
//! element's own name in the source dimension and consulting the element map
//! only when that misses. So there are three candidate answers per fixture,
//! and [`assert_cell`] excludes every one that did not happen instead of
//! merely matching the one that did.
//!
//! # What the Vensim reference actually says
//!
//! Quoted from <https://www.vensim.com/documentation/ref_subscript_mapping.html>
//! (retrieved 2026-08-01):
//!
//! - the trigger: "Quite simply a mapping is an indication to Vensim that a
//!   Subscript that appears on the right but not the left of an equation has
//!   a valid interpretation."
//! - the shape it is written for: "Normally, an equation such as
//!   `Quality[product] = work quality[worker type]` would generate an
//!   error." -- the right-hand subscript names the SOURCE's own range, which
//!   is [`Spelling::SourceOwnDim`] here.
//! - an explicit, order-bearing map: the general form
//!   `Rhsub:rh1,rh2->(Lhsub:lh1,lh2), (Lhbigger:Lhbsubr1,Lhbsubr2),
//!   (lhopposite:lho2,lho1)`, where "The map-to choices consist of the name
//!   of the map-to subscript range, followed by a colon : and the elements
//!   or subranges in the order the mapping should occur." The `lhopposite`
//!   choice lists `lho2,lho1` against a range declared `lho1,lho2` -- a
//!   PERMUTED map, which is [`MappingKind::Permuted`].
//! - many-to-one: Example 5's `class: class1,class2->(metal:class1
//!   metal,class2 metal)` with `attractiveness[metal] = class
//!   attractivness[class] * ...`, and "Note that while metal has 4 elements
//!   class only has two. ... In this case two elements of metal belong to
//!   each element of class, but it could also have been 3 and 1." That is
//!   [`MappingKind::ManyToOne`] on [`Spelling::SourceOwnDim`], and it is the
//!   shape C-LEARN ships.
//! - the iterated spelling IS legal Vensim, and the page credits a mapping
//!   for it: Example 3 declares `PTASKS <-> TASKS` (stated to be the same as
//!   `PTASKS : CLEAR,DIG,BUILD -> TASKS`), writes `prereq qual[task,ptask] =
//!   quality factors[ptask]`, and says "This will work even though quality
//!   factors is actually defined by task, not by ptask." The right-hand
//!   subscript names a range the LEFT side iterates while the source is
//!   declared over another -- structurally [`Spelling::IteratedDim`].
//! - BOTH declaration directions appear. Examples 1, 4 and 5 declare the
//!   mapping on the source variable's own range pointing at the left-hand one
//!   (`Rhsub -> Lhsub`, the form the prose describes) = [`Direction::OnSourceDim`];
//!   Example 3 declares it on `PTASKS`, a range the left side iterates,
//!   pointing at `TASKS`, the source's own range = [`Direction::OnIteratedDim`].
//!   Neither direction is a Simlin extension. (Example 2 does not bear on
//!   this, PROVIDED `aging` is declared over `AGE`: both of its ranges are
//!   then subranges of the source's own. The page never states `aging`'s
//!   declaration; it is inferable only from its three subscripted uses,
//!   which between them cover all five `AGE` cohorts.)
//!
//! What the page does NOT settle, recorded as UNVERIFIED rather than assumed:
//!
//! - which RULE Vensim applies to the iterated spelling. Example 3 cannot
//!   say, because `PTASKS` is a full subrange copy of `TASKS` with identical
//!   element names in the same order, so positional, name-identity and
//!   map-following all coincide there. The residual doubt therefore runs
//!   toward Vensim MAPPING on that spelling where this engine resolves
//!   positionally -- not toward Vensim rejecting it.
//! - anything about [`MappingKind::ReverseCardinality`]. That fixture leaves
//!   one source element with no correspondent, which the page's Example 4
//!   forbids ("the Subscript Ranges ... must not overlap and must completely
//!   exhaust the second group"), so it is not expressible in Vensim's syntax
//!   at all. The row exists to isolate this engine's range check -- it is why
//!   [`MappingKind::ManyToOne`] is refused on the positional spellings -- and
//!   no Vensim-parity inference should be drawn from it.
//!
//! Element maps themselves are a `simlin:mapping`/`simlin:elem` vendor
//! extension: XMILE 1.0's §2.5 `<dimensions>` block admits only
//! `<dim name size>` and `<elem name>` and the spec has no dimension-mapping
//! construct, the sole prose occurrence of "mapping" being §3.6's discussion
//! of mapping unsupported FUNCTIONS. Checked by stripping the tags from
//! `docs/reference/xmile-v1.0.html` and searching the prose -- the method the
//! root `CLAUDE.md` prescribes, and the only one that works here, since the
//! file is non-UTF-8 and plain `grep` reports no match for ANY query.
//!
//! # The two routes a subscript-less reference can take
//!
//! The measured surprise, and the reason [`Spelling::StockFlow`] exists: a
//! reference spelled with NO subscript has two different lowerings with
//! different answers.
//!
//! - Inside an equation body (`target[State] = x`), `Context::lower_pass0`
//!   rewrites the bare `Expr2::Var` into an `Expr2::Subscript` spelled with
//!   the ACTIVE dimension's name -- so a bare in-equation reference IS the
//!   [`Spelling::IteratedDim`] spelling by the time anything resolves it,
//!   and is positional. When `match_dimensions_with_mapping` finds no
//!   correspondence at all it emits a wildcard instead, and the reference
//!   becomes a whole-array broadcast -- a third behavior, which
//!   `no_mapping_unequal_cardinality` separates from the other two.
//! - As a stock's inflow/outflow (`level[State] = INTEG(feed, 0)` with
//!   `feed` declared over `Region`), the reference never passes through
//!   pass 0: `Context::fold_flows` calls `get_ref` directly, reaching
//!   `get_implicit_subscript_off`, whose
//!   `dim.get_offset(&element).or_else(...)` tries the active element's own
//!   NAME in the source dimension first and consults
//!   `DimensionsContext::translate_via_mapping` only when that misses.
//!
//! So the two subscript-less spellings disagree, and
//! `a_bare_equation_reference_and_a_flow_reference_disagree` pins that
//! disagreement directly, since it is the observable consequence of the
//! routing and the thing a future refactor is most likely to erase by
//! accident.

use crate::common::ErrorCode;
use crate::datamodel;
use crate::test_common::TestProject;

/// How the reference to the `Region`-declared source is written.
#[derive(Copy, Clone)]
enum Spelling {
    /// `target[State] = x[State]` -- the subscript names the dimension the
    /// equation ITERATES. Pass 1 (`ast::expr3::Pass1Context`) folds an
    /// active dimension name to that dimension's ordinal, which then indexes
    /// the source's storage raw.
    IteratedDim,
    /// `target[State] = x[Region]` -- the subscript names a dimension that is
    /// NOT active, here the source's own. It survives pass 1 as an
    /// `IndexExpr3::Dimension`, is normalized to an `IndexOp::ActiveDimRef`
    /// by the free function `compiler::subscript::normalize_subscripts3`, and
    /// is resolved in that module's `build_view_from_ops`, whose
    /// `dim.get_offset(subscript).or_else(...)` tries the active element's own
    /// NAME in the source dimension before falling back to
    /// `DimensionsContext::translate_via_mapping`.
    SourceOwnDim,
    /// `target[State] = x` -- no subscript, inside an equation body.
    BareInEquation,
    /// `level[State] = INTEG(x, 0)` where `x` is a `Region`-declared flow --
    /// no subscript, and a different lowering route from `BareInEquation`.
    StockFlow,
}

/// The correspondence between `State` (the target's dimension) and `Region`
/// (the source's), and how it is declared.
#[derive(Copy, Clone)]
enum MappingKind {
    /// `maps_to` with no element map: correspondence is by position, so
    /// positional resolution and map-following cannot be told apart.
    Positional,
    /// An explicit element map over equal cardinalities that is NOT the
    /// identity permutation.
    Permuted,
    /// An explicit element map from 3 target elements onto 2 source
    /// elements (C-LEARN's shape, and Vensim's Example 5).
    ManyToOne,
    /// An explicit element map from 2 target elements onto 3 source
    /// elements -- the many-to-one arrangement with the cardinalities
    /// swapped, so a positional read stays in range where `ManyToOne`'s runs
    /// off the end. Not expressible in Vensim (it leaves a source element
    /// with no correspondent); see the module docs.
    ReverseCardinality,
    /// An explicit element map between two dimensions that declare the SAME
    /// element names in a DIFFERENT order -- Vensim's Example 3 idiom, where
    /// a subrange copy shares its parent's element names.
    ///
    /// This is the row that shows map-following is really NAME-first: all
    /// three candidate answers are distinct here, and the two map-following
    /// spellings return the name-identity one.
    SharedElementNames,
}

/// Every variant of the three axis enumerations, walked as a successor
/// chain rather than written as a literal array.
///
/// The chain is what makes the matrix exhaustive. A literal array compiles
/// fine when a variant is added and is missing from it, so the new variant
/// would never be run; here the `match` has no arm for it and the module
/// fails to build. [`expected`] enforces the other half -- a variant with no
/// row is likewise a compile error there.
impl Spelling {
    fn all() -> Vec<Self> {
        successors(Spelling::IteratedDim, |s| match s {
            Spelling::IteratedDim => Some(Spelling::SourceOwnDim),
            Spelling::SourceOwnDim => Some(Spelling::BareInEquation),
            Spelling::BareInEquation => Some(Spelling::StockFlow),
            Spelling::StockFlow => None,
        })
    }
}

impl MappingKind {
    fn all() -> Vec<Self> {
        successors(MappingKind::Positional, |k| match k {
            MappingKind::Positional => Some(MappingKind::Permuted),
            MappingKind::Permuted => Some(MappingKind::ManyToOne),
            MappingKind::ManyToOne => Some(MappingKind::ReverseCardinality),
            MappingKind::ReverseCardinality => Some(MappingKind::SharedElementNames),
            MappingKind::SharedElementNames => None,
        })
    }
}

fn successors<T: Copy>(first: T, next: impl Fn(T) -> Option<T>) -> Vec<T> {
    let mut out = vec![first];
    while let Some(n) = next(*out.last().expect("seeded above")) {
        out.push(n);
    }
    out
}

/// Which dimension carries the mapping declaration.
#[derive(Copy, Clone)]
enum Direction {
    /// Declared on `State`, the dimension the target equation iterates.
    /// Vensim's Example 3 (see module docs).
    OnIteratedDim,
    /// Declared on `Region`, the source variable's own dimension. Vensim's
    /// Examples 1, 4 and 5 (see module docs).
    OnSourceDim,
}

impl Direction {
    fn all() -> Vec<Self> {
        successors(Direction::OnIteratedDim, |d| match d {
            Direction::OnIteratedDim => Some(Direction::OnSourceDim),
            Direction::OnSourceDim => None,
        })
    }
}

/// What a cell of the matrix does when it runs.
enum Expected {
    /// The target's elements, in declared order, read these source values.
    Reads(&'static [f64]),
    /// The model does not compile, with this diagnostic code.
    Refused(ErrorCode),
}

impl Spelling {
    fn label(self) -> &'static str {
        match self {
            Spelling::IteratedDim => "target[State] = x[State]",
            Spelling::SourceOwnDim => "target[State] = x[Region]",
            Spelling::BareInEquation => "target[State] = x",
            Spelling::StockFlow => "target[State] = INTEG(x, 0)",
        }
    }
}

impl MappingKind {
    fn label(self) -> &'static str {
        match self {
            MappingKind::Positional => "positional (maps_to)",
            MappingKind::Permuted => "permuted element map",
            MappingKind::ManyToOne => "many-to-one element map (3 State onto 2 Region)",
            MappingKind::ReverseCardinality => {
                "reverse-cardinality element map (2 State onto 3 Region)"
            }
            MappingKind::SharedElementNames => {
                "element map over dimensions sharing element names, reordered"
            }
        }
    }

    /// The source elements and their (pairwise distinct) values.
    fn source(self) -> &'static [(&'static str, &'static str)] {
        match self {
            MappingKind::Positional | MappingKind::Permuted | MappingKind::ReverseCardinality => {
                &[("Ruby", "10"), ("Rose", "20"), ("Reed", "30")]
            }
            MappingKind::ManyToOne => &[("Ruby", "10"), ("Rose", "20")],
            MappingKind::SharedElementNames => &[("Ann", "10"), ("Bob", "20"), ("Cal", "30")],
        }
    }

    /// The target's elements in declared order.
    fn target_elements(self) -> &'static [&'static str] {
        match self {
            MappingKind::Positional | MappingKind::Permuted | MappingKind::ManyToOne => {
                &["Steel", "Slate", "Stone"]
            }
            MappingKind::ReverseCardinality => &["Steel", "Slate"],
            // The SOURCE's names, ROTATED: that is what pulls name identity
            // apart from position, so the row has three distinct answers
            // rather than two.
            MappingKind::SharedElementNames => &["Cal", "Ann", "Bob"],
        }
    }

    /// The declared correspondence as (target element, source element)
    /// pairs, in the target's declared order.
    fn correspondence(self) -> &'static [(&'static str, &'static str)] {
        match self {
            // Identity by position -- `maps_to` carries no element list.
            MappingKind::Positional => &[("Steel", "Ruby"), ("Slate", "Rose"), ("Stone", "Reed")],
            MappingKind::Permuted => &[("Steel", "Reed"), ("Slate", "Ruby"), ("Stone", "Rose")],
            MappingKind::ManyToOne => &[("Steel", "Ruby"), ("Slate", "Rose"), ("Stone", "Ruby")],
            MappingKind::ReverseCardinality => &[("Steel", "Reed"), ("Slate", "Ruby")],
            // Chosen to differ from BOTH the positional and the name-identity
            // answer, so a cell that follows the map is unmistakable.
            MappingKind::SharedElementNames => &[("Cal", "Bob"), ("Ann", "Cal"), ("Bob", "Ann")],
        }
    }

    /// What each target element reads if the declared element map is
    /// followed.
    fn map_reads(self) -> Vec<f64> {
        self.correspondence()
            .iter()
            .map(|(_, src)| {
                self.source()
                    .iter()
                    .find(|(name, _)| name == src)
                    .map(|(_, v)| v.parse::<f64>().unwrap())
                    .unwrap_or_else(|| panic!("correspondence names an unknown source {src}"))
            })
            .collect()
    }

    /// What each target element reads if resolved POSITIONALLY (target
    /// ordinal indexes the source's storage), or `None` when an ordinal runs
    /// off the end of the source.
    fn positional_reads(self) -> Option<Vec<f64>> {
        let source = self.source();
        self.target_elements()
            .iter()
            .enumerate()
            .map(|(i, _)| source.get(i).map(|(_, v)| v.parse::<f64>().unwrap()))
            .collect()
    }

    /// The element names this fixture's two dimensions have in common.
    ///
    /// [`Self::name_identity_reads`] cannot answer this: it collects into an
    /// `Option`, so ONE missing element makes the whole thing `None` and a
    /// partially-overlapping fixture would look disjoint. Since `None` is
    /// also what switches off the name-identity exclusion in [`assert_cell`],
    /// reading disjointness off it would let a row silently lose that
    /// exclusion while a test asserted it had none to lose.
    fn shared_element_names(self) -> Vec<&'static str> {
        self.target_elements()
            .iter()
            .copied()
            .filter(|elem| self.source().iter().any(|(name, _)| name == elem))
            .collect()
    }

    /// What each target element reads if the source is indexed by the target
    /// element's own NAME, or `None` unless EVERY target element is present
    /// in the source (a partial overlap has no well-defined answer for this
    /// rule, so there is nothing to exclude).
    fn name_identity_reads(self) -> Option<Vec<f64>> {
        let source = self.source();
        self.target_elements()
            .iter()
            .map(|elem| {
                source
                    .iter()
                    .find(|(name, _)| name == elem)
                    .map(|(_, v)| v.parse::<f64>().unwrap())
            })
            .collect()
    }

    /// Every resolution rule's answer, labelled. `assert_cell` asserts the
    /// measured read equals the expected rule's answer and differs from each
    /// other rule's, so a cell excludes what did not happen rather than only
    /// matching what did.
    fn candidate_answers(self) -> [(&'static str, Option<Vec<f64>>); 3] {
        [
            ("positional", self.positional_reads()),
            ("name identity", self.name_identity_reads()),
            ("element map", Some(self.map_reads())),
        ]
    }
}

/// The dimension pair for one (kind, direction), with the mapping declared
/// on exactly one of them.
fn dimensions(kind: MappingKind, direction: Direction) -> Vec<datamodel::Dimension> {
    let region_elems: Vec<String> = kind
        .source()
        .iter()
        .map(|(name, _)| name.to_string())
        .collect();
    let state_elems: Vec<String> = kind
        .target_elements()
        .iter()
        .map(|s| s.to_string())
        .collect();
    let mut region = datamodel::Dimension::named("Region".to_string(), region_elems);
    let mut state = datamodel::Dimension::named("State".to_string(), state_elems);

    match (kind, direction) {
        (MappingKind::Positional, Direction::OnIteratedDim) => {
            state.set_maps_to("Region".to_string())
        }
        (MappingKind::Positional, Direction::OnSourceDim) => {
            region.set_maps_to("State".to_string())
        }
        (_, Direction::OnIteratedDim) => {
            state.mappings = vec![datamodel::DimensionMapping {
                target: "Region".to_string(),
                element_map: kind
                    .correspondence()
                    .iter()
                    .map(|(t, s)| (t.to_string(), s.to_string()))
                    .collect(),
            }];
        }
        (_, Direction::OnSourceDim) => {
            region.mappings = vec![datamodel::DimensionMapping {
                target: "State".to_string(),
                element_map: kind
                    .correspondence()
                    .iter()
                    .map(|(t, s)| (s.to_string(), t.to_string()))
                    .collect(),
            }];
        }
    }
    vec![region, state]
}

/// Build the whole model for one cell.
fn model(kind: MappingKind, direction: Direction, spelling: Spelling) -> TestProject {
    let mut project = TestProject::new("mapped_reference");
    project.dimensions = dimensions(kind, direction);
    match spelling {
        Spelling::StockFlow => project
            .array_flow_with_ranges("x[Region]", kind.source().to_vec())
            .array_stock("target[State]", "0", &["x"], &[], None),
        Spelling::IteratedDim => project
            .array_with_ranges("x[Region]", kind.source().to_vec())
            .array_aux("target[State]", "x[State]"),
        Spelling::SourceOwnDim => project
            .array_with_ranges("x[Region]", kind.source().to_vec())
            .array_aux("target[State]", "x[Region]"),
        Spelling::BareInEquation => project
            .array_with_ranges("x[Region]", kind.source().to_vec())
            .array_aux("target[State]", "x"),
    }
}

/// Run one cell and report what the target's elements read, or the message
/// that stopped it -- kept verbatim so a VM RUN failure is distinguishable
/// from a compile refusal when a cell's expectation turns out to be wrong.
fn run_cell(
    kind: MappingKind,
    direction: Direction,
    spelling: Spelling,
) -> Result<Vec<f64>, String> {
    let project = model(kind, direction, spelling);
    let results = project.run_vm()?;
    Ok(kind
        .target_elements()
        .iter()
        .map(|elem| {
            let key = format!("target[{}]", crate::canonicalize(elem));
            *results
                .get(&key)
                .unwrap_or_else(|| panic!("no series for {key}"))
                .last()
                .expect("empty series")
        })
        .collect())
}

/// The executed behavior of every (mapping kind, spelling) pair.
///
/// Exhaustive over the product of the two enumerations: adding a variant to
/// either is a compile error here, not a silently uncovered cell. Direction
/// is deliberately NOT a parameter -- see the module docs.
fn expected(kind: MappingKind, spelling: Spelling) -> Expected {
    use MappingKind::*;
    use Spelling::*;
    match (kind, spelling) {
        // A positional mapping makes the two resolutions agree, so all four
        // spellings return the same numbers. This row is why the fork below
        // could go unnoticed for so long.
        (Positional, IteratedDim)
        | (Positional, SourceOwnDim)
        | (Positional, BareInEquation)
        | (Positional, StockFlow) => Expected::Reads(&[10.0, 20.0, 30.0]),

        // The permuted row is the fork, in its clearest form: the same
        // three source values, read in two different orders depending only
        // on how the reference is spelled.
        (Permuted, IteratedDim) => Expected::Reads(&[10.0, 20.0, 30.0]),
        (Permuted, BareInEquation) => Expected::Reads(&[10.0, 20.0, 30.0]),
        (Permuted, SourceOwnDim) => Expected::Reads(&[30.0, 10.0, 20.0]),
        (Permuted, StockFlow) => Expected::Reads(&[30.0, 10.0, 20.0]),

        // Many-to-one: the positional spellings have no third source
        // element to index and are refused. `Generic` is what the static
        // subscript resolution reports ("Index out of bounds for dimension
        // 0", `compiler::subscript`) -- it is pinned as the code that ships,
        // not endorsed; a mapping-aware diagnostic would be better and is
        // GH #753's territory.
        (ManyToOne, IteratedDim) => Expected::Refused(ErrorCode::Generic),
        (ManyToOne, BareInEquation) => Expected::Refused(ErrorCode::Generic),
        (ManyToOne, SourceOwnDim) => Expected::Reads(&[10.0, 20.0, 10.0]),
        (ManyToOne, StockFlow) => Expected::Reads(&[10.0, 20.0, 10.0]),

        // Reverse cardinality isolates WHY many-to-one is refused: it is the
        // positional index leaving the source's range, not unequal
        // cardinality as such. Here every target ordinal is in range, so the
        // positional spellings compile -- and read the wrong elements.
        (ReverseCardinality, IteratedDim) => Expected::Reads(&[10.0, 20.0]),
        (ReverseCardinality, BareInEquation) => Expected::Reads(&[10.0, 20.0]),
        (ReverseCardinality, SourceOwnDim) => Expected::Reads(&[30.0, 10.0]),
        (ReverseCardinality, StockFlow) => Expected::Reads(&[30.0, 10.0]),

        // Shared element names. The positional spellings are unmoved -- they
        // never look at a name. The other two return NAME IDENTITY, not the
        // element map: `Cal` reads `Cal` (30) though the map says `Bob` (20).
        // Source values 10/20/30 over {Ann,Bob,Cal}; target {Cal,Ann,Bob}.
        (SharedElementNames, IteratedDim) => Expected::Reads(&[10.0, 20.0, 30.0]),
        (SharedElementNames, BareInEquation) => Expected::Reads(&[10.0, 20.0, 30.0]),
        (SharedElementNames, SourceOwnDim) => Expected::Reads(&[30.0, 10.0, 20.0]),
        (SharedElementNames, StockFlow) => Expected::Reads(&[30.0, 10.0, 20.0]),
    }
}

fn assert_cell(kind: MappingKind, direction: Direction, spelling: Spelling) {
    let where_ = format!(
        "{} / {} / declared on {}",
        kind.label(),
        spelling.label(),
        match direction {
            Direction::OnIteratedDim => "State (the iterated dim)",
            Direction::OnSourceDim => "Region (the source's dim)",
        }
    );
    match expected(kind, spelling) {
        Expected::Reads(want) => {
            let got = run_cell(kind, direction, spelling).unwrap_or_else(|e| {
                panic!("{where_}: expected {want:?}, but the model did not run: {e}")
            });
            assert_eq!(got, want, "{where_}");

            // Every OTHER resolution rule's answer must be excluded, not
            // merely un-asserted. A rule that cannot apply to this fixture
            // yields `None`, and one that agrees with `want` has nothing to
            // exclude -- `fixture_discriminates` records which rows are in
            // that position, so it is a stated property rather than a gap.
            for (rule, answer) in kind.candidate_answers() {
                if let Some(answer) = answer
                    && answer != want
                {
                    assert_ne!(got, answer, "{where_}: read the {rule} answer {answer:?}");
                }
            }
        }
        Expected::Refused(code) => {
            assert_refused(&model(kind, direction, spelling), code, &where_);
        }
    }
}

/// A compile refusal, pinned to the FAILING VARIABLE as well as the code.
///
/// `TestProject::assert_compile_error_vm` accepts any Error-severity
/// diagnostic anywhere in the project carrying the code, which for a code as
/// broad as `Generic` is close to no constraint at all. `TestProject::compile`
/// reports `("model.variable", code)` pairs from the same
/// `collect_all_diagnostics` pass, so the pin can name the target. A message
/// substring would be stronger still, but the diagnostic these cells produce
/// is an `EquationError`, which carries a code and a span and no text.
fn assert_refused(project: &TestProject, code: ErrorCode, where_: &str) {
    let errors = match project.compile() {
        Ok(_) => panic!("{where_}: expected a compile failure, but it compiled"),
        Err(errors) => errors,
    };
    let want = ("main.target".to_string(), code);
    assert!(
        errors.contains(&want),
        "{where_}: expected {want:?} among the diagnostics, got {errors:?}"
    );
}

#[test]
fn every_cell_of_the_matrix() {
    for kind in MappingKind::all() {
        for direction in Direction::all() {
            for spelling in Spelling::all() {
                assert_cell(kind, direction, spelling);
            }
        }
    }
}

/// Which rows of the matrix can tell the resolution rules apart.
///
/// Without this, a reader cannot distinguish "this cell pins map-following"
/// from "this cell would pass either way", and the `Positional` row -- which
/// is the second kind -- looks like coverage it is not. The `match` is
/// exhaustive, so a new mapping kind has to declare its discriminating power
/// here as well as its rows in [`expected`].
#[test]
fn fixture_discriminates() {
    for kind in MappingKind::all() {
        let map = kind.map_reads();
        let positional = kind.positional_reads();
        let name_identity = kind.name_identity_reads();
        match kind {
            MappingKind::Positional => assert_eq!(
                Some(map),
                positional,
                "the positional row must NOT discriminate -- if it does, the \
                 fixture no longer models a positional mapping"
            ),
            MappingKind::ManyToOne => assert!(
                positional.is_none(),
                "the many-to-one row's positional read must run off the end of \
                 the source; that is what makes its two refused cells meaningful"
            ),
            MappingKind::Permuted | MappingKind::ReverseCardinality => {
                assert_ne!(
                    Some(map),
                    positional,
                    "{}: map-following and positional must differ, or its cells \
                     pass either way",
                    kind.label()
                );
                assert_eq!(
                    kind.shared_element_names(),
                    Vec::<&str>::new(),
                    "{}: these rows must share NO element names, so that name \
                     identity cannot apply and the row is a clean two-way test",
                    kind.label()
                );
            }
            // The only row where all THREE rules apply and disagree. That is
            // the whole point of it: without three distinct answers it could
            // not show that map-following is really name-first.
            MappingKind::SharedElementNames => {
                let map = Some(map);
                assert_ne!(map, positional, "{}", kind.label());
                assert_ne!(map, name_identity, "{}", kind.label());
                assert_ne!(positional, name_identity, "{}", kind.label());
                assert_eq!(
                    kind.shared_element_names(),
                    kind.target_elements(),
                    "{}: EVERY target element must also be a source element, or \
                     the name-identity rule is only partly applicable here",
                    kind.label()
                );
            }
        }
    }
}

/// Control: with NO mapping declared at all, at equal cardinality.
///
/// The point is that two of the four spellings do not require a mapping to
/// exist. `IteratedDim` never consults one (pass 1 folds the active
/// dimension to an ordinal and indexes the source raw), and
/// `BareInEquation` falls back to a whole-array broadcast -- so a
/// cross-dimension read between two dimensions declared to have NOTHING to
/// do with each other compiles and silently produces numbers. The two
/// spellings that DO consult the mapping are refused.
#[test]
fn no_mapping_equal_cardinality() {
    let dims = || {
        vec![
            datamodel::Dimension::named(
                "Region".to_string(),
                vec!["Ruby".to_string(), "Rose".to_string(), "Reed".to_string()],
            ),
            datamodel::Dimension::named(
                "State".to_string(),
                vec![
                    "Steel".to_string(),
                    "Slate".to_string(),
                    "Stone".to_string(),
                ],
            ),
        ]
    };
    let cases: [(Spelling, Expected); 4] = [
        (Spelling::IteratedDim, Expected::Reads(&[10.0, 20.0, 30.0])),
        (
            Spelling::BareInEquation,
            Expected::Reads(&[10.0, 20.0, 30.0]),
        ),
        (
            Spelling::SourceOwnDim,
            Expected::Refused(ErrorCode::MismatchedDimensions),
        ),
        (
            Spelling::StockFlow,
            Expected::Refused(ErrorCode::MismatchedDimensions),
        ),
    ];
    assert_no_mapping_cases(dims, &["Steel", "Slate", "Stone"], &cases);
}

/// Control: NO mapping, and the target has FEWER elements than the source.
///
/// This is what separates `BareInEquation` from `IteratedDim`. They agree in
/// every other no-mapping cell, which reads like one behavior; here the
/// broadcast fallback needs the two extents to match and is refused, while
/// the iterated spelling -- which only needs its ordinal to be in range --
/// still compiles. Two routes, not one.
#[test]
fn no_mapping_unequal_cardinality() {
    let dims = || {
        vec![
            datamodel::Dimension::named(
                "Region".to_string(),
                vec!["Ruby".to_string(), "Rose".to_string(), "Reed".to_string()],
            ),
            datamodel::Dimension::named(
                "State".to_string(),
                vec!["Steel".to_string(), "Slate".to_string()],
            ),
        ]
    };
    let cases: [(Spelling, Expected); 4] = [
        (Spelling::IteratedDim, Expected::Reads(&[10.0, 20.0])),
        (
            Spelling::BareInEquation,
            Expected::Refused(ErrorCode::MismatchedDimensions),
        ),
        (
            Spelling::SourceOwnDim,
            Expected::Refused(ErrorCode::MismatchedDimensions),
        ),
        (
            Spelling::StockFlow,
            Expected::Refused(ErrorCode::MismatchedDimensions),
        ),
    ];
    assert_no_mapping_cases(dims, &["Steel", "Slate"], &cases);
}

fn assert_no_mapping_cases(
    dims: impl Fn() -> Vec<datamodel::Dimension>,
    target_elements: &[&str],
    cases: &[(Spelling, Expected)],
) {
    let source = [("Ruby", "10"), ("Rose", "20"), ("Reed", "30")];
    for (spelling, want) in cases {
        let mut project = TestProject::new("no_mapping");
        project.dimensions = dims();
        let project = match spelling {
            Spelling::StockFlow => project
                .array_flow_with_ranges("x[Region]", source.to_vec())
                .array_stock("target[State]", "0", &["x"], &[], None),
            Spelling::IteratedDim => project
                .array_with_ranges("x[Region]", source.to_vec())
                .array_aux("target[State]", "x[State]"),
            Spelling::SourceOwnDim => project
                .array_with_ranges("x[Region]", source.to_vec())
                .array_aux("target[State]", "x[Region]"),
            Spelling::BareInEquation => project
                .array_with_ranges("x[Region]", source.to_vec())
                .array_aux("target[State]", "x"),
        };
        let label = spelling.label();
        match want {
            Expected::Reads(want) => {
                let results = project
                    .run_vm()
                    .unwrap_or_else(|e| panic!("no mapping / {label}: expected it to run: {e}"));
                let got: Vec<f64> = target_elements
                    .iter()
                    .map(|elem| {
                        *results[&format!("target[{}]", crate::canonicalize(elem))]
                            .last()
                            .expect("empty series")
                    })
                    .collect();
                assert_eq!(got, *want, "no mapping / {label}");
            }
            Expected::Refused(code) => {
                assert_refused(&project, *code, &format!("no mapping / {label}"));
            }
        }
    }
}

/// The two subscript-less spellings disagree, and this is the single
/// assertion that says so out loud.
///
/// `mapped_element_correspondence`'s rustdoc splits "bare" into two bullets
/// for exactly this reason: a bare reference in an EQUATION is positional,
/// while a stock's flow reference -- equally subscript-less -- resolves
/// name-first and, where the names differ, follows the element map. The
/// difference is entirely the lowering route (module docs), so any refactor
/// that unifies the two -- which is a natural thing to want -- changes
/// executed numbers on one side or the other, and this test is what makes
/// that loud.
///
/// `Permuted` is the kind to run it on: its two dimensions share no element
/// names, so the flow reference reaches its element-map fallback rather than
/// stopping at name identity.
#[test]
fn a_bare_equation_reference_and_a_flow_reference_disagree() {
    let kind = MappingKind::Permuted;
    for direction in Direction::all() {
        let bare = run_cell(kind, direction, Spelling::BareInEquation)
            .expect("the bare equation reference compiles");
        let flow =
            run_cell(kind, direction, Spelling::StockFlow).expect("the flow reference compiles");
        assert_eq!(bare, kind.positional_reads().unwrap());
        assert_eq!(flow, kind.map_reads());
        assert_ne!(
            bare, flow,
            "the two subscript-less spellings must still disagree under a \
             permuted element map"
        );
    }
}

/// GH #996, on the EXECUTED path: an earlier dependency axis must not claim
/// BY MAPPING the active slot a later axis matches BY NAME.
///
/// This is the hazard shape as a whole model rather than a hand-built call
/// to `allocate_implicit_axes_partial`. Reaching the allocator from a real
/// model constrains the fixture: ordinary expression references never get
/// there, because `Context::lower_pass0` rewrites a bare arrayed reference
/// into an explicit subscript first (module docs). Tagging each call with its
/// caller and running the whole lib suite found EXACTLY TWO production
/// callers, both wiring rather than expressions -- `Context::fold_flows` (a
/// stock's flow references) and `compiler::Var::new` (the stock
/// self-reference and module input wiring), with zero arriving via
/// `lower_from_expr3` -- so a stock whose FLOW is declared over two
/// dimensions is the way in. The counts and their measurement condition are
/// on `compiler::context`'s `get_implicit_subscripts`.
///
/// `feed` is declared `[Board Type, Line]` and `level` iterates
/// `[Line, Shift]`. `Board Type` maps to BOTH `Line` (positionally) and
/// `Shift` (through a non-identity element map), so it can claim either
/// slot; `Line` matches slot 0 by name and has nothing else. Name-first
/// allocation therefore gives `Line` its slot and leaves `Board Type` the
/// `Shift` slot, which it resolves through the element map:
/// `Day Shift -> Oak Board`, `Night Shift -> Pine Board`.
///
/// The values are what makes this a pin rather than a smoke test. Reading
/// the element map gives `level[Line One, Day Shift] = feed[Oak Board, Line
/// One] = 104`; the swap gives 101, and every one of the four cells has a
/// distinct wrong answer. Under the pre-#996 order-greedy allocation the
/// model does not compile at all (`Board Type`, processed first, takes the
/// `Line` slot by mapping and `Line` finds it gone) -- verified by
/// temporarily restoring the per-dimension staging, which turns this test
/// into `MismatchedDimensions` on `level`.
///
/// The dimension and element names carry capitals and spaces deliberately:
/// GH #996 records that a lowercase single-word name is already canonical,
/// so a fixture built from one passes vacuously.
#[test]
fn the_996_hazard_shape_compiles_and_reads_name_first() {
    let project = TestProject::new("implicit_axis_precedence")
        .named_dimension("Line", &["Line One", "Line Two"])
        .named_dimension("Shift", &["Day Shift", "Night Shift"])
        .named_dimension_with_mappings(
            "Board Type",
            &["Pine Board", "Oak Board"],
            &[
                // Positional, and present only so the mapping pass can
                // reach the `Line` slot -- the steal the fix prevents.
                ("Line", &[]),
                // Non-identity, so the slot it legitimately gets is
                // resolved by the map rather than by position.
                (
                    "Shift",
                    &[("Pine Board", "Night Shift"), ("Oak Board", "Day Shift")],
                ),
            ],
        )
        .array_with_ranges(
            "boardweight[Board Type]",
            vec![("Pine Board", "1"), ("Oak Board", "4")],
        )
        .array_with_ranges(
            "lineweight[Line]",
            vec![("Line One", "100"), ("Line Two", "200")],
        )
        .array_flow(
            "feed[Board Type, Line]",
            "boardweight[\"Board Type\"] + lineweight[Line]",
            None,
        )
        .array_stock("level[Line, Shift]", "0", &["feed"], &[], None);

    let results = project
        .run_vm()
        .expect("the hazard shape must compile and run");

    for (cell, want, wrong) in [
        ("level[line_one,day_shift]", 104.0, 101.0),
        ("level[line_one,night_shift]", 101.0, 104.0),
        ("level[line_two,day_shift]", 204.0, 201.0),
        ("level[line_two,night_shift]", 201.0, 204.0),
    ] {
        let got = *results[cell].last().expect("empty series");
        assert_eq!(got, want, "{cell}");
        assert_ne!(got, wrong, "{cell}: read the axis-swapped element instead");
    }
}
