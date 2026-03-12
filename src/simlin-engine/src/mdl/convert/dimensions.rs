// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Dimension building methods for MDL to datamodel conversion.

use crate::datamodel::{self, Dimension, DimensionElements};

use crate::mdl::ast::{Equation as MdlEquation, MdlItem, SubscriptElement};
use crate::mdl::xmile_compat::space_to_underbar;

use super::ConversionContext;
use super::external_data;
use super::helpers::{canonical_name, expand_range};
use super::types::ConvertError;
use crate::mdl::builtins::eq_lower_space;

impl<'input> ConversionContext<'input> {
    /// Pass 2: Build dimensions from subscript definitions.
    pub(super) fn build_dimensions(&mut self) -> Result<(), ConvertError> {
        // Phase 1a: Collect ALL raw subscript definitions (before expansion)
        for item in &self.items {
            if let MdlItem::Equation(eq) = item
                && let MdlEquation::SubscriptDef(name, def) = &eq.equation
            {
                let canonical = canonical_name(name);
                self.raw_subscript_defs
                    .insert(canonical, def.elements.clone());
            }
        }

        // Phase 1b: Collect equivalences
        for item in &self.items {
            if let MdlItem::Equation(eq) = item
                && let MdlEquation::Equivalence(src, dst, _) = &eq.equation
            {
                let src_canonical = canonical_name(src);
                let dst_canonical = canonical_name(dst);
                self.equivalence_original_names
                    .insert(src_canonical.clone(), src.to_string());
                self.equivalences.insert(src_canonical, dst_canonical);
            }
        }

        // Phase 2: Build dimensions using recursive expansion
        // We need to collect the subscript def names first to avoid borrowing issues
        let subscript_names: Vec<(String, String)> = self
            .items
            .iter()
            .filter_map(|item| {
                if let MdlItem::Equation(eq) = item
                    && let MdlEquation::SubscriptDef(name, _) = &eq.equation
                {
                    Some((name.to_string(), canonical_name(name)))
                } else {
                    None
                }
            })
            .collect();

        for (original_name, canonical) in subscript_names {
            let dim = self.build_dimension_recursive(&original_name, &canonical)?;
            self.dimensions.push(dim);
        }

        // Phase 3: build dimension_elements map
        for dim in &self.dimensions {
            if let DimensionElements::Named(elements) = &dim.elements {
                let dim_canonical = canonical_name(&dim.name);
                self.dimension_elements
                    .insert(dim_canonical, elements.clone());
            }
        }

        // Phase 4: establish element ownership (larger dimension owns elements)
        let mut dims_by_size: Vec<_> = self
            .dimension_elements
            .iter()
            .map(|(name, elems)| (name.clone(), elems.clone()))
            .collect();
        dims_by_size.sort_by(|a, b| b.1.len().cmp(&a.1.len()));

        for (dim_name, elements) in dims_by_size {
            for elem in elements {
                let elem_canonical = canonical_name(&elem);
                self.element_owners
                    .entry(elem_canonical)
                    .or_insert_with(|| dim_name.clone());
            }
        }

        // Phase 5: materialize equivalence dimensions as actual Dimension entries
        // and add them to dimension_elements for expand_subscript to find them
        for (src, dst) in &self.equivalences {
            if let Some(target_dim) = self
                .dimensions
                .iter()
                .find(|d| eq_lower_space(&d.name, dst))
            {
                let original_name = self
                    .equivalence_original_names
                    .get(src)
                    .map(|s| s.as_str())
                    .unwrap_or(src);
                let mut alias = Dimension {
                    name: space_to_underbar(original_name),
                    elements: target_dim.elements.clone(),
                    mappings: vec![],
                };
                alias.set_maps_to(dst.clone());
                // Add alias to dimension_elements so expand_subscript can find it
                if let DimensionElements::Named(elements) = &alias.elements {
                    self.dimension_elements
                        .insert(src.clone(), elements.clone());
                }
                self.dimensions.push(alias);
            }
        }

        Ok(())
    }

