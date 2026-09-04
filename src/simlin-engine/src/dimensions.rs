// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::sync::Mutex;

use smallvec::SmallVec;

use crate::common::{Canonical, CanonicalDimensionName, CanonicalElementName, Ident, IdentMap};
use crate::datamodel;

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub struct NamedDimension {
    pub elements: Vec<CanonicalElementName>,
    pub indexed_elements: IdentMap<CanonicalElementName, usize>,
    /// If this dimension maps to another (e.g., DimA -> DimB), the target dimension name.
    /// Elements correspond positionally: elements[i] of this dimension corresponds to
    /// elements[i] of the target dimension.
    pub maps_to: Option<CanonicalDimensionName>,
    /// All dimension mappings including element-level correspondence.
    /// Empty for dimensions with no mappings.
    pub mappings: Vec<DimensionMappingInfo>,
}

/// Element-level dimension mapping info.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub struct DimensionMappingInfo {
    pub target: CanonicalDimensionName,
    /// Source element -> target element pairs. When empty, positional mapping is assumed.
    pub element_map: Vec<(CanonicalElementName, CanonicalElementName)>,
}

impl NamedDimension {
    /// The 0-based index of the element an ALREADY-CANONICAL identifier names,
    /// by O(1) hash lookup.
    ///
    /// Typed on the caller's side rather than trusting a `&str`: an
    /// `Ident<Canonical>` cannot need canonicalizing, so this is the whole
    /// lookup -- no case folding, no allocation. Probing by `&str`
    /// (`CanonicalElementName: Borrow<str>`) rather than building a
    /// `CanonicalElementName` is deliberate too: a lookup has no reason to take
    /// a shard of the global interner and install an `Arc` payload for a name
    /// it is only asking about.
    pub fn index_of(&self, element: &Ident<Canonical>) -> Option<usize> {
        self.indexed_elements
            .get(element.as_str())
            .map(|&idx| idx - 1) // Convert from 1-based to 0-based
    }

    /// [`Self::index_of`] for a name that is NOT known to be canonical yet: it
    /// case-folds first. A caller already holding an `Ident<Canonical>` should
    /// call `index_of` and skip that work.
    pub fn get_element_index(&self, element: &str) -> Option<usize> {
        self.indexed_elements
            .get(crate::common::canonicalize(element).as_ref())
            .map(|&idx| idx - 1) // Convert from 1-based to 0-based
    }
}

/// Relationship between a subdimension and parent dimension.
/// Maps each subdim element index to its offset in the parent.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub struct SubdimensionRelation {
    /// Maps subdim element index -> parent offset (0-based).
    /// For SubA=[A2,A3] from DimA=[A1,A2,A3]: parent_offsets=[1,2]
    pub parent_offsets: Vec<usize>,
}

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
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Default)]
struct RelationshipCache {
    cache: Mutex<
        IdentMap<(CanonicalDimensionName, CanonicalDimensionName), Option<SubdimensionRelation>>,
    >,
}

impl Clone for RelationshipCache {
    fn clone(&self) -> Self {
        RelationshipCache {
            cache: Mutex::new(self.cache.lock().unwrap().clone()),
        }
    }
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub enum Dimension {
    Indexed(CanonicalDimensionName, u32),
    Named(CanonicalDimensionName, NamedDimension),
}

/// How a bare-identifier subscript index resolves against ONE axis of the
/// variable being subscripted -- see [`resolve_axis_index_name`].
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub enum AxisIndexName {
    /// The axis's own dimension declares this name as an ELEMENT. Carries the
    /// element's canonical name.
    Element(String),
    /// Not an element of this axis, but a dimension the enclosing equation
    /// ITERATES -- the apply-to-all placeholder form, which stands for whatever
    /// element the current iteration selects.
    IteratedDim,
    /// Neither of the two readings this function decides between. Three things
    /// land here and the caller tells them apart, because they are its business
    /// rather than this precedence rule's:
    ///
    /// * a NON-ACTIVE dimension name -- typically the source's own
    ///   (`x[Region]` under a `State`-iterating equation) -- which execution
    ///   pairs with an iterated dimension through a declared mapping and
    ///   resolves name-first, then through the element map (GH #997; see
    ///   [`DimensionsContext::mapped_read_partner_dim`], which
    ///   `ltm_agg::classify_axis_access` and
    ///   `ltm_augment_post_transform::pin_dimension_name_indices` both consult
    ///   from this arm);
    /// * a VARIABLE read selecting the element at runtime (`pop[Region, idx]`);
    /// * a name nothing in scope declares.
    ///
    /// Adding the first of those did not change what this function returns for
    /// any index: it is a third reading BELOW both of this one's, checked only
    /// after `Element` and `IteratedDim` have missed.
    Unresolved,
}

/// Resolve a bare-identifier subscript index against the axis it indexes: the
/// engine's SINGLE precedence rule for the element-vs-dimension-name question.
///
/// The axis's own declared ELEMENTS are tried first; only a name the axis does
/// not declare is read as a dimension the enclosing equation iterates. The two
/// readings collide when a dimension declares an element whose name is also a
/// dimension name -- `Category = [Region, x]` beside a `Region` dimension --
/// which XMILE permits, and the order is what decides which row a reference
/// like `effect[Region, Region]` reads.
///
/// **Element-first is the engine's executed behaviour**, and that is the
/// binding reason: `compiler::subscript::normalize_subscripts3`'s
/// `IndexExpr3::Expr` / `Expr3::Var` arm looks the name up in the axis's own
/// `indexed_elements` ("First check if it's a named dimension element (takes
/// priority)") and only then emits an `IndexOp::ActiveDimRef` for a dimension
/// name; its `IndexExpr3::Dimension` arm does the same. Anything that DESCRIBES
/// a reference -- an LTM read slice, an access shape, an element-graph edge --
/// must resolve it the way the simulation does, or it describes rows the
/// simulation never reads.
///
/// The XMILE spec (`docs/reference/xmile-v1.0.html`) points the same way but
/// does not settle this exact pair, and the difference is worth being precise
/// about. Footnote [9] settles the ADJACENT pair outright -- "if a variable name
/// is the same as an element name, the element name prevails (i.e., the variable
/// name is hidden within that subscript)" -- but that is variable-vs-element,
/// not dimension-name-vs-element. What bears on this pair is section 2.1's
/// namespace rule, "Dimension names, in turn, define their own Element
/// namespace ... Element names are resolved by context when they appear inside
/// square brackets of a variable", together with section 3.7.1's "Subscript
/// index names MAY be used unambiguously as part of a subscript (i.e., inside
/// the square brackets) ... once the dimensions assigned to the variable have
/// been specified". Inside brackets, at a position whose dimension is known, the
/// spec designates the index-name reading and calls it unambiguous; the
/// apply-to-all dimension-name form is introduced afterwards, as an additional
/// spelling, and cannot retroactively make the base one ambiguous. That is an
/// argument from the namespace rule, not a quotation of a rule for this pair.
///
/// `target_iterates` answers "does the enclosing equation iterate this
/// dimension?" -- the caller's own notion of scope (the LTM callers pass the
/// target equation's iterated dimensions).
pub fn resolve_axis_index_name(
    name: &str,
    axis_dim: &Dimension,
    target_iterates: impl FnOnce(&str) -> bool,
) -> AxisIndexName {
    if let Some(elem) = axis_dim.canonical_element(name) {
        return AxisIndexName::Element(elem);
    }
    if target_iterates(name) {
        return AxisIndexName::IteratedDim;
    }
    AxisIndexName::Unresolved
}

/// Resolve a CONSTANT subscript index against the axis it indexes: the
/// positional companion to [`resolve_axis_index_name`], and the engine's single
/// statement of what a constified index selects.
///
/// A constant index is POSITIONAL. It reaches this point in two spellings and
/// both mean the same thing:
///
/// * a numeric literal the modeller wrote (`pop[2]`), and
/// * a QUALIFIED `dimension·element` reference, which
///   [`DimensionsContext::lookup`] turns into that element's 1-based index
///   *within the dimension it names* and `Expr1::constify_dimensions` rewrites
///   into an `Expr1::Const`. This is the form the LTM equation generator
///   writes for an element it pins (`ltm_augment`).
///
/// In both cases `compiler::subscript::normalize_subscripts3` lowers the
/// constant to `IndexOp::Single(value - 1)` -- a raw offset into the
/// SUBSCRIPTED variable's own axis, with no reference back to the dimension the
/// qualified name mentioned. So `stock[region·north]`, where `stock` is declared
/// over `Other`, reads `Other`'s element at `north`'s position in `Region`, NOT
/// the element of `Other` that happens to be named `north`. Where the two
/// dimensions list the same names in different orders those readings differ, and
/// the positional one is what runs -- pinned end-to-end against the VM by
/// `db::ltm_element_instance_tests::qualified_index_edge_is_positional_not_by_name`.
///
/// A describer that resolved this by name would name rows the simulation never
/// reads, which is the failure `resolve_axis_index_name`'s docs above call out.
///
/// Returns the canonical element name at that position, or `None` when the
/// position is past the axis's end -- there is no element to name, so the caller
/// stays conservative.
///
/// Everything else mirrors `normalize_subscripts3`'s own arithmetic rather than
/// re-validating it, because the job is to describe the slot the simulation
/// reads, malformed index included: the conversion is the same saturating
/// `as isize` (so a NaN position becomes 0, exactly as the VM's does), and a
/// position below 1 clamps to the first element via the same `.max(0)`. A model
/// writing `pop[0]` or `pop[nan]` compiles today and reads element 1, so that is
/// the edge this reports. An infinite position saturates past every axis and
/// falls out through the bounds check above.
pub fn resolve_axis_index_position(position: f64, axis_dim: &Dimension) -> Option<String> {
    let idx = (position as isize - 1).max(0) as usize;
    if idx >= axis_dim.len() {
        return None;
    }
    match axis_dim {
        Dimension::Named(_, named) => named.elements.get(idx).map(|e| e.as_str().to_string()),
        // An indexed dimension's element names are `"1".."N"`, matching
        // `ltm_augment::dimension_element_names`.
        Dimension::Indexed(_, _) => Some((idx + 1).to_string()),
    }
}

impl Dimension {
    pub fn len(&self) -> usize {
        match self {
            Dimension::Indexed(_, size) => *size as usize,
            Dimension::Named(_, named) => named.elements.len(),
        }
    }

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

    /// The canonical name of the element `name` selects on this axis, or `None`
    /// when the axis declares no such element.
    ///
    /// For a NAMED dimension that is the element name itself; for an INDEXED one
    /// it is the 1-based position re-formatted (`"01"` -> `"1"`), so the answer
    /// always matches the names `ltm_augment::dimension_element_names` produces
    /// for the same axis. Membership is the same test `get_offset` performs and
    /// the same one `compiler::subscript` resolves an index against.
    pub fn canonical_element(&self, name: &str) -> Option<String> {
        match self {
            Dimension::Named(_, named) => named
                .indexed_elements
                .get_key_value(&CanonicalElementName::from_raw(name))
                .map(|(elem, _)| elem.as_str().to_string()),
            Dimension::Indexed(_, size) => name
                .parse::<u32>()
                .ok()
                .filter(|n| *n >= 1 && n <= size)
                .map(|n| n.to_string()),
        }
    }

