// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;

use crate::dimensions::{Dimension, DimensionsContext};

/// For each dimension of `dims`, the POSITION in `active_dims` whose subscript
/// supplies it, or `None` for a dimension no active axis supplies -- the
/// engine's SINGLE implicit-subscript axis allocation, the rule behind a BARE
/// arrayed reference inside an apply-to-all body.
///
/// Two properties matter to every caller and neither is re-derivable safely:
///
/// - the answer is POSITIONAL, so a target that repeats a dimension
///   (`target[D,D]`) is handled by index rather than by name. A map keyed by
///   dimension name collapses the two occurrences and answers with whichever was
///   inserted last;
/// - the allocation is ONE-TO-ONE: each active axis is consumed at most once
///   (`used`), so two dependency axes that could each match the same active axis
///   are given different ones. Searching per dependency axis independently lets
///   both claim the first match.
///
///   Which of the two gets it is decided by MATCH STRENGTH, not by declaration
///   order: the three passes are staged flat -- every name match across all
///   dependency axes, then every mapping match, then every size match -- so a
///   stronger match always outranks a weaker one no matter which axis is
///   declared first. Order only breaks ties WITHIN a pass. Running the passes
///   per dimension instead made declaration order decisive, which is GH #996
///   (see the FIRST PASS comment in the body), and both
///   `a_mapping_match_does_not_steal_a_later_name_match` and
///   `a_size_match_does_not_steal_a_later_mapping_match` pin the corrected rule.
///   This is the same flat staging the sibling `match_dimensions_with_mapping`
///   uses; the two functions state one precedence rule and must not drift.
///
///   Restaging is inert for the COMPILER path on the corpus: computing the old
///   allocation alongside the new one at every call and flagging disagreement
///   found zero across 7,617 calls / 53 distinct shapes (lib + integration), and
///   556 / 4 while compiling C-LEARN with LTM. Treat that as empirical, not as
///   proof -- the corpus is a WEAK instrument here. 18 of the 20 lib shapes are
///   exact identity name matches, and only one reaches the mapping branch at all,
///   with a single active axis where no reordering is possible. The shapes that
///   could distinguish the orders are barely exercised, so "no disagreement" is
///   mostly a statement about the corpus. Re-measuring on the compiler path
///   alone found the same thing more sharply: over the lib suite every shape
///   but two is `dims == active_dims` by name and the two exceptions are
///   single-axis, and on C-LEARN the mapping pass is never consulted at all --
///   278 calls spanning 4 shapes, ALL identity (`["scenario"]`, `["cop"]`,
///   `["scenario","layers"]`, `["hfc_type"]`, each against itself). To
///   reproduce the C-LEARN figure, print `dims`/`active_dims` at the top of
///   `compiler::context`'s `get_implicit_subscripts` and run
///   `cargo run --release -p simlin-engine --example ltm_fragment_failures`,
///   which compiles the model with LTM enabled. (The lib-suite call counts
///   and the two-caller invariant behind them are recorded on that same
///   function, with the condition they need to be reproducible.)
///
///   The hazard is nevertheless REACHABLE from a real model, which is the part
///   the corpus does not show:
///   `mapped_reference_semantics_tests::the_996_hazard_shape_compiles_and_reads_name_first`
///   is a stock over `[Line, Shift]` fed by a flow over `[Board Type, Line]`,
///   where `Board Type` maps to both `Line` and `Shift`. Under this flat
///   staging it compiles and reads the element map; under the per-dimension
///   staging it does not compile at all. An ordinary expression cannot reach
///   this function (see `compiler::context`'s `get_implicit_subscripts`), so a
///   stock's flow is the way in and a fixture built from an aux equation will
///   silently exercise nothing.
///
/// Both were live silent-wrong-row defects in the LTM per-element projection
/// (P2-1 / P2-2 of the whole-branch review) precisely because that projection
/// re-derived this rule instead of asking for it. `crate::ltm_augment` now calls
/// this, so its pins and the executed reads agree by construction rather than by
/// parallel implementation. Do not add a second copy of this decision.
///
/// The compiler wants the TOTAL answer and errors without it, so it calls
/// [`allocate_implicit_axes`]; the LTM projection wants the partial one, because
/// a SUBSCRIPTED reference spells some of its own axes and only needs the rest
/// resolved. One algorithm, two projections of its result.
pub(crate) fn allocate_implicit_axes_partial(
    dims: &[Dimension],
    active_dims: &[Dimension],
    dimensions_ctx: &DimensionsContext,
) -> Vec<Option<usize>> {
    let mut alloc: Vec<Option<usize>> = vec![None; dims.len()];

    // Track which active dimensions have been used.
    let mut used: Vec<bool> = vec![false; active_dims.len()];

    // The three passes are STAGED FLAT: every exact name match across all
    // dependency dimensions, then every mapping match, then every size match.
    // That is what makes the documented precedence name > mapping > size hold
    // for the WHOLE allocation rather than only within one dimension's turn,
    // and it is the structure the sibling `match_dimensions_with_mapping` in
    // this file already used.
    //
    // Run per dimension instead, an earlier axis consumes -- through a weaker
    // match -- the active axis a later one matches more strongly, and since
    // `used` is one-to-one the later axis is then left unresolved. That is
    // GH #996. On C-LEARN it hit TWO production shapes, not one:
    // `aggregated_definition[cop, aggregated_regions]` read under an
    // `[aggregated_regions]` target and `[cop, semi_agg]` under `[semi_agg]` --
    // both because the target's own dimension declares an element map ONTO
    // `cop`, so `cop` (declared first) took the slot by MAPPING before the
    // name-matching axis was considered. Each allocated `[Some(0), None]`, the
    // LTM pin table dropped the dep, and 135 link scores were declined.
    //
    // The FIRST PASS's original comment already stated this rule for the SIZE
    // fallback ("prevents size-based fallback from grabbing the wrong dimension
    // when the correct name match exists later in the list"); the mapping pass
    // was added afterwards, between name and size, and reintroduced for mappings
    // the hazard that comment was written against.
    for (dim_idx, dim) in dims.iter().enumerate() {
        let name_match_idx = active_dims.iter().enumerate().find_map(|(i, candidate)| {
            if !used[i] && candidate.name() == dim.name() {
                Some(i)
            } else {
                None
            }
        });

        if let Some(idx) = name_match_idx {
            alloc[dim_idx] = Some(idx);
            used[idx] = true;
        }
    }

    for (dim_idx, dim) in dims.iter().enumerate() {
        if alloc[dim_idx].is_some() {
            continue;
        }

        // SECOND PASS: Check for dimension mapping matches in both directions.
        // Forward: dim has any mapping to an active dimension
        // Reverse: active_dim has any mapping to dim
        let mapping_match_idx = {
            // Forward: dim has mapping to active dim (or active is subdim of mapping target)
            let mut found = active_dims.iter().enumerate().find_map(|(i, candidate)| {
                if used[i] {
                    return None;
                }
                let candidate_name = candidate.canonical_name();
                if dimensions_ctx.has_mapping_to(dim.canonical_name(), candidate_name) {
                    return Some(i);
                }
                if dimensions_ctx.has_mapping_to_parent_of(dim.canonical_name(), candidate_name) {
                    return Some(i);
                }
                None
            });
            // Reverse: active_dim has mapping to dim
            if found.is_none() {
                found = active_dims.iter().enumerate().find_map(|(i, candidate)| {
                    if used[i] {
                        return None;
                    }
                    if dimensions_ctx
                        .has_mapping_to(candidate.canonical_name(), dim.canonical_name())
                    {
                        return Some(i);
                    }
                    None
                });
            }
            found
        };

        if let Some(idx) = mapping_match_idx {
            alloc[dim_idx] = Some(idx);
            used[idx] = true;
        }
    }

    for (dim_idx, dim) in dims.iter().enumerate() {
        if alloc[dim_idx].is_some() {
            continue;
        }

        // THIRD PASS: Only if no name or mapping match exists, try size-based matching
        // for indexed dimensions. Find the first unused indexed dimension with
        // the same size.
        //
        // IMPORTANT: Size-based fallback only applies when BOTH dimensions are
        // indexed. Named dimensions must match by name (or subdimension relationship)
        // because their elements have semantic meaning. For example, Cities=[Boston,
        // Seattle] and Products=[Widgets,Gadgets] shouldn't match just because both
        // have size 2 - that would be semantically incorrect.
        //
        // NOTE: The two-pass (name -> size) matching logic is shared with the VM via
        // dimensions::match_dimensions_two_pass. This compiler version adds a mapping
        // pass between name and size matching.
        let size_match_idx = if let Dimension::Indexed(_, dim_size) = dim {
            active_dims.iter().enumerate().find_map(|(i, candidate)| {
                if !used[i]
                    && let Dimension::Indexed(_, candidate_size) = candidate
                    && dim_size == candidate_size
                {
                    return Some(i);
                }
                None
            })
        } else {
            None
        };

        if let Some(idx) = size_match_idx {
            alloc[dim_idx] = Some(idx);
            used[idx] = true;
        }
    }

    alloc
}

