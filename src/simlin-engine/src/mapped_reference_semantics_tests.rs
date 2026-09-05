// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! What the EXECUTED simulation reads for a cross-dimension arrayed
//! reference, pinned cell by cell against the VM (GH #997, and the
//! execution-side issues #756 / #753 it describes).
//!
//! ONE rule answers every spelling: the active element's own NAME in the
//! source dimension, then the declared ELEMENT MAP in either direction, then a
//! mapped parent of the active subdimension
//! (`DimensionsContext::resolve_mapped_read`, GH #997). Reading by ORDINAL is
//! the last resort, reached only where the two dimensions declare no
//! correspondence at all -- the `no_mapping_*` controls, and the unpaired-axis
//! case `Context::resolve_iteration_element` handles one axis-collapse later.
//!
//! That is worth measuring rather than reading off the lowering, because the
//! four spellings reach the rule by four different ROUTES (below) and nothing
//! but this module makes them answer the same. A route that folded a subscript
//! naming the ITERATED dimension to that dimension's ordinal before anything
//! mapped it would read a different array under a permuted element map, and
//! would go unnoticed for as long as models declared only the
//! `MappingKind::Positional` row, where every rule agrees -- which is the one
//! people write.
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
//! bullet to the correspondence rustdocs.
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
//! `no_mapping_unequal_cardinality`, and
//! `no_mapping_reads_by_ordinal_on_both_subscript_paths` walks the undeclared
//! pair over a fourth axis, the SIBLING index ([`SiblingIndex`], 6 variants),
//! which decides whether the static or the dynamic subscript path resolves
//! the reference. The four axis enumerations are walked
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
//! # Name-first, not element-map-first
//!
//! The obvious framing is positional-versus-element-map. It is wrong, and
//! [`MappingKind::SharedElementNames`] is the row that shows it: resolution is
//! **name-first**, trying the active element's own name in the source
//! dimension and consulting the element map only when that misses. So there
//! are three candidate answers per fixture, and [`assert_cell`] excludes every
//! one that did not happen instead of merely matching the one that did.
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
//!   map-following all coincide there. This engine maps on that spelling like
//!   every other, which is the reading the page's prose leans toward -- it
//!   credits a mapping for the shape -- but the page does not settle it, and
//!   nothing here should be read as a parity claim.
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
//!   and is positional. When `make_dimension_subscripts`'s axis matching
//!   finds no correspondence at all it emits a wildcard instead, and the reference
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
//! So the two subscript-less spellings take different routes to the same rule,
//! and `a_bare_equation_reference_and_a_flow_reference_agree` pins the
//! agreement directly, since it is the thing a change to either route is most
//! likely to break without noticing.

use crate::common::ErrorCode;
use crate::datamodel;
use crate::test_common::TestProject;

