// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;

use crate::ast::ArrayView;
use crate::dimensions::Dimension;

/// Result of matching source dimensions to target dimensions.
///
/// For each target dimension, provides either:
/// - Some(source_idx): which source dimension maps here
/// - None: no source dimension (broadcast with stride 0)
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
#[allow(dead_code)] // Scaffolding for future broadcast_view usage
pub struct DimensionMapping {
    /// mapping[target_idx] = Some(source_idx) or None
    /// For each target dimension, which source dimension maps to it (or None for broadcasting)
    pub mapping: Vec<Option<usize>>,
    /// For each source dimension, which target dimension it matched
    pub source_to_target: Vec<usize>,
}

/// Match source dimensions to target dimensions.
///
/// Algorithm (dimension-agnostic, works for any N):
/// 1. FIRST PASS: Assign all exact name matches (reserve them)
/// 2. SECOND PASS: For remaining sources, do size-based matching (indexed dims only)
/// 3. Build the reverse mapping (target -> source)
///
/// This two-pass approach ensures that name matches take priority over size matches.
/// Without it, a greedy single-pass approach could let a size match "steal" a target
/// that a later source dimension would have matched by name.
///
/// Returns None if any source dimension cannot be matched.
#[allow(dead_code)] // Scaffolding for future broadcast_view usage
pub fn match_dimensions(
    source_dims: &[Dimension],
    target_dims: &[Dimension],
) -> Option<DimensionMapping> {
    let source_to_target =
        match_dimensions_two_pass(source_dims, target_dims, &vec![false; target_dims.len()])?;

    // Build reverse mapping
    let mut mapping = vec![None; target_dims.len()];
    for (source_idx, &target_idx) in source_to_target.iter().enumerate() {
        mapping[target_idx] = Some(source_idx);
    }

    Some(DimensionMapping {
        mapping,
        source_to_target,
    })
}

/// Two-pass dimension matching that reserves name matches before size matches.
///
/// Pass 1: Find and assign all exact name matches
/// Pass 2: For remaining unmatched sources, try size-based matching (indexed dims only)
///
/// Returns source_to_target mapping, or None if matching fails.
fn match_dimensions_two_pass(
    source_dims: &[Dimension],
    target_dims: &[Dimension],
    initially_used: &[bool],
) -> Option<Vec<usize>> {
    let partial = match_dimensions_two_pass_partial(source_dims, target_dims, initially_used);

    // Verify all sources were matched
    partial.into_iter().collect()
}

