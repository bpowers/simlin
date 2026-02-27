// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;

use crate::dimensions::{Dimension, DimensionsContext};

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
        let indexed_elements: std::collections::HashMap<
            crate::common::CanonicalElementName,
            usize,
        > = canonical_elements
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
        use crate::common::canonicalize;
        use crate::datamodel::{
            self, Aux as DatamodelAux, Model as DatamodelModel, SimMethod, SimSpecs,
            Variable as DatamodelVariable, Visibility,
        };
        use crate::project::Project;

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
            }],
            source: None,
            ai_information: None,
        };

        // Convert to engine project
        let project: Project = datamodel_project.into();

        // Create a Module and compile it
        let model = project
            .models
            .get(&*canonicalize("main"))
            .expect("main model should exist");
        let module: super::super::Module = super::super::Module::new(
            &project,
            model.clone(),
            &std::collections::BTreeSet::new(),
            true,
        )
        .expect("Module creation should succeed");

        // Compile the module
        let compiled = module.compile().expect("Compilation should succeed");

        // Verify dimension metadata is populated
        let context = &compiled.context;

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
        use crate::common::canonicalize;
        use crate::datamodel::{
            self, Aux as DatamodelAux, Model as DatamodelModel, SimMethod, SimSpecs,
            Variable as DatamodelVariable, Visibility,
        };
        use crate::project::Project;

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
            }],
            source: None,
            ai_information: None,
        };

        let project: Project = datamodel_project.into();

        let model = project
            .models
            .get(&*canonicalize("main"))
            .expect("main model should exist");
        let module: super::super::Module = super::super::Module::new(
            &project,
            model.clone(),
            &std::collections::BTreeSet::new(),
            true,
        )
        .expect("Module creation should succeed");

        let compiled = module.compile().expect("Compilation should succeed");
        let context = &compiled.context;

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
    fn test_lazy_subdimension_relation() {
        use crate::common::{CanonicalDimensionName, canonicalize};
        use crate::datamodel::{
            self, Aux as DatamodelAux, Model as DatamodelModel, SimMethod, SimSpecs,
            Variable as DatamodelVariable, Visibility,
        };
        use crate::project::Project;

        // Create a datamodel project with a parent dimension and subdimension
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
            dimensions: vec![
                datamodel::Dimension::named(
                    "Parent".to_string(),
                    vec![
                        "A".to_string(),
                        "B".to_string(),
                        "C".to_string(),
                        "D".to_string(),
                    ],
                ),
                datamodel::Dimension::named(
                    "Child".to_string(),
                    vec!["B".to_string(), "C".to_string()],
                ),
            ],
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
            }],
            source: None,
            ai_information: None,
        };

        let project: Project = datamodel_project.into();

        let model = project
            .models
            .get(&*canonicalize("main"))
            .expect("main model should exist");
        let module: super::super::Module = super::super::Module::new(
            &project,
            model.clone(),
            &std::collections::BTreeSet::new(),
            true,
        )
        .expect("Module creation should succeed");

        // Create a Compiler directly to test lazy subdim_relation population
        let mut compiler = super::super::codegen::Compiler::new(&module);

        // Initially, subdim_relations should be empty (lazy population)
        assert!(
            compiler.subdim_relations.is_empty(),
            "subdim_relations should be empty before lazy lookup"
        );

        // Dimensions should be populated
        assert_eq!(
            compiler.dimensions.len(),
            2,
            "Should have 2 dimensions populated"
        );

        // Now call get_or_add_subdim_relation to lazily add the relation
        let child_name = CanonicalDimensionName::from_raw("Child");
        let parent_name = CanonicalDimensionName::from_raw("Parent");

        let rel_id = compiler.get_or_add_subdim_relation(&child_name, &parent_name);
        assert!(rel_id.is_some(), "Child should be subdimension of Parent");
        assert_eq!(rel_id, Some(0), "First relation should have id 0");

        // Now subdim_relations should have one entry
        assert_eq!(
            compiler.subdim_relations.len(),
            1,
            "Should have 1 subdim_relation after lazy lookup"
        );

        let relation = &compiler.subdim_relations[0];
        // B is at index 1 in Parent, C is at index 2
        assert_eq!(
            relation.parent_offsets.as_slice(),
            &[1, 2],
            "Child elements should map to parent indices 1, 2"
        );
        assert!(relation.is_contiguous, "B, C are contiguous in parent");
        assert_eq!(relation.start_offset, 1);

        // Calling again should return the same id (cached)
        let rel_id_again = compiler.get_or_add_subdim_relation(&child_name, &parent_name);
        assert_eq!(
            rel_id_again,
            Some(0),
            "Should return same id for same lookup"
        );
        assert_eq!(
            compiler.subdim_relations.len(),
            1,
            "Should still have only 1 relation (no duplicate)"
        );

        // Looking up a non-existent relation should return None
        let unrelated_name = CanonicalDimensionName::from_raw("Nonexistent");
        let no_rel = compiler.get_or_add_subdim_relation(&unrelated_name, &parent_name);
        assert!(
            no_rel.is_none(),
            "Non-existent dimension should return None"
        );

        // Parent is not a subdimension of Child
        let reverse_rel = compiler.get_or_add_subdim_relation(&parent_name, &child_name);
        assert!(
            reverse_rel.is_none(),
            "Parent is not a subdimension of Child"
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

        // Trying to build a simulation should fail gracefully, not panic.
        // The stock references "nonexistent_inflow" which doesn't exist.
        let result = project.build_sim();
        assert!(
            result.is_err(),
            "Expected an error for missing flow reference, but got Ok"
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

        let results = project.run_interpreter();
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

        let results = project.run_interpreter();
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

        let results = project.run_interpreter();
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