/// How the reference to the `Region`-declared source is written.
#[derive(Copy, Clone)]
enum Spelling {
    /// `target[State] = x[State]` -- the subscript names the dimension the
    /// equation ITERATES.
    IteratedDim,
    /// `target[State] = x[Region]` -- the subscript names a dimension that is
    /// NOT active, here the source's own.
    ///
    /// Both this and [`Self::IteratedDim`] reach the rule the same way: the
    /// subscript is an `IndexExpr3::Dimension`, normalized to an
    /// `IndexOp::ActiveDimRef` by
    /// `compiler::subscript::normalize_subscripts3` (which is what picks WHICH
    /// active dimension it names) and resolved in that module's
    /// `build_view_from_ops`, whose `dim.get_offset(subscript).or_else(...)`
    /// runs `DimensionsContext::resolve_mapped_read` and, only where no
    /// correspondence is declared at all, the active element's ordinal.
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
    // Spelling is NOT a parameter of the answer: all four resolve a
    // dimension-named subscript through the one executed rule
    // (`DimensionsContext::resolve_mapped_read` -- the active element's own
    // name on the source axis, then the declared mapping in either direction,
    // then a mapped parent of the active subdimension). The enumeration is kept
    // as a parameter because WHICH ROUTE each spelling takes to that rule is
    // still different, and a change that reintroduced a fork would show up here
    // as a cell that no longer matches its siblings.
    match spelling {
        IteratedDim | SourceOwnDim | BareInEquation | StockFlow => {}
    }
    match kind {
        // A positional mapping makes map-following and ordinal reading agree,
        // so this row cannot tell the rules apart. It is the control.
        Positional => Expected::Reads(&[10.0, 20.0, 30.0]),

        // The permuted row is the clearest statement of the rule: the map is
        // followed, so the three source values come back in the map's order.
        Permuted => Expected::Reads(&[30.0, 10.0, 20.0]),

        // Many-to-one: two target elements map onto one source element, which
        // the map answers for every one of them. An ordinal read would have no
        // third source element to index.
        ManyToOne => Expected::Reads(&[10.0, 20.0, 10.0]),

        // The many-to-one arrangement with the cardinalities swapped, where an
        // ordinal read would stay in range and read the wrong elements.
        ReverseCardinality => Expected::Reads(&[30.0, 10.0]),

        // Shared element names: the rule is NAME identity FIRST, so `Cal`
        // reads `Cal` (30) though the element map says `Bob` (20). Source
        // values 10/20/30 over {Ann,Bob,Cal}; target {Cal,Ann,Bob}.
        SharedElementNames => Expected::Reads(&[30.0, 10.0, 20.0]),
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
/// broad as `Generic` is close to no constraint at all.
/// `TestProject::error_diagnostics` reports `("model.variable", code)` pairs
/// from the same `collect_all_diagnostics` pass, so the pin can name the
/// target. A message substring would be stronger still, but the diagnostic
/// these cells produce is an `EquationError`, which carries a code and a span
/// and no text.
fn assert_refused(project: &TestProject, code: ErrorCode, where_: &str) {
    let errors = project.error_diagnostics();
    assert!(
        !errors.is_empty(),
        "{where_}: expected a compile failure, but no Error-severity diagnostic was emitted"
    );
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
/// exist. `IteratedDim` reaches the rule's LAST RESORT -- with no
/// correspondence declared between the two dimensions the active element's
/// ordinal indexes the source raw -- and `BareInEquation` falls back to a
/// whole-array broadcast, so a cross-dimension read between two dimensions
/// declared to have NOTHING to do with each other compiles and silently
/// produces numbers. The two spellings that reach the mapping through a
/// different route are refused.
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

/// How the index BESIDE the dimension-named one is spelled. The sibling
/// decides the ROUTE a reference takes -- every index static keeps it on
/// `normalize_subscripts3` + `build_view_from_ops`, one index needing runtime
/// evaluation sends the whole subscript to `Context::lower_index_expr3` -- and
/// must never decide the rule.
#[derive(Copy, Clone, Debug)]
enum SiblingIndex {
    /// `m[State, 1]`: static.
    Literal,
    /// `m[State, c]`: static, an element name of the sibling axis.
    ElementName,
    /// `m[State, idx]`: dynamic, a variable reference.
    Aux,
    /// `m[State, 1 + TIME]`: dynamic, and the column it selects moves between
    /// steps, which shows the sibling really is evaluated at runtime.
    Time,
    /// `m[State, k + 1]`: dynamic, arithmetic over a variable.
    Arithmetic,
    /// `m[State, @1]`: dynamic -- a dimension position inside an apply-to-all
    /// body is bound per element by `lower_index_expr3`
    /// (`Context::lower_static_subscript` returns `None` for it).
    DimPosition,
}

impl SiblingIndex {
    fn all() -> Vec<Self> {
        successors(SiblingIndex::Literal, |s| match s {
            SiblingIndex::Literal => Some(SiblingIndex::ElementName),
            SiblingIndex::ElementName => Some(SiblingIndex::Aux),
            SiblingIndex::Aux => Some(SiblingIndex::Time),
            SiblingIndex::Time => Some(SiblingIndex::Arithmetic),
            SiblingIndex::Arithmetic => Some(SiblingIndex::DimPosition),
            SiblingIndex::DimPosition => None,
        })
    }

    fn spelling(self) -> &'static str {
        match self {
            SiblingIndex::Literal => "1",
            SiblingIndex::ElementName => "c",
            SiblingIndex::Aux => "idx",
            SiblingIndex::Time => "1 + TIME",
            SiblingIndex::Arithmetic => "k + 1",
            SiblingIndex::DimPosition => "@1",
        }
    }

    /// Whether every index of `m[State, <sibling>]` is static, which is what
    /// keeps the reference on the static subscript path.
    fn is_static(self) -> bool {
        matches!(self, SiblingIndex::Literal | SiblingIndex::ElementName)
    }

    /// The code an unresolvable dimension-named subscript is refused with on
    /// this sibling's route: `Generic` from `build_view_from_ops`,
    /// `MismatchedDimensions` from `lower_index_expr3`. `@N` takes the static
    /// code although its in-range read is the dynamic arm's, because
    /// `Context::lower_static_subscript` builds the view first and hands the
    /// subscript to the dynamic path only afterwards.
    fn refusal_code(self) -> ErrorCode {
        match self {
            SiblingIndex::Literal | SiblingIndex::ElementName | SiblingIndex::DimPosition => {
                ErrorCode::Generic
            }
            SiblingIndex::Aux | SiblingIndex::Time | SiblingIndex::Arithmetic => {
                ErrorCode::MismatchedDimensions
            }
        }
    }

    /// The 0-based column each saved step reads: every sibling evaluates to 1
    /// (`c`) at `t = 0`, and `Time` alone moves to 2 (`d`) at `t = 1`.
    fn columns(self) -> [usize; 2] {
        match self {
            SiblingIndex::Time => [0, 1],
            _ => [0, 0],
        }
    }
}

/// Control: NO mapping and NO shared element name, on BOTH subscript paths.
///
/// `target[State] = m[State, <sibling>]` over `m[Region, D2]`, with `Region`
/// and `State` declaring nothing and sharing no element name. The
/// dimension-named index is spelled identically in every row; only its
/// sibling differs, and the sibling picks the route ([`SiblingIndex`]). Both
/// routes end in `DimensionsContext::resolve_dimension_subscript`, so every
/// row reads `Region` at `State`'s ordinal -- `target[s1]` is `m[a, ..]` and
/// `target[s2]` is `m[b, ..]` -- with the numbers `origin/main` computes for
/// the same six models (`100`/`200` at `t = 0` on every row, measured with
/// the CLI built from `d04593e6`; GH #1044 holds the static and `idx` rows).
/// A route that refused the dynamic rows would turn a running model into a
/// refusal by the edit `1` -> `idx`.
///
/// The other arms of `resolve_dimension_subscript` -- name, the declared map,
/// a mapped parent, a declared map that fails to translate -- are
/// `every_cell_of_the_matrix` and the `no_mapping_*` controls above, all on
/// the static route; its two refusals are the next two tests. Not rowed
/// anywhere: the hoisted and captured twins of a reference with a dynamic
/// sibling.
#[test]
fn no_mapping_reads_by_ordinal_on_both_subscript_paths() {
    let cell = |row: usize, column: usize| 100.0 * (row + 1) as f64 + 10.0 * column as f64;
    for sibling in SiblingIndex::all() {
        let label = sibling.label("target[State]", &["s1", "s2"]);
        let results = sibling_model("target[State]", sibling)
            .named_dimension("State", &["s1", "s2"])
            .run_vm()
            .unwrap_or_else(|e| panic!("{label}: expected it to run: {e}"));
        let [col0, col1] = sibling.columns();
        assert_eq!(
            results["target[s1]"],
            vec![cell(0, col0), cell(0, col1)],
            "{label}: s1 reads Region's first element"
        );
        assert_eq!(
            results["target[s2]"],
            vec![cell(1, col0), cell(1, col1)],
            "{label}: s2 reads Region's second element"
        );
    }
}

/// Past the source's extent -- three `State` elements over two `Region` ones
/// -- the ordinal is out of range and both routes refuse, each with its own
/// code ([`SiblingIndex::refusal_code`]). `origin/main` refused the static
/// spellings too (`Index out of bounds`) but RAN the dynamic ones, the folded
/// ordinal reaching the runtime bounds check and writing NaN into
/// `target[s3]`; the refusal in its place is "Phase 6b semantic divergences"
/// item 11.
#[test]
fn an_ordinal_past_the_sources_extent_is_refused_on_both_subscript_paths() {
    for sibling in SiblingIndex::all() {
        let state = ["s1", "s2", "s3"];
        assert_eq!(
            sibling_model("target[State]", sibling)
                .named_dimension("State", &state)
                .error_diagnostics(),
            vec![("main.target".to_string(), sibling.refusal_code())],
            "{}: three State elements over two Region ones",
            sibling.label("target[State]", &state)
        );
    }
}

/// A subscript paired with its active dimension THROUGH a mapping never takes
/// the ordinal: `target[Foo] = m[State, <sibling>]` with `Foo maps_to State`
/// pairs `State` with `Foo` on both routes, and over `m[Region, D2]` -- `Region`
/// related to neither -- there is no correspondence to follow and no ordinal
/// to take, so both routes refuse with the route's code
/// ([`SiblingIndex::refusal_code`]), as `origin/main` does (`Invalid active
/// subscript 'f1'` static, `mismatched_dimensions` dynamic). The ordinal is
/// for a subscript that NAMES its active dimension, the rows above; a pairing
/// the mapping made has no positional meaning on an unrelated axis.
#[test]
fn a_candidate_paired_through_a_mapping_never_takes_the_ordinal() {
    for sibling in SiblingIndex::all() {
        let foo = ["f1", "f2"];
        let project = sibling_model("target[Foo]", sibling)
            .named_dimension("State", &["s1", "s2"])
            .named_dimension_with_mapping("Foo", &foo, "State");
        assert_eq!(
            project.error_diagnostics(),
            vec![("main.target".to_string(), sibling.refusal_code())],
            "{}, Foo maps_to State: paired through the mapping, no ordinal",
            sibling.label("target[Foo]", &foo)
        );
    }
}

/// `<target> = m[State, <sibling>]` over `m[Region, D2]` with `m[a,c] = 100,
/// m[a,d] = 110, m[b,c] = 200, m[b,d] = 210`: the hundreds identify the ROW
/// (`Region`) and the tens the COLUMN (the sibling's value). `Region` declares
/// nothing and shares no element name with any other dimension. The caller
/// declares the target's dimension (and `State`, when the target is not it).
fn sibling_model(target: &str, sibling: SiblingIndex) -> TestProject {
    let m = [
        ("a,c", "100"),
        ("a,d", "110"),
        ("b,c", "200"),
        ("b,d", "210"),
    ];
    TestProject::new("no_mapping_sibling")
        .with_sim_time(0.0, 1.0, 1.0)
        .named_dimension("Region", &["a", "b"])
        .named_dimension("D2", &["c", "d"])
        .array_with_ranges("m[Region,D2]", m.to_vec())
        .aux("idx", "1", None)
        .aux("k", "0", None)
        .array_aux(target, &format!("m[State, {}]", sibling.spelling()))
}

impl SiblingIndex {
    fn label(self, target: &str, target_elements: &[&str]) -> String {
        format!(
            "no mapping / {target} = m[State, {}] ({}, {} target elements)",
            self.spelling(),
            if self.is_static() {
                "static path"
            } else {
                "dynamic path"
            },
            target_elements.len()
        )
    }
}

// ===========================================================================
// The hoisted-argument column
// ===========================================================================
//
// A module-function call inside an apply-to-all body hoists each computed
// argument out of the body into a helper aux, one per element, and the
// compiler lowers that helper as the element's slice of the body
// (`variable::ElementScope`, GH #1035) -- the same `Context` the plain
// equation's element is lowered under, so there is ONE place a
// cross-dimension reference is resolved. The column below asserts that, for
// every row above, `target[State] = SMTH1(<expr>, 1)` reads what
// `target[State] = <expr>` reads, and is refused with the same code where the
// plain equation is refused. A parse-time replay of the compiler's rule is
// what must not be added: two resolvers of one spelling drift exactly where
// the rule is non-trivial. (With a constant input a smooth equals its input
// from the first step, so the reads are comparable.)
//
// A snapshot argument takes the other route out of the body: a snapshot-only
// apply-to-all body is captured ONCE, structurally, as an apply-to-all
// capture over the parent's dimensions whose body is the source subtree. The
// third twin, `target[State] = PREVIOUS(<expr> + 0, 0)`, pins that route to
// the same rule (`+ 0` keeps the argument from being a static slot the parse
// reads directly; the lag is invisible against a constant source).

/// What one cell does when run: the target's reads, or the distinct diagnostic
/// codes it was refused with. The variable a refusal lands on is deliberately
/// not part of the verdict: a hoisted argument's refusal is reported on the
/// parent at the argument's span, and `error_diagnostics` also carries the
/// assembly-level rows.
#[derive(Debug, Clone, PartialEq)]
enum Verdict {
    Reads(Vec<f64>),
    Refused(Vec<ErrorCode>),
}

fn verdict(project: &TestProject, target_elements: &[&str]) -> Verdict {
    match project.run_vm() {
        Ok(results) => Verdict::Reads(
            target_elements
                .iter()
                .map(|elem| {
                    *results[&format!("target[{}]", crate::canonicalize(elem))]
                        .last()
                        .expect("empty series")
                })
                .collect(),
        ),
        Err(_) => {
            let mut codes: Vec<ErrorCode> = Vec::new();
            for (_, code) in project.error_diagnostics() {
                if !codes.contains(&code) {
                    codes.push(code);
                }
            }
            codes.sort_by_key(|c| format!("{c:?}"));
            Verdict::Refused(codes)
        }
    }
}

impl Spelling {
    /// The right-hand side of the target's equation, for the spellings that
    /// are an equation (a stock's flow reference has no argument to hoist).
    fn rhs(self) -> Option<&'static str> {
        match self {
            Spelling::IteratedDim => Some("x[State]"),
            Spelling::SourceOwnDim => Some("x[Region]"),
            Spelling::BareInEquation => Some("x"),
            Spelling::StockFlow => None,
        }
    }
}

/// One cell of the column: the plain model, its hoisted twin (the target's
/// right-hand side wrapped in `SMTH1(.., 1)`), its captured twin (wrapped in
/// `PREVIOUS(.. + 0, 0)`), and the target's elements.
struct HoistedCell {
    label: String,
    plain: TestProject,
    hoisted: TestProject,
    captured: TestProject,
    target_elements: Vec<&'static str>,
}

/// The two routes out of an apply-to-all body, as the right-hand side each
/// twin spells `rhs` with.
fn hoisted_rhs(rhs: &str) -> String {
    format!("SMTH1({rhs}, 1)")
}

fn captured_rhs(rhs: &str) -> String {
    format!("PREVIOUS({rhs} + 0, 0)")
}

/// The matrix cell and its twins.
fn matrix_cell(kind: MappingKind, direction: Direction, spelling: Spelling) -> Option<HoistedCell> {
    let rhs = spelling.rhs()?;
    let twin = |equation: &str| {
        let mut twin = TestProject::new("mapped_reference_twin");
        twin.dimensions = dimensions(kind, direction);
        twin.array_with_ranges("x[Region]", kind.source().to_vec())
            .array_aux("target[State]", equation)
    };
    Some(HoistedCell {
        label: format!(
            "{} / {} / declared on {}",
            kind.label(),
            spelling.label(),
            match direction {
                Direction::OnIteratedDim => "State",
                Direction::OnSourceDim => "Region",
            }
        ),
        plain: model(kind, direction, spelling),
        hoisted: twin(&hoisted_rhs(rhs)),
        captured: twin(&captured_rhs(rhs)),
        target_elements: kind.target_elements().to_vec(),
    })
}

/// The no-mapping control cell and its twins.
fn no_mapping_cell(
    state_elements: &'static [&'static str],
    spelling: Spelling,
) -> Option<HoistedCell> {
    let rhs = spelling.rhs()?;
    let dims = || {
        vec![
            datamodel::Dimension::named(
                "Region".to_string(),
                vec!["Ruby".to_string(), "Rose".to_string(), "Reed".to_string()],
            ),
            datamodel::Dimension::named(
                "State".to_string(),
                state_elements.iter().map(|s| s.to_string()).collect(),
            ),
        ]
    };
    let source = vec![("Ruby", "10"), ("Rose", "20"), ("Reed", "30")];
    let build = |equation: &str| {
        let mut project = TestProject::new("no_mapping");
        project.dimensions = dims();
        project
            .array_with_ranges("x[Region]", source.clone())
            .array_aux("target[State]", equation)
    };
    Some(HoistedCell {
        label: format!(
            "no mapping, {} State elements / {}",
            state_elements.len(),
            spelling.label()
        ),
        plain: build(rhs),
        hoisted: build(&hoisted_rhs(rhs)),
        captured: build(&captured_rhs(rhs)),
        target_elements: state_elements.to_vec(),
    })
}

