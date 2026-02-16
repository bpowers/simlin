// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Dimension building methods for MDL to datamodel conversion.

use crate::datamodel::{Dimension, DimensionElements};

use crate::mdl::ast::{Equation as MdlEquation, MdlItem, SubscriptElement};
use crate::mdl::xmile_compat::space_to_underbar;

use super::ConversionContext;
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
                let alias = Dimension {
                    name: space_to_underbar(src),
                    elements: target_dim.elements.clone(),
                    maps_to: Some(dst.clone()),
                };
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

        // Get mapping info from raw subscript def
        let maps_to = self.get_dimension_mapping(original_name);

        Ok(Dimension {
            name: space_to_underbar(original_name),
            elements: DimensionElements::Named(elements),
            maps_to,
        })
    }

    /// Get the maps_to target for a dimension from its SubscriptDef.
    fn get_dimension_mapping(&self, name: &str) -> Option<String> {
        let name_canonical = canonical_name(name);
        for item in &self.items {
            if let MdlItem::Equation(eq) = item
                && let MdlEquation::SubscriptDef(def_name, def) = &eq.equation
                && eq_lower_space(def_name, &name_canonical)
            {
                return def.mapping.as_ref().and_then(|m| {
                    if m.entries.len() != 1 {
                        return None;
                    }
                    match &m.entries[0] {
                        crate::mdl::ast::MappingEntry::Name(n, _) => Some(canonical_name(n)),
                        crate::mdl::ast::MappingEntry::DimensionMapping { dimension, .. } => {
                            Some(canonical_name(dimension))
                        }
                        _ => None,
                    }
                });
            }
        }
        None
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
            project.dimensions.iter().any(|d| d.name == "dima"),
            "Should have DimA dimension (alias)"
        );

        // DimA should map to DimB
        let dim_a = project.dimensions.iter().find(|d| d.name == "dima");
        if let Some(dim) = dim_a {
            assert_eq!(
                dim.maps_to,
                Some("dimb".to_string()),
                "DimA should map to dimb"
            );
        }
    }

    #[test]
    fn test_explicit_dimension_mapping() {
        // Explicit mapping `DimA: a1, a2, a3 -> (DimB: b1, b2, b3)` should extract DimB as maps_to
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

        // DimA should exist with maps_to = dimb
        let dim_a = project.dimensions.iter().find(|d| d.name == "DimA");
        assert!(dim_a.is_some(), "Should have DimA dimension");
        if let Some(dim) = dim_a {
            assert_eq!(
                dim.maps_to,
                Some("dimb".to_string()),
                "DimA should map to dimb via explicit mapping"
            );
        }
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
                crate::datamodel::Equation::Arrayed(dims, elements) => {
                    assert_eq!(dims, &["dima"]);
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
                crate::datamodel::Equation::Arrayed(dims, elements) => {
                    // Dimension should be dima (from the first equation)
                    assert_eq!(dims, &["dima"]);
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
}
