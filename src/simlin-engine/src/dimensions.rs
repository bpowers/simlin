// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use crate::common::{CanonicalDimensionName, CanonicalElementName};
use crate::datamodel;

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct NamedDimension {
    pub elements: Vec<CanonicalElementName>,
    pub indexed_elements: HashMap<CanonicalElementName, usize>,
    /// O(1) containment check for subdimension detection
    pub element_set: HashSet<CanonicalElementName>,
}

/// Relationship between a subdimension and parent dimension.
/// Maps each subdim element index to its offset in the parent.
#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubdimensionRelation {
    /// Maps subdim element index -> parent offset (0-based).
    /// For SubA=[A2,A3] from DimA=[A1,A2,A3]: parent_offsets=[1,2]
    pub parent_offsets: Vec<usize>,
}

#[allow(dead_code)]
impl SubdimensionRelation {
    /// Check if parent offsets are contiguous (can use range instead of sparse iteration)
    pub fn is_contiguous(&self) -> bool {
        if self.parent_offsets.len() <= 1 {
            return true;
        }
        for i in 1..self.parent_offsets.len() {
            if self.parent_offsets[i] != self.parent_offsets[i - 1] + 1 {
                return false;
            }
        }
        true
    }

    /// For contiguous relations, get the start offset
    pub fn start_offset(&self) -> usize {
        self.parent_offsets.first().copied().unwrap_or(0)
    }
}

/// Cache for subdimension relationships. Uses RefCell for O(1) lookup after first computation.
/// The cache key is (child_name, parent_name), and the value is the relation if child is
/// a subdimension of parent, or None if we've determined it's not a subdimension.
#[allow(dead_code)]
#[derive(Debug, Default)]
struct RelationshipCache {
    cache: RefCell<
        HashMap<(CanonicalDimensionName, CanonicalDimensionName), Option<SubdimensionRelation>>,
    >,
}