/// The two-axes shapes of the section below, hoisted and captured.
fn two_axes_cells() -> Vec<HoistedCell> {
    let mapped = |equation: &str| {
        two_mapped_axes_project("two_mapped_hoisted")
            .array_with_ranges("matrix[Ra,Rb]", diagonal_matrix_cells())
            .array_aux("target[State]", equation)
    };
    let iterated = |equation: &str| {
        TestProject::new("two_iterated_hoisted")
            .named_dimension_with_mappings(
                "State",
                &["s1", "s2", "s3"],
                &[("Ra", &[]), ("Rb", &[])],
            )
            .named_dimension("Ra", &["ra1", "ra2", "ra3"])
            .named_dimension("Rb", &["rb1", "rb2", "rb3"])
            .array_with_ranges("matrix[Ra,Rb]", diagonal_matrix_cells())
            .array_aux("target[State]", equation)
    };
    let same = |equation: &str| {
        TestProject::new("same_dim_twice_hoisted")
            .named_dimension("State", &["d1", "d2", "d3"])
            .array_with_ranges(
                "matrix[State,State]",
                vec![
                    ("d1,d1", "11"),
                    ("d1,d2", "12"),
                    ("d1,d3", "13"),
                    ("d2,d1", "21"),
                    ("d2,d2", "22"),
                    ("d2,d3", "23"),
                    ("d3,d1", "31"),
                    ("d3,d2", "32"),
                    ("d3,d3", "33"),
                ],
            )
            .array_aux("target[State]", equation)
    };
    let repeated = |equation: &str| {
        TestProject::new("square_owner_hoisted")
            .named_dimension("State", &["r1", "r2"])
            .array_with_ranges(
                "pop[State,State]",
                vec![
                    ("r1,r1", "11"),
                    ("r1,r2", "12"),
                    ("r2,r1", "21"),
                    ("r2,r2", "22"),
                ],
            )
            .array_aux("target[State,State]", equation)
    };
    let cell = |label: &str,
                build: &dyn Fn(&str) -> TestProject,
                rhs: &str,
                target_elements: &[&'static str]| HoistedCell {
        label: label.to_string(),
        plain: build(rhs),
        hoisted: build(&hoisted_rhs(rhs)),
        captured: build(&captured_rhs(rhs)),
        target_elements: target_elements.to_vec(),
    };
    vec![
        cell(
            "two axes mapped to one target dimension: target[State] = matrix[Ra,Rb]",
            &mapped,
            "matrix[Ra,Rb]",
            &["s1", "s2", "s3"],
        ),
        cell(
            "two iterated axes: target[State] = matrix[State,State]",
            &iterated,
            "matrix[State,State]",
            &["s1", "s2", "s3"],
        ),
        cell(
            "one dimension named twice: target[State] = matrix[State,State]",
            &same,
            "matrix[State,State]",
            &["d1", "d2", "d3"],
        ),
        // The compiler pairs a repeated target dimension's positions one to
        // one (`square[D,D]` under `[D,D]` reads the cell), for the helper as
        // for the plain equation.
        cell(
            "a repeated target dimension: target[State,State] = pop[State,State]",
            &repeated,
            "pop[State,State]",
            &["r1,r1", "r1,r2", "r2,r1", "r2,r2"],
        ),
    ]
}