/// [`allocate_implicit_axes_partial`] with every dimension resolved, or `None`.
///
/// `None` is the compiler's `MismatchedDimensions`: no complete allocation
/// exists. That subsumes the old explicit `dims.len() > active_dims.len()` bail
/// by pigeonhole -- the allocation is one-to-one, so a longer `dims` always
/// leaves an axis unresolved -- and it subsumes the old equal-arity
/// [`find_dimension_reordering`] fast path, which could only fire on a
/// duplicate-free permutation, where the partial allocation's first pass finds
/// the same unique partner for every dimension.
pub(crate) fn allocate_implicit_axes(
    dims: &[Dimension],
    active_dims: &[Dimension],
    dimensions_ctx: &DimensionsContext,
) -> Option<Vec<usize>> {
    allocate_implicit_axes_partial(dims, active_dims, dimensions_ctx)
        .into_iter()
        .collect()
}

/// Three-pass dimension matching: name -> mapping -> size.
///
/// For each source dimension, finds the target dimension by trying in order:
/// 1. Exact name match
/// 2. Mapping match (source maps_to target, or target maps_to source, or both map to same dim)
/// 3. Size-based match (indexed dimensions only)
///
/// This handles cross-dimension array assignments like `a[DimA] = b[DimB]` when DimA maps_to DimB.
pub(super) fn match_dimensions_with_mapping(
    source_dims: &[Dimension],
    target_dims: &[Dimension],
    initially_used: &[bool],
    dims_ctx: &DimensionsContext,
) -> Vec<Option<usize>> {
    let mut target_used = initially_used.to_vec();
    let mut source_to_target: Vec<Option<usize>> = vec![None; source_dims.len()];

    // PASS 1: Exact name matches (highest priority)
    for (source_idx, source_dim) in source_dims.iter().enumerate() {
        for (target_idx, target) in target_dims.iter().enumerate() {
            if !target_used[target_idx] && target.name() == source_dim.name() {
                target_used[target_idx] = true;
                source_to_target[source_idx] = Some(target_idx);
                break;
            }
        }
    }

    // PASS 2: Dimension mapping matches (source has mapping to target or vice versa)
    for (source_idx, source_dim) in source_dims.iter().enumerate() {
        if source_to_target[source_idx].is_some() {
            continue;
        }

        for (target_idx, target) in target_dims.iter().enumerate() {
            if target_used[target_idx] {
                continue;
            }

            // source has mapping to target
            if dims_ctx.has_mapping_to(source_dim.canonical_name(), target.canonical_name()) {
                target_used[target_idx] = true;
                source_to_target[source_idx] = Some(target_idx);
                break;
            }

            // target has mapping to source
            if dims_ctx.has_mapping_to(target.canonical_name(), source_dim.canonical_name()) {
                target_used[target_idx] = true;
                source_to_target[source_idx] = Some(target_idx);
                break;
            }

            // source and target both map to at least one common dimension.
            let source_targets = dims_ctx.get_all_mapping_targets(source_dim.canonical_name());
            let target_targets = dims_ctx.get_all_mapping_targets(target.canonical_name());
            if source_targets
                .iter()
                .any(|source_target| target_targets.contains(source_target))
            {
                target_used[target_idx] = true;
                source_to_target[source_idx] = Some(target_idx);
                break;
            }
        }
    }

    // PASS 3: Size-based matches for remaining sources (indexed dimensions only)
    for (source_idx, source_dim) in source_dims.iter().enumerate() {
        if source_to_target[source_idx].is_some() {
            continue;
        }

        if let Dimension::Indexed(_, source_size) = source_dim {
            for (target_idx, target) in target_dims.iter().enumerate() {
                if !target_used[target_idx]
                    && let Dimension::Indexed(_, target_size) = target
                    && source_size == target_size
                {
                    target_used[target_idx] = true;
                    source_to_target[source_idx] = Some(target_idx);
                    break;
                }
            }
        }
    }

    source_to_target
}

