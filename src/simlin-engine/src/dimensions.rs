// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::common::{CanonicalDimensionName, CanonicalElementName};
use crate::datamodel;

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct NamedDimension {
    pub elements: Vec<CanonicalElementName>,
    pub indexed_elements: HashMap<CanonicalElementName, usize>,
    /// If this dimension maps to another (e.g., DimA -> DimB), the target dimension name.
    /// Elements correspond positionally: elements[i] of this dimension corresponds to
    /// elements[i] of the target dimension.
    pub maps_to: Option<CanonicalDimensionName>,
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

/// Cache for subdimension relationships. Uses Mutex for thread-safe O(1) lookup after first computation.
/// The cache key is (child_name, parent_name), and the value is the relation if child is
/// a subdimension of parent, or None if we've determined it's not a subdimension.
#[allow(dead_code)]
#[derive(Debug, Default)]
struct RelationshipCache {
    cache: Mutex<
        HashMap<(CanonicalDimensionName, CanonicalDimensionName), Option<SubdimensionRelation>>,
    >,
}

impl Clone for RelationshipCache {
    fn clone(&self) -> Self {
        RelationshipCache {
            cache: Mutex::new(self.cache.lock().unwrap().clone()),
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

    /// Get the canonical dimension name
    pub fn canonical_name(&self) -> &CanonicalDimensionName {
        match self {
            Dimension::Indexed(name, _) | Dimension::Named(name, _) => name,
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

impl From<&datamodel::Dimension> for Dimension {
    fn from(dim: &datamodel::Dimension) -> Dimension {
        let maps_to = dim
            .maps_to
            .as_ref()
            .map(|m| CanonicalDimensionName::from_raw(m));
        match &dim.elements {
            datamodel::DimensionElements::Indexed(size) => {
                Dimension::Indexed(CanonicalDimensionName::from_raw(&dim.name), *size)
            }
            datamodel::DimensionElements::Named(elements) => {
                let canonical_elements: Vec<CanonicalElementName> = elements
                    .iter()
                    .map(|e| CanonicalElementName::from_raw(e))
                    .collect();
                let indexed_elements: HashMap<CanonicalElementName, usize> = canonical_elements
                    .iter()
                    .enumerate()
                    // system dynamic indexes are 1-indexed
                    .map(|(i, elem): (usize, &CanonicalElementName)| (elem.clone(), i + 1))
                    .collect();
                Dimension::Named(
                    CanonicalDimensionName::from_raw(&dim.name),
                    NamedDimension {
                        indexed_elements,
                        elements: canonical_elements,
                        maps_to,
                    },
                )
            }
        }
    }
}

impl From<datamodel::Dimension> for Dimension {
    fn from(dim: datamodel::Dimension) -> Dimension {
        Dimension::from(&dim)
    }
}

#[derive(Clone, Debug, Default)]
pub struct DimensionsContext {
    dimensions: HashMap<CanonicalDimensionName, Dimension>,
    relationship_cache: RelationshipCache,
}

// Manual PartialEq implementation that ignores the cache (caches don't affect equality)
impl PartialEq for DimensionsContext {
    fn eq(&self, other: &Self) -> bool {
        self.dimensions == other.dimensions
    }
}

impl DimensionsContext {
    pub(crate) fn from(dimensions: &[datamodel::Dimension]) -> DimensionsContext {
        // Validate: indexed dimensions should not have maps_to set.
        // Dimension mappings only make sense for named dimensions where we can
        // establish positional correspondence between element names.
        for dim in dimensions {
            if let datamodel::DimensionElements::Indexed(_) = &dim.elements
                && dim.maps_to.is_some()
            {
                eprintln!(
                    "warning: indexed dimension '{}' has maps_to='{}' which will be ignored; \
                     dimension mappings are only supported for named dimensions",
                    dim.name(),
                    dim.maps_to.as_ref().unwrap()
                );
            }
        }

        DimensionsContext {
            dimensions: dimensions
                .iter()
                .map(|dim| {
                    (
                        CanonicalDimensionName::from_raw(dim.name()),
                        Dimension::from(dim),
                    )
                })
                .collect(),
            relationship_cache: RelationshipCache::default(),
        }
    }

    /// Get a dimension by its canonical name
    #[allow(dead_code)]
    pub fn get(&self, name: &CanonicalDimensionName) -> Option<&Dimension> {
        self.dimensions.get(name)
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

    /// Get the maps_to target for a dimension (e.g., DimA -> DimB).
    /// Returns None for indexed dimensions or dimensions without a mapping.
    pub fn get_maps_to(
        &self,
        dim_name: &CanonicalDimensionName,
    ) -> Option<&CanonicalDimensionName> {
        if let Some(Dimension::Named(_, named)) = self.dimensions.get(dim_name) {
            named.maps_to.as_ref()
        } else {
            None
        }
    }

    /// Translate an element from a target context dimension to a source variable dimension
    /// using positional correspondence from dimension mapping.
    ///
    /// This is used when a variable indexed by source_dim is referenced from a context
    /// that has target_dim, and source_dim.maps_to == target_dim.
    ///
    /// For example, if DimA maps to DimB, and we have:
    /// - A variable indexed by DimA (source_dim)
    /// - A context with subscript "b3" from DimB (target_dim)
    /// - We need to find the corresponding DimA element: "a3"
    ///
    /// Returns the source dimension element that corresponds positionally to the
    /// target element, or None if:
    /// - No mapping relationship exists between source and target
    /// - Either dimension is indexed (not named)
    /// - The dimensions have different sizes (invalid mapping configuration)
    /// - The target element is not found in the target dimension
    pub fn translate_to_source_via_mapping(
        &self,
        source_dim: &CanonicalDimensionName,
        target_dim: &CanonicalDimensionName,
        target_element: &CanonicalElementName,
    ) -> Option<CanonicalElementName> {
        // Verify source maps to target
        if self.get_maps_to(source_dim) != Some(target_dim) {
            return None;
        }

        // Get source dimension first to validate it's named
        let source_named = match self.dimensions.get(source_dim)? {
            Dimension::Named(_, named) => named,
            Dimension::Indexed(_, _) => return None,
        };

        // Get the target dimension to find element's position
        let target_named = match self.dimensions.get(target_dim)? {
            Dimension::Named(_, named) => named,
            Dimension::Indexed(_, _) => return None,
        };

        // Validate dimensions have same size for valid positional mapping.
        // If sizes don't match, this is a configuration error in the model -
        // dimension mappings require 1:1 positional correspondence.
        if source_named.elements.len() != target_named.elements.len() {
            return None;
        }

        // Find position of target element (1-indexed in indexed_elements)
        let position = *target_named.indexed_elements.get(target_element)?;

        // Get element at same position in source (convert 1-indexed to 0-indexed)
        source_named.elements.get(position - 1).cloned()
    }

    /// Check if child is a subdimension of parent (all child elements exist in parent).
    /// Only Named dimensions can have subdimension relationships.
    #[allow(dead_code)]
    pub fn is_subdimension_of(
        &self,
        child: &CanonicalDimensionName,
        parent: &CanonicalDimensionName,
    ) -> bool {
        self.get_subdimension_relation(child, parent).is_some()
    }

    /// Get the subdimension relationship between child and parent dimensions.
    /// Returns Some(SubdimensionRelation) if child is a subdimension of parent,
    /// or None if it's not. Results are cached for O(1) lookup on subsequent calls.
    ///
    /// Note: Indexed dimension subdimensions are not currently supported.
    /// The datamodel lacks metadata to express which range of parent indices
    /// the child maps to. This returns None for indexed dimensions.
    #[allow(dead_code)]
    pub fn get_subdimension_relation(
        &self,
        child: &CanonicalDimensionName,
        parent: &CanonicalDimensionName,
    ) -> Option<SubdimensionRelation> {
        let cache_key = (child.clone(), parent.clone());

        // Check cache first (short lock scope)
        {
            let guard = self.relationship_cache.cache.lock().unwrap();
            if let Some(cached) = guard.get(&cache_key) {
                return cached.clone();
            }
        }

        // Compute outside the lock to avoid potential deadlock on nested calls
        let result = self.compute_subdimension_relation(child, parent);

        // Cache the result (short lock scope)
        self.relationship_cache
            .cache
            .lock()
            .unwrap()
            .insert(cache_key, result.clone());

        result
    }

    fn compute_subdimension_relation(
        &self,
        child: &CanonicalDimensionName,
        parent: &CanonicalDimensionName,
    ) -> Option<SubdimensionRelation> {
        let child_dim = self.dimensions.get(child)?;
        let parent_dim = self.dimensions.get(parent)?;

        match (child_dim, parent_dim) {
            (Dimension::Named(_, child_named), Dimension::Named(_, parent_named)) => {
                // Check all child elements exist in parent and build offset mapping
                let mut parent_offsets = Vec::with_capacity(child_named.elements.len());
                for child_elem in &child_named.elements {
                    match parent_named.indexed_elements.get(child_elem) {
                        Some(&idx) => parent_offsets.push(idx - 1), // 1-based to 0-based
                        None => return None,                        // Element not in parent
                    }
                }
                Some(SubdimensionRelation { parent_offsets })
            }
            (Dimension::Indexed(_, _), Dimension::Indexed(_, _)) => {
                // TODO: Indexed subdimensions deferred - datamodel lacks parent mapping metadata.
                // Would need to express which range of parent indices the child maps to.
                None
            }
            _ => None, // Mixed types cannot be subdimensions of each other
        }
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

    // ========== Tests for get_maps_to ==========

    #[test]
    fn test_get_maps_to_basic_mapping() {
        use crate::common::CanonicalDimensionName;

        // DimA maps to DimB
        let mut dim_a = datamodel::Dimension::named(
            "DimA".to_string(),
            vec!["A1".to_string(), "A2".to_string(), "A3".to_string()],
        );
        dim_a.maps_to = Some("DimB".to_string());

        let dim_b = datamodel::Dimension::named(
            "DimB".to_string(),
            vec!["B1".to_string(), "B2".to_string(), "B3".to_string()],
        );

        let dims = vec![dim_a, dim_b];
        let ctx = DimensionsContext::from(&dims);

        let dim_a_name = CanonicalDimensionName::from_raw("DimA");
        let dim_b_name = CanonicalDimensionName::from_raw("DimB");

        // DimA should map to DimB
        assert_eq!(ctx.get_maps_to(&dim_a_name), Some(&dim_b_name));

        // DimB should not have a mapping
        assert_eq!(ctx.get_maps_to(&dim_b_name), None);
    }

    #[test]
    fn test_get_maps_to_no_mapping() {
        use crate::common::CanonicalDimensionName;

        // Dimension without mapping
        let dim = datamodel::Dimension::named(
            "Region".to_string(),
            vec!["North".to_string(), "South".to_string()],
        );

        let ctx = DimensionsContext::from(&[dim]);
        let region_name = CanonicalDimensionName::from_raw("Region");

        assert_eq!(ctx.get_maps_to(&region_name), None);
    }

    #[test]
    fn test_get_maps_to_indexed_dimension_returns_none() {
        use crate::common::CanonicalDimensionName;

        // Indexed dimensions don't support maps_to
        let dim = datamodel::Dimension::indexed("Index".to_string(), 5);

        let ctx = DimensionsContext::from(&[dim]);
        let index_name = CanonicalDimensionName::from_raw("Index");

        // Indexed dimensions should return None for get_maps_to
        assert_eq!(ctx.get_maps_to(&index_name), None);
    }

    #[test]
    fn test_get_maps_to_unknown_dimension_returns_none() {
        use crate::common::CanonicalDimensionName;

        let dim = datamodel::Dimension::named(
            "Region".to_string(),
            vec!["North".to_string(), "South".to_string()],
        );

        let ctx = DimensionsContext::from(&[dim]);
        let unknown_name = CanonicalDimensionName::from_raw("Unknown");

        assert_eq!(ctx.get_maps_to(&unknown_name), None);
    }

    // ========== Tests for translate_to_source_via_mapping ==========

    #[test]
    fn test_translate_basic_dimension_mapping() {
        use crate::common::CanonicalDimensionName;

        // DimA maps to DimB: A1->B1, A2->B2, A3->B3 (positional correspondence)
        let mut dim_a = datamodel::Dimension::named(
            "DimA".to_string(),
            vec!["A1".to_string(), "A2".to_string(), "A3".to_string()],
        );
        dim_a.maps_to = Some("DimB".to_string());

        let dim_b = datamodel::Dimension::named(
            "DimB".to_string(),
            vec!["B1".to_string(), "B2".to_string(), "B3".to_string()],
        );

        let dims = vec![dim_a, dim_b];
        let ctx = DimensionsContext::from(&dims);

        let dim_a_name = CanonicalDimensionName::from_raw("DimA");
        let dim_b_name = CanonicalDimensionName::from_raw("DimB");

        // Translate B1 in DimB context to corresponding DimA element
        let b1 = CanonicalElementName::from_raw("B1");
        let result = ctx.translate_to_source_via_mapping(&dim_a_name, &dim_b_name, &b1);
        assert_eq!(result, Some(CanonicalElementName::from_raw("a1")));

        // Translate B2 in DimB context to corresponding DimA element
        let b2 = CanonicalElementName::from_raw("B2");
        let result = ctx.translate_to_source_via_mapping(&dim_a_name, &dim_b_name, &b2);
        assert_eq!(result, Some(CanonicalElementName::from_raw("a2")));

        // Translate B3 in DimB context to corresponding DimA element
        let b3 = CanonicalElementName::from_raw("B3");
        let result = ctx.translate_to_source_via_mapping(&dim_a_name, &dim_b_name, &b3);
        assert_eq!(result, Some(CanonicalElementName::from_raw("a3")));
    }

    #[test]
    fn test_translate_no_mapping_returns_none() {
        use crate::common::CanonicalDimensionName;

        // DimA and DimB have no mapping relationship
        let dim_a = datamodel::Dimension::named(
            "DimA".to_string(),
            vec!["A1".to_string(), "A2".to_string()],
        );

        let dim_b = datamodel::Dimension::named(
            "DimB".to_string(),
            vec!["B1".to_string(), "B2".to_string()],
        );

        let ctx = DimensionsContext::from(&[dim_a, dim_b]);

        let dim_a_name = CanonicalDimensionName::from_raw("DimA");
        let dim_b_name = CanonicalDimensionName::from_raw("DimB");

        // No mapping between DimA and DimB, should return None
        let b1 = CanonicalElementName::from_raw("B1");
        let result = ctx.translate_to_source_via_mapping(&dim_a_name, &dim_b_name, &b1);
        assert_eq!(result, None);
    }

    #[test]
    fn test_translate_invalid_element_returns_none() {
        use crate::common::CanonicalDimensionName;

        // DimA maps to DimB
        let mut dim_a = datamodel::Dimension::named(
            "DimA".to_string(),
            vec!["A1".to_string(), "A2".to_string()],
        );
        dim_a.maps_to = Some("DimB".to_string());

        let dim_b = datamodel::Dimension::named(
            "DimB".to_string(),
            vec!["B1".to_string(), "B2".to_string()],
        );

        let ctx = DimensionsContext::from(&[dim_a, dim_b]);

        let dim_a_name = CanonicalDimensionName::from_raw("DimA");
        let dim_b_name = CanonicalDimensionName::from_raw("DimB");

        // Invalid element (not in DimB), should return None
        let invalid = CanonicalElementName::from_raw("invalid");
        let result = ctx.translate_to_source_via_mapping(&dim_a_name, &dim_b_name, &invalid);
        assert_eq!(result, None);
    }

    #[test]
    fn test_translate_indexed_dimensions_returns_none() {
        use crate::common::CanonicalDimensionName;

        // Indexed dimensions can't use translate_to_source_via_mapping
        let dim_a = datamodel::Dimension::indexed("DimA".to_string(), 3);
        let dim_b = datamodel::Dimension::indexed("DimB".to_string(), 3);

        let ctx = DimensionsContext::from(&[dim_a, dim_b]);

        let dim_a_name = CanonicalDimensionName::from_raw("DimA");
        let dim_b_name = CanonicalDimensionName::from_raw("DimB");

        // Indexed dimensions should return None
        let elem = CanonicalElementName::from_raw("1");
        let result = ctx.translate_to_source_via_mapping(&dim_a_name, &dim_b_name, &elem);
        assert_eq!(result, None);
    }

    #[test]
    fn test_translate_maps_to_wrong_target_returns_none() {
        use crate::common::CanonicalDimensionName;

        // DimA maps to DimB, but we try to translate via DimC
        let mut dim_a = datamodel::Dimension::named(
            "DimA".to_string(),
            vec!["A1".to_string(), "A2".to_string()],
        );
        dim_a.maps_to = Some("DimB".to_string());

        let dim_b = datamodel::Dimension::named(
            "DimB".to_string(),
            vec!["B1".to_string(), "B2".to_string()],
        );

        let dim_c = datamodel::Dimension::named(
            "DimC".to_string(),
            vec!["C1".to_string(), "C2".to_string()],
        );

        let ctx = DimensionsContext::from(&[dim_a, dim_b, dim_c]);

        let dim_a_name = CanonicalDimensionName::from_raw("DimA");
        let dim_c_name = CanonicalDimensionName::from_raw("DimC");

        // DimA maps to DimB, not DimC, so translation via DimC should fail
        let c1 = CanonicalElementName::from_raw("C1");
        let result = ctx.translate_to_source_via_mapping(&dim_a_name, &dim_c_name, &c1);
        assert_eq!(result, None);
    }

    #[test]
    fn test_translate_mismatched_sizes_returns_none() {
        use crate::common::CanonicalDimensionName;

        // DimA has 2 elements, DimB has 3 - mismatched sizes should fail entirely
        // This is a configuration error: dimension mappings require 1:1 correspondence
        let mut dim_a = datamodel::Dimension::named(
            "DimA".to_string(),
            vec!["A1".to_string(), "A2".to_string()],
        );
        dim_a.maps_to = Some("DimB".to_string());

        let dim_b = datamodel::Dimension::named(
            "DimB".to_string(),
            vec!["B1".to_string(), "B2".to_string(), "B3".to_string()],
        );

        let ctx = DimensionsContext::from(&[dim_a, dim_b]);

        let dim_a_name = CanonicalDimensionName::from_raw("DimA");
        let dim_b_name = CanonicalDimensionName::from_raw("DimB");

        // All translations should fail due to size mismatch
        let b1 = CanonicalElementName::from_raw("B1");
        assert_eq!(
            ctx.translate_to_source_via_mapping(&dim_a_name, &dim_b_name, &b1),
            None,
            "Mismatched dimension sizes should fail gracefully"
        );

        let b2 = CanonicalElementName::from_raw("B2");
        assert_eq!(
            ctx.translate_to_source_via_mapping(&dim_a_name, &dim_b_name, &b2),
            None,
            "Mismatched dimension sizes should fail gracefully"
        );

        let b3 = CanonicalElementName::from_raw("B3");
        assert_eq!(
            ctx.translate_to_source_via_mapping(&dim_a_name, &dim_b_name, &b3),
            None,
            "Mismatched dimension sizes should fail gracefully"
        );
    }

    // ========== Existing tests ==========

    #[test]
    fn test_get_offset_named_dimension() {
        // Create a named dimension with canonical elements
        let datamodel_dim = datamodel::Dimension::named(
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
        let datamodel_dim = datamodel::Dimension::indexed("Index".to_string(), 5);
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
        let datamodel_dim = datamodel::Dimension::named(
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
        let datamodel_dim = datamodel::Dimension::named("Empty".to_string(), vec![]);
        let dim = Dimension::from(datamodel_dim);

        assert_eq!(
            dim.get_offset(&CanonicalElementName::from_raw("anything")),
            None
        );

        // Edge case: indexed dimension with size 0
        let datamodel_dim = datamodel::Dimension::indexed("Zero".to_string(), 0);
        let dim = Dimension::from(datamodel_dim);

        assert_eq!(dim.get_offset(&CanonicalElementName::from_raw("1")), None);
        assert_eq!(dim.get_offset(&CanonicalElementName::from_raw("0")), None);
    }

    #[test]
    fn test_get_offset_large_indexed_dimension() {
        // Test with a larger indexed dimension
        let datamodel_dim = datamodel::Dimension::indexed("Large".to_string(), 1000);
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
        let datamodel_dim = datamodel::Dimension::named(
            "Test Dimension".to_string(),
            vec!["A".to_string(), "B".to_string(), "C".to_string()],
        );
        let dim = Dimension::from(datamodel_dim);

        // Name should be canonicalized
        assert_eq!(dim.name(), "test_dimension");
        assert_eq!(dim.len(), 3);

        // Test indexed dimension
        let datamodel_dim = datamodel::Dimension::indexed("Index Dim".to_string(), 10);
        let dim = Dimension::from(datamodel_dim);

        assert_eq!(dim.name(), "index_dim");
        assert_eq!(dim.len(), 10);
    }

    #[test]
    fn test_dimensions_context_lookup() {
        // Test the DimensionsContext lookup method which uses get_offset internally
        let dims = vec![datamodel::Dimension::named(
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
        assert!(cache.cache.lock().unwrap().is_empty());

        // Insert a subdimension relation
        let relation = super::SubdimensionRelation {
            parent_offsets: vec![1, 2],
        };
        cache
            .cache
            .lock()
            .unwrap()
            .insert((child.clone(), parent.clone()), Some(relation.clone()));

        // Verify we can retrieve it
        let retrieved = cache
            .cache
            .lock()
            .unwrap()
            .get(&(child.clone(), parent.clone()))
            .cloned();
        assert_eq!(retrieved, Some(Some(relation)));

        // Insert a negative result (not a subdimension)
        let other_child = CanonicalDimensionName::from_raw("NotSubA");
        cache
            .cache
            .lock()
            .unwrap()
            .insert((other_child.clone(), parent.clone()), None);

        // Verify negative result is cached
        let retrieved = cache
            .cache
            .lock()
            .unwrap()
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
            .lock()
            .unwrap()
            .insert((child.clone(), parent.clone()), Some(relation));

        // Clone the cache
        let cloned_cache = cache.clone();

        // Verify cloned cache has the same content
        assert!(
            cloned_cache
                .cache
                .lock()
                .unwrap()
                .contains_key(&(child, parent))
        );
    }

    #[test]
    fn test_subdimension_contiguous() {
        use crate::common::CanonicalDimensionName;

        // DimA = [A1, A2, A3], SubA = [A2, A3] (contiguous subdimension)
        let dims = vec![
            datamodel::Dimension::named(
                "DimA".to_string(),
                vec!["A1".to_string(), "A2".to_string(), "A3".to_string()],
            ),
            datamodel::Dimension::named(
                "SubA".to_string(),
                vec!["A2".to_string(), "A3".to_string()],
            ),
        ];

        let ctx = DimensionsContext::from(&dims);
        let dim_a = CanonicalDimensionName::from_raw("DimA");
        let sub_a = CanonicalDimensionName::from_raw("SubA");

        // SubA should be a subdimension of DimA
        assert!(ctx.is_subdimension_of(&sub_a, &dim_a));

        let relation = ctx.get_subdimension_relation(&sub_a, &dim_a).unwrap();
        assert_eq!(relation.parent_offsets, vec![1, 2]); // A2 is at index 1, A3 is at index 2
        assert!(relation.is_contiguous());
        assert_eq!(relation.start_offset(), 1);
    }

    #[test]
    fn test_subdimension_non_contiguous() {
        use crate::common::CanonicalDimensionName;

        // DimA = [A1, A2, A3], SubA = [A1, A3] (non-contiguous subdimension)
        let dims = vec![
            datamodel::Dimension::named(
                "DimA".to_string(),
                vec!["A1".to_string(), "A2".to_string(), "A3".to_string()],
            ),
            datamodel::Dimension::named(
                "SubA".to_string(),
                vec!["A1".to_string(), "A3".to_string()],
            ),
        ];

        let ctx = DimensionsContext::from(&dims);
        let dim_a = CanonicalDimensionName::from_raw("DimA");
        let sub_a = CanonicalDimensionName::from_raw("SubA");

        assert!(ctx.is_subdimension_of(&sub_a, &dim_a));

        let relation = ctx.get_subdimension_relation(&sub_a, &dim_a).unwrap();
        assert_eq!(relation.parent_offsets, vec![0, 2]); // A1 is at index 0, A3 is at index 2
        assert!(!relation.is_contiguous());
    }

    #[test]
    fn test_subdimension_single_element() {
        use crate::common::CanonicalDimensionName;

        // DimA = [A1, A2, A3], SubA = [A2] (single element subdimension)
        let dims = vec![
            datamodel::Dimension::named(
                "DimA".to_string(),
                vec!["A1".to_string(), "A2".to_string(), "A3".to_string()],
            ),
            datamodel::Dimension::named("SubA".to_string(), vec!["A2".to_string()]),
        ];

        let ctx = DimensionsContext::from(&dims);
        let dim_a = CanonicalDimensionName::from_raw("DimA");
        let sub_a = CanonicalDimensionName::from_raw("SubA");

        assert!(ctx.is_subdimension_of(&sub_a, &dim_a));

        let relation = ctx.get_subdimension_relation(&sub_a, &dim_a).unwrap();
        assert_eq!(relation.parent_offsets, vec![1]);
        assert!(relation.is_contiguous());
    }

    #[test]
    fn test_not_subdimension() {
        use crate::common::CanonicalDimensionName;

        // DimA = [A1, A2], DimB = [B1, B2] (no overlap)
        let dims = vec![
            datamodel::Dimension::named(
                "DimA".to_string(),
                vec!["A1".to_string(), "A2".to_string()],
            ),
            datamodel::Dimension::named(
                "DimB".to_string(),
                vec!["B1".to_string(), "B2".to_string()],
            ),
        ];

        let ctx = DimensionsContext::from(&dims);
        let dim_a = CanonicalDimensionName::from_raw("DimA");
        let dim_b = CanonicalDimensionName::from_raw("DimB");

        assert!(!ctx.is_subdimension_of(&dim_b, &dim_a));
        assert!(ctx.get_subdimension_relation(&dim_b, &dim_a).is_none());
    }

    #[test]
    fn test_subdimension_cache() {
        use crate::common::CanonicalDimensionName;

        let dims = vec![
            datamodel::Dimension::named(
                "DimA".to_string(),
                vec!["A1".to_string(), "A2".to_string(), "A3".to_string()],
            ),
            datamodel::Dimension::named(
                "SubA".to_string(),
                vec!["A2".to_string(), "A3".to_string()],
            ),
        ];

        let ctx = DimensionsContext::from(&dims);
        let dim_a = CanonicalDimensionName::from_raw("DimA");
        let sub_a = CanonicalDimensionName::from_raw("SubA");

        // First call computes and caches
        let relation1 = ctx.get_subdimension_relation(&sub_a, &dim_a);
        assert!(relation1.is_some());

        // Second call should return cached result
        let relation2 = ctx.get_subdimension_relation(&sub_a, &dim_a);
        assert_eq!(relation1, relation2);

        // Verify cache was populated
        let cache = ctx.relationship_cache.cache.lock().unwrap();
        assert!(cache.contains_key(&(sub_a.clone(), dim_a.clone())));
    }

    #[test]
    fn test_indexed_subdimension_not_supported() {
        use crate::common::CanonicalDimensionName;

        // Indexed dimensions don't support subdimension relationships yet
        let dims = vec![
            datamodel::Dimension::indexed("DimA".to_string(), 5),
            datamodel::Dimension::indexed("SubA".to_string(), 3),
        ];

        let ctx = DimensionsContext::from(&dims);
        let dim_a = CanonicalDimensionName::from_raw("DimA");
        let sub_a = CanonicalDimensionName::from_raw("SubA");

        // Should return None because indexed subdimensions aren't supported
        assert!(!ctx.is_subdimension_of(&sub_a, &dim_a));
        assert!(ctx.get_subdimension_relation(&sub_a, &dim_a).is_none());
    }

    #[test]
    fn test_mixed_dimension_types() {
        use crate::common::CanonicalDimensionName;

        // Named and Indexed dimensions can't be subdimensions of each other
        let dims = vec![
            datamodel::Dimension::named(
                "DimA".to_string(),
                vec!["A1".to_string(), "A2".to_string()],
            ),
            datamodel::Dimension::indexed("DimB".to_string(), 2),
        ];

        let ctx = DimensionsContext::from(&dims);
        let dim_a = CanonicalDimensionName::from_raw("DimA");
        let dim_b = CanonicalDimensionName::from_raw("DimB");

        assert!(!ctx.is_subdimension_of(&dim_b, &dim_a));
        assert!(!ctx.is_subdimension_of(&dim_a, &dim_b));
    }

    #[test]
    fn test_dimension_get() {
        use crate::common::CanonicalDimensionName;

        let dims = vec![datamodel::Dimension::named(
            "Region".to_string(),
            vec!["North".to_string(), "South".to_string()],
        )];

        let ctx = DimensionsContext::from(&dims);

        let region = CanonicalDimensionName::from_raw("Region");
        assert!(ctx.get(&region).is_some());
        assert_eq!(ctx.get(&region).unwrap().len(), 2);

        let unknown = CanonicalDimensionName::from_raw("Unknown");
        assert!(ctx.get(&unknown).is_none());
    }

    #[test]
    fn test_indexed_dimension_with_maps_to_is_ignored() {
        // Indexed dimensions should not have maps_to - this test verifies
        // that if one is erroneously provided, it's ignored and doesn't
        // affect the dimension context behavior.
        use crate::common::CanonicalDimensionName;

        // Create an indexed dimension with maps_to set (invalid configuration)
        let mut indexed_dim = datamodel::Dimension::indexed("IndexedDim".to_string(), 3);
        indexed_dim.maps_to = Some("TargetDim".to_string());

        // Also create the target dimension
        let target_dim = datamodel::Dimension::named(
            "TargetDim".to_string(),
            vec!["A".to_string(), "B".to_string(), "C".to_string()],
        );

        // This will print a warning to stderr, but we can verify the maps_to is ignored
        let ctx = DimensionsContext::from(&[indexed_dim, target_dim]);

        // Verify the indexed dimension exists but has no mapping
        let dim_name = CanonicalDimensionName::from_raw("IndexedDim");
        let target_name = CanonicalDimensionName::from_raw("TargetDim");

        // get_maps_to should return None for the indexed dimension
        // (since the Dimension::Indexed variant doesn't store maps_to)
        assert!(ctx.get_maps_to(&dim_name).is_none());

        // translate_to_source_via_mapping should return None (no mapping exists)
        assert!(
            ctx.translate_to_source_via_mapping(
                &dim_name,
                &target_name,
                &CanonicalElementName::from_raw("A"),
            )
            .is_none()
        );

        // The dimension should still function correctly for offset lookups
        let dim = ctx.get(&dim_name).unwrap();
        assert_eq!(
            dim.get_offset(&CanonicalElementName::from_raw("1")),
            Some(0)
        );
        assert_eq!(
            dim.get_offset(&CanonicalElementName::from_raw("3")),
            Some(2)
        );
    }
}