/// The wildcard spelling under an apply-to-all body, `target[C] = vals[*]`,
/// reads the active element -- the compiler's rule for a wildcard over the
/// target's own dimension in scalar position -- and its hoisted and captured
/// twins read the same. Under a SCALAR parent the same argument is a whole
/// array in a scalar position, which is what
/// `db::implicit_diag_tests::failing_implicit_fixture` refuses.
#[test]
fn a_wildcard_argument_under_an_apply_to_all_body_reads_the_active_element() {
    let build = |equation: &str| {
        TestProject::new("wildcard_a2a")
            .with_sim_time(0.0, 2.0, 1.0)
            .named_dimension("C", &["c1", "c2", "c3"])
            .array_with_ranges("vals[C]", vec![("c1", "10"), ("c2", "20"), ("c3", "30")])
            .array_aux("target[C]", equation)
    };
    let elements = ["c1", "c2", "c3"];
    let plain = verdict(&build("vals[*]"), &elements);
    assert_eq!(plain, Verdict::Reads(vec![10.0, 20.0, 30.0]));
    assert_eq!(verdict(&build(&hoisted_rhs("vals[*]")), &elements), plain);
    assert_eq!(verdict(&build(&captured_rhs("vals[*]")), &elements), plain);
}

/// Every cell of the matrix, of the no-mapping controls and of the two-axes
/// section, hoisted into a module argument and captured under a snapshot,
/// reads what the plain equation reads, and is refused with the plain
/// equation's code where that is refused. No cell diverges: the compiler
/// lowers each helper as the element of the body it is (GH #1035).
#[test]
fn a_hoisted_argument_reads_what_the_plain_equation_reads() {
    let mut cells: Vec<HoistedCell> = Vec::new();
    for kind in MappingKind::all() {
        for direction in Direction::all() {
            for spelling in Spelling::all() {
                cells.extend(matrix_cell(kind, direction, spelling));
            }
        }
    }
    for state_elements in [&["Steel", "Slate", "Stone"][..], &["Steel", "Slate"][..]] {
        for spelling in Spelling::all() {
            cells.extend(no_mapping_cell(state_elements, spelling));
        }
    }
    cells.extend(two_axes_cells());

    let mut disagreements: Vec<String> = Vec::new();
    let mut reads = 0usize;
    let mut refusals = 0usize;
    for cell in &cells {
        let plain = verdict(&cell.plain, &cell.target_elements);
        match &plain {
            Verdict::Reads(_) => reads += 1,
            Verdict::Refused(_) => refusals += 1,
        }
        for (route, twin) in [("hoisted", &cell.hoisted), ("captured", &cell.captured)] {
            let got = verdict(twin, &cell.target_elements);
            if got != plain {
                disagreements.push(format!(
                    "{} / {route}: plain {plain:?}, {route} {got:?}",
                    cell.label
                ));
            }
        }
    }
    assert!(
        disagreements.is_empty(),
        "twins that do not read what the plain equation reads:\n{}",
        disagreements.join("\n")
    );
    // Both verdict kinds are exercised, so agreement on refusal codes is
    // asserted and not merely absent.
    assert!(
        reads > 0 && refusals > 0,
        "the column must hold both reading and refused cells: {reads} reads, {refusals} refusals"
    );
}