    /// Build a single dimension with recursive element expansion.
    /// If an element name matches a dimension name, expand to that dimension's elements.
    fn build_dimension_recursive(
        &self,
        original_name: &str,
        canonical: &str,
    ) -> Result<Dimension, ConvertError> {
        let mut visited = std::collections::HashSet::new();
        let elements = self.expand_subscript_elements(canonical, &mut visited)?;

        let mappings = self.get_dimension_mappings(original_name, &elements);

        Ok(Dimension {
            name: space_to_underbar(original_name),
            elements: DimensionElements::Named(elements),
            mappings,
        })
    }

    /// Build dimension mappings from the SubscriptDef mapping clause.
    ///
    /// Handles:
    /// - Simple positional: `DimA -> DimB` (empty element_map)
    /// - Multiple targets: `DimA -> DimB, DimC` (one mapping per target)
    /// - Element reordering: `DimA -> (DimB: B3, B2, B1)` (element_map with pairs)
    /// - Subdimension mapping: `DimB -> (DimA: SubA, A3)` (expands subdimension to elements)
    fn get_dimension_mappings(
        &self,
        name: &str,
        source_elements: &[String],
    ) -> Vec<datamodel::DimensionMapping> {
        let name_canonical = canonical_name(name);
        for item in &self.items {
            if let MdlItem::Equation(eq) = item
                && let MdlEquation::SubscriptDef(def_name, def) = &eq.equation
                && eq_lower_space(def_name, &name_canonical)
                && let Some(m) = &def.mapping
            {
                return self.convert_mapping_entries(source_elements, &m.entries);
            }
        }
        vec![]
    }