/// Two-pass dimension matching that allows partial matches (some sources unmatched).
///
/// This is used for cases like SUM(arr[A,B]) in context [A] where B won't match.
/// Returns a vector where each element is Some(target_idx) or None.
pub(super) fn match_dimensions_two_pass_partial(
    source_dims: &[Dimension],
    target_dims: &[Dimension],
    initially_used: &[bool],
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

    // PASS 2: Size-based matches for remaining sources (indexed dimensions only)
    for (source_idx, source_dim) in source_dims.iter().enumerate() {
        if source_to_target[source_idx].is_some() {
            continue; // Already matched by name
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

/// Find target dimension for a source dimension (single dimension lookup).
///
/// NOTE: For matching multiple source dimensions, prefer `match_dimensions_two_pass`
/// which correctly reserves name matches before allowing size-based matches.
/// This function is kept for cases where we need to match a single dimension
/// and the caller manages the used array properly.
#[allow(dead_code)] // Kept for potential single-dimension matching use cases
fn find_target_for_source(
    source_dim: &Dimension,
    target_dims: &[Dimension],
    used: &[bool],
) -> Option<usize> {
    // First pass: exact name match (works for both named and indexed)
    for (i, target) in target_dims.iter().enumerate() {
        if !used[i] && target.name() == source_dim.name() {
            return Some(i);
        }
    }

    // Second pass: size-based match (indexed dimensions only)
    // IMPORTANT: This should only be called when there's no name match pending
    // for any other source dimension. See match_dimensions_two_pass for proper handling.
    if let Dimension::Indexed(_, source_size) = source_dim {
        for (i, target) in target_dims.iter().enumerate() {
            if !used[i]
                && let Dimension::Indexed(_, target_size) = target
                && source_size == target_size
            {
                return Some(i);
            }
        }
    }

    None
}

/// Broadcast a source view to match target dimensions.
///
/// For each target dimension:
/// - If source has a matching dimension: use its stride
/// - If no match: use stride 0 (broadcast/repeat)
///
/// This is dimension-agnostic: works for any N.
///
/// NOTE: This function does not preserve sparse array information from the source view.
/// The resulting view always has an empty sparse vector. If sparse data preservation
/// is needed in the future, this would require transforming sparse indices to account
/// for the new dimension order and any broadcast dimensions.
#[allow(dead_code)] // Scaffolding for future optimization
pub fn broadcast_view(
    source_view: &ArrayView,
    source_dims: &[Dimension],
    target_dims: &[Dimension],
) -> Option<ArrayView> {
    let mapping = match_dimensions(source_dims, target_dims)?;

    let mut new_dims = Vec::with_capacity(target_dims.len());
    let mut new_strides = Vec::with_capacity(target_dims.len());
    let mut new_dim_names = Vec::with_capacity(target_dims.len());

    for (target_idx, target_dim) in target_dims.iter().enumerate() {
        new_dims.push(target_dim.len());
        new_dim_names.push(target_dim.name().to_string());

        match mapping.mapping[target_idx] {
            Some(source_idx) => {
                // Source dimension maps here - use its stride
                new_strides.push(source_view.strides[source_idx]);
            }
            None => {
                // No source dimension - broadcast (stride 0)
                new_strides.push(0);
            }
        }
    }

    Some(ArrayView {
        dims: new_dims,
        strides: new_strides,
        offset: source_view.offset,
        // Sparse info not preserved - see doc comment for rationale
        sparse: Vec::new(),
        dim_names: new_dim_names,
    })
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
    use crate::common::CanonicalDimensionName;

    fn indexed_dim(name: &str, size: u32) -> Dimension {
        Dimension::Indexed(CanonicalDimensionName::from_raw(name), size)
    }

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
                    equation: datamodel::Equation::Scalar("1".to_string(), None),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: Visibility::Public,
                    ai_state: None,
                    uid: None,
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
            .get(&canonicalize("main"))
            .expect("main model should exist");
        let module: super::super::Module<f64> = super::super::Module::new(
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
                    equation: datamodel::Equation::Scalar("1".to_string(), None),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: Visibility::Public,
                    ai_state: None,
                    uid: None,
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
            .get(&canonicalize("main"))
            .expect("main model should exist");
        let module: super::super::Module<f64> = super::super::Module::new(
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
                    equation: datamodel::Equation::Scalar("1".to_string(), None),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: Visibility::Public,
                    ai_state: None,
                    uid: None,
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
            .get(&canonicalize("main"))
            .expect("main model should exist");
        let module: super::super::Module<f64> = super::super::Module::new(
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
    fn test_find_target_for_source_name_match() {
        // Test name matching for indexed dimensions
        let source = indexed_dim("products", 3);
        let targets = vec![indexed_dim("products", 3)];
        let used = vec![false];

        let result = find_target_for_source(&source, &targets, &used);
        assert_eq!(result, Some(0), "Should match by name");
    }

    #[test]
    fn test_find_target_for_source_size_match() {
        // Test size-based matching for indexed dimensions with different names
        let source = indexed_dim("regions", 3);
        let targets = vec![indexed_dim("products", 3)];
        let used = vec![false];

        let result = find_target_for_source(&source, &targets, &used);
        assert_eq!(
            result,
            Some(0),
            "Should match by size for different-named indexed dims"
        );
    }

    #[test]
    fn test_find_target_for_source_size_mismatch() {
        // Test that size must match for indexed dimensions
        let source = indexed_dim("regions", 3);
        let targets = vec![indexed_dim("products", 5)];
        let used = vec![false];

        let result = find_target_for_source(&source, &targets, &used);
        assert_eq!(result, None, "Should not match when sizes differ");
    }

    #[test]
    fn test_find_target_for_source_named_no_size_match() {
        // Named dimensions should NOT match by size, only by name
        let source = named_dim("cities", &["boston", "seattle"]);
        let targets = vec![named_dim("products", &["widgets", "gadgets"])];
        let used = vec![false];

        let result = find_target_for_source(&source, &targets, &used);
        assert_eq!(result, None, "Named dims should not match by size");
    }

    #[test]
    fn test_find_target_for_source_respects_used() {
        // Test that already-used targets are skipped
        let source = indexed_dim("regions", 3);
        let targets = vec![indexed_dim("products", 3), indexed_dim("categories", 3)];
        let used = vec![true, false]; // products already used

        let result = find_target_for_source(&source, &targets, &used);
        assert_eq!(
            result,
            Some(1),
            "Should match second target when first is used"
        );
    }

    #[test]
    fn test_match_dimensions_same_name() {
        // Test matching dimensions with same names
        let source = vec![indexed_dim("x", 2), indexed_dim("y", 3)];
        let target = vec![indexed_dim("x", 2), indexed_dim("y", 3)];

        let result = match_dimensions(&source, &target);
        assert!(result.is_some());
        let mapping = result.unwrap();
        assert_eq!(mapping.mapping, vec![Some(0), Some(1)]);
        assert_eq!(mapping.source_to_target, vec![0, 1]);
    }

    #[test]
    fn test_match_dimensions_different_names_same_size() {
        // Test matching indexed dimensions with different names but same sizes
        let source = vec![indexed_dim("a", 3)];
        let target = vec![indexed_dim("b", 3)];

        let result = match_dimensions(&source, &target);
        assert!(result.is_some());
        let mapping = result.unwrap();
        assert_eq!(mapping.mapping, vec![Some(0)]);
        assert_eq!(mapping.source_to_target, vec![0]);
    }

    #[test]
    fn test_match_dimensions_broadcasting() {
        // Test broadcasting: 1D source to 2D target
        let source = vec![indexed_dim("x", 2)];
        let target = vec![indexed_dim("x", 2), indexed_dim("y", 3)];

        let result = match_dimensions(&source, &target);
        assert!(result.is_some());
        let mapping = result.unwrap();
        assert_eq!(mapping.mapping, vec![Some(0), None]); // x matched, y is broadcast
        assert_eq!(mapping.source_to_target, vec![0]);
    }

    #[test]
    fn test_broadcast_view() {
        // Test broadcast_view creates correct strides
        let source_dims = vec![indexed_dim("x", 2)];
        let target_dims = vec![indexed_dim("x", 2), indexed_dim("y", 3)];

        // Source view: 1D contiguous [2], strides [1]
        let source_view = ArrayView::contiguous_with_names(vec![2], vec!["x".to_string()]);

        let result = broadcast_view(&source_view, &source_dims, &target_dims);
        assert!(result.is_some());
        let broadcast = result.unwrap();

        assert_eq!(broadcast.dims, vec![2, 3]);
        assert_eq!(broadcast.strides, vec![1, 0]); // x uses stride 1, y uses stride 0 (broadcast)
        assert_eq!(broadcast.offset, 0);
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
}