/// The two subscript-less spellings AGREE, and this is the single assertion
/// that says so out loud.
///
/// They reach the answer by different routes and always did -- a bare
/// reference in an EQUATION is rewritten into the iterated-dimension spelling
/// by `Context::lower_pass0` and resolves through the subscript path, while a
/// stock's flow reference goes straight to `get_implicit_subscript_off`
/// (module docs) -- so this is the assertion that keeps the two routes reading
/// the same element. The direction matters: an equation route reading the
/// target's ORDINAL returns a different array under a permuted element map,
/// and the third assertion says the row can tell.
///
/// `Permuted` is the kind to run it on: its two dimensions share no element
/// names, so both spellings reach the element map rather than stopping at name
/// identity, and an ordinal read would be visibly different.
#[test]
fn a_bare_equation_reference_and_a_flow_reference_agree() {
    let kind = MappingKind::Permuted;
    for direction in Direction::all() {
        let bare = run_cell(kind, direction, Spelling::BareInEquation)
            .expect("the bare equation reference compiles");
        let flow =
            run_cell(kind, direction, Spelling::StockFlow).expect("the flow reference compiles");
        assert_eq!(
            bare,
            kind.map_reads(),
            "the bare equation reference must follow the element map"
        );
        assert_eq!(bare, flow, "the two subscript-less spellings must agree");
        assert_ne!(
            bare,
            kind.positional_reads().unwrap(),
            "under a permuted element map the ordinal read is a different array, \
             so this row is not vacuous"
        );
    }
}

/// GH #996, on the EXECUTED path: an earlier dependency axis must not claim
/// BY MAPPING the active slot a later axis matches BY NAME.
///
/// This is the hazard shape as a whole model rather than a hand-built call
/// to `dimensions::match_axes_partial`. Reaching the allocator
/// (`compiler::dimensions::allocate_implicit_axes`) from a real
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

// ===========================================================================
// Two axes driven by ONE target dimension, and what LTM describes about them
// ===========================================================================
//
// The matrix above varies ONE source axis. A reference can also spell two axes
// that resolve from the SAME active target element -- `matrix[Region1, Region2]`
// under a `State`-iterating equation with both regions mapped to `State`, or its
// iterated twin `matrix[State, State]`, or the degenerate `matrix[D, D]`. Each
// index goes through the same resolution the matrix pins for a single axis, and
// against the same active element, so the read is that dimension's DIAGONAL: N
// reads over an NxN source, not N^2.
//
// LTM has to describe exactly those reads. It derives them twice -- the element
// GRAPH from `db::ltm::read_slice_row_parts`, the link-score NAMES from
// `ltm_augment::per_element_row_for_target` -- and the two disagreed: the second
// projects one target element through each axis and so always produced the
// diagonal, while the first enumerated each axis independently and crossed them.
// The cross rows became element edges the simulation never traverses and loop
// candidates built on them (9 loops over a 3x3 source where there are 3). The
// assertions below are on one fixture per shape so the VM oracle and the
// description are the same model, not two models that happen to agree.