    /// The element at 0-based `offset`, the inverse of [`Self::get_offset`].
    ///
    /// `None` past the end. An INDEXED dimension's elements are their 1-based
    /// positions spelled as numerals, which is the spelling `get_offset` reads
    /// back and the one `canonical_element` normalizes to.
    pub fn element_name(&self, offset: usize) -> Option<CanonicalElementName> {
        match self {
            Dimension::Named(_, named) => named.elements.get(offset).cloned(),
            Dimension::Indexed(_, size) => (offset < *size as usize)
                .then(|| CanonicalElementName::from_raw(&(offset + 1).to_string())),
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
        let maps_to = dim.maps_to().map(CanonicalDimensionName::from_raw);
        match &dim.elements {
            datamodel::DimensionElements::Indexed(size) => {
                Dimension::Indexed(CanonicalDimensionName::from_raw(&dim.name), *size)
            }
            datamodel::DimensionElements::Named(elements) => {
                let canonical_elements: Vec<CanonicalElementName> = elements
                    .iter()
                    .map(|e| CanonicalElementName::from_raw(e))
                    .collect();
                let indexed_elements: IdentMap<CanonicalElementName, usize> = canonical_elements
                    .iter()
                    .enumerate()
                    // system dynamic indexes are 1-indexed
                    .map(|(i, elem): (usize, &CanonicalElementName)| (elem.clone(), i + 1))
                    .collect();
                let mappings = dim
                    .mappings
                    .iter()
                    .map(|m| DimensionMappingInfo {
                        target: CanonicalDimensionName::from_raw(&m.target),
                        element_map: m
                            .element_map
                            .iter()
                            .map(|(s, t)| {
                                (
                                    CanonicalElementName::from_raw(s),
                                    CanonicalElementName::from_raw(t),
                                )
                            })
                            .collect(),
                    })
                    .collect();
                Dimension::Named(
                    CanonicalDimensionName::from_raw(&dim.name),
                    NamedDimension {
                        indexed_elements,
                        elements: canonical_elements,
                        maps_to,
                        mappings,
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

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Default)]
pub struct DimensionsContext {
    /// The dimension and parent maps are `Arc`-shared so cloning a context is
    /// a refcount bump rather than a deep copy of every dimension's element
    /// map. Per-variable fragment compilation clones the project context into
    /// each compiled `Module` (tens of thousands of times on large LTM
    /// builds, GH #655); both maps are immutable after construction, so
    /// sharing is safe. The `relationship_cache` memo stays per-instance
    /// (cloned cold), which only costs re-deriving subdimension relations on
    /// first use.
    dimensions: std::sync::Arc<IdentMap<CanonicalDimensionName, Dimension>>,
    /// For indexed subdimensions, maps child dimension name to its declared parent.
    indexed_parents: std::sync::Arc<IdentMap<CanonicalDimensionName, CanonicalDimensionName>>,
    relationship_cache: RelationshipCache,
}

// A context's identity is its `dimensions` + `indexed_parents` maps; the
// `relationship_cache` is pure memoization and must not affect equality.
// Salsa backdates a re-executed query's memo by `PartialEq`, so this is what
// lets `db::project_dimensions_context` (a `returns(ref)` query) early-cutoff
// even when one side's cache has been populated.
impl PartialEq for DimensionsContext {
    fn eq(&self, other: &Self) -> bool {
        self.dimensions == other.dimensions && self.indexed_parents == other.indexed_parents
    }
}

impl DimensionsContext {
    pub(crate) fn from(dimensions: &[datamodel::Dimension]) -> DimensionsContext {
        // Validate: indexed dimensions should not have mappings set.
        // Dimension mappings only make sense for named dimensions where we can
        // establish positional correspondence between element names.
        for dim in dimensions {
            if let datamodel::DimensionElements::Indexed(_) = &dim.elements
                && dim.maps_to().is_some()
            {
                eprintln!(
                    "warning: indexed dimension '{}' has maps_to='{}' which will be ignored; \
                     dimension mappings are only supported for named dimensions",
                    dim.name(),
                    dim.maps_to().unwrap()
                );
            }
        }

        let indexed_parents: IdentMap<CanonicalDimensionName, CanonicalDimensionName> = dimensions
            .iter()
            .filter_map(|dim| {
                dim.parent.as_ref().map(|parent_name| {
                    (
                        CanonicalDimensionName::from_raw(dim.name()),
                        CanonicalDimensionName::from_raw(parent_name),
                    )
                })
            })
            .collect();

        DimensionsContext {
            dimensions: std::sync::Arc::new(
                dimensions
                    .iter()
                    .map(|dim| {
                        (
                            CanonicalDimensionName::from_raw(dim.name()),
                            Dimension::from(dim),
                        )
                    })
                    .collect(),
            ),
            indexed_parents: std::sync::Arc::new(indexed_parents),
            relationship_cache: RelationshipCache::default(),
        }
    }

    /// Get a dimension by its canonical name
    pub fn get(&self, name: &CanonicalDimensionName) -> Option<&Dimension> {
        self.dimensions.get(name)
    }

    /// Get a dimension by a name that may not be in canonical form yet.
    ///
    /// The `&str` probe (via `CanonicalDimensionName: Borrow<str>`) is the
    /// point: a lookup must not intern. `CanonicalDimensionName::from_raw`
    /// takes a shard of the global interner and installs an `Arc` payload for
    /// a name that may not even be a dimension, and every miss leaves a
    /// refcount round trip behind -- on the parse path that is per dimension
    /// reference per variable.
    pub(crate) fn get_by_raw_name(&self, name: &str) -> Option<&Dimension> {
        self.dimensions
            .get(crate::common::canonicalize(name).as_ref())
    }

    pub(crate) fn is_dimension_name(&self, name: &str) -> bool {
        self.dimensions
            .contains_key(crate::common::canonicalize(name).as_ref())
    }

    pub(crate) fn lookup(&self, element: &str) -> Option<u32> {
        if let Some(pos) = element.find('·') {
            let dimension_name = crate::common::canonicalize(&element[..pos]);
            let element_name = crate::common::canonicalize(&element[pos + '·'.len_utf8()..]);
            if let Some(Dimension::Named(_, dimension)) =
                self.dimensions.get(dimension_name.as_ref())
                && let Some(off) = dimension.indexed_elements.get(element_name.as_ref())
            {
                return Some(*off as u32);
            }
        }
        None
    }

    /// A named dimension `element` can be unambiguously qualified against, or
    /// `None` if its index is ambiguous (or no dimension declares it).
    ///
    /// Used to decide whether a bare identifier in subscript position can be
    /// qualified to a `dimension·element` form. XMILE allows element names to
    /// overlap across dimensions (and to shadow variable names). The qualified
    /// form resolves to the element's 1-based index *within the qualifying
    /// dimension* (a plain integer constant by the time subscripts are
    /// normalized), so qualification is sound whenever every dimension that
    /// declares `element` agrees on that index -- the common case of a model
    /// declaring several same-shaped region/category dimensions. If the
    /// declaring dimensions disagree on the index, the element is genuinely
    /// ambiguous and `None` is returned.
    ///
    /// The returned dimension is the lexicographically smallest matching name,
    /// so generated equation text is deterministic (HashMap iteration order is
    /// not).
    /// Whether `element` is declared by at least one *named* dimension.
    ///
    /// Unlike [`Self::dimension_uniquely_containing_element`], this makes no
    /// uniqueness or index-agreement demand: it answers only "could this name
    /// be a dimension element at all?". Callers that know the *subscripted
    /// variable's* declared dimensions use this as the cheap pre-check before
    /// relying on the compiler's own position-matched element resolution
    /// (which always prefers the element interpretation over a variable
    /// reference -- see `compiler::context`'s subscript lowering).
    pub(crate) fn is_element_of_any_dimension(&self, element: &CanonicalElementName) -> bool {
        self.dimensions.values().any(|dim| match dim {
            Dimension::Named(_, named) => named.indexed_elements.contains_key(element),
            Dimension::Indexed(_, _) => false,
        })
    }

    pub(crate) fn dimension_uniquely_containing_element(
        &self,
        element: &CanonicalElementName,
    ) -> Option<&CanonicalDimensionName> {
        let mut matches: Vec<(&CanonicalDimensionName, usize)> = self
            .dimensions
            .iter()
            .filter_map(|(dim_name, dim)| match dim {
                Dimension::Named(_, named) => named
                    .indexed_elements
                    .get(element)
                    .map(|&idx| (dim_name, idx)),
                Dimension::Indexed(_, _) => None,
            })
            .collect();
        let (&(_, first_idx), rest) = matches.split_first()?;
        if rest.iter().any(|&(_, idx)| idx != first_idx) {
            return None;
        }
        matches.sort_by_key(|&(name, _)| name.as_str());
        matches.first().map(|&(d, _)| d)
    }

    /// Get the maps_to target for a dimension (e.g., DimA -> DimB).
    /// Returns None for indexed dimensions or dimensions without a mapping.
    /// For dimensions with multiple mapping targets, returns the first one.
    pub fn get_maps_to(
        &self,
        dim_name: &CanonicalDimensionName,
    ) -> Option<&CanonicalDimensionName> {
        if let Some(Dimension::Named(_, named)) = self.dimensions.get(dim_name) {
            // Prefer the legacy maps_to field (set for simple single positional mappings)
            if named.maps_to.is_some() {
                return named.maps_to.as_ref();
            }
            // Fall back to the first mapping target
            named.mappings.first().map(|m| &m.target)
        } else {
            None
        }
    }

    /// Check if a dimension has any mapping to a specific target dimension.
    pub fn has_mapping_to(
        &self,
        dim_name: &CanonicalDimensionName,
        target: &CanonicalDimensionName,
    ) -> bool {
        if let Some(Dimension::Named(_, named)) = self.dimensions.get(dim_name) {
            if named.maps_to.as_ref() == Some(target) {
                return true;
            }
            named.mappings.iter().any(|m| &m.target == target)
        } else {
            false
        }
    }

    /// Check if a dimension maps to any target that is a parent of the given
    /// candidate dimension. Handles multi-target mappings correctly.
    pub fn has_mapping_to_parent_of(
        &self,
        dim_name: &CanonicalDimensionName,
        candidate: &CanonicalDimensionName,
    ) -> bool {
        if let Some(Dimension::Named(_, named)) = self.dimensions.get(dim_name) {
            if let Some(maps_to) = &named.maps_to
                && self.is_subdimension_of(candidate, maps_to)
            {
                return true;
            }
            for m in &named.mappings {
                if self.is_subdimension_of(candidate, &m.target) {
                    return true;
                }
            }
        }
        false
    }

    /// Find the specific mapping target of `dim_name` that is a parent of
    /// `candidate`. Unlike `has_mapping_to_parent_of` which only returns a
    /// bool, this returns the actual parent dimension name. Needed when
    /// `dim_name` maps to multiple targets and we must pick the right one.
    pub fn find_mapping_parent_of(
        &self,
        dim_name: &CanonicalDimensionName,
        candidate: &CanonicalDimensionName,
    ) -> Option<&CanonicalDimensionName> {
        if let Some(Dimension::Named(_, named)) = self.dimensions.get(dim_name) {
            if let Some(maps_to) = &named.maps_to
                && self.is_subdimension_of(candidate, maps_to)
            {
                return Some(maps_to);
            }
            for m in &named.mappings {
                if self.is_subdimension_of(candidate, &m.target) {
                    return Some(&m.target);
                }
            }
        }
        None
    }

    /// Get all mapping targets for a dimension.
    pub fn get_all_mapping_targets(
        &self,
        dim_name: &CanonicalDimensionName,
    ) -> Vec<&CanonicalDimensionName> {
        if let Some(Dimension::Named(_, named)) = self.dimensions.get(dim_name) {
            let mut targets = Vec::new();
            if let Some(maps_to) = &named.maps_to {
                targets.push(maps_to);
            }
            for m in &named.mappings {
                if !targets.contains(&&m.target) {
                    targets.push(&m.target);
                }
            }
            targets
        } else {
            Vec::new()
        }
    }

    /// Find the mapping info for a specific source -> target pair.
    fn find_mapping_info(
        &self,
        source_dim: &CanonicalDimensionName,
        target_dim: &CanonicalDimensionName,
    ) -> Option<&DimensionMappingInfo> {
        if let Some(Dimension::Named(_, named)) = self.dimensions.get(source_dim) {
            named.mappings.iter().find(|m| &m.target == target_dim)
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
        // Verify source has a mapping to target
        if !self.has_mapping_to(source_dim, target_dim) {
            return None;
        }

        // Check if there's an element-level mapping
        if let Some(mapping_info) = self.find_mapping_info(source_dim, target_dim)
            && !mapping_info.element_map.is_empty()
        {
            return mapping_info
                .element_map
                .iter()
                .find(|(_, t)| t == target_element)
                .map(|(s, _)| s.clone());
        }

        // Positional mapping fallback
        let source_named = match self.dimensions.get(source_dim)? {
            Dimension::Named(_, named) => named,
            Dimension::Indexed(_, _) => return None,
        };

        let target_named = match self.dimensions.get(target_dim)? {
            Dimension::Named(_, named) => named,
            Dimension::Indexed(_, _) => return None,
        };

        if source_named.elements.len() != target_named.elements.len() {
            return None;
        }

        // Find position of target element (1-indexed in indexed_elements)
        let position = *target_named.indexed_elements.get(target_element)?;

        // Get element at same position in source (convert 1-indexed to 0-indexed)
        source_named.elements.get(position - 1).cloned()
    }

    /// Translate an element using dimension mapping in either direction.
    ///
    /// Unlike `translate_to_source_via_mapping` which only handles the forward case
    /// (source.maps_to == target), this method handles both directions:
    /// - Forward: source_dim.maps_to == target_dim
    /// - Reverse: target_dim.maps_to == source_dim
    ///
    /// In the reverse case, we know the target element and need to find the
    /// corresponding source element using positional correspondence.
    pub fn translate_via_mapping(
        &self,
        source_dim: &CanonicalDimensionName,
        target_dim: &CanonicalDimensionName,
        target_element: &CanonicalElementName,
    ) -> Option<CanonicalElementName> {
        // Try forward: source.maps_to == target
        if let Some(result) =
            self.translate_to_source_via_mapping(source_dim, target_dim, target_element)
        {
            return Some(result);
        }

        // Try reverse: target has a mapping to source
        if self.has_mapping_to(target_dim, source_dim) {
            // Check for element-level mapping in reverse
            if let Some(mapping_info) = self.find_mapping_info(target_dim, source_dim)
                && !mapping_info.element_map.is_empty()
            {
                // element_map is (target_elem, source_elem) pairs for target->source.
                // We have target_element and need the corresponding source element.
                return mapping_info
                    .element_map
                    .iter()
                    .find(|(t, _)| t == target_element)
                    .map(|(_, s)| s.clone());
            }

            // Positional fallback
            let target_named = match self.dimensions.get(target_dim)? {
                Dimension::Named(_, named) => named,
                Dimension::Indexed(_, _) => return None,
            };
            let source_named = match self.dimensions.get(source_dim)? {
                Dimension::Named(_, named) => named,
                Dimension::Indexed(_, _) => return None,
            };

            if source_named.elements.len() != target_named.elements.len() {
                return None;
            }

            let position = *target_named.indexed_elements.get(target_element)?;
            return source_named.elements.get(position - 1).cloned();
        }

        None
    }

    /// The `source_axis` element the EXECUTED simulation reads when an
    /// apply-to-all iteration sitting at `active_element` of `active_dim`
    /// resolves a reference against a source axis declared over a DIFFERENT
    /// dimension.
    ///
    /// This is the engine's single statement of the map-following resolution
    /// rule (GH #997). It has three steps, tried in this order:
    ///
    /// 1. **NAME.** If `source_axis` itself declares an element called
    ///    `active_element`, that element is read -- whatever any declared
    ///    mapping says. Two dimensions sharing element names is an ordinary
    ///    modelling idiom (Vensim's Example 3 subrange copy), so this arm is
    ///    not a corner case: a declared element map is simply not consulted
    ///    where the names already line up.
    /// 2. **MAPPING.** Otherwise [`Self::translate_via_mapping`], which
    ///    honours an explicit `element_map` when one is declared (in either
    ///    declaration direction) and falls back to positional correspondence
    ///    when it is not.
    /// 3. **MAPPED PARENT.** Otherwise, when `source_axis` maps to a
    ///    dimension of which `active_dim` is a SUBDIMENSION, translate through
    ///    that parent -- the active subdimension's elements are a subset of
    ///    the parent's, so the parent-directed map applies unchanged.
    ///
    /// `None` means this rule does not resolve the reference:
    /// `get_implicit_subscript_off` turns that into a compile diagnostic, and
    /// the two subscript arms below take one further step,
    /// [`Self::resolve_dimension_subscript`].
    ///
    /// # The three executed call sites
    ///
    /// * `compiler::context`'s `get_implicit_subscript_off` -- a reference
    ///   carrying NO subscript that reaches the implicit-axis allocator: a
    ///   stock's inflow/outflow, the stock self-reference, and module input
    ///   wiring. It calls this once per active dimension, having first
    ///   narrowed to the active dimensions that own `active_element` (that
    ///   narrowing is a SEARCH over candidate axes, not part of the rule, so
    ///   it stays at the call site).
    /// * `compiler::subscript`'s `build_view_from_ops`, on the
    ///   `IndexOp::ActiveDimRef` arm -- every dimension-named subscript whose
    ///   sibling indices are all static: one naming a dimension the equation
    ///   iterates (`target[State] = x[State]`, and the bare `target[State] = x`
    ///   that `lower_pass0` rewrites into it) and one naming a dimension it
    ///   does not, typically the source's own (`target[COP] = x[Region]`). Its
    ///   active dimension is already chosen by `normalize_subscripts3`, so it
    ///   resolves the element once, through
    ///   [`Self::resolve_dimension_subscript`].
    /// * `compiler::context`'s `IndexExpr3::Dimension` arm, the dynamic-path
    ///   twin of the previous one, reached when some OTHER index of the same
    ///   subscript needs runtime evaluation (`x[Region, i+1]`, `m[State, idx]`,
    ///   an `@N` inside an apply-to-all body). It searches the active
    ///   dimensions the subscript names or maps to and resolves the element
    ///   through the same [`Self::resolve_dimension_subscript`], so the sibling
    ///   a reference carries decides its ROUTE and never its rule.
    ///
    /// The whole 4-spelling x 5-mapping x 2-direction matrix is measured
    /// against the VM in `crate::mapped_reference_semantics_tests`, and
    /// [`Self::executed_read_correspondence`] is this rule projected over one
    /// iterated dimension -- the form the LTM describers read.
    pub fn resolve_mapped_read(
        &self,
        source_axis: &Dimension,
        active_dim: &Dimension,
        active_element: &CanonicalElementName,
    ) -> Option<CanonicalElementName> {
        if source_axis.get_offset(active_element).is_some() {
            return Some(active_element.clone());
        }
        if let Some(translated) = self.translate_via_mapping(
            source_axis.canonical_name(),
            active_dim.canonical_name(),
            active_element,
        ) {
            return Some(translated);
        }
        // Note what step 2 above does NOT do: it short-circuits. A mapping that
        // translates to an element `source_axis` does not declare returns that
        // element rather than falling through to step 3 below, so the caller's
        // `get_offset` misses and the reference goes unresolved for THIS active
        // dimension. That is the pre-GH #997 behaviour of
        // `get_implicit_subscript_off`, preserved rather than chosen: only a
        // malformed element map can produce it, and a caller that searches
        // several active dimensions still tries the rest.
        let parent =
            self.find_mapping_parent_of(source_axis.canonical_name(), active_dim.canonical_name())?;
        self.translate_to_source_via_mapping(source_axis.canonical_name(), parent, active_element)
    }

    /// The 0-based offset on `source_axis` that a subscript spelling the
    /// dimension `spelled`, paired with the active dimension `active_dim`,
    /// reads for `active_element`: [`Self::resolve_mapped_read`] where it
    /// resolves, and otherwise the active element's ORDINAL within its own
    /// dimension, indexing `source_axis` raw -- the last resort, taken only
    /// when the subscript NAMES its active dimension (`spelled` is
    /// `active_dim`) and `source_axis` declares no correspondence to it.
    ///
    /// Both subscript paths take that last resort through this one function:
    /// `compiler::subscript::build_view_from_ops`'s `IndexOp::ActiveDimRef` arm
    /// (every sibling index static) and `compiler::context`'s
    /// `IndexExpr3::Dimension` arm (a sibling evaluated at runtime). A
    /// reference is therefore read by one rule whichever sibling it carries:
    /// `target[State] = m[State, 1]` and `target[State] = m[State, idx]` read
    /// the same cell (GH #1044;
    /// `mapped_reference_semantics_tests::no_mapping_reads_by_ordinal_on_both_subscript_paths`
    /// rows every sibling kind).
    ///
    /// The by-name gate is what keeps a pairing made THROUGH a mapping from
    /// reading a row: `target[Foo] = m[State, ..]` with `Foo maps_to State`
    /// pairs the subscript with `Foo`, and over an `m[Region, ..]` that relates
    /// to neither dimension there is no correspondence to follow and no
    /// ordinal to take -- `None`, a refusal
    /// (`mapped_reference_semantics_tests::a_candidate_paired_through_a_mapping_never_takes_the_ordinal`).
    /// A declared correspondence that fails to translate is authoritative and
    /// stops short of the ordinal too: an element map that leaves this element
    /// unpaired, or a mapped element `source_axis` does not declare, is `None`
    /// rather than a read of a neighbouring element. An ordinal past the end
    /// of `source_axis` is `None` as well
    /// (`an_ordinal_past_the_sources_extent_is_refused_on_both_subscript_paths`).
    ///
    /// The ordinal is the engine's reading of a pair XMILE 1.0 section 3.7.1
    /// leaves open (it defines no mapping, and exemplifies the dimension-name
    /// placeholder only over the equation's own dimensions) and Vensim
    /// refuses (its subscript-mapping reference: a right-hand subscript absent
    /// from the left "would generate an error" without a mapping). GH #1044
    /// holds the decision on the single long-term rule.
    /// `compiler::context::Context::resolve_iteration_element`'s `target_offset`
    /// fallback is the same last resort for a positionally read axis one
    /// axis-collapse later, and the LTM describers deliberately leave the read
    /// undescribed ([`Self::executed_read_correspondence`], GH #527).
    pub fn resolve_dimension_subscript(
        &self,
        source_axis: &Dimension,
        spelled: &CanonicalDimensionName,
        active_dim: &Dimension,
        active_element: &CanonicalElementName,
    ) -> Option<usize> {
        if let Some(resolved) = self.resolve_mapped_read(source_axis, active_dim, active_element) {
            return source_axis.get_offset(&resolved);
        }
        let source = source_axis.canonical_name();
        let active = active_dim.canonical_name();
        let declared = self.has_mapping_to(source, active)
            || self.has_mapping_to(active, source)
            || self.has_mapping_to_parent_of(source, active);
        if declared || spelled != active {
            return None;
        }
        active_dim
            .get_offset(active_element)
            .filter(|offset| *offset < source_axis.len())
    }

    /// Per element of `iterated_dim` in declared order, the `source_dim`
    /// element a dimension-named subscript reads: [`Self::resolve_mapped_read`]
    /// applied element by element -- the per-dimension projection of the
    /// compiler's one resolution rule, and the describer every LTM surface
    /// reads for a read across two dimensions (the element graph's `Bare`
    /// projection, the per-element row derivations, the aggregate slot remap,
    /// the dependency pins).
    ///
    /// One rule serves every spelling because execution resolves them all
    /// through it (`crate::mapped_reference_semantics_tests` measures the
    /// 4-spelling x 5-mapping x 2-direction matrix against the VM):
    ///
    /// * a subscript naming a dimension the equation ITERATES
    ///   (`target[State] = x[State]`, `x` over `Region`), which is also what
    ///   `compiler::context`'s `lower_pass0` writes for a BARE reference in an
    ///   equation body (`target[State] = x`);
    /// * a subscript naming a NON-ACTIVE dimension, typically the source's own
    ///   (`target[COP] = x[Aggregated Regions]`, C-LEARN's shape);
    /// * a stock's FLOW reference (`level[State] = INTEG(x, 0)`), which
    ///   `get_implicit_subscript_off` resolves without passing through pass 0.
    ///
    /// Name first, then the declared element map, then a mapped parent: two
    /// dimensions sharing element names correspond by name whether or not a
    /// mapping is declared between them (Vensim's subrange-copy idiom), an
    /// explicit element map is followed at any cardinality (C-LEARN maps three
    /// `Aggregated Regions` elements onto seven `COP` ones, and the checked-in
    /// `Ref.vdf` identifies which source region each read; `simulates_clearn`
    /// gates it), and a positional `maps_to` reads the diagonal.
    ///
    /// What it deliberately does NOT describe: the ORDINAL a dimension-named
    /// subscript falls back to when the two dimensions declare no
    /// correspondence and share no element name (the last resort of
    /// [`Self::resolve_dimension_subscript`], taken on both subscript paths;
    /// `mapped_reference_semantics_tests::no_mapping_equal_cardinality` and
    /// `no_mapping_reads_by_ordinal_on_both_subscript_paths` measure it).
    /// Vensim rejects such a reference outright (its subscript
    /// mapping page: a right-hand subscript absent from the left "would
    /// generate an error" without a mapping), so a described diagonal follows
    /// a correspondence the model declares or spells by name (GH #527), and a
    /// declining caller keeps its broadcast, a superset of the ordinal read.
    ///
    /// `None` when either dimension is undeclared or any iterated element
    /// fails to resolve: an undeclared pair with disjoint element names, a
    /// positional mapping between dimensions of different sizes, an element
    /// map that leaves an iterated element unpaired.
    pub fn executed_read_correspondence(
        &self,
        iterated_dim: &CanonicalDimensionName,
        source_dim: &CanonicalDimensionName,
    ) -> Option<Vec<CanonicalElementName>> {
        let iterated = self.dimensions.get(iterated_dim)?;
        let source = self.dimensions.get(source_dim)?;
        (0..iterated.len())
            .map(|offset| {
                let elem = iterated.element_name(offset)?;
                self.resolve_mapped_read(source, iterated, &elem)
            })
            .collect()
    }

    /// Which of the target equation's iterated dimensions execution pairs an
    /// index naming the NON-ACTIVE dimension `index_dim` with (GH #997).
    ///
    /// This mirrors `compiler::subscript::normalize_subscripts3`'s
    /// `IndexExpr3::Dimension` arm, which is where the pairing is decided:
    /// having failed to match an active dimension by NAME, it takes the first
    /// active dimension carrying a mapping to or from the index's dimension
    /// and emits an `IndexOp::ActiveDimRef` for it.
    /// [`Self::resolve_mapped_read`] then resolves the element.
    ///
    /// Two deliberate narrowings relative to that arm, both conservative for a
    /// describer (they decline, and a declining caller keeps its
    /// cross-product, which is a superset of the true reads):
    ///
    /// * an `index_dim` that IS one of `target_iterated_dims` is not this
    ///   rule's business -- it is the positional spelling, and the caller has
    ///   already handled it;
    /// * AMBIGUITY declines. Execution breaks a tie between two viable active
    ///   dimensions by POSITION (`position()` returns the first), which is a
    ///   deterministic but arbitrary rule that a describer should not bake in:
    ///   a model whose index dimension maps to two of the target's iterated
    ///   dimensions is a modelling defect, and describing it by silently
    ///   picking one would attribute influence along edges chosen by
    ///   declaration order.
    ///
    /// `index_dim` is NOT required to be the source axis's own dimension.
    /// Execution does not require it either -- the index dimension only picks
    /// the active axis, and the element is then resolved against the source's
    /// own axis -- so requiring it would under-describe a legal spelling at no
    /// benefit.
    pub fn mapped_read_partner_dim(
        &self,
        index_dim: &CanonicalDimensionName,
        target_iterated_dims: &[String],
    ) -> Option<CanonicalDimensionName> {
        if !self.dimensions.contains_key(index_dim) {
            return None;
        }
        let mut partner: Option<CanonicalDimensionName> = None;
        for name in target_iterated_dims {
            let candidate = CanonicalDimensionName::from_raw(name);
            if &candidate == index_dim {
                return None;
            }
            if self.has_mapping_to(index_dim, &candidate)
                || self.has_mapping_to(&candidate, index_dim)
            {
                if partner.is_some() {
                    return None;
                }
                partner = Some(candidate);
            }
        }
        partner
    }

    /// Check if child is a subdimension of parent.
    /// For named dimensions, checks element containment. For indexed dimensions,
    /// uses the declared `parent` field.
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
    /// For named dimensions, checks element containment. For indexed dimensions,
    /// uses the declared `parent` field on the datamodel Dimension.
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
            (Dimension::Indexed(_, child_size), Dimension::Indexed(_, parent_size)) => {
                // Indexed subdimensions: if child declares this parent via the
                // `parent` field, the child maps to the first child_size elements
                // of the parent (0-based offsets 0..child_size).
                if *child_size > *parent_size {
                    return None;
                }
                if let Some(declared_parent) = self.indexed_parents.get(child)
                    && declared_parent == parent
                {
                    let parent_offsets = (0..*child_size as usize).collect();
                    return Some(SubdimensionRelation { parent_offsets });
                }
                None
            }
            _ => None, // Mixed types cannot be subdimensions of each other
        }
    }
}

// ============================================================================
// Dimension Matching Algorithm
// ============================================================================

/// How one source axis was paired with one target axis, strongest rule first.
///
/// The order of the variants IS the precedence [`match_axes`] applies.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub enum AxisMatch {
    /// The two axes name the same dimension.
    Exact,
    /// A declared dimension mapping relates them. `via` is the dimension the
    /// correspondence is declared through: the target for a forward mapping,
    /// the source for a reverse one, the mapped parent for a mapping onto a
    /// dimension the target is a subdimension of, and the shared dimension
    /// when both map onto one.
    Mapped { via: CanonicalDimensionName },
    /// One axis's dimension is a subdimension of the other's.
    Subdimension,
    /// Both axes are indexed dimensions of the same length. Named dimensions
    /// never match by size: their elements carry meaning, so `Cities{Boston,
    /// Seattle}` and `Products{Widgets, Gadgets}` are not interchangeable
    /// because both happen to hold two elements.
    BySize,
}

/// One axis of an array shape, as [`match_axes_partial`] reads it.
///
/// An `ArrayView`'s axes are names and lengths rather than [`Dimension`]s, and
/// a temp's axis has no name at all, so the matcher takes this rather than
/// `&[Dimension]` and every caller projects onto it.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Axis<'a> {
    /// Canonical dimension name, or `""` for an axis that has none. An unnamed
    /// axis never matches by name: there is no name to compare, and two temps'
    /// blank axes are not thereby the same dimension.
    pub name: &'a str,
    pub len: usize,
    /// Whether the axis's dimension is an [`Dimension::Indexed`] one, which is
    /// what admits [`AxisMatch::BySize`].
    pub indexed: bool,
}

impl<'a> Axis<'a> {
    /// The axis a declared dimension presents.
    pub fn of(dim: &'a Dimension) -> Self {
        Axis {
            name: dim.name(),
            len: dim.len(),
            indexed: matches!(dim, Dimension::Indexed(_, _)),
        }
    }

    /// A named axis of a shape that is not a declared dimension list -- an
    /// `ArrayView`'s. `indexed` is false, because a view records the name and
    /// the length of each axis but not which kind of dimension it came from;
    /// a caller that wants the size rule must supply [`Dimension`]s.
    pub fn named(name: &'a str, len: usize) -> Self {
        Axis {
            name,
            len,
            indexed: false,
        }
    }
}

/// [`Axis`] for each of `dims`.
pub fn axes_of(dims: &[Dimension]) -> Vec<Axis<'_>> {
    dims.iter().map(Axis::of).collect()
}

/// The declared relations between two dimension NAMES that
/// [`match_axes_partial`] consults.
///
/// [`DimensionsContext`] is the production implementation. The trait exists
/// because two callers cannot supply one: `ast::Expr2`'s bounds unification
/// reaches its dimension facts through `Expr2Context` rather than through a
/// `DimensionsContext`, and `compiler::mod`'s view join has neither -- it
/// compares two `ArrayView`s, which carry names and lengths only. Both pass a
/// projection instead of a second matcher, so there is one axis-matching
/// precedence and callers differ only in which of its rules they can answer.
/// Every method defaults to "no relation", so a projection states exactly what
/// it knows.
pub trait AxisRelations {
    /// `from` declares a mapping onto `to`.
    fn maps_to(&self, _from: &str, _to: &str) -> bool {
        false
    }

