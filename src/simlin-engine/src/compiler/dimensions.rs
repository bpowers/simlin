// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use crate::dimensions::{
    Axis, Dimension, DimensionsContext, NoAxisRelations, match_axes, match_axes_partial,
};

/// For each dimension of `dims`, the POSITION in `active_dims` whose subscript
/// supplies it -- the implicit-subscript axis allocation behind a BARE arrayed
/// reference -- or `None` when some dimension no active axis supplies, which is
/// the compiler's `MismatchedDimensions`.
///
/// The allocation itself is [`crate::dimensions::match_axes`], the engine's one
/// axis-matching precedence; this is the projection that drops HOW each axis
/// matched and keeps only WHICH active axis supplies it. Read that function for
/// the precedence and for the two properties every caller depends on
/// (positional, one-to-one).
///
/// **Which references arrive here.** An ordinary expression cannot reach this
/// allocation: `compiler::context`'s `lower_pass0` rewrites a bare arrayed
/// reference in an equation body into an explicit `Expr2::Subscript` before
/// Expr3, so it resolves through the subscript path. The two production
/// callers are wiring rather than expressions -- a stock's inflow/outflow
/// references (`Context::fold_flows`) and `compiler::Var::new`'s stock
/// self-reference plus module input wiring. That is why the GH #996 hazard
/// fixture in `crate::mapped_reference_semantics_tests` is built from a
/// two-axis FLOW under a stock: a fixture built from an aux equation exercises
/// nothing. See `compiler::context`'s `get_implicit_subscripts` for the rest
/// of that measurement.
///
/// The LTM describers ask the same matcher through `db::bare_axis_pairing`,
/// which keeps the match kind (a mapped pair carries its executed
/// correspondence), so a pin cannot spell a row the simulation does not read.
pub(crate) fn allocate_implicit_axes(
    dims: &[Dimension],
    active_dims: &[Dimension],
    dimensions_ctx: &DimensionsContext,
) -> Option<Vec<usize>> {
    match_axes(dims, active_dims, dimensions_ctx).map(|alloc| {
        alloc
            .into_iter()
            .map(|(active_idx, _)| active_idx)
            .collect()
    })
}

/// The permutation that reads `source_names`'s axes in `target_names`'s order:
/// `result[i]` is the source axis that belongs at target position `i`, the
/// form `@N` subscripts and [`crate::ast::ArrayView::reorder_dimensions`] take.
///
/// Reordering is a RELABELLING of one shape, so the two lists must name the
/// same axes: equal arity, and every target axis supplied by a distinct source
/// axis. That is [`crate::dimensions::match_axes_partial`] run from the target
/// side with no declared relations available -- these are dimension NAMES off
/// an `ArrayView` or an `ArrayBounds`, and pairing `[DimA]` with `[DimB]`
/// through a declared mapping would silently transpose one operand of an
/// elementwise expression rather than reorder it.
pub(super) fn axis_reordering(
    source_names: &[String],
    target_names: &[String],
) -> Option<Vec<usize>> {
    if source_names.len() != target_names.len() {
        return None;
    }
    fn axes(names: &[String]) -> Vec<Axis<'_>> {
        names.iter().map(|n| Axis::named(n.as_str(), 0)).collect()
    }
    match_axes_partial(&axes(target_names), &axes(source_names), &NoAxisRelations)
        .into_iter()
        .map(|m| m.map(|(source_idx, _)| source_idx))
        .collect()
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
    use crate::dimensions::axes_of;

    /// The partial allocation -- [`crate::dimensions::match_axes_partial`]
    /// with the match kind dropped -- so the precedence pins below read as
    /// position lists.
    fn allocate_partial(
        dims: &[Dimension],
        active_dims: &[Dimension],
        ctx: &DimensionsContext,
    ) -> Vec<Option<usize>> {
        crate::dimensions::match_axes_partial(&axes_of(dims), &axes_of(active_dims), ctx)
            .into_iter()
            .map(|m| m.map(|(active_idx, _)| active_idx))
            .collect()
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
            allocate_partial(
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
            allocate_partial(&[agg_d.clone(), cop_d], std::slice::from_ref(&agg_d), &ctx),
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
            allocate_partial(&[ib_d, y_d], std::slice::from_ref(&ia_d), &ctx),
            vec![None, Some(0)],
            "the mapping-matching axis must get the slot; a size match on an \
             earlier axis must not consume it first"
        );
    }

    /// [`axis_reordering`]'s own contract: equal arity, every target axis
    /// supplied by a distinct source axis, and the result in TARGET order.
    /// The name rule it runs on is a row of `dimensions::axis_match_tests`.
    #[test]
    fn axis_reordering_is_a_relabelling_or_nothing() {
        let names = |ns: &[&str]| -> Vec<String> { ns.iter().map(|s| s.to_string()).collect() };

        // identity, a 2-D transpose, and two 3-D rotations
        assert_eq!(
            axis_reordering(&names(&["a", "b", "c"]), &names(&["a", "b", "c"])),
            Some(vec![0, 1, 2])
        );
        assert_eq!(
            axis_reordering(&names(&["row", "col"]), &names(&["col", "row"])),
            Some(vec![1, 0])
        );
        assert_eq!(
            axis_reordering(&names(&["a", "b", "c"]), &names(&["b", "c", "a"])),
            Some(vec![1, 2, 0])
        );
        assert_eq!(
            axis_reordering(&names(&["a", "b", "c"]), &names(&["c", "a", "b"])),
            Some(vec![2, 0, 1])
        );

        // disjoint axes, a target axis the source does not have, unequal
        // arity, and a target that repeats an axis the source declares once
        assert_eq!(
            axis_reordering(&names(&["a", "b"]), &names(&["c", "d"])),
            None
        );
        assert_eq!(
            axis_reordering(&names(&["a", "b"]), &names(&["a", "c"])),
            None
        );
        assert_eq!(
            axis_reordering(&names(&["a", "b"]), &names(&["a", "b", "c"])),
            None
        );
        assert_eq!(
            axis_reordering(&names(&["a", "b"]), &names(&["a", "a"])),
            None
        );

        // the degenerate ends
        assert_eq!(
            axis_reordering(&names(&["x"]), &names(&["x"])),
            Some(vec![0])
        );
        assert_eq!(axis_reordering(&[], &[]), Some(vec![]));
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
}