/// `matrix[Ra,Rb]` values, 11..33, distinct per cell so the pair of source
/// elements a target element read is uniquely identified by the number.
fn diagonal_matrix_cells() -> Vec<(&'static str, &'static str)> {
    vec![
        ("ra1,rb1", "11"),
        ("ra1,rb2", "12"),
        ("ra1,rb3", "13"),
        ("ra2,rb1", "21"),
        ("ra2,rb2", "22"),
        ("ra2,rb3", "23"),
        ("ra3,rb1", "31"),
        ("ra3,rb2", "32"),
        ("ra3,rb3", "33"),
    ]
}

/// `State` element-mapped to BOTH `Ra` and `Rb`, by two DIFFERENT rotations, so
/// the diagonal `matrix[map1(s), map2(s)]` is off the matrix's own diagonal and
/// no cell can be reached by two different rules.
///
/// * `s1 -> ra2, rb3` (23)
/// * `s2 -> ra3, rb1` (31)
/// * `s3 -> ra1, rb2` (12)
fn two_mapped_axes_project(name: &str) -> TestProject {
    TestProject::new(name)
        .named_dimension_with_mappings(
            "State",
            &["s1", "s2", "s3"],
            &[
                ("Ra", &[("s1", "ra2"), ("s2", "ra3"), ("s3", "ra1")]),
                ("Rb", &[("s1", "rb3"), ("s2", "rb1"), ("s3", "rb2")]),
            ],
        )
        .named_dimension("Ra", &["ra1", "ra2", "ra3"])
        .named_dimension("Rb", &["rb1", "rb2", "rb3"])
}

/// Every element edge the model's LTM element graph carries, as
/// `"from[row] -> to[element]"` strings.
fn element_edge_pairs(project: &TestProject) -> Vec<String> {
    use crate::db::{SimlinDb, model_element_causal_edges, sync_from_datamodel};
    let datamodel = project.build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &datamodel);
    let edges = model_element_causal_edges(&db, sync.models["main"].source, sync.project).clone();
    let mut pairs: Vec<String> = edges
        .edges
        .iter()
        .flat_map(|(from, tos)| tos.iter().map(move |to| format!("{from} -> {to}")))
        .collect();
    pairs.sort();
    pairs
}

/// The edges out of `from_prefix`, so an assertion can be exhaustive about one
/// reference without restating the rest of the model's graph.
fn edges_from(project: &TestProject, from_prefix: &str) -> Vec<String> {
    element_edge_pairs(project)
        .into_iter()
        .filter(|p| p.starts_with(from_prefix))
        .collect()
}

/// Both indices of `matrix[Ra,Rb]` resolve against the one active `State`
/// element, so the read is the diagonal of the two mappings -- three reads over
/// a 3x3 source.
#[test]
fn two_axes_mapped_to_one_target_dimension_read_the_diagonal() {
    let results = two_mapped_axes_project("two_mapped_exec")
        .array_with_ranges("matrix[Ra,Rb]", diagonal_matrix_cells())
        .array_aux("target[State]", "matrix[Ra,Rb]")
        .run_vm()
        .expect("the two-mapped-axes shape must compile and run");
    for (cell, want) in [
        ("target[s1]", 23.0),
        ("target[s2]", 31.0),
        ("target[s3]", 12.0),
    ] {
        assert_eq!(
            *results[cell].last().expect("empty series"),
            want,
            "{cell}: each index must resolve through its own map against the \
             SAME active State element"
        );
    }
}

/// The element graph describes exactly those three reads.
///
/// Exhaustive over the reference's own edges rather than a spot check: the
/// defect was extra rows, and an assertion that only names the three real ones
/// passes just as well with six phantoms beside them.
#[test]
fn two_mapped_axes_emit_only_the_diagonal_element_edges() {
    let project = two_mapped_axes_project("two_mapped_edges")
        .array_with_ranges("matrix[Ra,Rb]", diagonal_matrix_cells())
        .array_aux("target[State]", "matrix[Ra,Rb]");
    assert_eq!(
        edges_from(&project, "matrix["),
        vec![
            "matrix[ra1,rb2] -> target[s3]".to_string(),
            "matrix[ra2,rb3] -> target[s1]".to_string(),
            "matrix[ra3,rb1] -> target[s2]".to_string(),
        ],
        "the cross rows (matrix[ra1,rb1] and the five others) are reads the \
         simulation never makes"
    );
}

/// The two `Iterated` spellings of the same shape. Both are older than the
/// mapped one and both had the same defect, reached through a different
/// classification: an all-`Iterated` subscript used to be `RefShape::Bare`
/// unconditionally, and `expand_same_element` -- which sees only the two
/// variables' dimension lists, never the reference -- cannot express "these two
/// axes share a coordinate". It unioned the candidates for `matrix[D,D]` (15
/// edges over a 3x3 source) and claimed the target position for the first axis
/// alone for `matrix[State,State]` (9 edges).
#[test]
fn two_iterated_axes_on_one_target_dimension_read_the_diagonal() {
    // Positional mappings, so the iterated spelling folds to an ordinal: s_i
    // reads ra_i and rb_i.
    let mapped = TestProject::new("two_iterated")
        .named_dimension_with_mappings("State", &["s1", "s2", "s3"], &[("Ra", &[]), ("Rb", &[])])
        .named_dimension("Ra", &["ra1", "ra2", "ra3"])
        .named_dimension("Rb", &["rb1", "rb2", "rb3"])
        .array_with_ranges("matrix[Ra,Rb]", diagonal_matrix_cells())
        .array_aux("target[State]", "matrix[State,State]");
    let results = mapped.run_vm().expect("must compile and run");
    for (cell, want) in [
        ("target[s1]", 11.0),
        ("target[s2]", 22.0),
        ("target[s3]", 33.0),
    ] {
        assert_eq!(*results[cell].last().expect("empty series"), want, "{cell}");
    }
    assert_eq!(
        edges_from(&mapped, "matrix["),
        vec![
            "matrix[ra1,rb1] -> target[s1]".to_string(),
            "matrix[ra2,rb2] -> target[s2]".to_string(),
            "matrix[ra3,rb3] -> target[s3]".to_string(),
        ],
        "matrix[State,State] reads the diagonal"
    );

    // The degenerate spelling: one dimension, named twice.
    let same = TestProject::new("same_dim_twice")
        .named_dimension("D", &["d1", "d2", "d3"])
        .array_with_ranges(
            "matrix[D,D]",
            vec![
                ("d1,d1", "11"),
                ("d1,d2", "12"),
                ("d1,d3", "13"),
                ("d2,d1", "21"),
                ("d2,d2", "22"),
                ("d2,d3", "23"),
                ("d3,d1", "31"),
                ("d3,d2", "32"),
                ("d3,d3", "33"),
            ],
        )
        .array_aux("target[D]", "matrix[D,D]");
    let results = same.run_vm().expect("must compile and run");
    for (cell, want) in [
        ("target[d1]", 11.0),
        ("target[d2]", 22.0),
        ("target[d3]", 33.0),
    ] {
        assert_eq!(*results[cell].last().expect("empty series"), want, "{cell}");
    }
    assert_eq!(
        edges_from(&same, "matrix["),
        vec![
            "matrix[d1,d1] -> target[d1]".to_string(),
            "matrix[d2,d2] -> target[d2]".to_string(),
            "matrix[d3,d3] -> target[d3]".to_string(),
        ],
        "matrix[D,D] reads the diagonal"
    );
}