    /// Convert MDL mapping entries to datamodel DimensionMappings.
    fn convert_mapping_entries(
        &self,
        source_elements: &[String],
        entries: &[crate::mdl::ast::MappingEntry<'_>],
    ) -> Vec<datamodel::DimensionMapping> {
        let mut result = Vec::new();
        for entry in entries {
            match entry {
                crate::mdl::ast::MappingEntry::Name(n, _) => {
                    // Simple positional mapping: `-> DimB`
                    result.push(datamodel::DimensionMapping {
                        target: canonical_name(n),
                        element_map: vec![],
                    });
                }
                crate::mdl::ast::MappingEntry::DimensionMapping {
                    dimension,
                    elements,
                    ..
                } => {
                    let target_name = canonical_name(dimension);
                    let element_map =
                        self.build_element_map(source_elements, elements, &target_name);
                    result.push(datamodel::DimensionMapping {
                        target: target_name,
                        element_map,
                    });
                }
                _ => {}
            }
        }
        result
    }

    /// Build element-level mapping from source elements to target dimension elements.
    ///
    /// Each entry in `target_elements` can be either a specific element name or
    /// a subdimension name (which expands to multiple elements). The source
    /// elements are matched positionally: the first source element maps to the
    /// first target entry, and so on. When a target entry is a subdimension,
    /// one source element maps to all elements in that subdimension.
    fn build_element_map(
        &self,
        source_elements: &[String],
        target_elements: &[crate::mdl::ast::Subscript<'_>],
        _target_dim_name: &str,
    ) -> Vec<(String, String)> {
        let mut element_map = Vec::new();
        for (source_idx, target_sub) in target_elements.iter().enumerate() {
            if source_idx >= source_elements.len() {
                break;
            }
            let source_canonical = canonical_name(&source_elements[source_idx]);
            if let crate::mdl::ast::Subscript::Element(target_name, _) = target_sub {
                let target_canonical = canonical_name(target_name);
                // Check if target_name is a subdimension by looking in raw_subscript_defs
                // (dimension_elements is not populated yet during build_dimension_recursive)
                if self.raw_subscript_defs.contains_key(&target_canonical) {
                    let mut visited = std::collections::HashSet::new();
                    if let Ok(sub_elements) =
                        self.expand_subscript_elements(&target_canonical, &mut visited)
                    {
                        for sub_elem in &sub_elements {
                            element_map.push((source_canonical.clone(), canonical_name(sub_elem)));
                        }
                    }
                } else {
                    element_map.push((source_canonical.clone(), target_canonical));
                }
            }
        }
        element_map
    }

    /// Recursively expand subscript elements.
    /// If an element name matches a dimension, expand to that dimension's elements.
    fn expand_subscript_elements(
        &self,
        dim_canonical: &str,
        visited: &mut std::collections::HashSet<String>,
    ) -> Result<Vec<String>, ConvertError> {
        // Cycle detection
        if visited.contains(dim_canonical) {
            return Err(ConvertError::CyclicDimensionDefinition(
                dim_canonical.to_string(),
            ));
        }

        let elements = match self.raw_subscript_defs.get(dim_canonical) {
            Some(elems) => elems.clone(),
            None => return Ok(vec![]), // Unknown dimension
        };

        visited.insert(dim_canonical.to_string());

        let mut result = Vec::new();
        for elem in &elements {
            match elem {
                SubscriptElement::Element(e, _) => {
                    // Check if this element is a GET DIRECT SUBSCRIPT reference
                    if external_data::is_get_direct_ref(e) {
                        let resolved = self.resolve_subscript_from_data(e)?;
                        result.extend(resolved);
                        continue;
                    }

                    let elem_canonical = canonical_name(e);
                    // Check if this element is actually a dimension reference
                    if self.raw_subscript_defs.contains_key(&elem_canonical) {
                        // Recursively expand
                        let expanded = self.expand_subscript_elements(&elem_canonical, visited)?;
                        result.extend(expanded);
                    } else {
                        // Plain element
                        result.push(space_to_underbar(e));
                    }
                }
                SubscriptElement::Range(start, end, _) => {
                    let expanded = expand_range(start, end)?;
                    result.extend(expanded);
                }
            }
        }

        visited.remove(dim_canonical);
        Ok(result)
    }

    /// Resolve a GET DIRECT SUBSCRIPT reference to a list of element names.
    fn resolve_subscript_from_data(&self, opaque_str: &str) -> Result<Vec<String>, ConvertError> {
        let call = external_data::parse_get_direct(opaque_str).ok_or_else(|| {
            ConvertError::Other(format!(
                "failed to parse GET DIRECT SUBSCRIPT reference: {}",
                opaque_str
            ))
        })?;

        let provider = self.data_provider.ok_or_else(|| {
            ConvertError::Other(
                "GET DIRECT SUBSCRIPT referenced but no DataProvider configured".to_string(),
            )
        })?;

        match external_data::resolve_get_direct(&call, provider, &self.file_aliases) {
            Ok(external_data::ResolvedData::Subscript(elements)) => Ok(elements
                .into_iter()
                .map(|e| space_to_underbar(&e))
                .collect()),
            Ok(_) => Err(ConvertError::Other(
                "expected GET DIRECT SUBSCRIPT but got a different GET DIRECT type".to_string(),
            )),
            Err(e) => Err(ConvertError::Other(format!(
                "failed to resolve GET DIRECT SUBSCRIPT: {}",
                e.details.unwrap_or_default()
            ))),
        }
    }

    /// Resolve an element subscript to its owning dimension.
    /// Returns the canonical dimension name if found.
    #[allow(dead_code)]
    pub(super) fn resolve_element_to_dimension(&self, element: &str) -> Option<&str> {
        self.element_owners
            .get(&canonical_name(element))
            .map(|s| s.as_str())
    }

    /// Get the elements of a dimension, or just the element itself if it's already an element.
    /// Returns (formatted_dimension_name, list_of_elements).
    pub(super) fn expand_subscript(&self, sub_name: &str) -> Option<(String, Vec<String>)> {
        let canonical = canonical_name(sub_name);

        // If it's an element, return just that element and its owning dimension (formatted)
        if let Some(dim_canonical) = self.element_owners.get(&canonical) {
            // Look up the original dimension name for proper casing
            let formatted_dim = self.get_formatted_dimension_name(dim_canonical);
            return Some((formatted_dim, vec![space_to_underbar(sub_name)]));
        }

        // If it's a dimension, return all its elements
        if self.dimension_elements.contains_key(&canonical) {
            // Look up the original dimension name for proper casing
            let formatted_dim = self.get_formatted_dimension_name(&canonical);
            let elements = self.dimension_elements.get(&canonical)?;
            return Some((formatted_dim, elements.clone()));
        }

        None
    }

    /// Find the 0-based index of an element within a dimension's element list.
    /// Used to compute offsets for arrayed GET DIRECT cell reference adjustment.
    pub(super) fn element_index_in_dimension(
        &self,
        element: &str,
        dim_formatted: &str,
    ) -> Option<usize> {
        let dim_canonical = canonical_name(dim_formatted);
        let elem_canonical = canonical_name(element);
        let elements = self.dimension_elements.get(&dim_canonical)?;
        elements
            .iter()
            .position(|e| canonical_name(e) == elem_canonical)
    }

    /// Get the formatted dimension name (space_to_underbar) from a canonical name.
    pub(super) fn get_formatted_dimension_name(&self, canonical: &str) -> String {
        // Find the original dimension name and format it
        for dim in &self.dimensions {
            if eq_lower_space(&dim.name, canonical) {
                return space_to_underbar(&dim.name);
            }
        }
        // Fallback to canonical if not found
        space_to_underbar(canonical)
    }

    /// Normalize a dimension name through equivalences and subrange resolution.
    /// Used for consistency checks (comparing dimension names from different equations).
    /// For example, if DimA <-> DimB, normalize_dimension("dima") returns "dimb".
    /// If "upper" is a subrange of "layers", normalize_dimension("upper") returns "layers".
    pub(super) fn normalize_dimension(&self, dim: &str) -> String {
        let canonical = canonical_name(dim);
        // Follow equivalence chain to target
        if let Some(equiv) = self.equivalences.get(&canonical) {
            return equiv.clone();
        }
        self.resolve_subrange_to_parent(&canonical)
    }

    /// Resolve a subrange dimension to its parent dimension.
    /// Does NOT follow equivalence aliases -- only resolves subranges.
    /// Used for formatting dimension names in output (where alias dimensions
    /// should keep their own name, but subranges should use the parent).
    pub(super) fn resolve_subrange_to_parent(&self, canonical: &str) -> String {
        // Skip equivalence aliases -- they should keep their own name
        if self.equivalences.contains_key(canonical) {
            return canonical.to_string();
        }
        if let Some(elements) = self.dimension_elements.get(canonical)
            && let Some(first_element) = elements.first()
            && let Some(owner) = self.element_owners.get(&canonical_name(first_element))
            && *owner != canonical
        {
            // Verify ALL elements share the same owner
            let all_same_owner = elements.iter().all(|e| {
                self.element_owners
                    .get(&canonical_name(e))
                    .map(|o| o == owner)
                    .unwrap_or(false)
            });
            if all_same_owner {
                return owner.clone();
            }
        }
        canonical.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::super::convert_mdl;
    use super::*;
    use crate::datamodel::Variable;

    #[test]
    fn test_element_ownership_larger_dimension_wins() {
        // When an element appears in multiple dimensions, the larger dimension
        // should own it (matching xmutil's ownership assignment)
        let mdl = "DimLarge: a, b, c, d
~ ~|
DimSmall: a, b
~ ~|
x = 1
~ ~|
\\\\\\---///
";
        let ctx = ConversionContext::new(mdl).unwrap();
        let mut ctx = ctx;
        ctx.collect_symbols();
        ctx.build_dimensions().unwrap();

        // Element 'a' should be owned by DimLarge (larger dimension)
        assert_eq!(
            ctx.resolve_element_to_dimension("a"),
            Some("dimlarge"),
            "Element 'a' should be owned by the larger dimension"
        );
        assert_eq!(
            ctx.resolve_element_to_dimension("b"),
            Some("dimlarge"),
            "Element 'b' should be owned by the larger dimension"
        );
        // Elements only in DimLarge should still be owned by DimLarge
        assert_eq!(
            ctx.resolve_element_to_dimension("c"),
            Some("dimlarge"),
            "Element 'c' should be owned by DimLarge"
        );
    }

    #[test]
    fn test_equivalence_creates_dimension_alias() {
        // Equivalence `DimA <-> DimB` should create DimA as an alias for DimB
        let mdl = "DimB: x1, x2, x3
~ ~|
DimA <-> DimB
~ ~|
y = 1
~ ~|
\\\\\\---///
";
        let result = convert_mdl(mdl);
        assert!(result.is_ok(), "Conversion should succeed: {:?}", result);
        let project = result.unwrap();

        // Should have both DimA and DimB dimensions
        assert!(
            project.dimensions.iter().any(|d| d.name == "DimB"),
            "Should have DimB dimension"
        );
        assert!(
            project.dimensions.iter().any(|d| d.name == "DimA"),
            "Should have DimA dimension (alias) with preserved casing"
        );

        // DimA should map to DimB
        let dim_a = project.dimensions.iter().find(|d| d.name == "DimA");
        if let Some(dim) = dim_a {
            assert_eq!(dim.maps_to(), Some("dimb"), "DimA should map to dimb");
        }
    }

    #[test]
    fn test_explicit_dimension_mapping() {
        // Explicit mapping `DimA: a1, a2, a3 -> (DimB: b1, b2, b3)` should create
        // a mapping to DimB with element-level correspondence.
        let mdl = "DimB: b1, b2, b3
~ ~|
DimA: a1, a2, a3 -> (DimB: b1, b2, b3)
~ ~|
x = 1
~ ~|
\\\\\\---///
";
        let result = convert_mdl(mdl);
        assert!(result.is_ok(), "Conversion should succeed: {:?}", result);
        let project = result.unwrap();

        let dim_a = project
            .dimensions
            .iter()
            .find(|d| d.name == "DimA")
            .expect("Should have DimA dimension");
        assert_eq!(dim_a.mappings.len(), 1, "Should have one mapping");
        assert_eq!(dim_a.mappings[0].target, "dimb");
        assert_eq!(
            dim_a.mappings[0].element_map,
            vec![
                ("a1".to_string(), "b1".to_string()),
                ("a2".to_string(), "b2".to_string()),
                ("a3".to_string(), "b3".to_string()),
            ]
        );
    }

    #[test]
    fn test_nested_dimension_definition_expands() {
        // DimA: DimB should expand to DimB's elements
        let mdl = "DimB: x1, x2, x3
~ ~|
DimA: DimB
~ ~|
\\\\\\---///
";
        let project = convert_mdl(mdl).unwrap();

        let dim_a = project
            .dimensions
            .iter()
            .find(|d| d.name == "DimA")
            .unwrap();
        if let DimensionElements::Named(elements) = &dim_a.elements {
            assert_eq!(
                elements,
                &["x1", "x2", "x3"],
                "DimA should have DimB's elements"
            );
        } else {
            panic!("Expected Named elements");
        }
    }

    #[test]
    fn test_chained_dimension_expansion() {
        // DimA: DimB, DimB: DimC - should recursively expand
        let mdl = "DimC: c1, c2
~ ~|
DimB: DimC
~ ~|
DimA: DimB
~ ~|
\\\\\\---///
";
        let project = convert_mdl(mdl).unwrap();

        let dim_a = project
            .dimensions
            .iter()
            .find(|d| d.name == "DimA")
            .unwrap();
        if let DimensionElements::Named(elements) = &dim_a.elements {
            assert_eq!(
                elements,
                &["c1", "c2"],
                "DimA should recursively expand through DimB to DimC"
            );
        } else {
            panic!("Expected Named elements");
        }
    }

    #[test]
    fn test_forward_reference_dimension_expansion() {
        // DimA: DimB defined BEFORE DimB: x1, x2 - forward reference
        let mdl = "DimA: DimB
~ ~|
DimB: x1, x2
~ ~|
\\\\\\---///
";
        let project = convert_mdl(mdl).unwrap();

        let dim_a = project
            .dimensions
            .iter()
            .find(|d| d.name == "DimA")
            .unwrap();
        if let DimensionElements::Named(elements) = &dim_a.elements {
            assert_eq!(elements, &["x1", "x2"], "Forward reference should resolve");
        } else {
            panic!("Expected Named elements");
        }
    }

    #[test]
    fn test_cyclic_dimension_definition_errors() {
        // DimA: DimB, DimB: DimA - cycle should error, not infinite loop
        let mdl = "DimA: DimB
~ ~|
DimB: DimA
~ ~|
\\\\\\---///
";
        let result = convert_mdl(mdl);
        assert!(result.is_err(), "Cyclic dimension definition should error");
    }

    #[test]
    fn test_mixed_elements_and_dimension_reference() {
        // DimA: x1, DimB, x4 where DimB: x2, x3 - should be [x1, x2, x3, x4]
        let mdl = "DimB: x2, x3
~ ~|
DimA: x1, DimB, x4
~ ~|
\\\\\\---///
";
        let project = convert_mdl(mdl).unwrap();

        let dim_a = project
            .dimensions
            .iter()
            .find(|d| d.name == "DimA")
            .unwrap();
        if let DimensionElements::Named(elements) = &dim_a.elements {
            assert_eq!(elements, &["x1", "x2", "x3", "x4"], "Should expand inline");
        } else {
            panic!("Expected Named elements");
        }
    }

    #[test]
    fn test_alias_dimension_recognized_in_expand_subscript() {
        // DimA <-> DimB, then x[DimA] should work
        let mdl = "DimB: b1, b2
~ ~|
DimA <-> DimB
~ ~|
x[DimA] = 1
~ Units ~|
\\\\\\---///
";
        let project = convert_mdl(mdl).unwrap();

        let x = project.models[0]
            .variables
            .iter()
            .find(|v| v.get_ident() == "x")
            .expect("Should have x variable");

        // Should be recognized and processed correctly
        if let Variable::Aux(a) = x {
            match &a.equation {
                crate::datamodel::Equation::Arrayed(dims, elements, _default_eq, _) => {
                    assert_eq!(dims, &["DimA"]);
                    assert_eq!(elements.len(), 2);
                }
                other => panic!("Expected Arrayed (dimension expansion), got {:?}", other),
            }
        } else {
            panic!("Expected Aux variable");
        }
    }

    #[test]
    fn test_alias_dimension_with_element_override() {
        // DimA <-> DimB, x[DimA]=1, x[b1]=2 should work
        // Note: b1 is owned by DimB, but DimA is an alias, so should be compatible
        let mdl = "DimB: b1, b2
~ ~|
DimA <-> DimB
~ ~|
x[DimA] = 1
~ Units ~|
x[b1] = 2
~ Units ~|
\\\\\\---///
";
        let project = convert_mdl(mdl).unwrap();

        let x = project.models[0]
            .variables
            .iter()
            .find(|v| v.get_ident() == "x")
            .expect("Should have x variable");

        if let Variable::Aux(a) = x {
            match &a.equation {
                crate::datamodel::Equation::Arrayed(dims, elements, _default_eq, _) => {
                    assert_eq!(dims, &["DimA"]);
                    assert_eq!(elements.len(), 2);

                    let b1_eq = elements.iter().find(|(k, _, _, _)| k == "b1");
                    let b2_eq = elements.iter().find(|(k, _, _, _)| k == "b2");

                    assert_eq!(b1_eq.unwrap().1, "2", "b1 should be overridden to 2");
                    assert_eq!(b2_eq.unwrap().1, "1", "b2 should have default 1");
                }
                other => panic!("Expected Arrayed, got {:?}", other),
            }
        } else {
            panic!("Expected Aux variable");
        }
    }

    #[test]
    fn test_normalize_dimension_subrange_to_parent() {
        // Subranges should normalize to their parent dimension
        let mdl = "layers: layer1, layer2, layer3, layer4
~ ~|
upper: layer1, layer2, layer3
~ ~|
bottom: layer4
~ ~|
x = 1
~ ~|
\\\\\\---///
";
        let mut ctx = ConversionContext::new(mdl).unwrap();
        ctx.collect_symbols();
        ctx.build_dimensions().unwrap();

        assert_eq!(
            ctx.normalize_dimension("upper"),
            "layers",
            "Subrange 'upper' should normalize to parent 'layers'"
        );
        assert_eq!(
            ctx.normalize_dimension("bottom"),
            "layers",
            "Subrange 'bottom' should normalize to parent 'layers'"
        );
        assert_eq!(
            ctx.normalize_dimension("layers"),
            "layers",
            "Parent dimension should remain unchanged"
        );
    }

    #[test]
    fn test_normalize_dimension_non_subrange_unchanged() {
        // A top-level dimension that is not a subrange should remain unchanged
        let mdl = "scenario: s1, s2
~ ~|
x = 1
~ ~|
\\\\\\---///
";
        let mut ctx = ConversionContext::new(mdl).unwrap();
        ctx.collect_symbols();
        ctx.build_dimensions().unwrap();

        assert_eq!(
            ctx.normalize_dimension("scenario"),
            "scenario",
            "Top-level dimension should remain unchanged"
        );
    }

    #[test]
    fn test_equivalence_preserves_original_casing() {
        // Equivalence `CamelCase <-> other` should produce a dimension named
        // "CamelCase", not "camelcase".
        let mdl = "TargetDim: x1, x2
~ ~|
MyCamelCase <-> TargetDim
~ ~|
\\\\\\---///
";
        let project = convert_mdl(mdl).unwrap();

        let alias = project
            .dimensions
            .iter()
            .find(|d| d.maps_to() == Some("targetdim"));
        assert!(alias.is_some(), "Should have alias dimension");
        assert_eq!(
            alias.unwrap().name,
            "MyCamelCase",
            "Alias dimension should preserve original casing"
        );
    }

    #[test]
    fn test_subdimension_mapping() {
        // DimB: B1, B2 -> (DimA: SubA, A3) where SubA: A1, A2
        // Should produce element_map: (b1, a1), (b1, a2), (b2, a3)
        let mdl = "{UTF-8}
DimA: A1, A2, A3 ~~|
DimB: B1, B2 -> (DimA: SubA, A3) ~~|
SubA: A1, A2 ~~|
b[DimB] = 1, 2 ~~|
INITIAL TIME = 0 ~~|
FINAL TIME = 1 ~~|
SAVEPER = 1 ~~|
TIME STEP = 1 ~~|
";
        let project = crate::compat::open_vensim(mdl).unwrap();
        let dim_b = project
            .dimensions
            .iter()
            .find(|d| d.name == "DimB")
            .expect("DimB should exist");

        assert_eq!(dim_b.mappings.len(), 1, "DimB should have 1 mapping");
        assert_eq!(dim_b.mappings[0].target, "dima");
        assert_eq!(
            dim_b.mappings[0].element_map,
            vec![
                ("b1".to_string(), "a1".to_string()),
                ("b1".to_string(), "a2".to_string()),
                ("b2".to_string(), "a3".to_string()),
            ],
            "B1 should map to SubA elements (A1, A2), B2 should map to A3"
        );
    }

    #[test]
    fn test_multimap_dimension_mappings() {
        let mdl = "{UTF-8}
DimA: A1, A2, A3 -> (DimB: B3, B2, B1), DimC ~~|
DimB: B1, B2, B3 ~~|
DimC: C1, C2, C3 ~~|
a[DimA] = 1, 2, 3 ~~|
INITIAL TIME = 0 ~~|
FINAL TIME = 1 ~~|
SAVEPER = 1 ~~|
TIME STEP = 1 ~~|
";
        let project = crate::compat::open_vensim(mdl).unwrap();
        let dim_a = project
            .dimensions
            .iter()
            .find(|d| d.name == "DimA")
            .expect("DimA should exist");

        assert_eq!(
            dim_a.mappings.len(),
            2,
            "DimA should have 2 mappings (DimB and DimC)"
        );

        // First mapping: DimA -> (DimB: B3, B2, B1) with element reordering
        assert_eq!(dim_a.mappings[0].target, "dimb");
        assert_eq!(
            dim_a.mappings[0].element_map,
            vec![
                ("a1".to_string(), "b3".to_string()),
                ("a2".to_string(), "b2".to_string()),
                ("a3".to_string(), "b1".to_string()),
            ]
        );

        // Second mapping: DimA -> DimC (positional)
        assert_eq!(dim_a.mappings[1].target, "dimc");
        assert!(
            dim_a.mappings[1].element_map.is_empty(),
            "Positional mapping should have empty element_map"
        );
    }
}