    /// The mapping target of `from` that `of` is a subdimension of, if any.
    ///
    /// An INDIRECT correspondence: the paired element runs target -> that
    /// parent -> source rather than being the target axis's own ordinal.
    /// [`DirectMappingsOnly`] withholds it for a caller that resolves the
    /// paired element by ordinal -- `compiler::context`'s
    /// `make_dimension_subscripts`, which can only emit a dimension-name
    /// subscript, and `ast::Expr2`'s bounds unification, which resolves no
    /// element at all. A DIRECT mapping is safe for those callers because the
    /// dimension-name subscript it emits resolves through
    /// [`DimensionsContext::resolve_mapped_read`] against the paired axis
    /// (GH #527 / #997); the parent one is
    /// not, and pairing through it makes `dst[SubA] = src` over `src[DimB]`
    /// with `DimB -> DimA` and `SubA` a subdimension of `DimA` read `DimB`'s
    /// first element instead of the mapped one.
    fn mapping_parent_of(&self, _from: &str, _of: &str) -> Option<CanonicalDimensionName> {
        None
    }

    /// A dimension both `a` and `b` declare a mapping onto, if any.
    fn common_mapping_target(&self, _a: &str, _b: &str) -> Option<CanonicalDimensionName> {
        None
    }

    /// `child`'s elements are all elements of `parent`.
    ///
    /// A subdimension relation is a PARTIAL correspondence -- only the
    /// subdimension's own elements exist on both axes -- so admitting it is a
    /// statement that the caller can act on a pairing that does not cover
    /// every element. [`DimensionsContext`] does NOT admit it, because a
    /// caller that resolves an ELEMENT through the pairing reads a row the
    /// subdimension does not declare in the SUPERSET direction.
    /// `Parent{A,B,C,D}` with `Sub{A,C}` spells both directions:
    ///
    /// * `out[Sub] = src` over `src[Parent]` would be correct. The only arm
    ///   affected is `compiler::context::make_dimension_subscripts`, which
    ///   withholds the rung through [`DirectMappingsOnly`]; what it can emit
    ///   for a paired axis is a dimension-name subscript, so admitting the
    ///   rung there rewrites the reference to `src[Sub]`, and that spelling
    ///   reads each element by NAME on the parent axis
    ///   ([`DimensionsContext::resolve_mapped_read`]'s first step: `src[C]`
    ///   for `Sub`'s `C`), the read GH #1029 asks for and
    ///   `simulate::a_subrange_dimension_reference_reads_by_name_in_every_spelling`
    ///   pins. The bare reference is refused with `MismatchedDimensions`
    ///   today because the rung is withheld for the other direction.
    /// * `out[Parent] = src` over `src[Sub]` is why it is withheld. Pairing
    ///   the axes rewrites the reference to `src[Parent]` and skips the
    ///   positional length check; the element step
    ///   ([`DimensionsContext::resolve_dimension_subscript`]) then finds no
    ///   `Sub` element named `B`, no mapping, and nothing declared between the
    ///   two dimensions, so it takes its ordinal last resort: `Parent`'s `B`
    ///   at ordinal 1 reads `Sub`'s `C`, a neighbouring row, silently. (`D` at
    ///   ordinal 3 is past a two-element `src` and refused, so this fixture
    ///   fails loudly at `D`; a three-element parent has no such element and
    ///   runs on the wrong number.) The bare reference is refused with
    ///   `MismatchedDimensions` today.
    ///
    /// The rung therefore stays opt-in until the superset direction has a
    /// rule of its own: an element the subdimension does not declare has no
    /// correct row to read, and the ordinal is a wrong one.
    ///
    /// [`SubdimensionRelations`] admits it, for the one caller that needs only
    /// to know WHICH axis to compare positions against and never resolves an
    /// element through the answer.
    fn is_subdimension(&self, _child: &str, _parent: &str) -> bool {
        false
    }
}

/// No declared relations: only [`AxisMatch::Exact`] and [`AxisMatch::BySize`]
/// can fire. What a caller comparing two `ArrayView`s can answer.
pub struct NoAxisRelations;

impl AxisRelations for NoAxisRelations {}

/// Only the DIRECTLY declared mappings between two axes: a mapping in either
/// direction, or both mapping onto one common dimension. For a caller that
/// resolves the paired element by ORDINAL, which the indirect correspondences
/// -- [`AxisRelations::mapping_parent_of`] and
/// [`AxisRelations::is_subdimension`] -- are not.
pub struct DirectMappingsOnly<'a>(pub &'a DimensionsContext);

impl AxisRelations for DirectMappingsOnly<'_> {
    fn maps_to(&self, from: &str, to: &str) -> bool {
        self.0.maps_to(from, to)
    }