/// A TARGET that repeats the dimension: `cube[D1,D1] = pop[D1,D1]` resolves
/// each of the reference's indices to its OWN active axis, so `cube[r1,r2]`
/// reads `pop[r1,r2]` -- the whole matrix, four reads -- and the element graph
/// names exactly those four.
///
/// Both sides pair axes positionally and one to one:
/// `compiler::subscript::normalize_subscripts3` allocates the active positions
/// across a reference's subscripts in order, and
/// `db::analysis::bare_axis_pairing` -- the matcher behind `expand_same_element`
/// -- pairs the two declared dimension lists the same way, so a repeated name is
/// two axes on both sides rather than one map key. A name-keyed pairing kept
/// only the LAST `D1` axis of the target and let both source axes claim it,
/// which minted the phantom `pop[r1,r1] -> cube[r2,r1]` while dropping the real
/// `pop[r1,r1] -> cube[r1,r2]`; the third instance of "a dimension name is not
/// an axis identity" here after GH #974 and GH #986.
///
/// The shape stays `Bare` (the lists reproduce the subscript's pairing), which
/// is what keeps `pop -> cube` on the arrayed A2A score -- asserted below
/// through the loop-carrying twin, since the per-element emitter declines a
/// repeated-dimension target outright and a `PerElement` retarget would trade
/// the score for a loud skip.
///
/// **Blast radius, measured.** Vensim REJECTS a repeated-dimension declaration
/// ("DimA appears more than once on LHS", `vensim-probes/repeated_dimension.mdl`
/// in Vensim DSS 2026-08-04), so no MDL-imported model reaches this shape and
/// it is confined to hand-authored XMILE/JSON/protobuf. The XMILE v1.0 spec does
/// exemplify the declaration, so the shape stays legitimate.
#[test]
fn a_repeated_target_dimension_reads_each_axis_on_the_executed_path() {
    let project = TestProject::new("square_owner_reads")
        .named_dimension("D1", &["r1", "r2"])
        .array_with_ranges(
            "pop[D1,D1]",
            vec![
                ("r1,r1", "11"),
                ("r1,r2", "12"),
                ("r2,r1", "21"),
                ("r2,r2", "22"),
            ],
        )
        .array_aux("cube[D1,D1]", "pop[D1,D1]");
    let results = project.run_vm().expect("must compile and run");
    // Each `D1` subscript reads its OWN active axis, so the copy is the whole
    // matrix -- the executed read, and the oracle for the edges below.
    for (cell, want) in [
        ("cube[r1,r1]", 11.0),
        ("cube[r1,r2]", 12.0),
        ("cube[r2,r1]", 21.0),
        ("cube[r2,r2]", 22.0),
    ] {
        assert_eq!(
            *results[cell].last().expect("empty series"),
            want,
            "{cell}: each D1 subscript reads its own active axis"
        );
    }

    // The element graph names exactly the four executed reads: no phantom
    // and nothing missing.
    assert_eq!(
        edges_from(&project, "pop["),
        vec![
            "pop[r1,r1] -> cube[r1,r1]".to_string(),
            "pop[r1,r2] -> cube[r1,r2]".to_string(),
            "pop[r2,r1] -> cube[r2,r1]".to_string(),
            "pop[r2,r2] -> cube[r2,r2]".to_string(),
        ],
        "each source cell feeds the target cell that reads it"
    );

    // And the edge stays on the arrayed `Bare` score (the per-element emitter
    // would decline a repeated-dimension target with a loud skip instead).
    let scored = square_owner_link_score_names();
    assert!(
        scored
            .iter()
            .any(|n| n.ends_with("link_score\u{205A}pop\u{2192}cube")),
        "the repeated-dimension read keeps its arrayed Bare score; got: {scored:?}"
    );
}

/// The link scores a LOOP-carrying twin of the square-owner shape emits.
///
/// Split out because a score only exists for an edge inside a feedback loop, so
/// the acyclic fixture above cannot ask the question. Same shape: `cube` and
/// `grow` both repeat `D1`, and the reference `pop[D1,D1]` inside `cube` is the
/// one whose `RefShape` the narrowing decides.
fn square_owner_link_score_names() -> Vec<String> {
    use crate::db::{SimlinDb, model_ltm_variables, sync_from_datamodel_incremental};
    let datamodel = TestProject::new("square_owner_scores")
        .named_dimension("D1", &["r1", "r2"])
        .array_stock("pop[D1,D1]", "10", &["grow"], &[], None)
        .array_flow("grow[D1,D1]", "cube[D1,D1] * 0.01", None)
        .array_aux("cube[D1,D1]", "pop[D1,D1]")
        .build_datamodel();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel, None);
    let mut names: Vec<String> =
        model_ltm_variables(&db, sync.models["main"].source_model, sync.project)
            .vars
            .iter()
            .map(|v| v.name.clone())
            .filter(|n| n.contains("link_score"))
            .collect();
    names.sort();
    names
}