/// Determines if dimensions can be reordered to match target dimensions and returns the reordering
///
/// Given source dimensions and target dimensions, determines if the source can be
/// reordered to match the target. If so, returns a vector of indices indicating
/// how to reorder the source dimensions (suitable for use as @N subscripts).
///
/// # Arguments
/// * `source_dims` - The dimension names of the source array
/// * `target_dims` - The dimension names of the target array
///
/// # Returns
/// * `Some(reordering)` - A vector where reordering[i] is the source dimension index
///   that should go in position i of the target
/// * `None` - If the dimensions cannot be reordered to match (different sets of dimensions)
///
/// # Examples
/// ```
/// // source: [A, B, C], target: [B, C, A]
/// // returns: Some([1, 2, 0]) meaning [@2, @3, @1] in XMILE notation (1-indexed)
/// ```
pub fn find_dimension_reordering(
    source_dims: &[String],
    target_dims: &[String],
) -> Option<Vec<usize>> {
    if source_dims.len() != target_dims.len() {
        return None;
    }

    // Build a map of dimension name to index in source
    let mut source_map: HashMap<&str, usize> = HashMap::new();
    for (i, dim) in source_dims.iter().enumerate() {
        source_map.insert(dim.as_str(), i);
    }

    // Check if all target dimensions exist in source and build reordering
    let mut reordering = Vec::with_capacity(target_dims.len());
    for target_dim in target_dims {
        match source_map.get(target_dim.as_str()) {
            Some(&source_idx) => reordering.push(source_idx),
            None => return None, // Target dimension not found in source
        }
    }

    // Verify we've used all source dimensions (no duplicates in target)
    let mut used = vec![false; source_dims.len()];
    for &idx in &reordering {
        if used[idx] {
            return None; // Duplicate dimension in target
        }
        used[idx] = true;
    }

    Some(reordering)
}

// simplified/lowered from ast::UnaryOp version
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(PartialEq, Eq, Hash, Copy, Clone)]
pub enum UnaryOp {
    Not,
    Transpose,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::ArrayView;
    use crate::common::CanonicalDimensionName;

    fn named_dim(name: &str, elements: &[&str]) -> Dimension {
        use crate::dimensions::NamedDimension;
        let canonical_elements: Vec<crate::common::CanonicalElementName> = elements
            .iter()
            .map(|e| crate::common::CanonicalElementName::from_raw(e))
            .collect();
        let indexed_elements: crate::common::IdentMap<crate::common::CanonicalElementName, usize> =
            canonical_elements
                .iter()
                .enumerate()
                .map(|(i, elem)| (elem.clone(), i + 1))
                .collect();
        Dimension::Named(
            CanonicalDimensionName::from_raw(name),
            NamedDimension {
                indexed_elements,
                elements: canonical_elements,
                maps_to: None,
                mappings: vec![],
            },
        )
    }