    fn common_mapping_target(&self, a: &str, b: &str) -> Option<CanonicalDimensionName> {
        self.0.common_mapping_target(a, b)
    }
}

/// [`DimensionsContext`]'s relations plus the subdimension rung, for a caller
/// that acts on a partial correspondence. See [`AxisRelations::is_subdimension`]
/// for why that is opt-in.
pub struct SubdimensionRelations<'a>(pub &'a DimensionsContext);

impl AxisRelations for SubdimensionRelations<'_> {
    fn maps_to(&self, from: &str, to: &str) -> bool {
        self.0.maps_to(from, to)
    }

    fn mapping_parent_of(&self, from: &str, of: &str) -> Option<CanonicalDimensionName> {
        self.0.mapping_parent_of(from, of)
    }

    fn common_mapping_target(&self, a: &str, b: &str) -> Option<CanonicalDimensionName> {
        self.0.common_mapping_target(a, b)
    }

    fn is_subdimension(&self, child: &str, parent: &str) -> bool {
        self.0.is_subdimension_of(
            &CanonicalDimensionName::from_raw(child),
            &CanonicalDimensionName::from_raw(parent),
        )
    }
}

impl AxisRelations for DimensionsContext {
    fn maps_to(&self, from: &str, to: &str) -> bool {
        self.has_mapping_to(
            &CanonicalDimensionName::from_raw(from),
            &CanonicalDimensionName::from_raw(to),
        )
    }