/// A canonical element name containing a COMMA does not derail the mapped
/// projection.
///
/// The `PerElement` element-edge arm used to take the row derivation's
/// comma-JOINED slot string and re-split it on `,` to recover the per-axis
/// coordinates. A canonical element name can itself contain a comma -- a quoted
/// XMILE element `"a,b"` canonicalizes to `a,b` (measured: `canonicalize("a,b")`
/// is `a,b`, and a model declaring one compiles and simulates) -- so that
/// round-trip read one coordinate as two: the real edge was dropped and one to a
/// target element that does not exist was minted in its place. The arm now reads
/// `ReadSliceRowParts::slot_parts` directly and never serializes.
///
/// `region` deliberately puts the comma element FIRST, so a mis-split shifts
/// every following coordinate rather than only the last.
///
/// WHAT THIS CLOSES, AND WHAT IT DOES NOT. This fix covers the element-EDGE
/// surface. The link-SCORE surface still round-trips coordinate tuples through
/// comma-joined strings in several `db::ltm` emitters, and the defect is live
/// there (measured, pre-existing this branch): carrying the same `a,b` element
/// into an iterated-projection-feeder agg
/// (`x[state] = 1 + SUM(matrix[state,*] * frac[state])`) emits the agg->target
/// half as `$⁚ltm⁚link_score⁚$⁚ltm⁚agg⁚0[a]→x[a,b]` -- the agg's slot subscript
/// lost the `,b`, so the two halves of one agg name DIFFERENT variables and the
/// co-source row degrades to the delta-ratio fallback, while the comma-free
/// `s2` control carries the real partial. The arity guards in
/// `qualify_element_csv`/`target_elem_by_dim_for` are why the outcome is a
/// wrong/degraded score rather than a phantom edge. Sweeping the remaining
/// `split(',')` sites (link_scores.rs x8, loops.rs x1) onto structured parts is
/// its own change; until then the invariant to hold in NEW code is: never
/// serialize a coordinate tuple you will re-split.
#[test]
fn a_comma_bearing_element_name_survives_the_mapped_projection() {
    let project = TestProject::new("comma_elem")
        .named_dimension_with_element_mapping(
            "state",
            &["a,b", "s2"],
            "region",
            &[("a,b", "r2"), ("s2", "r1")],
        )
        .named_dimension("region", &["r1", "r2"])
        .array_with_ranges("x[region]", vec![("r1", "10"), ("r2", "20")])
        .array_aux("target[state]", "x[region]");

    // The executed read, so the edges below are checked against behaviour
    // rather than against the derivation that produces them.
    let results = project.run_vm().expect("must compile and run");
    assert_eq!(*results["target[a,b]"].last().expect("series"), 20.0);
    assert_eq!(*results["target[s2]"].last().expect("series"), 10.0);

    assert_eq!(
        edges_from(&project, "x["),
        vec![
            "x[r1] -> target[s2]".to_string(),
            "x[r2] -> target[a,b]".to_string(),
        ],
        "the comma element must stay ONE coordinate: splitting it drops \
         x[r2] -> target[a,b] and mints an edge to a target that does not exist"
    );
}

/// The two derivations agree, on a model with a LOOP through the shape.
///
/// The link-score NAMES already came out diagonal (they are projected from one
/// target element by `ltm_augment::per_element_row_for_target`), so a test that
/// only checked them would have been green throughout. The claim worth pinning
/// is that the element graph now names the same rows -- and the loop count is
/// how that becomes visible: an element loop is built out of element edges, so
/// six phantom edges through `matrix` minted six extra circuits, each carrying
/// a link score that does not exist.
#[test]
fn a_loop_through_two_mapped_axes_is_described_once_per_executed_read() {
    use crate::db::{SimlinDb, model_ltm_variables, sync_from_datamodel_incremental};

    // `lvl` breaks the algebraic loop: matrix -> target -> fb -> grow -> lvl ->
    // matrix. At t=0 `lvl` is 0, so `target` still reads the fixture's own
    // diagonal.
    let project = two_mapped_axes_project("two_mapped_loop")
        .array_with_ranges("base[Ra,Rb]", diagonal_matrix_cells())
        .array_stock("lvl[Ra,Rb]", "0", &["grow"], &[], None)
        .array_flow("grow[Ra,Rb]", "fb * 0.01", None)
        .array_aux("matrix[Ra,Rb]", "base[Ra,Rb] + lvl[Ra,Rb]")
        .array_aux("target[State]", "matrix[Ra,Rb]")
        .aux("fb", "SUM(target[*])", None);

    #[allow(deprecated)]
    let circuits = {
        use crate::db::{model_element_loop_circuits, sync_from_datamodel};
        let datamodel = project.build_datamodel();
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &datamodel);
        model_element_loop_circuits(&db, sync.models["main"].source, sync.project)
            .circuits
            .len()
    };
    assert_eq!(
        circuits, 3,
        "one element loop per executed read; crossing the two axes minted nine"
    );

    let datamodel = project.build_datamodel();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel, None);
    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);
    let mut scores: Vec<&str> = ltm
        .vars
        .iter()
        .map(|v| v.name.as_str())
        .filter(|n| n.contains("link_score\u{205A}matrix["))
        .collect();
    scores.sort_unstable();
    assert_eq!(
        scores,
        [
            "$\u{205A}ltm\u{205A}link_score\u{205A}matrix[ra1,rb2]\u{2192}target[s3]",
            "$\u{205A}ltm\u{205A}link_score\u{205A}matrix[ra2,rb3]\u{2192}target[s1]",
            "$\u{205A}ltm\u{205A}link_score\u{205A}matrix[ra3,rb1]\u{2192}target[s2]",
        ],
        "the scores name the same three rows the edges do"
    );

    // And the loops are scored rather than dropped: every element loop through
    // the shape has a score variable, which is what an edge with no matching
    // score would have cost.
    let loop_scores = ltm
        .vars
        .iter()
        .filter(|v| v.name.contains("\u{205A}loop_score\u{205A}"))
        .count();
    assert_eq!(loop_scores, 3, "every executed loop keeps a score");
}