    /// GH #996: a NAME match must win globally, not lose to an earlier
    /// dimension's MAPPING match just because that dimension is processed first.
    ///
    /// The hand-built input is exactly what production supplies. Instrumenting
    /// `ltm_augment_post_transform::dep_element_pins` and running C-LEARN
    /// (`test/xmutil_test_models/C-LEARN v77 for Vensim.mdl`) dumped, for the
    /// `rs_ff_co2_ff_aggregated[developed_countries]` link score:
    ///
    /// ```text
    /// dep=aggregated_definition
    /// dep_dims=["cop","aggregated_regions"]  target_dims=["aggregated_regions"]
    /// target_elems=["developed_countries"]
    /// elems=[None, None]  axes=[]  complete=false
    /// ```
    ///
    /// `Aggregated Regions` declares an explicit element map onto `COP`, so
    /// `cop` -- processed FIRST -- matched the single active slot through the
    /// mapping pass's reverse branch (`has_mapping_to(aggregated_regions, cop)`),
    /// and `aggregated_regions`, which matches that slot BY NAME, found it taken.
    /// Both axes resolved to `None`, `dep_element_pins` dropped the dep, its
    /// dimension-name subscript survived into a scalar fragment, and 135 C-LEARN
    /// link scores were declined.
    ///
    /// The first pass's own comment already states this rule for the SIZE
    /// fallback ("prevents size-based fallback from grabbing the wrong dimension
    /// when the correct name match exists later in the list"); the mapping pass
    /// was added afterwards and reintroduced the hazard it was written against.
    #[test]
    fn a_mapping_match_does_not_steal_a_later_name_match() {
        use crate::datamodel;

        let cop = datamodel::Dimension::named(
            "COP".to_string(),
            vec!["c1".to_string(), "c2".to_string()],
        );
        let mut agg = datamodel::Dimension::named(
            "Aggregated Regions".to_string(),
            vec!["r1".to_string(), "r2".to_string()],
        );
        // Many-to-one, exactly C-LEARN's shape: the map is what makes the
        // reverse mapping branch fire for `cop`.
        agg.mappings = vec![datamodel::DimensionMapping {
            target: "COP".to_string(),
            element_map: vec![
                ("r1".to_string(), "c1".to_string()),
                ("r2".to_string(), "c2".to_string()),
            ],
        }];
        let ctx = crate::dimensions::DimensionsContext::from(&[cop, agg]);
        let cop_d = ctx
            .get(&CanonicalDimensionName::from_raw("COP"))
            .expect("cop")
            .clone();
        let agg_d = ctx
            .get(&CanonicalDimensionName::from_raw("Aggregated Regions"))
            .expect("agg")
            .clone();

        // The dep is declared [COP, Aggregated Regions]; the target iterates
        // Aggregated Regions alone. Only axis 1 corresponds, and it does so BY
        // NAME -- axis 0 must not consume the slot through its mapping.
        assert_eq!(
            allocate_implicit_axes_partial(
                &[cop_d.clone(), agg_d.clone()],
                std::slice::from_ref(&agg_d),
                &ctx
            ),
            vec![None, Some(0)],
            "the name-matching axis must get the slot; a mapping match on an \
             earlier axis must not consume it first"
        );

        // Control: with the name-matching axis FIRST the old order already
        // worked, so this pins that the fix did not simply invert a preference.
        assert_eq!(
            allocate_implicit_axes_partial(
                &[agg_d.clone(), cop_d],
                std::slice::from_ref(&agg_d),
                &ctx
            ),
            vec![Some(0), None],
            "declaration order must not change which axis wins"
        );
    }

    /// The same precedence, one rung down: a SIZE match on an earlier axis must
    /// not consume the slot a later axis matches by MAPPING.
    ///
    /// This is why all three passes are staged flat rather than two. Hoisting
    /// only the name pass fixes `name > {mapping, size}` and leaves
    /// `mapping > size` per-dimension, so this shape still mis-allocated:
    /// an indexed dep axis grabs the sole indexed active axis by SIZE before the
    /// axis that maps onto it is ever considered. Not observed in any model --
    /// it takes two indexed dimensions of equal size plus a mapping -- but the
    /// rule the function documents is `name > mapping > size`, and a rule that
    /// holds for two of its three rungs is one a reader cannot rely on.
    #[test]
    fn a_size_match_does_not_steal_a_later_mapping_match() {
        use crate::datamodel;

        let ia = datamodel::Dimension::indexed("IA".to_string(), 2);
        let ib = datamodel::Dimension::indexed("IB".to_string(), 2);
        let mut y =
            datamodel::Dimension::named("Y".to_string(), vec!["y1".to_string(), "y2".to_string()]);
        y.set_maps_to("IA".to_string());
        let ctx = crate::dimensions::DimensionsContext::from(&[ia, ib, y]);
        let get = |n: &str| {
            ctx.get(&CanonicalDimensionName::from_raw(n))
                .unwrap_or_else(|| panic!("{n}"))
                .clone()
        };
        let (ia_d, ib_d, y_d) = (get("IA"), get("IB"), get("Y"));

        assert_eq!(
            allocate_implicit_axes_partial(&[ib_d, y_d], std::slice::from_ref(&ia_d), &ctx),
            vec![None, Some(0)],
            "the mapping-matching axis must get the slot; a size match on an \
             earlier axis must not consume it first"
        );
    }