    fn mapping_parent_of(&self, from: &str, of: &str) -> Option<CanonicalDimensionName> {
        self.find_mapping_parent_of(
            &CanonicalDimensionName::from_raw(from),
            &CanonicalDimensionName::from_raw(of),
        )
        .cloned()
    }

    fn common_mapping_target(&self, a: &str, b: &str) -> Option<CanonicalDimensionName> {
        let a_targets = self.get_all_mapping_targets(&CanonicalDimensionName::from_raw(a));
        if a_targets.is_empty() {
            return None;
        }
        let b_targets = self.get_all_mapping_targets(&CanonicalDimensionName::from_raw(b));
        a_targets
            .into_iter()
            .find(|t| b_targets.contains(t))
            .cloned()
    }
}

/// One rung of the precedence, as a predicate on a (source, target) axis pair.
type AxisRule<'r> = dyn Fn(&Axis<'_>, &Axis<'_>) -> Option<AxisMatch> + 'r;

/// How each axis of `source` pairs with an axis of `target`, or `None` for a
/// source axis nothing supplies -- the engine's SINGLE axis-matching
/// precedence.
///
/// # Precedence
///
/// Exact name, then declared mapping, then subdimension, then size -- the
/// order [`AxisMatch`]'s variants are declared in. Every caller that pairs two
/// axis lists asks this: implicit subscripts for a bare arrayed reference,
/// the subscripts a bare reference under a reducer is given, the reordering
/// that makes two operands' axes line up, `Expr2`'s bounds unification, the
/// per-element resolution inside an apply-to-all body, and the view join.
///
/// # Two properties every caller depends on
///
/// The answer is POSITIONAL, so a shape that repeats a dimension
/// (`target[D,D]`) is matched by index rather than by name; a map keyed by
/// dimension name collapses the two occurrences and answers with whichever was
/// inserted last.
///
/// The allocation is ONE-TO-ONE (`used`), so two source axes that could each
/// take the same target axis are given different ones. Searching per source
/// axis independently lets both claim the first match, and the second axis
/// then reads a row the first is already reading.
///
/// # Why the passes are staged flat
///
/// Which of two competing source axes wins a target axis is decided by MATCH
/// STRENGTH, not by declaration order: each rule runs across ALL source axes
/// before the next rule runs for any of them. Order only breaks ties WITHIN a
/// rule. Running the rules per source axis instead makes declaration order
/// decisive -- an earlier axis consumes, through a weaker rule, the target a
/// later axis matches more strongly, and since the allocation is one-to-one
/// the later axis is then left unresolved. That is GH #996: on C-LEARN it hit
/// two production shapes, `aggregated_definition[cop, aggregated_regions]`
/// read under an `[aggregated_regions]` target and `[cop, semi_agg]` under
/// `[semi_agg]`, both because the target's own dimension declares an element
/// map onto `cop`, so `cop` (declared first) took the slot by MAPPING before
/// the name-matching axis was considered. Each allocated `[Some(0), None]`,
/// the LTM pin table dropped the dep, and 135 link scores were declined.
/// `compiler::dimensions::tests::a_mapping_match_does_not_steal_a_later_name_match`
/// and `a_size_match_does_not_steal_a_later_mapping_match` pin the rule.
///
/// # Where this is NOT the matcher
///
/// [`match_dimensions_two_pass`] is the RUNTIME broadcast matcher, shared by
/// the VM's `LoadIterViewAt` and the wasm backend's `ViewDesc`. It pairs
/// `dim_id`s rather than dimensions and runs where no `DimensionsContext`
/// exists, so it cannot consult a declared mapping or a subdimension relation
/// and deliberately states a weaker rule (id, then size). Do not fold the two
/// together: an execution-time broadcast that silently followed a declared
/// mapping would read a different element than the compile-time resolution
/// that produced the view.
pub fn match_axes_partial(
    source: &[Axis<'_>],
    target: &[Axis<'_>],
    relations: &dyn AxisRelations,
) -> Vec<Option<(usize, AxisMatch)>> {
    let mut alloc: Vec<Option<(usize, AxisMatch)>> = vec![None; source.len()];
    let mut used: Vec<bool> = vec![false; target.len()];

    // Each closure answers ONE rule for one (source, target) pair. They are run
    // as four flat passes below.
    let exact = |s: &Axis<'_>, t: &Axis<'_>| {
        (!s.name.is_empty() && s.name == t.name).then_some(AxisMatch::Exact)
    };
    let mapped = |s: &Axis<'_>, t: &Axis<'_>| {
        if s.name.is_empty() || t.name.is_empty() {
            return None;
        }
        if relations.maps_to(s.name, t.name) {
            return Some(AxisMatch::Mapped {
                via: CanonicalDimensionName::from_raw(t.name),
            });
        }
        if let Some(parent) = relations.mapping_parent_of(s.name, t.name) {
            return Some(AxisMatch::Mapped { via: parent });
        }
        if relations.maps_to(t.name, s.name) {
            return Some(AxisMatch::Mapped {
                via: CanonicalDimensionName::from_raw(s.name),
            });
        }
        relations
            .common_mapping_target(s.name, t.name)
            .map(|via| AxisMatch::Mapped { via })
    };
    let subdimension = |s: &Axis<'_>, t: &Axis<'_>| {
        (!s.name.is_empty()
            && !t.name.is_empty()
            && (relations.is_subdimension(s.name, t.name)
                || relations.is_subdimension(t.name, s.name)))
        .then_some(AxisMatch::Subdimension)
    };
    let by_size = |s: &Axis<'_>, t: &Axis<'_>| {
        (s.indexed && t.indexed && s.len == t.len).then_some(AxisMatch::BySize)
    };

    let rules: [&AxisRule<'_>; 4] = [&exact, &mapped, &subdimension, &by_size];

    for rule in rules {
        for (source_idx, s) in source.iter().enumerate() {
            if alloc[source_idx].is_some() {
                continue;
            }
            for (target_idx, t) in target.iter().enumerate() {
                if used[target_idx] {
                    continue;
                }
                if let Some(how) = rule(s, t) {
                    alloc[source_idx] = Some((target_idx, how));
                    used[target_idx] = true;
                    break;
                }
            }
        }
    }

    alloc
}

/// [`match_axes_partial`] over two declared dimension lists, with every source
/// axis resolved, or `None`.
///
/// `None` is the compiler's `MismatchedDimensions`: no complete allocation
/// exists. It subsumes an explicit `source.len() > target.len()` bail by
/// pigeonhole -- the allocation is one-to-one, so a longer `source` always
/// leaves an axis unresolved.
pub fn match_axes(
    source: &[Dimension],
    target: &[Dimension],
    ctx: &DimensionsContext,
) -> Option<Vec<(usize, AxisMatch)>> {
    match_axes_partial(&axes_of(source), &axes_of(target), ctx)
        .into_iter()
        .collect()
}

#[cfg(test)]
#[path = "axis_match_tests.rs"]
mod axis_match_tests;

/// Match source dimensions to target dimensions using a two-pass algorithm.
///
/// This algorithm is used for broadcasting array operations where a source array
/// may have fewer dimensions than the target iteration space. It determines how
/// to map source dimension indices to target dimension indices.
///
/// # Algorithm
///
/// **Pass 1: Exact ID match**
/// Source dimension ID must match target dimension ID exactly. This handles
/// named dimensions and ensures semantic correctness.
///
/// **Pass 2: Size-based fallback for indexed dimensions**
/// For unmatched source dimensions that are indexed (not named), find the first
/// unused target dimension with the same size that is also indexed.
///
/// The size-based fallback only applies when BOTH dimensions are indexed.
/// Named dimensions must match by ID because their elements have semantic meaning.
/// For example, Cities=[Boston,Seattle] and Products=[Widgets,Gadgets] should not
/// match just because both have size 2.
///
/// # Parameters
///
/// - `source_ids`: Dimension IDs for the source array
/// - `source_sizes`: Size of each source dimension
/// - `source_is_indexed`: Whether each source dimension is indexed (vs named)
/// - `target_ids`: Dimension IDs for the target iteration space
/// - `target_sizes`: Size of each target dimension
/// - `target_is_indexed`: Whether each target dimension is indexed (vs named)
///
/// # Returns
///
/// A vector where `result[src_idx] = Some(target_idx)` if source dimension
/// `src_idx` matches target dimension `target_idx`, or `None` if no match found.
///
/// # Note
///
/// This algorithm is shared between the VM (for runtime broadcasting in
/// `LoadIterViewAt`) and the compiler (for implicit subscript resolution).
/// If you modify this logic, ensure both usages remain correct.
pub fn match_dimensions_two_pass(
    source_ids: &[u16],
    source_sizes: &[u16],
    source_is_indexed: &[bool],
    target_ids: &[u16],
    target_sizes: &[u16],
    target_is_indexed: &[bool],
) -> SmallVec<[Option<usize>; 4]> {
    debug_assert_eq!(source_ids.len(), source_sizes.len());
    debug_assert_eq!(source_ids.len(), source_is_indexed.len());
    debug_assert_eq!(target_ids.len(), target_sizes.len());
    debug_assert_eq!(target_ids.len(), target_is_indexed.len());

    let mut source_to_target: SmallVec<[Option<usize>; 4]> =
        smallvec::smallvec![None; source_ids.len()];
    let mut used_target_positions: SmallVec<[bool; 4]> =
        smallvec::smallvec![false; target_ids.len()];

    // Pass 1: Exact dim_id matches
    for (src_pos, src_id) in source_ids.iter().enumerate() {
        if let Some(target_pos) = target_ids.iter().position(|&id| id == *src_id) {
            source_to_target[src_pos] = Some(target_pos);
            used_target_positions[target_pos] = true;
        }
    }

    // Pass 2: Size-based fallback for unmatched indexed dimensions
    for src_pos in 0..source_ids.len() {
        if source_to_target[src_pos].is_some() {
            continue; // Already matched in pass 1
        }

        // Only indexed dimensions can use size-based matching
        if !source_is_indexed[src_pos] {
            continue;
        }

        let src_size = source_sizes[src_pos];

        // Find first unused indexed target dim of same size
        for target_pos in 0..target_ids.len() {
            if !used_target_positions[target_pos]
                && target_sizes[target_pos] == src_size
                && target_is_indexed[target_pos]
            {
                source_to_target[src_pos] = Some(target_pos);
                used_target_positions[target_pos] = true;
                break;
            }
        }
    }

    source_to_target
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::CanonicalElementName;
    use crate::datamodel;

    // ========== Tests for get_element_index ==========

    #[test]
    fn test_get_element_index_basic() {
        let dim = datamodel::Dimension::named(
            "Region".to_string(),
            vec!["North".to_string(), "South".to_string(), "East".to_string()],
        );
        let dim = Dimension::from(dim);
        if let Dimension::Named(_, named_dim) = dim {
            // Test exact matches (canonical form)
            assert_eq!(named_dim.get_element_index("north"), Some(0));
            assert_eq!(named_dim.get_element_index("south"), Some(1));
            assert_eq!(named_dim.get_element_index("east"), Some(2));

            // Test non-existent element
            assert_eq!(named_dim.get_element_index("west"), None);
            assert_eq!(named_dim.get_element_index(""), None);
        } else {
            panic!("Expected Named dimension");
        }
    }

    #[test]
    fn test_get_element_index_with_spaces() {
        let dim = datamodel::Dimension::named(
            "Product Type".to_string(),
            vec!["Product A".to_string(), "Product B".to_string()],
        );
        let dim = Dimension::from(dim);
        if let Dimension::Named(_, named_dim) = dim {
            // Spaces are converted to underscores in canonical form
            assert_eq!(named_dim.get_element_index("product_a"), Some(0));
            assert_eq!(named_dim.get_element_index("product_b"), Some(1));
        } else {
            panic!("Expected Named dimension");
        }
    }

    // ========== Tests for get_maps_to ==========

    #[test]
    fn test_get_maps_to_basic_mapping() {
        use crate::common::CanonicalDimensionName;

        // DimA maps to DimB
        let mut dim_a = datamodel::Dimension::named(
            "DimA".to_string(),
            vec!["A1".to_string(), "A2".to_string(), "A3".to_string()],
        );
        dim_a.set_maps_to("DimB".to_string());

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
        dim_a.set_maps_to("DimB".to_string());

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
        dim_a.set_maps_to("DimB".to_string());

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
        dim_a.set_maps_to("DimB".to_string());

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
        dim_a.set_maps_to("DimB".to_string());

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

    // ========== Tests for find_mapping_parent_of ==========

    #[test]
    fn test_find_mapping_parent_of_single_target() {
        use crate::common::CanonicalDimensionName;

        // DimA -> DimB (positional), DimB_sub is a subdimension of DimB
        let mut dim_a = datamodel::Dimension::named(
            "DimA".to_string(),
            vec!["A1".to_string(), "A2".to_string(), "A3".to_string()],
        );
        dim_a.set_maps_to("DimB".to_string());

        let dim_b = datamodel::Dimension::named(
            "DimB".to_string(),
            vec!["B1".to_string(), "B2".to_string(), "B3".to_string()],
        );
        let dim_b_sub = datamodel::Dimension::named(
            "DimB_sub".to_string(),
            vec!["B1".to_string(), "B2".to_string()],
        );

        let ctx = DimensionsContext::from(&[dim_a, dim_b, dim_b_sub]);
        let dim_a_name = CanonicalDimensionName::from_raw("DimA");
        let dim_b_name = CanonicalDimensionName::from_raw("DimB");
        let dim_b_sub_name = CanonicalDimensionName::from_raw("DimB_sub");

        assert_eq!(
            ctx.find_mapping_parent_of(&dim_a_name, &dim_b_sub_name),
            Some(&dim_b_name)
        );
    }

    #[test]
    fn test_find_mapping_parent_of_multi_target() {
        use crate::common::CanonicalDimensionName;

        // DimA maps to both DimB and DimC (multi-target).
        // DimC_sub is a subdimension of DimC.
        // find_mapping_parent_of should return DimC (not DimB).
        let mut dim_a = datamodel::Dimension::named(
            "DimA".to_string(),
            vec!["A1".to_string(), "A2".to_string()],
        );
        dim_a.mappings = vec![
            datamodel::DimensionMapping {
                target: "DimB".to_string(),
                element_map: vec![],
            },
            datamodel::DimensionMapping {
                target: "DimC".to_string(),
                element_map: vec![],
            },
        ];

        let dim_b = datamodel::Dimension::named(
            "DimB".to_string(),
            vec!["B1".to_string(), "B2".to_string()],
        );
        let dim_c = datamodel::Dimension::named(
            "DimC".to_string(),
            vec!["C1".to_string(), "C2".to_string()],
        );
        let dim_c_sub = datamodel::Dimension::named("DimC_sub".to_string(), vec!["C1".to_string()]);

        let ctx = DimensionsContext::from(&[dim_a, dim_b, dim_c, dim_c_sub]);
        let dim_a_name = CanonicalDimensionName::from_raw("DimA");
        let dim_c_name = CanonicalDimensionName::from_raw("DimC");
        let dim_c_sub_name = CanonicalDimensionName::from_raw("DimC_sub");

        assert_eq!(
            ctx.find_mapping_parent_of(&dim_a_name, &dim_c_sub_name),
            Some(&dim_c_name),
            "should find DimC as the parent, not DimB"
        );
    }

    // ========== Tests for translate_via_mapping with element-level mappings ==========

    #[test]
    fn test_translate_via_mapping_reordered_elements() {
        use crate::common::CanonicalDimensionName;

        // DimA -> DimB with reordered element mapping: A1->B2, A2->B1
        let mut dim_a = datamodel::Dimension::named(
            "DimA".to_string(),
            vec!["A1".to_string(), "A2".to_string()],
        );
        dim_a.mappings = vec![datamodel::DimensionMapping {
            target: "DimB".to_string(),
            element_map: vec![
                ("A1".to_string(), "B2".to_string()),
                ("A2".to_string(), "B1".to_string()),
            ],
        }];

        let dim_b = datamodel::Dimension::named(
            "DimB".to_string(),
            vec!["B1".to_string(), "B2".to_string()],
        );

        let ctx = DimensionsContext::from(&[dim_a, dim_b]);
        let dim_a_name = CanonicalDimensionName::from_raw("DimA");
        let dim_b_name = CanonicalDimensionName::from_raw("DimB");

        // Translating B1 from DimB context to DimA: A2->B1, so B1 maps to A2
        let b1 = CanonicalElementName::from_raw("B1");
        let result = ctx.translate_via_mapping(&dim_a_name, &dim_b_name, &b1);
        assert_eq!(result, Some(CanonicalElementName::from_raw("a2")));

        // Translating B2 from DimB context to DimA: A1->B2, so B2 maps to A1
        let b2 = CanonicalElementName::from_raw("B2");
        let result = ctx.translate_via_mapping(&dim_a_name, &dim_b_name, &b2);
        assert_eq!(result, Some(CanonicalElementName::from_raw("a1")));
    }

    // ===== Tests for the two spelling-keyed correspondences (GH #527/#997) =====
    //
    // The rows below are the (mapping kind x spelling) product, and they are
    // deliberately the SAME kinds `crate::mapped_reference_semantics_tests`
    // measures against the VM -- that module is the authority for what
    // execution does, and these assert that the two describers report it. A
    // kind measured there and missing here is a describer with no gate.

    fn canon_elems(names: &[&str]) -> Vec<CanonicalElementName> {
        names
            .iter()
            .map(|n| CanonicalElementName::from_raw(n))
            .collect()
    }

    fn dim(name: &str, elems: &[&str]) -> datamodel::Dimension {
        datamodel::Dimension::named(
            name.to_string(),
            elems.iter().map(|e| e.to_string()).collect(),
        )
    }

    fn with_element_map(
        mut d: datamodel::Dimension,
        target: &str,
        pairs: &[(&str, &str)],
    ) -> datamodel::Dimension {
        d.mappings = vec![datamodel::DimensionMapping {
            target: target.to_string(),
            element_map: pairs
                .iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect(),
        }];
        d
    }

    fn state_region(ctx: &DimensionsContext) -> Option<Vec<CanonicalElementName>> {
        use crate::common::CanonicalDimensionName;
        let s = CanonicalDimensionName::from_raw("State");
        let r = CanonicalDimensionName::from_raw("Region");
        ctx.executed_read_correspondence(&s, &r)
    }

    /// A positional (`maps_to`) mapping reads the diagonal, whichever dimension
    /// declares it -- declaration direction changes nothing, measured across all
    /// 40 mapped cells of `mapped_reference_semantics_tests`.
    #[test]
    fn a_positional_mapping_reads_the_diagonal_in_both_declaration_directions() {
        let mut state = dim("State", &["s1", "s2"]);
        state.set_maps_to("Region".to_string());
        let ctx = DimensionsContext::from(&[state, dim("Region", &["a", "b"])]);
        assert_eq!(state_region(&ctx), Some(canon_elems(&["a", "b"])));

        let mut region = dim("Region", &["a", "b"]);
        region.set_maps_to("State".to_string());
        let ctx = DimensionsContext::from(&[dim("State", &["s1", "s2"]), region]);
        assert_eq!(state_region(&ctx), Some(canon_elems(&["a", "b"])));
    }

    /// A PERMUTED explicit element map is followed, and it is followed by
    /// EVERY spelling: `mapped_reference_semantics_tests`' `Permuted` row
    /// measures the iterated, bare, source-own-dim and stock-flow references
    /// all reading `[30, 10, 20]` -- the map's order -- against the VM. Both
    /// declaration directions are covered, since the pair is queried both ways
    /// round.
    #[test]
    fn a_permuted_element_map_is_followed() {
        use crate::common::CanonicalDimensionName;
        let state = with_element_map(
            dim("State", &["s1", "s2"]),
            "Region",
            &[("s1", "b"), ("s2", "a")],
        );
        let ctx = DimensionsContext::from(&[state, dim("Region", &["a", "b"])]);
        assert_eq!(state_region(&ctx), Some(canon_elems(&["b", "a"])));

        // The same pair with the roles swapped: the element map now sits on the
        // SOURCE side, which `translate_via_mapping` reads in reverse.
        let s = CanonicalDimensionName::from_raw("State");
        let r = CanonicalDimensionName::from_raw("Region");
        assert_eq!(
            ctx.executed_read_correspondence(&r, &s),
            Some(canon_elems(&["s2", "s1"]))
        );
    }

    /// A MANY-TO-ONE element map (C-LEARN's shape) resolves every target
    /// element: there is no third source element for a third ordinal to read,
    /// and the map is what every spelling follows
    /// (`mapped_reference_semantics_tests`' `ManyToOne` row).
    ///
    /// This is the correspondence C-LEARN's `FF stop growth year[COP] = FF stop
    /// growth year Aggregated[Aggregated Regions]` needs, and the checked-in
    /// `Ref.vdf` identifies the map as the rule Vensim applies there.
    #[test]
    fn a_many_to_one_element_map_resolves_every_target_element() {
        let state = with_element_map(
            dim("State", &["s1", "s2", "s3"]),
            "Region",
            &[("s1", "a"), ("s2", "a"), ("s3", "b")],
        );
        let ctx = DimensionsContext::from(&[state, dim("Region", &["a", "b"])]);
        assert_eq!(state_region(&ctx), Some(canon_elems(&["a", "a", "b"])));
    }

    /// Two dimensions declaring the SAME element names in a different order,
    /// related by a positional mapping: NAME identity wins over the ordinal.
    /// `mapped_reference_semantics_tests`' `SharedElementNames` row is the VM
    /// oracle -- it is the row that shows map-following is really name-FIRST --
    /// and Vensim's own Example 3 (`PTASKS <-> TASKS`, a subrange copy) makes
    /// shared element names an ordinary idiom rather than an oddity.
    #[test]
    fn shared_element_names_resolve_by_name_under_a_positional_mapping() {
        let mut state = dim("State", &["cal", "ann", "bob"]);
        state.set_maps_to("Region".to_string());
        let ctx = DimensionsContext::from(&[state, dim("Region", &["ann", "bob", "cal"])]);
        assert_eq!(
            state_region(&ctx),
            Some(canon_elems(&["cal", "ann", "bob"])),
            "by name: each State element reads the Region element it shares a \
             name with, not Region's element at its own ordinal"
        );
    }

    /// Shared element names need NO declared mapping to correspond: the
    /// compiler looks the active element's own name up on the source axis
    /// before consulting anything (`build_view_from_ops`, and the VM oracle
    /// `db::ltm_element_instance_tests::qualified_index_edge_follows_the_plain_equations_name_first_read`).
    #[test]
    fn shared_element_names_resolve_by_name_without_a_mapping() {
        let ctx = DimensionsContext::from(&[
            dim("State", &["north", "south"]),
            dim("Region", &["south", "north"]),
        ]);
        assert_eq!(state_region(&ctx), Some(canon_elems(&["north", "south"])));
    }

    /// A positional mapping between different-size dimensions has no
    /// element-level map to disambiguate, so nothing translates.
    #[test]
    fn a_positional_size_mismatch_declines() {
        let mut state = dim("State", &["s1", "s2", "s3"]);
        state.set_maps_to("Region".to_string());
        let ctx = DimensionsContext::from(&[state, dim("Region", &["a", "b"])]);
        assert_eq!(state_region(&ctx), None);
    }

    /// A PARTIAL element map (an iterated element with no pair) declines:
    /// there is no source element to name for `s2`, and a describer that
    /// answered for the paired half would name rows for a reference the
    /// compiler refuses.
    #[test]
    fn a_partial_element_map_declines() {
        let state = with_element_map(dim("State", &["s1", "s2"]), "Region", &[("s1", "a")]);
        let ctx = DimensionsContext::from(&[state, dim("Region", &["a", "b"])]);
        assert_eq!(state_region(&ctx), None);
    }

    /// With NO mapping declared and NO shared element name the rule declines,
    /// so an unrelated pair keeps the caller's conservative broadcast.
    ///
    /// The iterated spelling does in fact compile and read the ORDINAL between
    /// two such dimensions -- `mapped_reference_semantics_tests::
    /// no_mapping_equal_cardinality` measures it -- so the decline is a
    /// deliberate superset rather than a description of execution (GH #527:
    /// the described diagonal follows a correspondence the model declares or
    /// spells by name; see `executed_read_correspondence`'s rustdoc).
    #[test]
    fn an_undeclared_pair_with_disjoint_names_declines() {
        let ctx =
            DimensionsContext::from(&[dim("State", &["s1", "s2"]), dim("Region", &["a", "b"])]);
        assert_eq!(state_region(&ctx), None);
    }

    /// Single-hop only, matching `has_mapping_to` (and the LTM classifier): a
    /// chained `A→B→C` mapping yields `None` for `(A, C)`.
    #[test]
    fn a_transitive_chain_declines() {
        use crate::common::CanonicalDimensionName;

        let mut dim_a = dim("DimA", &["a1", "a2"]);
        dim_a.set_maps_to("DimB".to_string());
        let mut dim_b = dim("DimB", &["b1", "b2"]);
        dim_b.set_maps_to("DimC".to_string());
        let ctx = DimensionsContext::from(&[dim_a, dim_b, dim("DimC", &["c1", "c2"])]);

        let a = CanonicalDimensionName::from_raw("DimA");
        let c = CanonicalDimensionName::from_raw("DimC");
        assert_eq!(ctx.executed_read_correspondence(&a, &c), None);
    }

    // ===== resolve_mapped_read: the per-element executed rule (GH #997) =====

    /// The three steps in order, on one context: NAME beats the declared
    /// element map, the map is consulted only where the name misses, and an
    /// element neither names nor the map reaches declines.
    #[test]
    fn resolve_mapped_read_tries_name_before_the_element_map() {
        // `State` and `Region` share the element name `shared`, and the map
        // sends `shared` somewhere else -- so the two rules give different
        // answers for it and the assertion is not vacuous.
        let state = with_element_map(
            dim("State", &["shared", "s2"]),
            "Region",
            &[("shared", "a"), ("s2", "shared")],
        );
        let ctx = DimensionsContext::from(&[state, dim("Region", &["a", "shared"])]);
        let source = ctx
            .get(&CanonicalDimensionName::from_raw("Region"))
            .unwrap();
        let active = ctx.get(&CanonicalDimensionName::from_raw("State")).unwrap();

        assert_eq!(
            ctx.resolve_mapped_read(source, active, &CanonicalElementName::from_raw("shared")),
            Some(CanonicalElementName::from_raw("shared")),
            "the source axis declares `shared` itself, so the map is not consulted"
        );
        assert_eq!(
            ctx.resolve_mapped_read(source, active, &CanonicalElementName::from_raw("s2")),
            Some(CanonicalElementName::from_raw("shared")),
            "`s2` is not a Region element, so the declared map answers"
        );
        assert_eq!(
            ctx.resolve_mapped_read(source, active, &CanonicalElementName::from_raw("nope")),
            None
        );
    }

    /// Step 3: the source dimension maps to a PARENT of the active
    /// subdimension, so an element of the subdimension translates through the
    /// parent-directed map. This is the arm `get_implicit_subscript_off` has
    /// always had and `build_view_from_ops` gained when the two were unified.
    #[test]
    fn resolve_mapped_read_translates_through_a_mapped_parent() {
        let mut source = dim("Source", &["x", "y", "z"]);
        source.set_maps_to("Parent".to_string());
        let parent = dim("Parent", &["p1", "p2", "p3"]);
        // A subdimension of `Parent`: its elements are a subset, in order.
        let sub = dim("Sub", &["p2", "p3"]);
        let ctx = DimensionsContext::from(&[source, parent, sub]);

        let source_dim = ctx
            .get(&CanonicalDimensionName::from_raw("Source"))
            .unwrap();
        let active = ctx.get(&CanonicalDimensionName::from_raw("Sub")).unwrap();
        assert_eq!(
            ctx.resolve_mapped_read(source_dim, active, &CanonicalElementName::from_raw("p2")),
            Some(CanonicalElementName::from_raw("y")),
            "p2 is Parent's second element, so it reads Source's second"
        );
    }

    // ===== mapped_read_partner_dim: the pairing (GH #997) =====

    /// The partner is the target-iterated dimension the index's dimension is
    /// mapped to, in either declaration direction; a dimension the target does
    /// not iterate is not a candidate.
    #[test]
    fn mapped_read_partner_dim_finds_the_mapped_iterated_dimension() {
        let mut region = dim("Region", &["a", "b"]);
        region.set_maps_to("State".to_string());
        let ctx = DimensionsContext::from(&[
            region,
            dim("State", &["s1", "s2"]),
            dim("Age", &["young", "old"]),
        ]);

        let region_name = CanonicalDimensionName::from_raw("Region");
        assert_eq!(
            ctx.mapped_read_partner_dim(&region_name, &["state".to_string(), "age".to_string()]),
            Some(CanonicalDimensionName::from_raw("State"))
        );
        assert_eq!(
            ctx.mapped_read_partner_dim(&region_name, &["age".to_string()]),
            None,
            "no iterated dimension is mapped to Region"
        );
    }

    /// An index naming a dimension the target ITSELF iterates is the positional
    /// spelling, not this rule's business: it declines so the caller's own
    /// iterated-dim arm keeps it.
    #[test]
    fn mapped_read_partner_dim_declines_an_iterated_dimension_name() {
        let mut region = dim("Region", &["a", "b"]);
        region.set_maps_to("State".to_string());
        let ctx = DimensionsContext::from(&[region, dim("State", &["s1", "s2"])]);

        assert_eq!(
            ctx.mapped_read_partner_dim(
                &CanonicalDimensionName::from_raw("Region"),
                &["region".to_string(), "state".to_string()]
            ),
            None
        );
    }

    /// AMBIGUITY declines. Execution breaks the tie by position; a describer
    /// that copied that would attribute influence along edges chosen by
    /// declaration order, so both candidates disqualify the pairing.
    #[test]
    fn mapped_read_partner_dim_declines_when_two_iterated_dims_are_viable() {
        let mut region = dim("Region", &["a", "b"]);
        region.mappings = vec![
            datamodel::DimensionMapping {
                target: "State".to_string(),
                element_map: vec![],
            },
            datamodel::DimensionMapping {
                target: "County".to_string(),
                element_map: vec![],
            },
        ];
        let ctx = DimensionsContext::from(&[
            region,
            dim("State", &["s1", "s2"]),
            dim("County", &["c1", "c2"]),
        ]);

        assert_eq!(
            ctx.mapped_read_partner_dim(
                &CanonicalDimensionName::from_raw("Region"),
                &["state".to_string(), "county".to_string()]
            ),
            None
        );
    }

    /// A name that is no dimension at all declines (it is a variable read or a
    /// literal, and neither is this rule's).
    #[test]
    fn mapped_read_partner_dim_declines_a_non_dimension_name() {
        let ctx = DimensionsContext::from(&[dim("State", &["s1", "s2"])]);
        assert_eq!(
            ctx.mapped_read_partner_dim(
                &CanonicalDimensionName::from_raw("not_a_dim"),
                &["state".to_string()]
            ),
            None
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
        assert_eq!(relation.start_offset(), 1);
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

    /// `Dimension::name()` is canonical for EVERY constructor, which is what
    /// lets a caller comparing against it skip re-canonicalizing: both arms of
    /// `From<&datamodel::Dimension>` build the name with
    /// `CanonicalDimensionName::from_raw`, and so does every other production
    /// construction of the two variants. `compiler::context`'s
    /// `is_dimension_name` relies on this to compare a canonicalized subscript
    /// against `dim.name()` directly; re-canonicalizing there was a provable
    /// no-op that still scanned the string once per declared dimension per
    /// reference, and on a 126-dimension model it was ~5% of a compile.
    ///
    /// The rows are the shapes canonicalization actually changes -- case,
    /// interior whitespace, a leading/trailing pad, and a dotted name (the
    /// period becomes the module-separator middle dot) -- over both the Named
    /// and the Indexed arm, since they canonicalize at separate call sites.
    #[test]
    fn dimension_name_is_canonical_for_every_constructor() {
        for raw in [
            "Region",
            "My Region",
            "  Padded Region  ",
            "MIXED.Case",
            "already_canonical",
        ] {
            let named = Dimension::from(&datamodel::Dimension::named(
                raw.to_string(),
                vec!["North".to_string()],
            ));
            let indexed = Dimension::from(&datamodel::Dimension::indexed(raw.to_string(), 3));
            let expected = crate::common::canonicalize(raw);
            assert_eq!(
                named.name(),
                &*expected,
                "Named dimension name not canonical for {raw:?}"
            );
            assert_eq!(
                indexed.name(),
                &*expected,
                "Indexed dimension name not canonical for {raw:?}"
            );
        }
    }

    #[test]
    fn test_indexed_dimension_with_maps_to_is_ignored() {
        // Indexed dimensions should not have maps_to - this test verifies
        // that if one is erroneously provided, it's ignored and doesn't
        // affect the dimension context behavior.
        use crate::common::CanonicalDimensionName;

        // Create an indexed dimension with maps_to set (invalid configuration)
        let mut indexed_dim = datamodel::Dimension::indexed("IndexedDim".to_string(), 3);
        indexed_dim.set_maps_to("TargetDim".to_string());

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

    // ========== salsa backdating (PartialEq) smoke test ==========

    /// `DimensionsContext` is a salsa `returns(ref)` query value
    /// (`db::project_dimensions_context`), and salsa backdates a memo purely
    /// by `PartialEq`. Its identity is its `dimensions` + `indexed_parents`
    /// maps; the relationship cache is pure memoization and must not affect
    /// equality, or a populated cache would spuriously invalidate every
    /// downstream query.
    #[test]
    fn equality_ignores_relationship_cache_state() {
        let dim_a = datamodel::Dimension::named(
            "DimA".to_string(),
            vec!["a1".to_string(), "a2".to_string()],
        );
        let dim_a3 = datamodel::Dimension::named(
            "DimA".to_string(),
            vec!["a1".to_string(), "a2".to_string(), "a3".to_string()],
        );
        assert_ne!(
            DimensionsContext::from(std::slice::from_ref(&dim_a)),
            DimensionsContext::from(std::slice::from_ref(&dim_a3)),
            "different dimensions must compare unequal"
        );

        let parent = datamodel::Dimension::named(
            "DimA".to_string(),
            vec!["a1".to_string(), "a2".to_string(), "a3".to_string()],
        );
        let mut child = datamodel::Dimension::named(
            "SubA".to_string(),
            vec!["a2".to_string(), "a3".to_string()],
        );
        child.parent = Some("DimA".to_string());
        let dims = vec![parent, child];

        // Populate the relationship cache on one side via a lookup; a
        // value-equal context with a cold cache must still compare equal.
        let warm = DimensionsContext::from(dims.as_slice());
        let sub_name = crate::common::CanonicalDimensionName::from_raw("SubA");
        let parent_name = crate::common::CanonicalDimensionName::from_raw("DimA");
        let _ = warm.get_subdimension_relation(&sub_name, &parent_name);
        assert_eq!(
            warm,
            DimensionsContext::from(dims.as_slice()),
            "cache state must not affect equality: equal dims => equal contexts"
        );
    }
}

// ============================================================================
// Subscript Iteration
// ============================================================================

/// Iterates over all combinations of subscript offsets for a set of dimensions.
///
/// For example, given dimensions [A(3), B(2)], produces:
/// [0,0], [0,1], [1,0], [1,1], [2,0], [2,1]
pub struct SubscriptOffsetIterator {
    n: usize,
    size: usize,
    lengths: Vec<usize>,
    next: Vec<usize>,
}

impl SubscriptOffsetIterator {
    pub fn new(arrays: &[Dimension]) -> Self {
        SubscriptOffsetIterator {
            n: 0,
            size: arrays.iter().map(|v| v.len()).product(),
            lengths: arrays.iter().map(|v| v.len()).collect(),
            next: vec![0; arrays.len()],
        }
    }
}

impl Iterator for SubscriptOffsetIterator {
    type Item = Vec<usize>;

    fn next(&mut self) -> Option<Vec<usize>> {
        if self.n >= self.size {
            return None;
        }

        let curr = self.next.clone();

        assert_eq!(self.lengths.len(), self.next.len());

        let mut carry = 1_usize;
        for (i, n) in self.next.iter_mut().enumerate().rev() {
            let orig_n = *n;
            let orig_carry = carry;
            *n = (*n + carry) % self.lengths[i];
            carry = ((orig_n != 0 && *n == 0) || (orig_carry == 1 && self.lengths[i] < 2)) as usize;
        }

        self.n += 1;

        Some(curr)
    }
}

/// Iterates over all combinations of subscript element names for a set of dimensions.
///
/// Like `SubscriptOffsetIterator`, but yields the element names (strings) instead
/// of numeric offsets.  For indexed dimensions the names are "1", "2", etc.
pub struct SubscriptIterator<'a> {
    dims: &'a [Dimension],
    offsets: SubscriptOffsetIterator,
}

impl<'a> SubscriptIterator<'a> {
    pub fn new(dims: &'a [Dimension]) -> Self {
        SubscriptIterator {
            dims,
            offsets: SubscriptOffsetIterator::new(dims),
        }
    }
}

impl<'a> Iterator for SubscriptIterator<'a> {
    type Item = Vec<String>;

    fn next(&mut self) -> Option<Vec<String>> {
        self.offsets.next().map(|subscripts| {
            subscripts
                .iter()
                .enumerate()
                .map(|(i, elem)| match &self.dims[i] {
                    Dimension::Named(_, named_dim) => {
                        named_dim.elements[*elem].as_str().to_string()
                    }
                    Dimension::Indexed(_name, _size) => format!("{}", elem + 1),
                })
                .collect()
        })
    }
}

#[cfg(test)]
mod subscript_iter_tests {
    use super::*;
    use crate::datamodel;

    #[test]
    fn test_subscript_offset_iter() {
        let empty_dim = Dimension::from(datamodel::Dimension::named("".to_string(), vec![]));
        let one_dim = Dimension::from(datamodel::Dimension::named(
            "".to_string(),
            vec!["0".to_owned()],
        ));
        let two_dim = Dimension::from(datamodel::Dimension::named(
            "".to_string(),
            vec!["0".to_owned(), "1".to_owned()],
        ));
        let three_dim = Dimension::from(datamodel::Dimension::named(
            "".to_string(),
            vec!["0".to_owned(), "1".to_owned(), "2".to_owned()],
        ));
        let cases: &[(Vec<Dimension>, Vec<Vec<usize>>)] = &[
            (vec![empty_dim.clone()], vec![]),
            (vec![empty_dim.clone(), empty_dim], vec![]),
            (vec![three_dim.clone()], vec![vec![0], vec![1], vec![2]]),
            (
                vec![three_dim.clone(), two_dim.clone()],
                vec![
                    vec![0, 0],
                    vec![0, 1],
                    vec![1, 0],
                    vec![1, 1],
                    vec![2, 0],
                    vec![2, 1],
                ],
            ),
            (
                vec![three_dim, one_dim, two_dim],
                vec![
                    vec![0, 0, 0],
                    vec![0, 0, 1],
                    vec![1, 0, 0],
                    vec![1, 0, 1],
                    vec![2, 0, 0],
                    vec![2, 0, 1],
                ],
            ),
        ];

        for (input, expected) in cases {
            let mut n = 0;
            for (i, subscripts) in SubscriptOffsetIterator::new(input).enumerate() {
                assert_eq!(expected[i], subscripts);
                n += 1;
            }
            assert_eq!(expected.len(), n);
        }
    }

    #[test]
    fn test_subscript_iter() {
        let empty_dim = Dimension::from(datamodel::Dimension::named("".to_string(), vec![]));
        let one_dim = Dimension::from(datamodel::Dimension::named(
            "".to_string(),
            vec!["0".to_owned()],
        ));
        let two_dim = Dimension::from(datamodel::Dimension::named(
            "".to_string(),
            vec!["0".to_owned(), "1".to_owned()],
        ));
        let three_dim = Dimension::from(datamodel::Dimension::named(
            "".to_string(),
            vec!["0".to_owned(), "1".to_owned(), "2".to_owned()],
        ));
        let cases: &[(Vec<Dimension>, Vec<Vec<&str>>)] = &[
            (vec![empty_dim.clone()], vec![]),
            (vec![empty_dim.clone(), empty_dim], vec![]),
            (
                vec![three_dim.clone()],
                vec![vec!["0"], vec!["1"], vec!["2"]],
            ),
            (
                vec![three_dim.clone(), two_dim.clone()],
                vec![
                    vec!["0", "0"],
                    vec!["0", "1"],
                    vec!["1", "0"],
                    vec!["1", "1"],
                    vec!["2", "0"],
                    vec!["2", "1"],
                ],
            ),
            (
                vec![three_dim, one_dim, two_dim],
                vec![
                    vec!["0", "0", "0"],
                    vec!["0", "0", "1"],
                    vec!["1", "0", "0"],
                    vec!["1", "0", "1"],
                    vec!["2", "0", "0"],
                    vec!["2", "0", "1"],
                ],
            ),
        ];

        for (input, expected) in cases {
            let mut n = 0;
            for (i, subscripts) in SubscriptIterator::new(input).enumerate() {
                let refs: Vec<&str> = subscripts.iter().map(|s| s.as_str()).collect();
                assert_eq!(expected[i], refs);
                n += 1;
            }
            assert_eq!(expected.len(), n);
        }
    }
}