impl Clone for RelationshipCache {
    fn clone(&self) -> Self {
        RelationshipCache {
            cache: RefCell::new(self.cache.borrow().clone()),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Dimension {
    Indexed(CanonicalDimensionName, u32),
    Named(CanonicalDimensionName, NamedDimension),
}

impl Dimension {
    pub fn len(&self) -> usize {
        match self {
            Dimension::Indexed(_, size) => *size as usize,
            Dimension::Named(_, named) => named.elements.len(),
        }
    }

    #[allow(unused)]
    pub fn name(&self) -> &str {
        match self {
            Dimension::Indexed(name, _) | Dimension::Named(name, _) => name.as_str(),
        }
    }

    /// Get the offset of an element by name (for named dimensions) or by index string (for indexed dimensions).
    /// Returns 0-based offset for use in array indexing.
    pub fn get_offset(&self, subscript: &CanonicalElementName) -> Option<usize> {
        match self {
            Dimension::Named(_, named) => {
                // Try canonical lookup first
                let canonical_element = subscript;
                named
                    .indexed_elements
                    .get(canonical_element)
                    .map(|&idx| idx - 1) // Convert from 1-based to 0-based
            }
            Dimension::Indexed(_, size) => {
                // Parse as number for indexed dimensions
                subscript.as_str().parse::<u32>().ok().and_then(|n| {
                    if n >= 1 && n <= *size {
                        Some((n - 1) as usize) // Convert from 1-based to 0-based
                    } else {
                        None
                    }
                })
            }
        }
    }
}

impl From<datamodel::Dimension> for Dimension {
    fn from(dim: datamodel::Dimension) -> Dimension {
        match dim {
            datamodel::Dimension::Indexed(name, size) => {
                Dimension::Indexed(CanonicalDimensionName::from_raw(&name), size)
            }
            datamodel::Dimension::Named(name, elements) => {
                let canonical_elements: Vec<CanonicalElementName> = elements
                    .iter()
                    .map(|e| CanonicalElementName::from_raw(e))
                    .collect();
                let indexed_elements: HashMap<CanonicalElementName, usize> = canonical_elements
                    .iter()
                    .enumerate()
                    // system dynamic indexes are 1-indexed
                    .map(|(i, elem)| (elem.clone(), i + 1))
                    .collect();
                let element_set: HashSet<CanonicalElementName> =
                    canonical_elements.iter().cloned().collect();
                Dimension::Named(
                    CanonicalDimensionName::from_raw(&name),
                    NamedDimension {
                        indexed_elements,
                        elements: canonical_elements,
                        element_set,
                    },
                )
            }
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct DimensionsContext {
    dimensions: HashMap<CanonicalDimensionName, Dimension>,
}

impl DimensionsContext {
    pub(crate) fn from(dimensions: &[datamodel::Dimension]) -> DimensionsContext {
        DimensionsContext {
            dimensions: dimensions
                .iter()
                .map(|dim| {
                    (
                        CanonicalDimensionName::from_raw(dim.name()),
                        Dimension::from(dim.clone()),
                    )
                })
                .collect(),
        }
    }

    pub(crate) fn is_dimension_name(&self, name: &str) -> bool {
        let canonical_name = CanonicalDimensionName::from_raw(name);
        self.dimensions.contains_key(&canonical_name)
    }

    pub(crate) fn lookup(&self, element: &str) -> Option<u32> {
        if let Some(pos) = element.find('·') {
            let dimension_name = CanonicalDimensionName::from_raw(&element[..pos]);
            let element_name = CanonicalElementName::from_raw(&element[pos + '·'.len_utf8()..]);
            if let Some(Dimension::Named(_, dimension)) = self.dimensions.get(&dimension_name)
                && let Some(off) = dimension.indexed_elements.get(&element_name)
            {
                return Some(*off as u32);
            }
        }
        None
    }
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DimensionRange {
    dim: Option<Dimension>,
    start: u32,
    end: u32,
}

#[allow(dead_code)]
impl DimensionRange {
    pub fn new(dim: Dimension, start: u32, end: u32) -> Self {
        DimensionRange {
            dim: Some(dim),
            start,
            end,
        }
    }

    pub fn len(&self) -> u32 {
        self.end.saturating_sub(self.start)
    }
}

/// DimensionInfo represents the array dimensions of an expression.
/// It uses the existing Dimension enum which already encapsulates
/// both name and size together.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DimensionVec {
    dims: Vec<DimensionRange>,
}

#[allow(dead_code)]
impl DimensionVec {
    /// Create dimension info from a vector of dimensions
    pub fn new(dims: Vec<DimensionRange>) -> Self {
        DimensionVec { dims }
    }
}

/// Dimension information for non-contiguous array views
///
/// `StridedDimension` is used only in `ArrayView::Strided` for views where
/// elements are not stored consecutively in memory (e.g., after transpose,
/// column/row selection, or slicing operations). For normal contiguous arrays,
/// `ArrayView::Contiguous` is used instead, which only needs dimension sizes
/// since strides can be computed implicitly assuming row-major order.
///
/// A `StridedDimension` describes how to iterate through one dimension of a
/// strided array view. The key insight is that `dimension.len()` represents
/// the number of elements in this dimension of the *view*, not the underlying
/// storage.
///
/// For example, if you have a 3x4 matrix and select column 1, you get a view
/// with shape [3]. The `StridedDimension` would be:
/// - `dimension`: a Dimension with length 3 (the view has 3 elements)
/// - `stride`: 4 (skip 4 elements in storage to get to the next row)
///
/// The stride tells you how many elements to skip in the underlying flat
/// storage to move by 1 in this dimension. For a contiguous row-major array,
/// the rightmost dimension has stride 1, and each dimension to the left has
/// a stride equal to the product of all dimension sizes to its right.
///
/// Example: A 2x3x4 array in row-major order has strides [12, 4, 1]:
/// - To move by 1 in the first dimension: skip 12 elements (3*4)
/// - To move by 1 in the second dimension: skip 4 elements (4)
/// - To move by 1 in the third dimension: skip 1 element
#[derive(PartialEq, Clone, Debug)]
#[allow(dead_code)]
pub struct StridedDimension {
    pub dimension: Dimension,
    pub stride: isize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::CanonicalElementName;
    use crate::datamodel;

    #[test]
    fn test_get_offset_named_dimension() {
        // Create a named dimension with canonical elements
        let datamodel_dim = datamodel::Dimension::Named(
            "Region".to_string(),
            vec!["North".to_string(), "South".to_string(), "East".to_string()],
        );
        let dim = Dimension::from(datamodel_dim);

        // Test exact matches (canonical form)
        assert_eq!(
            dim.get_offset(&CanonicalElementName::from_raw("north")),
            Some(0)
        );
        assert_eq!(
            dim.get_offset(&CanonicalElementName::from_raw("south")),
            Some(1)
        );
        assert_eq!(
            dim.get_offset(&CanonicalElementName::from_raw("east")),
            Some(2)
        );

        // Test case insensitive matching (should canonicalize)
        assert_eq!(
            dim.get_offset(&CanonicalElementName::from_raw("North")),
            Some(0)
        );
        assert_eq!(
            dim.get_offset(&CanonicalElementName::from_raw("SOUTH")),
            Some(1)
        );
        assert_eq!(
            dim.get_offset(&CanonicalElementName::from_raw("EaSt")),
            Some(2)
        );

        // Test non-existent element
        assert_eq!(
            dim.get_offset(&CanonicalElementName::from_raw("west")),
            None
        );
        assert_eq!(dim.get_offset(&CanonicalElementName::from_raw("")), None);
    }

    #[test]
    fn test_get_offset_indexed_dimension() {
        // Create an indexed dimension
        let datamodel_dim = datamodel::Dimension::Indexed("Index".to_string(), 5);
        let dim = Dimension::from(datamodel_dim);

        // Test valid indices (1-based input, 0-based output)
        assert_eq!(
            dim.get_offset(&CanonicalElementName::from_raw("1")),
            Some(0)
        );
        assert_eq!(
            dim.get_offset(&CanonicalElementName::from_raw("2")),
            Some(1)
        );
        assert_eq!(
            dim.get_offset(&CanonicalElementName::from_raw("3")),
            Some(2)
        );
        assert_eq!(
            dim.get_offset(&CanonicalElementName::from_raw("4")),
            Some(3)
        );
        assert_eq!(
            dim.get_offset(&CanonicalElementName::from_raw("5")),
            Some(4)
        );

        // Test out of bounds indices
        assert_eq!(dim.get_offset(&CanonicalElementName::from_raw("0")), None);
        assert_eq!(dim.get_offset(&CanonicalElementName::from_raw("6")), None);
        assert_eq!(dim.get_offset(&CanonicalElementName::from_raw("100")), None);
        assert_eq!(dim.get_offset(&CanonicalElementName::from_raw("-1")), None);

        // Test invalid input (not a number)
        assert_eq!(dim.get_offset(&CanonicalElementName::from_raw("abc")), None);
        assert_eq!(dim.get_offset(&CanonicalElementName::from_raw("")), None);
        assert_eq!(dim.get_offset(&CanonicalElementName::from_raw("1.5")), None);
    }

    #[test]
    fn test_get_offset_with_special_characters() {
        // Test dimension with elements containing spaces and dots
        let datamodel_dim = datamodel::Dimension::Named(
            "Product Type".to_string(),
            vec![
                "Product A".to_string(),
                "Product.B".to_string(),
                "Product_C".to_string(),
            ],
        );
        let dim = Dimension::from(datamodel_dim);

        // Spaces should be converted to underscores
        assert_eq!(
            dim.get_offset(&CanonicalElementName::from_raw("Product A")),
            Some(0)
        );
        assert_eq!(
            dim.get_offset(&CanonicalElementName::from_raw("Product_A")),
            Some(0)
        );
        assert_eq!(
            dim.get_offset(&CanonicalElementName::from_raw("product a")),
            Some(0)
        );
        assert_eq!(
            dim.get_offset(&CanonicalElementName::from_raw("product_a")),
            Some(0)
        );

        // Dots should be converted to middle dots
        assert_eq!(
            dim.get_offset(&CanonicalElementName::from_raw("Product.B")),
            Some(1)
        );
        assert_eq!(
            dim.get_offset(&CanonicalElementName::from_raw("product.b")),
            Some(1)
        );

        // Underscores are preserved
        assert_eq!(
            dim.get_offset(&CanonicalElementName::from_raw("Product_C")),
            Some(2)
        );
        assert_eq!(
            dim.get_offset(&CanonicalElementName::from_raw("product_c")),
            Some(2)
        );
    }

    #[test]
    fn test_get_offset_empty_dimension() {
        // Edge case: empty named dimension
        let datamodel_dim = datamodel::Dimension::Named("Empty".to_string(), vec![]);
        let dim = Dimension::from(datamodel_dim);

        assert_eq!(
            dim.get_offset(&CanonicalElementName::from_raw("anything")),
            None
        );

        // Edge case: indexed dimension with size 0
        let datamodel_dim = datamodel::Dimension::Indexed("Zero".to_string(), 0);
        let dim = Dimension::from(datamodel_dim);

        assert_eq!(dim.get_offset(&CanonicalElementName::from_raw("1")), None);
        assert_eq!(dim.get_offset(&CanonicalElementName::from_raw("0")), None);
    }

    #[test]
    fn test_get_offset_large_indexed_dimension() {
        // Test with a larger indexed dimension
        let datamodel_dim = datamodel::Dimension::Indexed("Large".to_string(), 1000);
        let dim = Dimension::from(datamodel_dim);

        // Test boundary values
        assert_eq!(
            dim.get_offset(&CanonicalElementName::from_raw("1")),
            Some(0)
        );
        assert_eq!(
            dim.get_offset(&CanonicalElementName::from_raw("500")),
            Some(499)
        );
        assert_eq!(
            dim.get_offset(&CanonicalElementName::from_raw("1000")),
            Some(999)
        );

        // Test out of bounds
        assert_eq!(dim.get_offset(&CanonicalElementName::from_raw("0")), None);
        assert_eq!(
            dim.get_offset(&CanonicalElementName::from_raw("1001")),
            None
        );
    }

    #[test]
    fn test_dimension_name_and_len() {
        // Test name() and len() methods work correctly with canonical types
        let datamodel_dim = datamodel::Dimension::Named(
            "Test Dimension".to_string(),
            vec!["A".to_string(), "B".to_string(), "C".to_string()],
        );
        let dim = Dimension::from(datamodel_dim);

        // Name should be canonicalized
        assert_eq!(dim.name(), "test_dimension");
        assert_eq!(dim.len(), 3);

        // Test indexed dimension
        let datamodel_dim = datamodel::Dimension::Indexed("Index Dim".to_string(), 10);
        let dim = Dimension::from(datamodel_dim);

        assert_eq!(dim.name(), "index_dim");
        assert_eq!(dim.len(), 10);
    }

    #[test]
    fn test_dimensions_context_lookup() {
        // Test the DimensionsContext lookup method which uses get_offset internally
        let dims = vec![datamodel::Dimension::Named(
            "Region".to_string(),
            vec!["North".to_string(), "South".to_string()],
        )];

        let ctx = DimensionsContext::from(&dims);

        // Test element lookup with dimension·element notation
        assert_eq!(ctx.lookup("region·north"), Some(1)); // 1-based in context
        assert_eq!(ctx.lookup("Region·South"), Some(2)); // Should canonicalize
        assert_eq!(ctx.lookup("REGION·NORTH"), Some(1)); // Case insensitive

        // Test invalid lookups
        assert_eq!(ctx.lookup("region·west"), None);
        assert_eq!(ctx.lookup("invalid·north"), None);
        assert_eq!(ctx.lookup("no_dot"), None);
    }

    #[test]
    fn test_element_set_populated() {
        let datamodel_dim = datamodel::Dimension::Named(
            "Region".to_string(),
            vec!["North".to_string(), "South".to_string(), "East".to_string()],
        );
        let dim = Dimension::from(datamodel_dim);

        if let Dimension::Named(_, named) = &dim {
            assert_eq!(named.element_set.len(), 3);
            assert!(
                named
                    .element_set
                    .contains(&CanonicalElementName::from_raw("north"))
            );
            assert!(
                named
                    .element_set
                    .contains(&CanonicalElementName::from_raw("south"))
            );
            assert!(
                named
                    .element_set
                    .contains(&CanonicalElementName::from_raw("east"))
            );
            assert!(
                !named
                    .element_set
                    .contains(&CanonicalElementName::from_raw("west"))
            );
        } else {
            panic!("Expected Named dimension");
        }
    }

    #[test]
    fn test_subdimension_relation_contiguous() {
        // Contiguous offsets: A2, A3 (indices 1, 2) from A1, A2, A3
        let relation = super::SubdimensionRelation {
            parent_offsets: vec![1, 2],
        };
        assert!(relation.is_contiguous());
        assert_eq!(relation.start_offset(), 1);
    }

    #[test]
    fn test_subdimension_relation_non_contiguous() {
        // Non-contiguous offsets: A1, A3 (indices 0, 2) from A1, A2, A3
        let relation = super::SubdimensionRelation {
            parent_offsets: vec![0, 2],
        };
        assert!(!relation.is_contiguous());
        assert_eq!(relation.start_offset(), 0);
    }

    #[test]
    fn test_subdimension_relation_single_element() {
        // Single element is always contiguous
        let relation = super::SubdimensionRelation {
            parent_offsets: vec![1],
        };
        assert!(relation.is_contiguous());
        assert_eq!(relation.start_offset(), 1);
    }

    #[test]
    fn test_subdimension_relation_empty() {
        // Empty is contiguous by definition
        let relation = super::SubdimensionRelation {
            parent_offsets: vec![],
        };
        assert!(relation.is_contiguous());
        assert_eq!(relation.start_offset(), 0);
    }

    #[test]
    fn test_subdimension_relation_three_elements_contiguous() {
        let relation = super::SubdimensionRelation {
            parent_offsets: vec![2, 3, 4],
        };
        assert!(relation.is_contiguous());
        assert_eq!(relation.start_offset(), 2);
    }

    #[test]
    fn test_subdimension_relation_three_elements_non_contiguous() {
        // Gap in the middle
        let relation = super::SubdimensionRelation {
            parent_offsets: vec![0, 1, 4],
        };
        assert!(!relation.is_contiguous());
    }

    #[test]
    fn test_relationship_cache_basic() {
        use crate::common::CanonicalDimensionName;

        let cache = super::RelationshipCache::default();

        let parent = CanonicalDimensionName::from_raw("DimA");
        let child = CanonicalDimensionName::from_raw("SubA");

        // Initially cache is empty
        assert!(cache.cache.borrow().is_empty());

        // Insert a subdimension relation
        let relation = super::SubdimensionRelation {
            parent_offsets: vec![1, 2],
        };
        cache
            .cache
            .borrow_mut()
            .insert((child.clone(), parent.clone()), Some(relation.clone()));

        // Verify we can retrieve it
        let retrieved = cache
            .cache
            .borrow()
            .get(&(child.clone(), parent.clone()))
            .cloned();
        assert_eq!(retrieved, Some(Some(relation)));

        // Insert a negative result (not a subdimension)
        let other_child = CanonicalDimensionName::from_raw("NotSubA");
        cache
            .cache
            .borrow_mut()
            .insert((other_child.clone(), parent.clone()), None);

        // Verify negative result is cached
        let retrieved = cache
            .cache
            .borrow()
            .get(&(other_child.clone(), parent.clone()))
            .cloned();
        assert_eq!(retrieved, Some(None));
    }

    #[test]
    fn test_relationship_cache_clone() {
        use crate::common::CanonicalDimensionName;

        let cache = super::RelationshipCache::default();
        let parent = CanonicalDimensionName::from_raw("DimA");
        let child = CanonicalDimensionName::from_raw("SubA");

        let relation = super::SubdimensionRelation {
            parent_offsets: vec![0, 2],
        };
        cache
            .cache
            .borrow_mut()
            .insert((child.clone(), parent.clone()), Some(relation));

        // Clone the cache
        let cloned_cache = cache.clone();

        // Verify cloned cache has the same content
        assert!(cloned_cache.cache.borrow().contains_key(&(child, parent)));
    }
}