    #[test]
    fn test_find_dimension_reordering() {
        // Test identical dimensions
        let source = vec!["A".to_string(), "B".to_string(), "C".to_string()];
        let target = vec!["A".to_string(), "B".to_string(), "C".to_string()];
        assert_eq!(
            find_dimension_reordering(&source, &target),
            Some(vec![0, 1, 2])
        );

        // Test simple transpose (2D)
        let source = vec!["Row".to_string(), "Col".to_string()];
        let target = vec!["Col".to_string(), "Row".to_string()];
        assert_eq!(
            find_dimension_reordering(&source, &target),
            Some(vec![1, 0])
        );

        // Test 3D reordering: [A, B, C] -> [B, C, A]
        let source = vec!["A".to_string(), "B".to_string(), "C".to_string()];
        let target = vec!["B".to_string(), "C".to_string(), "A".to_string()];
        assert_eq!(
            find_dimension_reordering(&source, &target),
            Some(vec![1, 2, 0])
        );

        // Test 3D reordering: [A, B, C] -> [C, A, B]
        let source = vec!["A".to_string(), "B".to_string(), "C".to_string()];
        let target = vec!["C".to_string(), "A".to_string(), "B".to_string()];
        assert_eq!(
            find_dimension_reordering(&source, &target),
            Some(vec![2, 0, 1])
        );

        // Test different dimensions - should return None
        let source = vec!["A".to_string(), "B".to_string()];
        let target = vec!["C".to_string(), "D".to_string()];
        assert_eq!(find_dimension_reordering(&source, &target), None);

        // Test missing dimension - should return None
        let source = vec!["A".to_string(), "B".to_string()];
        let target = vec!["A".to_string(), "C".to_string()];
        assert_eq!(find_dimension_reordering(&source, &target), None);

        // Test different lengths - should return None
        let source = vec!["A".to_string(), "B".to_string()];
        let target = vec!["A".to_string(), "B".to_string(), "C".to_string()];
        assert_eq!(find_dimension_reordering(&source, &target), None);

        // Test duplicate dimensions in target - should return None
        let source = vec!["A".to_string(), "B".to_string()];
        let target = vec!["A".to_string(), "A".to_string()];
        assert_eq!(find_dimension_reordering(&source, &target), None);

        // Test single dimension
        let source = vec!["X".to_string()];
        let target = vec!["X".to_string()];
        assert_eq!(find_dimension_reordering(&source, &target), Some(vec![0]));

        // Test empty dimensions
        let source: Vec<String> = vec![];
        let target: Vec<String> = vec![];
        assert_eq!(find_dimension_reordering(&source, &target), Some(vec![]));
    }

    #[test]
    fn test_array_view_contiguous() {
        // Test creating a contiguous 2D array view
        let view = ArrayView::contiguous(vec![3, 4]);

        assert_eq!(view.dims, vec![3, 4]);
        assert_eq!(view.strides, vec![4, 1]); // Row-major order
        assert_eq!(view.offset, 0);
        assert_eq!(view.size(), 12);
        assert!(view.is_contiguous());
    }

    #[test]
    fn test_array_view_contiguous_1d() {
        // Test creating a contiguous 1D array view
        let view = ArrayView::contiguous(vec![5]);

        assert_eq!(view.dims, vec![5]);
        assert_eq!(view.strides, vec![1]);
        assert_eq!(view.offset, 0);
        assert_eq!(view.size(), 5);
        assert!(view.is_contiguous());
    }

    #[test]
    fn test_array_view_contiguous_3d() {
        // Test creating a contiguous 3D array view
        let view = ArrayView::contiguous(vec![2, 3, 4]);

        assert_eq!(view.dims, vec![2, 3, 4]);
        assert_eq!(view.strides, vec![12, 4, 1]); // Row-major: 3*4, 4, 1
        assert_eq!(view.offset, 0);
        assert_eq!(view.size(), 24);
        assert!(view.is_contiguous());
    }

    #[test]
    fn test_array_view_apply_range_first_dim() {
        // Test applying a range to the first dimension
        let view = ArrayView::contiguous(vec![5, 3]);
        let sliced = view.apply_range_subscript(0, 2, 5).unwrap();

        assert_eq!(sliced.dims, vec![3, 3]); // [2:5] gives 3 elements
        assert_eq!(sliced.strides, vec![3, 1]); // Same strides
        assert_eq!(sliced.offset, 6); // Skip first 2 rows (2 * 3 = 6)
        assert_eq!(sliced.size(), 9);
        assert!(!sliced.is_contiguous()); // No longer contiguous due to offset
    }

    #[test]
    fn test_array_view_apply_range_second_dim() {
        // Test applying a range to the second dimension
        let view = ArrayView::contiguous(vec![3, 5]);
        let sliced = view.apply_range_subscript(1, 1, 3).unwrap();

        assert_eq!(sliced.dims, vec![3, 2]); // [1:3] gives 2 elements
        assert_eq!(sliced.strides, vec![5, 1]); // Row stride unchanged
        assert_eq!(sliced.offset, 1); // Skip first column
        assert_eq!(sliced.size(), 6);
        assert!(!sliced.is_contiguous());
    }

    #[test]
    fn test_array_view_apply_range_1d() {
        // Test applying a range to a 1D array (like source[3:5])
        let view = ArrayView::contiguous(vec![5]);
        let sliced = view.apply_range_subscript(0, 2, 5).unwrap(); // 0-based: [2:5)

        assert_eq!(sliced.dims, vec![3]); // Elements at indices 2, 3, 4
        assert_eq!(sliced.strides, vec![1]);
        assert_eq!(sliced.offset, 2);
        assert_eq!(sliced.size(), 3);
        assert!(!sliced.is_contiguous()); // Has non-zero offset
    }

    #[test]
    fn test_array_view_range_bounds_checking() {
        let view = ArrayView::contiguous(vec![5, 3]);

        // Test out of bounds dimension index
        assert!(view.apply_range_subscript(2, 0, 1).is_err());

        // Test invalid range (start >= end)
        assert!(view.apply_range_subscript(0, 3, 3).is_err());
        assert!(view.apply_range_subscript(0, 4, 2).is_err());

        // Test range exceeding dimension size
        assert!(view.apply_range_subscript(0, 0, 6).is_err());
        assert!(view.apply_range_subscript(0, 4, 6).is_err());
    }

    #[test]
    fn test_array_view_empty_array() {
        // Test edge case of empty array
        let view = ArrayView::contiguous(vec![]);

        assert_eq!(view.dims, Vec::<usize>::new());
        assert_eq!(view.strides, Vec::<isize>::new());
        assert_eq!(view.offset, 0);
        assert_eq!(view.size(), 1); // Empty product is 1
        assert!(view.is_contiguous());
    }

    #[test]
    fn test_array_view_is_contiguous() {
        // Test various cases for is_contiguous

        // Contiguous: fresh array
        let view1 = ArrayView::contiguous(vec![3, 4]);
        assert!(view1.is_contiguous());

        // Not contiguous: has offset
        let view2 = ArrayView {
            dims: vec![3, 4],
            strides: vec![4, 1],
            offset: 5,
            sparse: Vec::new(),
            dim_names: vec![String::new(), String::new()],
        };
        assert!(!view2.is_contiguous());

        // Not contiguous: wrong strides for row-major
        let view3 = ArrayView {
            dims: vec![3, 4],
            strides: vec![1, 3], // Column-major strides
            offset: 0,
            sparse: Vec::new(),
            dim_names: vec![String::new(), String::new()],
        };
        assert!(!view3.is_contiguous());

        // Contiguous: manually constructed but correct
        let view4 = ArrayView {
            dims: vec![2, 3, 4],
            strides: vec![12, 4, 1],
            offset: 0,
            sparse: Vec::new(),
            dim_names: vec![String::new(), String::new(), String::new()],
        };
        assert!(view4.is_contiguous());
    }

    #[test]
    fn test_dimension_metadata_population() {
        use crate::datamodel::{
            self, Aux as DatamodelAux, Model as DatamodelModel, SimMethod, SimSpecs,
            Variable as DatamodelVariable, Visibility,
        };
        use crate::test_common::TestProject;

        // Create a datamodel project with a named dimension
        let datamodel_project = datamodel::Project {
            name: "test".to_string(),
            sim_specs: SimSpecs {
                start: 0.0,
                stop: 10.0,
                dt: datamodel::Dt::Dt(1.0),
                save_step: None,
                sim_method: SimMethod::Euler,
                time_units: Some("time".to_string()),
            },
            dimensions: vec![datamodel::Dimension::named(
                "letters".to_string(),
                vec![
                    "a".to_string(),
                    "b".to_string(),
                    "c".to_string(),
                    "d".to_string(),
                    "e".to_string(),
                ],
            )],
            units: vec![],
            models: vec![DatamodelModel {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![DatamodelVariable::Aux(DatamodelAux {
                    ident: "x".to_string(),
                    equation: datamodel::Equation::Scalar("1".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat {
                        visibility: Visibility::Public,
                        ..datamodel::Compat::default()
                    },
                })],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
                macro_spec: None,
            }],
            source: None,
            ai_information: None,
        };

        // Compile through the production path and read the root module's
        // bytecode context: the table the VM resolves every `DimId` against.
        let compiled = TestProject::from_datamodel(datamodel_project)
            .compile_incremental()
            .expect("compilation should succeed");
        let context = &compiled.modules[&compiled.root].context;

        // Verify dimension metadata is populated

        // Should have one dimension: "letters" with 5 elements
        assert!(
            !context.dimensions.is_empty(),
            "Dimensions should be populated"
        );
        assert!(!context.names.is_empty(), "Names should be populated");

        // Find the "letters" dimension
        let letters_dim = context.dimensions.iter().find(|dim| {
            context
                .names
                .get(dim.name_id as usize)
                .is_some_and(|n| n == "letters")
        });

        assert!(
            letters_dim.is_some(),
            "Should have a 'letters' dimension. Names: {:?}, Dimensions: {:?}",
            context.names,
            context.dimensions
        );

        let letters_dim = letters_dim.unwrap();
        assert_eq!(
            letters_dim.size, 5,
            "letters dimension should have 5 elements"
        );
        assert!(
            !letters_dim.is_indexed,
            "letters should be a named dimension, not indexed"
        );
        assert_eq!(
            letters_dim.element_name_ids.len(),
            5,
            "Should have 5 element name IDs"
        );

        // Verify element names are interned
        let element_names: Vec<&str> = letters_dim
            .element_name_ids
            .iter()
            .filter_map(|&id| context.names.get(id as usize).map(|s| s.as_str()))
            .collect();
        assert_eq!(element_names.len(), 5);
        // Element names should be canonicalized (lowercase)
        assert!(element_names.contains(&"a"));
        assert!(element_names.contains(&"b"));
        assert!(element_names.contains(&"c"));
        assert!(element_names.contains(&"d"));
        assert!(element_names.contains(&"e"));
    }

    #[test]
    fn test_indexed_dimension_metadata() {
        use crate::datamodel::{
            self, Aux as DatamodelAux, Model as DatamodelModel, SimMethod, SimSpecs,
            Variable as DatamodelVariable, Visibility,
        };
        use crate::test_common::TestProject;

        // Create a datamodel project with an indexed dimension
        let datamodel_project = datamodel::Project {
            name: "test".to_string(),
            sim_specs: SimSpecs {
                start: 0.0,
                stop: 10.0,
                dt: datamodel::Dt::Dt(1.0),
                save_step: None,
                sim_method: SimMethod::Euler,
                time_units: Some("time".to_string()),
            },
            dimensions: vec![datamodel::Dimension::indexed("Size".to_string(), 10)],
            units: vec![],
            models: vec![DatamodelModel {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![DatamodelVariable::Aux(DatamodelAux {
                    ident: "x".to_string(),
                    equation: datamodel::Equation::Scalar("1".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat {
                        visibility: Visibility::Public,
                        ..datamodel::Compat::default()
                    },
                })],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
                macro_spec: None,
            }],
            source: None,
            ai_information: None,
        };

        let compiled = TestProject::from_datamodel(datamodel_project)
            .compile_incremental()
            .expect("compilation should succeed");
        let context = &compiled.modules[&compiled.root].context;

        // Find the "size" dimension (name is canonicalized)
        let size_dim = context.dimensions.iter().find(|dim| {
            context
                .names
                .get(dim.name_id as usize)
                .is_some_and(|n| n == "size")
        });

        assert!(size_dim.is_some(), "Should have a 'size' dimension");
        let size_dim = size_dim.unwrap();
        assert_eq!(size_dim.size, 10, "Size dimension should have 10 elements");
        assert!(size_dim.is_indexed, "Size should be an indexed dimension");
        assert!(
            size_dim.element_name_ids.is_empty(),
            "Indexed dimensions should not have element names"
        );
    }

    #[test]
    fn test_stock_with_nonexistent_flow() {
        // Regression test for crash when a stock references a flow that doesn't exist.
        // This should return a proper error, not panic.
        use crate::test_common::TestProject;

        let project = TestProject::new("stock_missing_flow").stock(
            "inventory",
            "100",
            &["nonexistent_inflow"],
            &[],
            None,
        );

        // Trying to compile should fail gracefully, not panic.
        // The stock references "nonexistent_inflow" which doesn't exist.
        assert!(
            project.compile_incremental().is_err(),
            "expected compilation error for stock with nonexistent flow"
        );
    }

    #[test]
    fn test_cross_dimension_mapping_simple() {
        // DimB maps to DimA. Variable b[DimB] should be accessible from a[DimA] context.
        // This is the pattern: a[DimA] = b[DimB] where DimB -> DimA
        use crate::test_common::TestProject;

        let project = TestProject::new("cross_dim_mapping")
            .named_dimension("DimA", &["A1", "A2", "A3"])
            .named_dimension_with_mapping("DimB", &["B1", "B2", "B3"], "DimA")
            .array_with_ranges("b[DimB]", vec![("B1", "1"), ("B2", "2"), ("B3", "3")])
            .array_aux("a[DimA]", "b[DimB]");

        let results = project.run_vm();
        assert!(
            results.is_ok(),
            "Cross-dimension mapping should compile and simulate: {:?}",
            results.err()
        );
        let results = results.unwrap();

        // a[A1] = b[B1] = 1, a[A2] = b[B2] = 2, a[A3] = b[B3] = 3
        for (elem, expected) in [("a[a1]", 1.0), ("a[a2]", 2.0), ("a[a3]", 3.0)] {
            let values = results.get(elem).unwrap_or_else(|| {
                panic!(
                    "missing {elem} in results: {:?}",
                    results.keys().collect::<Vec<_>>()
                )
            });
            assert_eq!(*values.last().unwrap(), expected, "wrong value for {elem}");
        }
    }

    #[test]
    fn test_cross_dimension_mapping_reverse() {
        // DimA maps to DimB (reverse of above).
        // a[DimA] = b[DimB] where DimA -> DimB
        use crate::test_common::TestProject;

        let project = TestProject::new("cross_dim_mapping_rev")
            .named_dimension_with_mapping("DimA", &["A1", "A2", "A3"], "DimB")
            .named_dimension("DimB", &["B1", "B2", "B3"])
            .array_with_ranges("b[DimB]", vec![("B1", "1"), ("B2", "2"), ("B3", "3")])
            .array_aux("a[DimA]", "b[DimB]");

        let results = project.run_vm();
        assert!(
            results.is_ok(),
            "Reverse cross-dimension mapping should compile and simulate: {:?}",
            results.err()
        );
        let results = results.unwrap();

        for (elem, expected) in [("a[a1]", 1.0), ("a[a2]", 2.0), ("a[a3]", 3.0)] {
            let values = results.get(elem).unwrap_or_else(|| {
                panic!(
                    "missing {elem} in results: {:?}",
                    results.keys().collect::<Vec<_>>()
                )
            });
            assert_eq!(*values.last().unwrap(), expected, "wrong value for {elem}");
        }
    }

    #[test]
    fn test_implicit_subscript_through_mapped_parent_dimension() {
        use crate::test_common::TestProject;

        let project = TestProject::new("implicit_parent_mapping")
            .named_dimension("DimA", &["A1", "A2", "A3"])
            .named_dimension("SubA", &["A2", "A3"])
            .named_dimension_with_mapping("DimB", &["B1", "B2", "B3"], "DimA")
            .array_with_ranges("src[DimB]", vec![("B1", "10"), ("B2", "20"), ("B3", "30")])
            .array_aux("dst[SubA]", "src");

        let results = project.run_vm();
        assert!(
            results.is_ok(),
            "implicit subscript through mapped parent should compile and run: {:?}",
            results.err()
        );
        let results = results.unwrap();
        assert_eq!(results["dst[a2]"].last().copied().unwrap(), 20.0);
        assert_eq!(results["dst[a3]"].last().copied().unwrap(), 30.0);
    }

    #[test]
    fn test_match_dimensions_with_mapping_forward() {
        // Test that match_dimensions_with_mapping finds matches via maps_to
        use crate::dimensions::DimensionsContext;

        let dim_a = crate::datamodel::Dimension::named(
            "dima".to_string(),
            vec!["a1".to_string(), "a2".to_string(), "a3".to_string()],
        );
        let mut dim_b = crate::datamodel::Dimension::named(
            "dimb".to_string(),
            vec!["b1".to_string(), "b2".to_string(), "b3".to_string()],
        );
        dim_b.set_maps_to("dima".to_string());

        let dims_ctx = DimensionsContext::from(&[dim_a, dim_b]);

        let source = vec![named_dim("dimb", &["b1", "b2", "b3"])];
        let target = vec![named_dim("dima", &["a1", "a2", "a3"])];

        let result = match_dimensions_with_mapping(&source, &target, &[false], &dims_ctx);
        assert_eq!(result, vec![Some(0)], "DimB should match DimA via maps_to");
    }

    #[test]
    fn test_match_dimensions_with_mapping_reverse() {
        // Test reverse: target.maps_to == source
        use crate::dimensions::DimensionsContext;

        let mut dim_a = crate::datamodel::Dimension::named(
            "dima".to_string(),
            vec!["a1".to_string(), "a2".to_string(), "a3".to_string()],
        );
        dim_a.set_maps_to("dimb".to_string());
        let dim_b = crate::datamodel::Dimension::named(
            "dimb".to_string(),
            vec!["b1".to_string(), "b2".to_string(), "b3".to_string()],
        );

        let dims_ctx = DimensionsContext::from(&[dim_a, dim_b]);

        // Source is DimB, target is DimA (which maps to DimB)
        let source = vec![named_dim("dimb", &["b1", "b2", "b3"])];
        let target = vec![named_dim("dima", &["a1", "a2", "a3"])];

        let result = match_dimensions_with_mapping(&source, &target, &[false], &dims_ctx);
        assert_eq!(
            result,
            vec![Some(0)],
            "DimB should match DimA via reverse maps_to"
        );
    }

    #[test]
    fn test_match_dimensions_with_mapping_shared_parent_second_target() {
        use crate::dimensions::DimensionsContext;

        let dim_a = crate::datamodel::Dimension::named(
            "dima".to_string(),
            vec!["a1".to_string(), "a2".to_string(), "a3".to_string()],
        );
        let dim_b = crate::datamodel::Dimension::named(
            "dimb".to_string(),
            vec!["b1".to_string(), "b2".to_string(), "b3".to_string()],
        );
        let dim_c = crate::datamodel::Dimension::named(
            "dimc".to_string(),
            vec!["c1".to_string(), "c2".to_string(), "c3".to_string()],
        );

        let mut dim_x = crate::datamodel::Dimension::named(
            "dimx".to_string(),
            vec!["x1".to_string(), "x2".to_string(), "x3".to_string()],
        );
        dim_x.mappings = vec![
            crate::datamodel::DimensionMapping {
                target: "dimb".to_string(),
                element_map: vec![],
            },
            crate::datamodel::DimensionMapping {
                target: "dimc".to_string(),
                element_map: vec![],
            },
        ];

        let mut dim_y = crate::datamodel::Dimension::named(
            "dimy".to_string(),
            vec!["y1".to_string(), "y2".to_string(), "y3".to_string()],
        );
        dim_y.mappings = vec![
            crate::datamodel::DimensionMapping {
                target: "dima".to_string(),
                element_map: vec![],
            },
            crate::datamodel::DimensionMapping {
                target: "dimc".to_string(),
                element_map: vec![],
            },
        ];

        let dims_ctx = DimensionsContext::from(&[dim_a, dim_b, dim_c, dim_x, dim_y]);

        let source = vec![named_dim("dimx", &["x1", "x2", "x3"])];
        let target = vec![named_dim("dimy", &["y1", "y2", "y3"])];

        let result = match_dimensions_with_mapping(&source, &target, &[false], &dims_ctx);
        assert_eq!(
            result,
            vec![Some(0)],
            "dimensions sharing a non-first mapping target should match"
        );
    }
}
