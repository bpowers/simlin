// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Parser for Vensim VDF (binary data file) format.
//!
//! VDF is Vensim's proprietary binary format for simulation output. The format
//! is completely undocumented. See `docs/design/vdf.md` for the full format
//! specification, field-level analysis, and known pitfalls.
//!
//! This module handles:
//! - Parsing the file header, sections, records, slot table, name table,
//!   offset table, and sparse data blocks.
//! - Model-guided name-to-OT mapping via [`VdfFile::build_stocks_first_ot_map`]:
//!   classifies variables as stock/non-stock using the parsed model, sorts
//!   stocks first (alphabetically), then non-stocks (alphabetically), and
//!   cross-references against the VDF name table and record structure.
//! - Time series correlation (`build_empirical_ot_map`) for refining the
//!   structural OT map against a reference simulation when the model can be
//!   run on the same saved time grid.

use std::collections::{HashMap, HashSet};
use std::{error::Error, result::Result as StdResult};

use crate::{
    Variable,
    common::{Canonical, Ident},
    results::{Method, Results, Specs},
};

// ---- Stdlib call analysis and helpers for name-to-OT mapping ----
//
// These were previously in a separate `helpers` submodule but are
// tightly coupled to the VDF mapping logic and only used here.

/// Information about a stdlib function call extracted from an equation.
struct StdlibCallInfo {
    /// The function name (e.g., "SMOOTH", "DELAY1", "SMOOTH3", "DELAY3")
    func_name: String,
    /// Raw argument strings from the equation
    args: Vec<String>,
}

impl StdlibCallInfo {
    fn args_string(&self) -> String {
        self.args
            .iter()
            .map(|a| a.replace([' ', '_'], ""))
            .collect::<Vec<_>>()
            .join(",")
    }

    fn compiled_stdlib_module_name(&self) -> Option<&'static str> {
        let func_upper = self.func_name.to_uppercase();
        let n_args = self.args.len();
        match func_upper.as_str() {
            "SMOOTH" | "SMOOTHI" | "SMTH1" => Some("smth1"),
            "SMOOTH3" | "SMTH3" => Some("smth3"),
            "DELAY" | "DELAY1" => Some("delay1"),
            "DELAY3" => Some("delay3"),
            "DELAYN" => match n_args {
                0..=2 => Some("delay3"),
                _ => Some("delay3"),
            },
            "SMTHN" => match n_args {
                0..=2 => Some("smth3"),
                _ => Some("smth3"),
            },
            "TREND" => Some("trend"),
            "NPV" => Some("npv"),
            _ => None,
        }
    }

    /// Generate Vensim-style instantiation signature names for VDF ordering.
    ///
    /// Returns (signature, is_stock) pairs. The format matches what Vensim
    /// stores in the VDF name table. Names preserve original case from the
    /// MDL and remove spaces.
    ///
    /// The "I" variants (SMOOTHI, SMOOTH3I, DELAY1I, DELAY3I) take an extra
    /// initial-value argument. The MDL parser normalizes their function names
    /// to the non-I form (e.g., SMOOTHI -> SMTH1), so we distinguish them by
    /// argument count: 2 args = standard, 3 args = "I" variant.
    ///
    /// Observed patterns from VDF dumps:
    /// - SMOOTH: `#SMOOTH(arg1,arg2)#` (stock, 1 entry)
    /// - SMOOTHI: `#SMOOTHI(arg1,arg2,init)#` (stock, 1 entry)
    /// - SMOOTH3: `#SMOOTH3(...)#` (stock=output), `#LV1<SMOOTH3(...)#` (stock),
    ///   `#LV2<SMOOTH3(...)#` (stock), `#DL<SMOOTH3(...)#` (non-stock)
    /// - DELAY1: `#DELAY1(...)#` (non-stock=output), `#LV1<DELAY1(...)#` (stock)
    /// - DELAY3: `#DELAY3(...)#` (non-stock=output), `#LV1<...#` `#LV2<...#`
    ///   `#LV3<...#` (stocks), `#RT1<...#` `#RT2<...#` (non-stock rates),
    ///   `#DL<...#` (non-stock)
    fn vensim_signatures(&self) -> Vec<(String, bool)> {
        let args_str = self.args_string();
        let func_upper = self.func_name.to_uppercase();
        let n_args = self.args.len();

        match func_upper.as_str() {
            "SMOOTH" | "SMTH1" if n_args >= 3 => {
                vec![(format!("#SMOOTHI({args_str})#"), true)]
            }
            "SMOOTH" | "SMTH1" => {
                vec![(format!("#SMOOTH({args_str})#"), true)]
            }
            "SMOOTHI" => {
                vec![(format!("#SMOOTHI({args_str})#"), true)]
            }
            "SMOOTH3" | "SMTH3" => {
                let vensim_name = if n_args >= 3 { "SMOOTH3I" } else { "SMOOTH3" };
                let base = format!("{vensim_name}({args_str})");
                vec![
                    (format!("#DL<{base}#"), false),
                    (format!("#LV1<{base}#"), true),
                    (format!("#LV2<{base}#"), true),
                    (format!("#{base}#"), true), // output = 3rd stage stock
                ]
            }
            "DELAY1" | "DELAY" => {
                let vensim_name = if n_args >= 3 { "DELAY1I" } else { "DELAY1" };
                let base = format!("{vensim_name}({args_str})");
                vec![
                    (format!("#{base}#"), false),    // DEL output
                    (format!("#LV1<{base}#"), true), // stock
                ]
            }
            "DELAY3" | "DELAYN" => {
                let vensim_name = if n_args >= 3 { "DELAY3I" } else { "DELAY3" };
                let base = format!("{vensim_name}({args_str})");
                vec![
                    (format!("#{base}#"), false),     // output
                    (format!("#DL<{base}#"), false),  // delay line
                    (format!("#LV1<{base}#"), true),  // stock 1
                    (format!("#LV2<{base}#"), true),  // stock 2
                    (format!("#LV3<{base}#"), true),  // stock 3
                    (format!("#RT1<{base}#"), false), // rate 1
                    (format!("#RT2<{base}#"), false), // rate 2
                ]
            }
            "TREND" => {
                let base = format!("TREND({args_str})");
                vec![
                    (format!("#{base}#"), false),
                    (format!("#LV1<{base}#"), true),
                ]
            }
            _ => {
                vec![(format!("#{func_upper}({args_str})#"), false)]
            }
        }
    }

    /// The VDF signature that a user variable name aliases.
    fn output_signature(&self) -> String {
        let args_str = self.args_string();
        let func_upper = self.func_name.to_uppercase();
        let n_args = self.args.len();
        match func_upper.as_str() {
            "SMOOTH" | "SMTH1" if n_args >= 3 => format!("#SMOOTHI({args_str})#"),
            "SMOOTH" | "SMTH1" | "SMOOTHI" => format!("#SMOOTH({args_str})#"),
            "SMOOTH3" | "SMTH3" => {
                let name = if n_args >= 3 { "SMOOTH3I" } else { "SMOOTH3" };
                format!("#{name}({args_str})#")
            }
            "DELAY1" | "DELAY" => {
                let name = if n_args >= 3 { "DELAY1I" } else { "DELAY1" };
                format!("#{name}({args_str})#")
            }
            "DELAY3" | "DELAYN" => {
                let name = if n_args >= 3 { "DELAY3I" } else { "DELAY3" };
                format!("#{name}({args_str})#")
            }
            "TREND" => format!("#TREND({args_str})#"),
            _ => format!("#{func_upper}({args_str})#"),
        }
    }

    fn member_vdf_targets(&self) -> Vec<(&'static str, String)> {
        let args_str = self.args_string();
        let func_upper = self.func_name.to_uppercase();
        let n_args = self.args.len();
        match func_upper.as_str() {
            "SMOOTH" | "SMTH1" if n_args >= 3 => {
                vec![("output", format!("#SMOOTHI({args_str})#"))]
            }
            "SMOOTH" | "SMTH1" | "SMOOTHI" => {
                vec![("output", format!("#SMOOTH({args_str})#"))]
            }
            "SMOOTH3" | "SMTH3" => {
                let name = if n_args >= 3 { "SMOOTH3I" } else { "SMOOTH3" };
                let base = format!("{name}({args_str})");
                vec![
                    ("output", format!("#{base}#")),
                    ("stock_1", format!("#LV1<{base}#")),
                    ("stock_2", format!("#LV2<{base}#")),
                ]
            }
            "DELAY1" | "DELAY" => {
                let name = if n_args >= 3 { "DELAY1I" } else { "DELAY1" };
                let base = format!("{name}({args_str})");
                vec![
                    ("output", format!("#{base}#")),
                    ("stock", format!("#LV1<{base}#")),
                ]
            }
            "DELAY3" | "DELAYN" => {
                let name = if n_args >= 3 { "DELAY3I" } else { "DELAY3" };
                let base = format!("{name}({args_str})");
                vec![
                    ("output", format!("#{base}#")),
                    ("stock", format!("#LV1<{base}#")),
                    ("stock_2", format!("#LV2<{base}#")),
                    ("stock_3", format!("#LV3<{base}#")),
                    ("flow_1", format!("#RT1<{base}#")),
                    ("flow_2", format!("#RT2<{base}#")),
                ]
            }
            "TREND" => {
                let base = format!("TREND({args_str})");
                vec![
                    ("output", format!("#{base}#")),
                    ("stock", format!("#LV1<{base}#")),
                ]
            }
            "NPV" => vec![("output", format!("#NPV({args_str})#"))],
            _ => Vec::new(),
        }
    }

    /// Whether the user-visible output of this stdlib call is stored in a
    /// stock-backed OT entry.
    #[allow(dead_code)]
    fn output_is_stock(&self) -> bool {
        let output = self.output_signature();
        self.vensim_signatures()
            .into_iter()
            .any(|(sig, is_stock)| is_stock && sig == output)
    }
}

/// Names of stdlib module internal variables that DO consume OT entries.
/// LV1/LV2/LV3/ST are stock-backed; DEL/DL/RT1/RT2 are non-stock.
const STDLIB_PARTICIPANT_HELPERS: [&str; 8] =
    ["DEL", "LV1", "LV2", "LV3", "ST", "RT1", "RT2", "DL"];

/// Whether a stdlib participant helper name is stock-backed.
fn is_stdlib_helper_stock(name: &str) -> bool {
    matches!(name, "LV1" | "LV2" | "LV3" | "ST")
}

/// Check if a VDF name table entry is metadata rather than a variable.
fn is_vdf_metadata_entry(name: &str) -> bool {
    if !name.is_empty() && name.chars().all(|c| c.is_ascii_digit()) {
        return true;
    }
    if name.starts_with('-') {
        return true;
    }
    if name.starts_with('.') {
        return true;
    }
    if name.starts_with(':') {
        return true;
    }
    if name.starts_with('"') {
        return true;
    }
    if matches!(
        name,
        "IN" | "INI"
            | "OUTPUT"
            | "SMOOTH"
            | "SMOOTHI"
            | "SMOOTH3"
            | "SMOOTH3I"
            | "DELAY1"
            | "DELAY1I"
            | "DELAY3"
            | "DELAY3I"
            | "TREND"
            | "NPV"
    ) {
        return true;
    }
    let lower = name.replace([' ', '_'], "").to_lowercase();
    matches!(lower.as_str(), "ifthenelse" | "withlookup" | "lookup")
}

/// Heuristic for visible lookup/table definitions.
#[allow(dead_code)]
fn is_probable_lookup_table_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.contains(" lookup") || lower.contains(" table")
}

/// Normalize a VDF name for comparison: lowercase, strip spaces and underscores.
fn normalize_vdf_name(name: &str) -> String {
    name.replace([' ', '_'], "").to_lowercase()
}

fn collect_direct_stdlib_calls(model: &crate::datamodel::Model) -> HashMap<String, StdlibCallInfo> {
    let mut calls = HashMap::new();
    for var in &model.variables {
        let (ident, equation) = match var {
            crate::datamodel::Variable::Aux(a) => (&a.ident, &a.equation),
            crate::datamodel::Variable::Flow(f) => (&f.ident, &f.equation),
            _ => continue,
        };
        if let Some(info) = extract_stdlib_call_info(equation) {
            calls.insert(crate::common::canonicalize(ident).into_owned(), info);
        }
    }
    calls
}

fn parse_implicit_stdlib_module_ident(name: &str) -> Option<(String, String)> {
    let parts: Vec<&str> = name.split('\u{205A}').collect();
    if parts.len() < 4 || parts.first().copied() != Some("$") {
        return None;
    }
    let parent = parts.get(1)?.to_string();
    let func = parts.get(3)?.to_lowercase();
    Some((parent, func))
}

fn prefixed_ident(prefix: Option<&str>, ident: &str) -> String {
    if let Some(prefix) = prefix {
        format!("{prefix}.{ident}")
    } else {
        ident.to_string()
    }
}

fn collect_compiled_alias_edges(
    project: &crate::Project,
    datamodel_models: &HashMap<&str, &crate::datamodel::Model>,
    model_name: &str,
    prefix: Option<&str>,
) -> Vec<(Ident<Canonical>, String)> {
    let compiled_model =
        std::sync::Arc::clone(&project.models[&*crate::common::canonicalize(model_name)]);
    let direct_calls = datamodel_models
        .get(model_name)
        .map(|m| collect_direct_stdlib_calls(m))
        .unwrap_or_default();

    let mut out = Vec::new();
    let mut var_names: Vec<&str> = compiled_model
        .variables
        .keys()
        .map(|s| s.as_str())
        .collect();
    var_names.sort_unstable();

    for ident in var_names {
        let full_ident = prefixed_ident(prefix, ident);
        let var = &compiled_model.variables[&*crate::common::canonicalize(ident)];
        let Variable::Module {
            model_name: submodel_name,
            inputs,
            ..
        } = var
        else {
            continue;
        };

        for input in inputs {
            let member_name = format!("{full_ident}.{}", input.dst.to_source_repr());
            let source_name = prefixed_ident(prefix, &input.src.to_source_repr().to_string());
            out.push((
                Ident::<Canonical>::from_unchecked(member_name),
                normalize_vdf_name(&source_name),
            ));
        }

        if submodel_name.as_str().starts_with("stdlib\u{205A}")
            && let Some((parent_name, module_func)) = parse_implicit_stdlib_module_ident(ident)
            && let Some(info) = direct_calls.get(&parent_name)
            && info
                .compiled_stdlib_module_name()
                .is_some_and(|name| name == module_func)
        {
            for (member, target) in info.member_vdf_targets() {
                out.push((
                    Ident::<Canonical>::from_unchecked(format!("{full_ident}.{member}")),
                    normalize_vdf_name(&target),
                ));
            }
            continue;
        }

        out.extend(collect_compiled_alias_edges(
            project,
            datamodel_models,
            submodel_name.as_str(),
            Some(&full_ident),
        ));
    }

    out
}

/// Extract stdlib function call information from a datamodel equation.
///
/// Returns None if the equation is not a top-level stdlib call.
fn extract_stdlib_call_info(eqn: &crate::datamodel::Equation) -> Option<StdlibCallInfo> {
    let text = match eqn {
        crate::datamodel::Equation::Scalar(s) | crate::datamodel::Equation::ApplyToAll(_, s) => {
            s.as_str()
        }
        _ => return None,
    };

    let trimmed = text.trim();
    let paren_pos = trimmed.find('(')?;
    let func_name = trimmed[..paren_pos].trim();

    let func_lower = func_name.to_lowercase();
    if !crate::builtins::is_stdlib_module_function(&func_lower) {
        return None;
    }

    let after_paren = &trimmed[paren_pos + 1..];
    let close_paren = find_matching_close_paren(after_paren)?;
    let args_str = &after_paren[..close_paren];
    let args = split_top_level_args(args_str);

    Some(StdlibCallInfo {
        func_name: func_name.to_string(),
        args,
    })
}

fn find_matching_close_paren(s: &str) -> Option<usize> {
    let mut depth = 0;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                if depth == 0 {
                    return Some(i);
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    None
}

fn split_top_level_args(s: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut depth = 0;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                args.push(s[start..i].trim().to_string());
                start = i + 1;
            }
            _ => {}
        }
    }
    let last = s[start..].trim();
    if !last.is_empty() {
        args.push(last.to_string());
    }
    args
}

/// VDF file magic bytes (first 4 bytes of every VDF file).
pub const VDF_FILE_MAGIC: [u8; 4] = [0x7f, 0xf7, 0x17, 0x52];

/// VDF section header magic value: float32 -0.797724 = 0xbf4c37a1.
/// This 4-byte sequence delimits sections within the VDF file.
pub const VDF_SECTION_MAGIC: [u8; 4] = [0xa1, 0x37, 0x4c, 0xbf];

/// Sentinel value appearing in record fields 8, 9, and sometimes 14.
pub const VDF_SENTINEL: u32 = 0xf6800000;

/// Section-6 OT class code for the Time series (OT[0]) in all observed files.
pub const VDF_SECTION6_OT_CODE_TIME: u8 = 0x0f;

/// Section-6 OT class code marking stock-backed OT entries in all validated
/// files. This is the first VDF-only stock/non-stock discriminator we have
/// identified.
pub const VDF_SECTION6_OT_CODE_STOCK: u8 = 0x08;

/// Record field[1] value identifying system/control variables (INITIAL TIME,
/// FINAL TIME, TIME STEP, SAVEPER).
const RECORD_F1_SYSTEM: u32 = 23;

/// Record field[1] value identifying INITIAL TIME constant records. These
/// pass the standard model-variable filter but aren't model variables.
const RECORD_F1_INITIAL_TIME_CONST: u32 = 15;

/// Size of a VDF section header in bytes (magic + 5 u32 fields).
pub const SECTION_HEADER_SIZE: usize = 24;

/// Size of a VDF variable metadata record in bytes (16 u32 fields).
pub const RECORD_SIZE: usize = 64;

/// Size of the VDF file header in bytes.
pub const FILE_HEADER_SIZE: usize = 0x80;

/// Vensim system variable names that appear in every VDF name table.
pub const SYSTEM_NAMES: [&str; 5] = ["Time", "INITIAL TIME", "FINAL TIME", "TIME STEP", "SAVEPER"];

/// Vensim builtin function names that may appear in the name table
/// alongside model variables. Used to filter candidates during
/// name-to-data mapping.
pub const VENSIM_BUILTINS: [&str; 28] = [
    "abs", "cos", "exp", "integer", "ln", "log", "max", "min", "modulo", "pi", "sin", "sqrt",
    "tan", "step", "pulse", "ramp", "delay", "delay1", "delay3", "smooth", "smooth3", "trend",
    "sum", "prod", "product", "vmin", "vmax", "elmcount",
];

// ---- Low-level readers ----

/// Read a little-endian u32 from the given byte offset.
pub fn read_u32(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap())
}

/// Read a little-endian u16 from the given byte offset.
pub fn read_u16(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap())
}

/// Read a little-endian f32 from the given byte offset.
pub fn read_f32(data: &[u8], offset: usize) -> f32 {
    f32::from_le_bytes(data[offset..offset + 4].try_into().unwrap())
}

// ---- Parsed structures ----

/// A section within a VDF file, delimited by section magic bytes.
///
/// Each section has a 24-byte header followed by data. A section's extent runs
/// from its header to the start of the next section header (magic-to-magic),
/// captured by `region_end`. See `docs/design/vdf.md` for details.
#[derive(Debug, Clone)]
pub struct Section {
    /// Absolute file offset of the section magic bytes.
    pub file_offset: usize,
    /// Absolute file offset where this section's region ends. For sections
    /// 0..n-1, this is the next section's `file_offset`. For the last
    /// section, this is the file length.
    pub region_end: usize,
    /// Header field at +4. For the name table section, this determines how
    /// many names have slot table entries. Purpose in other sections is
    /// unknown. Not a section size (regions extend past this).
    pub field1: u32,
    // field2 (header offset +8) is omitted: it always equals field1 in
    // every observed VDF file, so storing both adds no information.
    /// Field3 in header (often 0x1F4 = 500).
    pub field3: u32,
    /// Field4: section type identifier (e.g. 19=model info, 2=variable slots).
    pub field4: u32,
    /// Field5: for name table sections, high 16 bits = first name length.
    pub field5: u32,
}

impl Section {
    /// Absolute file offset where section data begins (after 24-byte header).
    pub fn data_offset(&self) -> usize {
        self.file_offset + SECTION_HEADER_SIZE
    }

    /// Size in bytes of the section's full region data (after the header).
    /// For degenerate sections (like section 5 in small models, whose
    /// header overlaps the next section's header), this returns 0.
    pub fn region_data_size(&self) -> usize {
        self.region_end.saturating_sub(self.data_offset())
    }
}

/// A 64-byte variable metadata record (16 x u32 fields).
#[derive(Debug, Clone)]
pub struct VdfRecord {
    /// Absolute file offset of this record.
    pub file_offset: usize,
    /// The 16 u32 fields comprising this record.
    pub fields: [u32; 16],
}

impl VdfRecord {
    /// field[12]: byte offset into section 1 data (slot reference).
    /// Groups records into clusters sharing a variable name slot.
    pub fn slot_ref(&self) -> u32 {
        self.fields[12]
    }

    /// field[11]: appears to be an offset table index for small models.
    pub fn ot_index(&self) -> u32 {
        self.fields[11]
    }
}

/// Variable-length list entry used by section-5/section-6 decoded streams.
///
/// `refs` are section-1-relative byte offsets. Many (but not all) references
/// correspond to values present in the slot table.
#[derive(Debug, Clone)]
pub struct VdfRefListEntry {
    /// Absolute file offset where this entry begins.
    pub file_offset: usize,
    /// Referenced section-1 offsets carried by this entry.
    pub refs: Vec<u32>,
    /// Number of refs that are present in the slot table.
    pub slotted_ref_count: usize,
}

/// Parsed section-5 set entry (`u32 n; u32 0; u32 refs[n+1]`).
///
/// In array-heavy files, `n` is one less than the associated subscript-set
/// cardinality. This structure preserves `n` explicitly rather than inferring
/// only from `refs.len()`.
#[derive(Debug, Clone)]
pub struct VdfSection5SetEntry {
    /// Absolute file offset where this entry begins.
    pub file_offset: usize,
    /// Header count field (`n`), where the entry stores `n+1` refs.
    pub n: usize,
    /// Referenced section-1 offsets carried by this entry.
    pub refs: Vec<u32>,
    /// Number of refs that are present in the slot table.
    pub slotted_ref_count: usize,
}

impl VdfSection5SetEntry {
    /// Number of section-1 refs in the entry (`n+1` in observed files).
    pub fn set_size(&self) -> usize {
        self.refs.len()
    }

    /// Candidate dimension size implied by this set (`set_size - 1`).
    pub fn dimension_size(&self) -> usize {
        self.set_size().saturating_sub(1)
    }
}

/// Fixed-width metadata record stored in the trailing section-6 suffix.
///
/// After the section-6 OT class-code array and the OT-aligned final-value
/// vector, observed files store a `13 * u32` record stream terminated by a
/// single zero word. The exact semantic meaning of most fields is still being
/// reverse engineered, but field 10 is a stable OT index used by Vensim's
/// display metadata.
#[derive(Debug, Clone, PartialEq)]
pub struct VdfSection6DisplayRecord {
    /// Absolute file offset where this record begins.
    pub file_offset: usize,
    /// Raw words comprising the record.
    pub words: [u32; 13],
}

impl VdfSection6DisplayRecord {
    /// OT index referenced by the record's display metadata.
    pub fn ot_index(&self) -> usize {
        self.words[10] as usize
    }
}

/// A contiguous OT index range implied by record start indices (field[11]).
///
/// Each range starts at a unique in-range `f11` value and ends at the next
/// start (or `offset_table_count` for the last range). Range length > 1
/// indicates a multi-entry block (commonly array/lookups/table data).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VdfOtRange {
    /// Inclusive start OT index.
    pub start: usize,
    /// Exclusive end OT index.
    pub end: usize,
    /// Number of records whose `f11` equals `start`.
    pub record_count: usize,
}

impl VdfOtRange {
    /// Number of OT entries in this range.
    pub fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }

    /// Whether this range contains no OT entries.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Fully parsed VDF file holding all structural metadata.
///
/// Created via [`VdfFile::parse`], this struct provides access to all
/// decoded sections, names, records, slot table, and offset table entries.
pub struct VdfFile {
    /// Raw file bytes.
    pub data: Vec<u8>,
    /// Number of time points in the simulation.
    pub time_point_count: usize,
    /// Bitmap size in bytes: ceil(time_point_count / 8).
    pub bitmap_size: usize,
    /// All sections found in the file.
    pub sections: Vec<Section>,
    /// All parsed variable names from the name table section's region.
    pub names: Vec<String>,
    /// Index into `sections` for the name table section.
    pub name_section_idx: Option<usize>,
    /// Slot table: one u32 per name, each a byte offset into section 1 data.
    pub slot_table: Vec<u32>,
    /// File offset where the slot table starts.
    pub slot_table_offset: usize,
    /// Variable metadata records (64 bytes each).
    pub records: Vec<VdfRecord>,
    /// File offset of the first offset table entry.
    pub offset_table_start: usize,
    /// Number of entries in the offset table.
    pub offset_table_count: usize,
    /// File offset of the first data block (time series).
    pub first_data_block: usize,
    /// When the traditional offset table can't be found (e.g. medium-sized
    /// VDFs like the econ model), we build a synthetic OT by walking
    /// contiguous data blocks. Each entry is a block offset.
    synthetic_ot: Option<Vec<u32>>,
}

struct StocksFirstMapBuild {
    mapping: HashMap<Ident<Canonical>, usize>,
    participant_ots: HashMap<String, usize>,
    section6_codes: Option<Vec<u8>>,
}

type VdfVisibleResultsOrder = Vec<(Ident<Canonical>, usize)>;

fn equation_uses_lookup_placeholder(equation: &crate::datamodel::Equation) -> bool {
    match equation {
        crate::datamodel::Equation::Scalar(eq) | crate::datamodel::Equation::ApplyToAll(_, eq) => {
            eq.trim() == "0+0"
        }
        crate::datamodel::Equation::Arrayed(_, elements, default_eq, _) => {
            default_eq.as_deref().is_none_or(|eq| eq.trim() == "0+0")
                && !elements.is_empty()
                && elements
                    .iter()
                    .all(|(_, eq, _, gf)| eq.trim() == "0+0" && gf.is_some())
        }
    }
}

fn variable_has_graphical_function(var: &crate::datamodel::Variable) -> bool {
    match var {
        crate::datamodel::Variable::Aux(aux) => {
            aux.gf.is_some()
                || matches!(
                    &aux.equation,
                    crate::datamodel::Equation::Arrayed(_, elements, _, _)
                        if elements.iter().any(|(_, _, _, gf)| gf.is_some())
                )
        }
        crate::datamodel::Variable::Flow(flow) => {
            flow.gf.is_some()
                || matches!(
                    &flow.equation,
                    crate::datamodel::Equation::Arrayed(_, elements, _, _)
                        if elements.iter().any(|(_, _, _, gf)| gf.is_some())
                )
        }
        crate::datamodel::Variable::Stock(_) | crate::datamodel::Variable::Module(_) => false,
    }
}

fn is_data_only_lookup_definition(var: &crate::datamodel::Variable) -> bool {
    let (equation, compat) = match var {
        crate::datamodel::Variable::Aux(aux) => (&aux.equation, &aux.compat),
        crate::datamodel::Variable::Flow(flow) => (&flow.equation, &flow.compat),
        crate::datamodel::Variable::Stock(_) | crate::datamodel::Variable::Module(_) => {
            return false;
        }
    };

    if !variable_has_graphical_function(var) {
        return false;
    }

    equation_uses_lookup_placeholder(equation)
        || compat
            .data_source
            .as_ref()
            .is_some_and(|source| source.kind == crate::datamodel::DataSourceKind::Lookups)
}

fn collect_data_only_lookup_names(model: &crate::datamodel::Model) -> HashSet<String> {
    model
        .variables
        .iter()
        .filter(|var| is_data_only_lookup_definition(var))
        .map(|var| normalize_vdf_name(var.get_ident()))
        .collect()
}

fn collect_graphical_function_signature_names(model: &crate::datamodel::Model) -> HashSet<String> {
    model
        .variables
        .iter()
        .filter(|var| variable_has_graphical_function(var))
        .map(|var| normalize_vdf_name(&format!("#{}#", var.get_ident())))
        .collect()
}

impl VdfFile {
    /// Parse a VDF file from raw bytes.
    pub fn parse(data: Vec<u8>) -> StdResult<Self, Box<dyn Error>> {
        if data.len() < FILE_HEADER_SIZE {
            return Err("VDF file too small".into());
        }
        if data[0..4] != VDF_FILE_MAGIC {
            return Err("invalid VDF magic bytes".into());
        }

        let time_point_count = read_u32(&data, 0x78) as usize;
        let bitmap_size = time_point_count.div_ceil(8);

        let sections = find_sections(&data);
        let name_section_idx = find_name_table_section_idx(&data, &sections);

        let names = name_section_idx
            .map(|ns_idx| {
                parse_name_table_extended(&data, &sections[ns_idx], sections[ns_idx].region_end)
            })
            .unwrap_or_default();

        // Section at index 1 is the variable slot table section. Its field4
        // value varies across VDF versions (2, 42, etc.) so we identify it
        // by position rather than field4.
        let sec1_data_size = sections.get(1).map(|s| s.region_data_size()).unwrap_or(0);

        let (slot_table_offset, slot_table) = name_section_idx
            .map(|ns_idx| {
                let ns = &sections[ns_idx];
                find_slot_table(&data, ns, names.len(), sec1_data_size)
            })
            .unwrap_or((0, Vec::new()));

        // Find records between the slot data end and the slot/name table
        // boundary within section 1's region.  The slot table entries are
        // byte offsets into section 1's data area; the maximum sorted
        // entry plus one stride marks where records begin.
        let search_start = if !slot_table.is_empty() {
            let sec1_data_start = sections
                .get(1)
                .map(|s| s.data_offset())
                .unwrap_or(FILE_HEADER_SIZE);
            let mut sorted_slots: Vec<u32> = slot_table.clone();
            sorted_slots.sort();
            let max_offset = *sorted_slots.last().unwrap() as usize;
            let last_stride = if sorted_slots.len() >= 2 {
                let n = sorted_slots.len();
                (sorted_slots[n - 1] - sorted_slots[n - 2]) as usize
            } else {
                max_offset
            };
            sec1_data_start + max_offset + last_stride
        } else {
            sections
                .get(1)
                .map(|s| s.data_offset())
                .unwrap_or(FILE_HEADER_SIZE)
        };
        let search_bound = sections.get(1).map(|s| s.region_end).unwrap_or(data.len());
        let records_end = if slot_table_offset > 0 && slot_table_offset < search_bound {
            slot_table_offset
        } else {
            search_bound
        };
        let records = find_records(&data, search_start, records_end);

        let first_data_block = find_first_data_block(&data, time_point_count, bitmap_size)
            .ok_or("could not find first VDF data block")?;
        let (mut offset_table_start, mut offset_table_count) =
            find_offset_table(&data, first_data_block);

        // When the traditional OT isn't found (medium-sized VDFs like the
        // econ model), build a synthetic OT by walking contiguous blocks.
        let synthetic_ot = if offset_table_count == 0 {
            let blocks =
                enumerate_data_blocks(&data, first_data_block, bitmap_size, time_point_count);
            if blocks.len() > 1 {
                let ot: Vec<u32> = blocks.iter().map(|&(off, _, _)| off as u32).collect();
                // Overwrite offset_table_start/count so that callers (like
                // vdf-dump) can report a consistent location and entry count
                // regardless of whether the OT is real or synthetic.
                offset_table_start = first_data_block;
                offset_table_count = ot.len();
                Some(ot)
            } else {
                None
            }
        } else {
            None
        };

        Ok(VdfFile {
            data,
            time_point_count,
            bitmap_size,
            sections,
            names,
            name_section_idx,
            slot_table,
            slot_table_offset,
            records,
            offset_table_start,
            offset_table_count,
            first_data_block,
            synthetic_ot,
        })
    }

    /// Read a u32 offset table entry by index.
    pub fn offset_table_entry(&self, index: usize) -> Option<u32> {
        if index >= self.offset_table_count {
            return None;
        }
        if let Some(ref synthetic) = self.synthetic_ot {
            return synthetic.get(index).copied();
        }
        let off = self.offset_table_start + index * 4;
        if off + 4 > self.data.len() {
            return None;
        }
        Some(read_u32(&self.data, off))
    }

    /// Check if an offset table entry is a file offset to a data block
    /// (as opposed to an inline f32 constant).
    pub fn is_data_block_offset(&self, raw: u32) -> bool {
        let offset = raw as usize;
        offset >= self.first_data_block && offset < self.data.len()
    }

    /// Get the variable slot table section (always at section index 1).
    /// Its field4 value varies across VDF versions (2, 42, etc.).
    pub fn slot_section(&self) -> Option<&Section> {
        self.sections.get(1)
    }

    fn section_ref_stream_with_skip(
        &self,
        section_idx: usize,
        skip_words: usize,
        max_refs_per_entry: usize,
    ) -> (Vec<VdfRefListEntry>, usize) {
        let Some(sec) = self.sections.get(section_idx) else {
            return (Vec::new(), 0);
        };
        let start = sec.data_offset() + skip_words * 4;
        let end = sec.region_end.min(self.data.len());
        if start >= end {
            return (Vec::new(), start);
        }

        let sec1_data_size = self
            .sections
            .get(1)
            .map(|s| s.region_data_size())
            .unwrap_or(0);
        let slot_set: HashSet<u32> = self.slot_table.iter().copied().collect();

        let mut entries = Vec::new();
        let mut pos = start;
        while pos + 4 <= end {
            let n_refs = read_u32(&self.data, pos) as usize;
            if n_refs == 0 || n_refs > max_refs_per_entry {
                break;
            }
            let refs_start = pos + 4;
            let refs_end = refs_start + n_refs * 4;
            if refs_end > end {
                break;
            }
            let refs: Vec<u32> = (0..n_refs)
                .map(|i| read_u32(&self.data, refs_start + i * 4))
                .collect();
            if !refs
                .iter()
                .all(|&r| r > 0 && r % 4 == 0 && (r as usize) < sec1_data_size)
            {
                break;
            }
            let slotted_ref_count = refs.iter().filter(|r| slot_set.contains(r)).count();
            entries.push(VdfRefListEntry {
                file_offset: pos,
                refs,
                slotted_ref_count,
            });
            pos = refs_end;
        }
        (entries, pos)
    }

    fn section5_set_stream_with_skip(
        &self,
        skip_words: usize,
        max_n: usize,
    ) -> (Vec<VdfSection5SetEntry>, usize) {
        let Some(sec) = self.sections.get(5) else {
            return (Vec::new(), 0);
        };
        let start = sec.data_offset() + skip_words * 4;
        let end = sec.region_end.min(self.data.len());
        if start >= end {
            return (Vec::new(), start);
        }

        let sec1_data_size = self
            .sections
            .get(1)
            .map(|s| s.region_data_size())
            .unwrap_or(0);
        let slot_set: HashSet<u32> = self.slot_table.iter().copied().collect();

        let mut entries = Vec::new();
        let mut pos = start;
        while pos + 8 <= end {
            let n = read_u32(&self.data, pos) as usize;
            let marker = read_u32(&self.data, pos + 4);
            if n == 0 || n > max_n || marker != 0 {
                break;
            }
            let refs_len = n + 1;
            let refs_start = pos + 8;
            let refs_end = refs_start + refs_len * 4;
            if refs_end > end {
                break;
            }
            let refs: Vec<u32> = (0..refs_len)
                .map(|i| read_u32(&self.data, refs_start + i * 4))
                .collect();
            if !refs
                .iter()
                .all(|&r| r > 0 && r % 4 == 0 && (r as usize) < sec1_data_size)
            {
                break;
            }
            let slotted_ref_count = refs.iter().filter(|r| slot_set.contains(r)).count();
            entries.push(VdfSection5SetEntry {
                file_offset: pos,
                n,
                refs,
                slotted_ref_count,
            });
            pos = refs_end;
        }
        (entries, pos)
    }

    /// Parse the leading section-6 stream as variable-length entries:
    ///
    /// `u32 n_refs; u32 refs[n_refs]`
    ///
    /// Returns `(skip_words, entries, stop_offset)`, where `skip_words` is the
    /// chosen 4-byte prefix skip (0..=8) and `stop_offset` is the absolute file
    /// offset where stream parsing stopped.
    pub fn parse_section6_ref_stream(&self) -> Option<(usize, Vec<VdfRefListEntry>, usize)> {
        if self.sections.len() <= 6 {
            return None;
        }
        let mut best_skip = 0usize;
        let mut best_entries = Vec::new();
        let mut best_stop = 0usize;
        for skip in 0..=8usize {
            let (entries, stop) = self.section_ref_stream_with_skip(6, skip, 512);
            if entries.len() > best_entries.len()
                || (entries.len() == best_entries.len() && stop > best_stop)
            {
                best_skip = skip;
                best_entries = entries;
                best_stop = stop;
            }
        }
        Some((best_skip, best_entries, best_stop))
    }

    /// Extract the section-6 OT class-code array that immediately follows the
    /// leading `count+refs` stream.
    ///
    /// In every validated file, the bytes after the parsed ref stream begin
    /// with exactly `offset_table_count` class codes:
    ///
    /// - `0x0f` at OT[0] (Time)
    /// - `0x08` for stock-backed OT entries
    /// - other observed codes for non-stock OT entries
    ///
    /// This array is the strongest VDF-only stock/non-stock signal identified
    /// so far.
    pub fn section6_ot_class_codes(&self) -> Option<Vec<u8>> {
        if self.offset_table_count == 0 {
            return None;
        }
        let (_skip_words, _entries, stop_offset) = self.parse_section6_ref_stream()?;
        let sec = self.sections.get(6)?;
        let end = sec.region_end.min(self.data.len());
        let codes_end = stop_offset.checked_add(self.offset_table_count)?;
        if codes_end > end {
            return None;
        }
        Some(self.data[stop_offset..codes_end].to_vec())
    }

    /// Extract the OT-aligned final-value array stored in the section-6 tail.
    ///
    /// Immediately after the class-code array, validated VDFs store one
    /// `f32` per OT entry holding the final saved value for that OT (or the
    /// constant itself for inline-constant OTs). This gives a fast structural
    /// summary without walking the data blocks.
    pub fn section6_ot_final_values(&self) -> Option<Vec<f32>> {
        if self.offset_table_count == 0 {
            return None;
        }
        let sec = self.sections.get(6)?;
        let (_skip_words, _entries, stop_offset) = self.parse_section6_ref_stream()?;
        let codes_end = stop_offset.checked_add(self.offset_table_count)?;
        let values_end = codes_end.checked_add(self.offset_table_count.checked_mul(4)?)?;
        if values_end > sec.region_end.min(self.data.len()) {
            return None;
        }

        let mut values = Vec::with_capacity(self.offset_table_count);
        for i in 0..self.offset_table_count {
            let off = codes_end + i * 4;
            values.push(read_f32(&self.data, off));
        }
        Some(values)
    }

    /// Parse the fixed-width display record stream from the section-6 tail.
    ///
    /// Observed files store this stream immediately after the OT final-value
    /// array. The stream is `13 * u32` per record and ends with a single zero
    /// word. The record payload appears to drive Vensim's graph/display UI;
    /// only the OT index field has been identified so far.
    pub fn section6_display_records(&self) -> Option<Vec<VdfSection6DisplayRecord>> {
        if self.offset_table_count == 0 {
            return None;
        }
        let sec = self.sections.get(6)?;
        let tail_end = sec.region_end.min(self.data.len());
        let (_skip_words, _entries, stop_offset) = self.parse_section6_ref_stream()?;
        let codes_end = stop_offset.checked_add(self.offset_table_count)?;
        let values_end = codes_end.checked_add(self.offset_table_count.checked_mul(4)?)?;
        if values_end >= tail_end {
            return Some(Vec::new());
        }

        let suffix = &self.data[values_end..tail_end];
        if suffix.len() < 4 || !suffix.len().is_multiple_of(4) {
            return None;
        }

        let word_count = suffix.len() / 4;
        if read_u32(suffix, suffix.len() - 4) != 0 {
            return None;
        }
        if !(word_count - 1).is_multiple_of(13) {
            return None;
        }

        let record_count = (word_count - 1) / 13;
        let mut out = Vec::with_capacity(record_count);
        for i in 0..record_count {
            let rec_off = i * 13 * 4;
            let mut words = [0u32; 13];
            for (j, word) in words.iter_mut().enumerate() {
                *word = read_u32(suffix, rec_off + j * 4);
            }
            out.push(VdfSection6DisplayRecord {
                file_offset: values_end + rec_off,
                words,
            });
        }
        Some(out)
    }

    /// Return the section-6 OT class code for a single OT index.
    pub fn section6_ot_class_code(&self, ot_index: usize) -> Option<u8> {
        let codes = self.section6_ot_class_codes()?;
        codes.get(ot_index).copied()
    }

    /// Whether the section-6 OT class code marks this OT entry as stock-backed.
    pub fn section6_ot_is_stock(&self, ot_index: usize) -> Option<bool> {
        Some(self.section6_ot_class_code(ot_index)? == VDF_SECTION6_OT_CODE_STOCK)
    }

    /// Parse section-5 set-like entries seen in array-heavy files:
    ///
    /// `u32 n; u32 0; u32 refs[n+1]`
    ///
    /// Returns `(skip_words, entries, stop_offset)` for the best alignment.
    pub fn parse_section5_set_stream(&self) -> Option<(usize, Vec<VdfSection5SetEntry>, usize)> {
        if self.sections.len() <= 5 {
            return None;
        }
        let mut best_skip = 0usize;
        let mut best_entries = Vec::new();
        let mut best_stop = 0usize;
        for skip in 0..=8usize {
            let (entries, stop) = self.section5_set_stream_with_skip(skip, 4096);
            if entries.len() > best_entries.len()
                || (entries.len() == best_entries.len() && stop > best_stop)
            {
                best_skip = skip;
                best_entries = entries;
                best_stop = stop;
            }
        }
        Some((best_skip, best_entries, best_stop))
    }

    /// Build contiguous OT ranges from in-range record start indices (field[11]).
    ///
    /// This uses only structural metadata:
    /// 1. Collect unique starts where `0 < f11 < offset_table_count`
    /// 2. Sort starts ascending
    /// 3. Form ranges `[start_i, start_{i+1})`, last range to `offset_table_count`
    ///
    /// The returned ranges are deterministic for a given VDF and are useful
    /// for partitioning OT entries before resolving variable/element names.
    pub fn record_ot_ranges(&self) -> Vec<VdfOtRange> {
        if self.offset_table_count <= 1 {
            return Vec::new();
        }

        let mut starts = Vec::new();
        let mut start_counts: HashMap<usize, usize> = HashMap::new();
        for rec in &self.records {
            let start = rec.fields[11] as usize;
            if start == 0 || start >= self.offset_table_count {
                continue;
            }
            if !start_counts.contains_key(&start) {
                starts.push(start);
            }
            *start_counts.entry(start).or_default() += 1;
        }
        starts.sort_unstable();

        let mut out = Vec::new();
        for (i, &start) in starts.iter().enumerate() {
            let end = starts
                .get(i + 1)
                .copied()
                .unwrap_or(self.offset_table_count);
            if end <= start {
                continue;
            }
            out.push(VdfOtRange {
                start,
                end,
                record_count: *start_counts.get(&start).unwrap_or(&0),
            });
        }
        out
    }

    /// Build a `Results` struct using model-guided OT mapping.
    ///
    /// The primary path uses structural VDF/model alignment; when the model
    /// can be simulated on the same saved time grid, empirical series
    /// correlation refines that map so larger files can recover correct OT
    /// placement without giving up the structural fallback.
    ///
    /// The resulting `Results` contains only variables that could be mapped to
    /// OT entries from this VDF. Columns are ordered by model offset.
    pub fn to_results_with_model(
        &self,
        project: &crate::Project,
        main_model_name: &str,
    ) -> StdResult<Results, Box<dyn Error>> {
        let vdf_data = self.extract_data()?;
        let model = project
            .datamodel
            .models
            .iter()
            .find(|m| m.name == main_model_name)
            .ok_or_else(|| format!("model {main_model_name} not found"))?;
        let data_only_lookup_names = collect_data_only_lookup_names(model);
        let time_ident = Ident::<Canonical>::from_str_unchecked("time");
        let (mut ot_map, structural_error) =
            match self.build_stocks_first_ot_map_for_project(project, main_model_name) {
                Ok(map) => (map, None),
                Err(err) => {
                    let mut fallback = HashMap::new();
                    fallback.insert(time_ident.clone(), 0usize);
                    (fallback, Some(err))
                }
            };

        let empirical = (|| -> StdResult<HashMap<Ident<Canonical>, usize>, Box<dyn Error>> {
            let sim = crate::interpreter::Simulation::new(project, main_model_name)?;
            let reference = sim.run_to_end()?;
            build_empirical_ot_map_with_prior(&vdf_data, &reference, Some(&ot_map))
        })();

        match empirical {
            Ok(empirical_map) => merge_empirical_ot_map(&mut ot_map, empirical_map),
            Err(empirical_error) => {
                if let Some(structural_error) = structural_error {
                    return Err(format!(
                        "structural OT mapping failed ({structural_error}); \
                         empirical refinement also failed ({empirical_error})"
                    )
                    .into());
                }
            }
        }

        if ot_map.is_empty() {
            return Err("VDF/model mapping produced no variables".into());
        }

        let ordered = self.project_visible_results_order(&ot_map, &data_only_lookup_names)?;

        let step_count = vdf_data.time_values.len();
        let step_size = ordered.len();
        let mut step_data = vec![f64::NAN; step_count * step_size];
        let mut offsets: HashMap<Ident<Canonical>, usize> = HashMap::new();

        for (col, (id, ot_idx)) in ordered.iter().enumerate() {
            offsets.insert(id.clone(), col);
            let Some(series) = vdf_data.entries.get(*ot_idx) else {
                continue;
            };
            for step in 0..step_count {
                step_data[step * step_size + col] = series[step];
            }
        }

        let initial_time = vdf_data.time_values[0];
        let final_time = vdf_data.time_values[step_count - 1];
        let saveper = if step_count > 1 {
            vdf_data.time_values[1] - vdf_data.time_values[0]
        } else {
            1.0
        };

        Ok(Results {
            offsets,
            data: step_data.into_boxed_slice(),
            step_size,
            step_count,
            specs: Specs {
                start: initial_time,
                stop: final_time,
                dt: saveper,
                save_step: saveper,
                method: Method::Euler,
                n_chunks: step_count,
            },
            is_vensim: true,
        })
    }

    fn project_visible_results_order(
        &self,
        ot_map: &HashMap<Ident<Canonical>, usize>,
        data_only_lookup_names: &HashSet<String>,
    ) -> StdResult<VdfVisibleResultsOrder, Box<dyn Error>> {
        let mut ordered = vec![(Ident::<Canonical>::from_str_unchecked("time"), 0usize)];
        let mut seen = HashSet::from([normalize_vdf_name("time")]);
        let system_names: HashSet<&str> = SYSTEM_NAMES.into_iter().collect();
        let mut missing = Vec::new();

        for name in self.names.iter().take(self.slot_table.len()) {
            let normalized = normalize_vdf_name(name);
            if name.is_empty()
                || name.starts_with('#')
                || system_names.contains(name.as_str())
                || VENSIM_BUILTINS.iter().any(|b| b.eq_ignore_ascii_case(name))
                || (name.len() == 1 && name.starts_with(|c: char| !c.is_alphanumeric()))
                || data_only_lookup_names.contains(&normalized)
                || is_vdf_metadata_entry(name)
                || STDLIB_PARTICIPANT_HELPERS.contains(&name.as_str())
            {
                continue;
            }

            if !seen.insert(normalized) {
                continue;
            }

            let key = Ident::<Canonical>::new(name);
            if let Some(&ot) = ot_map.get(&key) {
                ordered.push((key, ot));
            } else {
                missing.push(name.clone());
            }
        }

        if !missing.is_empty() {
            missing.sort();
            eprintln!(
                "VDF/model mapping left {} visible VDF names unresolved: {:?}",
                missing.len(),
                missing.iter().take(16).collect::<Vec<_>>()
            );
        }

        if ordered.len() <= 1 {
            Err("VDF/model mapping produced no visible results".into())
        } else {
            Ok(ordered)
        }
    }

    /// Extract all time series data from data blocks, returning a `VdfData`.
    pub fn extract_data(&self) -> StdResult<VdfData, Box<dyn Error>> {
        let time_values = extract_time_series(
            &self.data,
            self.first_data_block,
            self.time_point_count,
            self.bitmap_size,
        )?;
        let step_count = time_values.len();

        let mut entries = Vec::with_capacity(self.offset_table_count);
        for i in 0..self.offset_table_count {
            let raw_val = self
                .offset_table_entry(i)
                .ok_or("offset table entry out of bounds")?;

            let series = if self.is_data_block_offset(raw_val) {
                extract_block_series(&self.data, raw_val as usize, self.bitmap_size, &time_values)?
            } else {
                let const_val = f32::from_le_bytes(raw_val.to_le_bytes()) as f64;
                vec![const_val; step_count]
            };
            entries.push(series);
        }

        Ok(VdfData {
            time_values,
            entries,
        })
    }

    fn model_record_ot_indices(&self) -> Vec<usize> {
        let mut record_ots: Vec<usize> = self
            .records
            .iter()
            .filter_map(|rec| {
                let ot_idx = rec.fields[11] as usize;
                if rec.fields[0] != 0
                    && rec.fields[1] != RECORD_F1_SYSTEM
                    && rec.fields[1] != RECORD_F1_INITIAL_TIME_CONST
                    && rec.fields[10] > 0
                    && ot_idx > 0
                    && ot_idx < self.offset_table_count
                {
                    Some(ot_idx)
                } else {
                    None
                }
            })
            .collect();
        record_ots.sort_unstable();
        record_ots.dedup();
        record_ots
    }

    /// Build a name->OT mapping using stocks-first-alphabetical ordering,
    /// cross-referenced against the VDF's structural information.
    ///
    /// Algorithm:
    /// 1. Extract model variable records from the VDF (same as deterministic map)
    ///    to get the set of actual OT values
    /// 2. Extract candidate variable names from the VDF name table (same filtering)
    ///    PLUS unslotted internal signature names
    /// 3. Classify each candidate as stock/non-stock using the parsed model
    /// 4. Sort: stocks alphabetically, then non-stocks (by VDF name)
    /// 5. Pair sorted candidates with sorted OT values from records
    /// 6. Map SMOOTH/DELAY user variable names to their output entry's OT
    ///
    /// Returns canonical variable name -> OT index mapping.
    fn build_stocks_first_ot_map_for_model(
        &self,
        model: &crate::datamodel::Model,
    ) -> StdResult<StocksFirstMapBuild, Box<dyn Error>> {
        // Step 1: extract model variable records and their OT indices.
        let record_ots = self.model_record_ot_indices();
        let section6_codes = self.section6_ot_class_codes();
        let data_only_lookup_names = collect_data_only_lookup_names(model);
        let graphical_signature_names = collect_graphical_function_signature_names(model);

        // Step 2: extract candidate variable names from VDF name table.
        // Use the same filtering as model_record_ot_indices for slotted
        // names, then add unslotted internal signature names.
        let system_names: HashSet<&str> = SYSTEM_NAMES.into_iter().collect();
        let mut candidates: Vec<String> = self.names[..self.slot_table.len()]
            .iter()
            .filter(|name| {
                !name.is_empty()
                    && !name.starts_with('.')
                    && !name.starts_with('-')
                    && !system_names.contains(name.as_str())
                    && !data_only_lookup_names.contains(&normalize_vdf_name(name))
                    && !graphical_signature_names.contains(&normalize_vdf_name(name))
            })
            .cloned()
            .collect();

        // Add unslotted names (internal #-prefixed signatures)
        if self.names.len() > self.slot_table.len() {
            for name in &self.names[self.slot_table.len()..] {
                if name.starts_with('#') {
                    candidates.push(name.clone());
                }
            }
        }

        // Step 3 (done early): classify model variables and build alias set.
        // We need the alias set before candidate filtering.
        let mut model_stock_set: HashSet<String> = HashSet::new();
        let mut model_sig_stocks: HashSet<String> = HashSet::new();
        let mut model_sig_stock_names: HashSet<String> = HashSet::new();
        let mut aliases: Vec<(String, String)> = Vec::new();
        let mut alias_names_normalized: HashSet<String> = HashSet::new();

        for var in &model.variables {
            let (ident, equation) = match var {
                crate::datamodel::Variable::Stock(s) => {
                    model_stock_set.insert(normalize_vdf_name(&s.ident));
                    continue;
                }
                crate::datamodel::Variable::Aux(a) => (&a.ident, &a.equation),
                crate::datamodel::Variable::Flow(f) => (&f.ident, &f.equation),
                crate::datamodel::Variable::Module(_) => continue,
            };

            if let Some(info) = extract_stdlib_call_info(equation) {
                let output_sig = info.output_signature();
                aliases.push((ident.clone(), output_sig));
                alias_names_normalized.insert(normalize_vdf_name(ident));
                for (sig, is_stock) in info.vensim_signatures() {
                    let normalized = normalize_vdf_name(&sig);
                    if is_stock {
                        model_sig_stocks.insert(normalized);
                        model_sig_stock_names.insert(sig);
                    }
                }
            }
        }

        // Remove names matching builtins, metadata, and SMOOTH/DELAY user
        // variable names (which share OTs with their internal signatures
        // and are handled as aliases later).
        // Progressive filtering: remove non-variable entries until
        // candidate count is within range.
        candidates
            .retain(|n| n.len() != 1 || n.chars().next().is_some_and(|c| c.is_alphanumeric()));
        {
            let vensim_builtins: HashSet<&str> = VENSIM_BUILTINS.into_iter().collect();
            candidates.retain(|n| !vensim_builtins.contains(n.to_lowercase().as_str()));
        }
        candidates.retain(|n| !is_vdf_metadata_entry(n));
        // SMOOTH/DELAY user variable names share OTs with their internal
        // signatures and are handled as aliases later.
        candidates.retain(|n| !alias_names_normalized.contains(&normalize_vdf_name(n)));

        let is_stock_name = |name: &str| -> bool {
            let normalized = normalize_vdf_name(name);
            model_stock_set.contains(&normalized) || model_sig_stocks.contains(&normalized)
        };

        let visible_candidate_names: HashSet<String> = candidates
            .iter()
            .map(|name| normalize_vdf_name(name))
            .collect();

        let mut stock_names: Vec<String> = candidates
            .iter()
            .filter(|n| is_stock_name(n))
            .cloned()
            .collect();
        let mut hidden_stock_names: Vec<String> = model_sig_stock_names
            .into_iter()
            .filter(|name| !visible_candidate_names.contains(&normalize_vdf_name(name)))
            .collect();
        let mut non_stock_names: Vec<String> = candidates
            .iter()
            .filter(|n| !is_stock_name(n))
            .cloned()
            .collect();

        // Step 4: sort each group case-insensitively by VDF name
        stock_names.sort_by_key(|n| n.to_lowercase());
        hidden_stock_names.sort_by_key(|n| n.to_lowercase());
        non_stock_names.sort_by_key(|n| n.to_lowercase());

        let mut mapping: HashMap<Ident<Canonical>, usize> = HashMap::new();
        mapping.insert(Ident::<Canonical>::from_str_unchecked("time"), 0);
        let mut participant_ots: HashMap<String, usize> = HashMap::new();

        let mut stock_participants: Vec<(String, bool)> =
            stock_names.into_iter().map(|name| (name, true)).collect();
        stock_participants.extend(hidden_stock_names.into_iter().map(|name| (name, false)));
        stock_participants.sort_by_key(|(name, _visible)| name.to_lowercase());

        let stock_ots: Vec<usize> = if let Some(codes) = &section6_codes {
            codes
                .iter()
                .enumerate()
                .skip(1)
                .filter_map(|(ot, &code)| (code == VDF_SECTION6_OT_CODE_STOCK).then_some(ot))
                .collect()
        } else {
            record_ots
                .iter()
                .copied()
                .take(stock_participants.len())
                .collect()
        };

        if !stock_ots.is_empty() && stock_ots.len() != stock_participants.len() {
            return Err(format!(
                "stock OT count ({}) != stock participant count ({})",
                stock_ots.len(),
                stock_participants.len()
            )
            .into());
        }

        for ((name, visible), ot) in stock_participants
            .into_iter()
            .zip(stock_ots.iter().copied())
        {
            participant_ots.insert(normalize_vdf_name(&name), ot);
            if !visible {
                continue;
            }
            let key = if name.starts_with('#') {
                Ident::<Canonical>::from_str_unchecked(&name)
            } else {
                Ident::<Canonical>::new(&name)
            };
            mapping.insert(key, ot);
        }

        let non_stock_record_ots: Vec<usize> = record_ots
            .iter()
            .copied()
            .filter(|ot| {
                section6_codes
                    .as_ref()
                    .and_then(|codes| codes.get(*ot))
                    .copied()
                    != Some(VDF_SECTION6_OT_CODE_STOCK)
            })
            .collect();

        // Step 5: assign non-stock names from the remaining record-derived OT
        // slots. Record metadata still appears to be the only structural anchor
        // for non-stock placement; section 6 fixes the missing stock slots but
        // does not yet fully expose non-stock name assignment.
        for (name, ot) in non_stock_names
            .into_iter()
            .zip(non_stock_record_ots.iter().copied())
        {
            participant_ots.insert(normalize_vdf_name(&name), ot);
            let key = if name.starts_with('#') {
                Ident::<Canonical>::from_str_unchecked(&name)
            } else {
                Ident::<Canonical>::new(&name)
            };
            mapping.insert(key, ot);
        }

        // Step 6: map SMOOTH/DELAY user variables to their output entry's OT
        for (user_ident, output_sig) in &aliases {
            let sig_normalized = normalize_vdf_name(output_sig);
            if let Some(&ot) = participant_ots.get(&sig_normalized) {
                let user_key = Ident::<Canonical>::new(user_ident);
                mapping.entry(user_key).or_insert(ot);
            }
        }

        Ok(StocksFirstMapBuild {
            mapping,
            participant_ots,
            section6_codes,
        })
    }

    fn build_stocks_first_ot_map_for_project(
        &self,
        project: &crate::Project,
        main_model_name: &str,
    ) -> StdResult<HashMap<Ident<Canonical>, usize>, Box<dyn Error>> {
        let model = project
            .datamodel
            .models
            .iter()
            .find(|m| m.name == main_model_name)
            .ok_or_else(|| format!("model {main_model_name} not found"))?;
        let StocksFirstMapBuild {
            mut mapping,
            mut participant_ots,
            section6_codes,
        } = self.build_stocks_first_ot_map_for_model(model)?;

        let datamodel_models: HashMap<&str, &crate::datamodel::Model> = project
            .datamodel
            .models
            .iter()
            .map(|m| (m.name.as_str(), m))
            .collect();

        let alias_edges =
            collect_compiled_alias_edges(project, &datamodel_models, main_model_name, None);

        let mut changed = true;
        while changed {
            changed = false;
            for (alias, target) in &alias_edges {
                let Some(&ot) = participant_ots.get(target) else {
                    continue;
                };
                let alias_normalized = normalize_vdf_name(alias.as_str());
                let old = participant_ots.insert(alias_normalized, ot);
                if old != Some(ot) {
                    changed = true;
                }
                mapping.entry(alias.clone()).or_insert(ot);
            }
        }

        if let Some(codes) = &section6_codes {
            let mapped_stock_count = mapping
                .iter()
                .filter(|(id, ot)| {
                    id.as_str() != "time"
                        && codes.get(**ot) == Some(&VDF_SECTION6_OT_CODE_STOCK)
                        && !id.as_str().starts_with("#")
                })
                .count();
            let vdf_stock_count = codes
                .iter()
                .skip(1)
                .filter(|&&code| code == VDF_SECTION6_OT_CODE_STOCK)
                .count();
            if mapped_stock_count < vdf_stock_count {
                return Err(format!(
                    "mapped stock-backed participants ({mapped_stock_count}) < VDF stock OT count ({vdf_stock_count})"
                )
                .into());
            }
        }

        Ok(mapping)
    }

    #[cfg(any(test, feature = "testing"))]
    pub fn build_stocks_first_ot_map(
        &self,
        project: &crate::datamodel::Project,
    ) -> StdResult<HashMap<Ident<Canonical>, usize>, Box<dyn Error>> {
        let compiled = crate::Project::from(project.clone());
        self.build_stocks_first_ot_map_for_project(&compiled, "main")
    }

    #[cfg(not(any(test, feature = "testing")))]
    pub fn build_stocks_first_ot_map(
        &self,
        project: &crate::datamodel::Project,
    ) -> StdResult<HashMap<Ident<Canonical>, usize>, Box<dyn Error>> {
        let model = project.models.first().ok_or("project has no models")?;
        Ok(self.build_stocks_first_ot_map_for_model(model)?.mapping)
    }

    /// Build a name→OT mapping using the section-6 contiguous stock/non-stock
    /// layout, model-based stock classification, and VDF name table filtering.
    ///
    /// This exploits the discovery that section-6 OT class codes are always
    /// contiguous: OT[1..S] are all stock (code 0x08), OT[S+1..N-1] are all
    /// non-stock. Within each block, names are sorted alphabetically
    /// (case-insensitive).
    ///
    /// The algorithm:
    /// 1. Extract candidate variable names from the VDF name table
    /// 2. Add `#`-prefixed internal signature names
    /// 3. Classify each candidate as stock or non-stock using the model
    /// 4. Match candidate counts against section-6 counts, trimming
    ///    excess lookupish names when needed
    /// 5. Sort each group alphabetically and assign OT indices
    /// 6. Map SMOOTH/DELAY user variables to their internal signature OTs
    pub fn build_section6_guided_ot_map(
        &self,
        model: &crate::datamodel::Model,
    ) -> StdResult<HashMap<Ident<Canonical>, usize>, Box<dyn Error>> {
        let codes = self
            .section6_ot_class_codes()
            .ok_or("no section-6 class codes available")?;
        if codes.is_empty() || self.offset_table_count < 2 {
            return Err("VDF too small for section-6-guided mapping".into());
        }

        let stock_count = codes
            .iter()
            .skip(1)
            .filter(|&&c| c == VDF_SECTION6_OT_CODE_STOCK)
            .count();
        let nonstock_count = self.offset_table_count - 1 - stock_count;

        // Step 1-2: build candidate list from VDF name table.
        // Use broad filtering: remove structural metadata and builtins,
        // but keep TABLE names (they may have OT entries).
        // System variable names (INITIAL TIME, etc.) are included as
        // participants because they consume OT entries.

        // Collect stdlib call info from model for alias handling
        let mut model_stock_set: HashSet<String> = HashSet::new();
        let mut model_sig_stocks: HashSet<String> = HashSet::new();
        let mut model_sig_stock_names: HashSet<String> = HashSet::new();
        let mut aliases: Vec<(String, String)> = Vec::new();
        let mut alias_names_normalized: HashSet<String> = HashSet::new();

        for var in &model.variables {
            let (ident, equation) = match var {
                crate::datamodel::Variable::Stock(s) => {
                    model_stock_set.insert(normalize_vdf_name(&s.ident));
                    continue;
                }
                crate::datamodel::Variable::Aux(a) => (&a.ident, &a.equation),
                crate::datamodel::Variable::Flow(f) => (&f.ident, &f.equation),
                crate::datamodel::Variable::Module(_) => continue,
            };

            if let Some(info) = extract_stdlib_call_info(equation) {
                let output_sig = info.output_signature();
                aliases.push((ident.clone(), output_sig));
                alias_names_normalized.insert(normalize_vdf_name(ident));
                for (sig, is_stock) in info.vensim_signatures() {
                    let normalized = normalize_vdf_name(&sig);
                    if is_stock {
                        model_sig_stocks.insert(normalized);
                        model_sig_stock_names.insert(sig);
                    }
                }
            }
        }

        // Broad candidate extraction: include system variable names (they
        // occupy OT entries), but remove structural metadata, builtins,
        // module internals, and SMOOTH/DELAY aliases.
        let mut candidates: Vec<String> = Vec::new();
        let mut lookupish_candidates: Vec<String> = Vec::new();
        let mut seen_normalized: HashSet<String> = HashSet::new();

        for name in &self.names[..self.slot_table.len()] {
            if name.is_empty()
                || name.starts_with('.')
                || name.starts_with('-')
                || name.starts_with(':')
            {
                continue;
            }
            // Single-char non-alphanumeric placeholders
            if name.len() == 1 && name.starts_with(|c: char| !c.is_alphanumeric()) {
                continue;
            }
            if VENSIM_BUILTINS.iter().any(|b| b.eq_ignore_ascii_case(name)) {
                continue;
            }
            if is_vdf_metadata_entry(name) {
                continue;
            }
            if alias_names_normalized.contains(&normalize_vdf_name(name)) {
                continue;
            }
            // "Time" is OT[0], handled separately
            if name == "Time" {
                continue;
            }

            let normalized = normalize_vdf_name(name);
            if !seen_normalized.insert(normalized) {
                continue;
            }

            // Track lookupish names separately so we can trim them later
            let lower = name.to_lowercase();
            if lower.contains(" lookup") || lower.contains(" table") {
                lookupish_candidates.push(name.clone());
            } else {
                candidates.push(name.clone());
            }
        }

        // Add #-prefixed names from the unslotted tail (if any exist
        // beyond the slotted prefix; for fully-slotted VDFs like WRLD3,
        // these were already processed above).
        if self.names.len() > self.slot_table.len() {
            for name in &self.names[self.slot_table.len()..] {
                if name.starts_with('#') {
                    let normalized = normalize_vdf_name(name);
                    if seen_normalized.insert(normalized) {
                        candidates.push(name.clone());
                    }
                }
            }
        }

        // Strip outer quotes from quoted names like `"Absorption Land (GHA)"`
        for name in &self.names[..self.slot_table.len()] {
            if name.starts_with('"') && name.ends_with('"') && name.len() > 2 {
                let inner = &name[1..name.len() - 1];
                let normalized = normalize_vdf_name(inner);
                if !inner.is_empty()
                    && inner.chars().all(|c| c.is_ascii_graphic() || c == ' ')
                    && !alias_names_normalized.contains(&normalized)
                    && seen_normalized.insert(normalized)
                {
                    candidates.push(inner.to_string());
                }
            }
        }

        // Total target: stock_count + nonstock_count = OT capacity - 1
        let target = stock_count + nonstock_count;
        let base_count = candidates.len();

        // Include lookupish names to reach target count; exclude excess
        // starting with standalone LOOKUPs (always excludable), then
        // TABLE names alphabetically.
        let lookupish_needed = target.saturating_sub(base_count);
        if lookupish_needed > 0 {
            // Sort lookupish names: TABLE names first (more likely to be
            // saved), LOOKUP names last.
            lookupish_candidates.sort_by(|a, b| {
                let a_is_lookup = a.to_lowercase().contains(" lookup");
                let b_is_lookup = b.to_lowercase().contains(" lookup");
                a_is_lookup
                    .cmp(&b_is_lookup)
                    .then_with(|| a.to_lowercase().cmp(&b.to_lowercase()))
            });
            candidates.extend(lookupish_candidates.into_iter().take(lookupish_needed));
        }

        if candidates.len() != target {
            return Err(format!(
                "section-6-guided candidate count ({}) != OT capacity ({target}); \
                 base={base_count} lookupish_needed={lookupish_needed}",
                candidates.len()
            )
            .into());
        }

        // Step 3: classify each candidate as stock or non-stock
        let is_stock_name = |name: &str| -> bool {
            let normalized = normalize_vdf_name(name);
            model_stock_set.contains(&normalized) || model_sig_stocks.contains(&normalized)
        };

        // Classify using model stock set, #-prefixed signature patterns,
        // and stdlib helper stock classification.
        let is_stock = |name: &str| -> bool {
            // Model stocks
            if is_stock_name(name) {
                return true;
            }
            // #-prefixed internal signature: stock if starts with
            // #SMOOTH(, #SMOOTHI(, #SMOOTH3(, #SMOOTH3I(, #LV1<, #LV2<, #LV3<
            if name.starts_with('#') {
                return name.starts_with("#SMOOTH(")
                    || name.starts_with("#SMOOTHI(")
                    || name.starts_with("#SMOOTH3(")
                    || name.starts_with("#SMOOTH3I(")
                    || name.starts_with("#LV1<")
                    || name.starts_with("#LV2<")
                    || name.starts_with("#LV3<");
            }
            // Stdlib participant helpers: LV1/LV2/LV3/ST are stocks
            if STDLIB_PARTICIPANT_HELPERS.contains(&name) {
                return is_stdlib_helper_stock(name);
            }
            false
        };

        let mut stock_names: Vec<String> =
            candidates.iter().filter(|n| is_stock(n)).cloned().collect();
        let mut non_stock_names: Vec<String> = candidates
            .iter()
            .filter(|n| !is_stock(n))
            .cloned()
            .collect();

        // Step 4: reconcile stock/non-stock counts with section-6.
        // Model-based classification may over- or under-count stocks
        // relative to section-6 codes (ACTIVE INITIAL creates stock OTs
        // invisible to the model classifier, and some model stocks may
        // not be saved). Section-6 is authoritative for the boundary.
        stock_names.sort_by_key(|n| n.to_lowercase());
        non_stock_names.sort_by_key(|n| n.to_lowercase());

        // Reconcile: move names between groups to match section-6 counts.
        while stock_names.len() > stock_count && !stock_names.is_empty() {
            // Excess stocks: demote the alphabetically last stock to non-stock
            let demoted = stock_names.pop().unwrap();
            non_stock_names.push(demoted);
            non_stock_names.sort_by_key(|n| n.to_lowercase());
        }
        while stock_names.len() < stock_count && !non_stock_names.is_empty() {
            // Missing stocks: promote the alphabetically first non-stock
            // to stock. This is a heuristic -- without more information,
            // we can't know which non-stocks are actually stock-backed.
            let promoted = non_stock_names.remove(0);
            stock_names.push(promoted);
            stock_names.sort_by_key(|n| n.to_lowercase());
        }

        if stock_names.len() != stock_count || non_stock_names.len() != nonstock_count {
            return Err(format!(
                "could not reconcile stock/non-stock counts: \
                 stocks={}/{stock_count} nonstocks={}/{nonstock_count}",
                stock_names.len(),
                non_stock_names.len()
            )
            .into());
        }

        // Step 5: assign OT indices
        let mut mapping: HashMap<Ident<Canonical>, usize> = HashMap::new();
        mapping.insert(Ident::<Canonical>::from_str_unchecked("time"), 0);
        let mut participant_ots: HashMap<String, usize> = HashMap::new();

        // Stocks at OT[1..S]
        for (i, name) in stock_names.into_iter().enumerate() {
            let ot = i + 1;
            participant_ots.insert(normalize_vdf_name(&name), ot);
            let key = if name.starts_with('#') {
                Ident::<Canonical>::from_str_unchecked(&name)
            } else {
                Ident::<Canonical>::new(&name)
            };
            mapping.insert(key, ot);
        }

        // Non-stocks at OT[S+1..N-1]
        for (i, name) in non_stock_names.into_iter().enumerate() {
            let ot = stock_count + 1 + i;
            participant_ots.insert(normalize_vdf_name(&name), ot);
            let key = if name.starts_with('#') {
                Ident::<Canonical>::from_str_unchecked(&name)
            } else {
                Ident::<Canonical>::new(&name)
            };
            mapping.insert(key, ot);
        }

        // Step 6: map SMOOTH/DELAY user variables to their output OT
        for (user_ident, output_sig) in &aliases {
            let sig_normalized = normalize_vdf_name(output_sig);
            if let Some(&ot) = participant_ots.get(&sig_normalized) {
                let user_key = Ident::<Canonical>::new(user_ident);
                mapping.entry(user_key).or_insert(ot);
            }
        }

        Ok(mapping)
    }
}

// ---- Parsing functions ----

/// Find all sections in a VDF file by scanning for section magic bytes.
///
/// After collecting all sections, computes each section's `region_end`:
/// for sections 0..n-1, `region_end` is the next section's `file_offset`;
/// for the last section, `region_end` is the file length.
pub fn find_sections(data: &[u8]) -> Vec<Section> {
    let mut sections = Vec::new();
    let mut pos = 0;
    while pos + SECTION_HEADER_SIZE <= data.len() {
        if let Some(idx) = data[pos..].windows(4).position(|w| w == VDF_SECTION_MAGIC) {
            let offset = pos + idx;
            if offset + SECTION_HEADER_SIZE <= data.len() {
                sections.push(Section {
                    file_offset: offset,
                    region_end: 0, // filled in below
                    field1: read_u32(data, offset + 4),
                    field3: read_u32(data, offset + 12),
                    field4: read_u32(data, offset + 16),
                    field5: read_u32(data, offset + 20),
                });
            }
            pos = offset + 1;
        } else {
            break;
        }
    }

    // Compute region_end for each section: magic-to-magic boundaries.
    let n = sections.len();
    for i in 0..n {
        sections[i].region_end = if i + 1 < n {
            sections[i + 1].file_offset
        } else {
            data.len()
        };
    }

    sections
}

/// Parse the name table up to `parse_end`. The name table may extend past
/// the section header's size field within its region. `parse_end` should
/// be the section's `region_end`.
///
/// The first entry has no u16 length prefix; its length comes from field5's
/// high 16 bits. Subsequent entries are u16-length-prefixed. u16=0 entries
/// are group separators (skipped).
///
/// Validates each entry (max 256 bytes, printable ASCII) and stops when it
/// encounters data that doesn't look like a name entry.
pub fn parse_name_table_extended(data: &[u8], section: &Section, parse_end: usize) -> Vec<String> {
    let mut names = Vec::new();
    let data_start = section.data_offset();
    let parse_end = parse_end.min(data.len());

    let first_len = (section.field5 >> 16) as usize;
    if first_len == 0 || data_start + first_len > data.len() {
        return names;
    }
    let s: String = data[data_start..data_start + first_len]
        .iter()
        .take_while(|&&b| b != 0)
        .map(|&b| b as char)
        .collect();
    names.push(s);

    let mut pos = data_start + first_len;

    while pos + 2 <= parse_end {
        let len = read_u16(data, pos) as usize;
        pos += 2;
        if len == 0 {
            continue;
        }

        if pos + len > parse_end {
            break;
        }

        // Reject entries that are implausibly long -- real Vensim variable
        // names max out well under this, and the tail of the region may
        // contain non-name binary data whose first u16 decodes to a large
        // "length" value.
        const MAX_NAME_ENTRY_LEN: usize = 256;
        if len > MAX_NAME_ENTRY_LEN {
            break;
        }

        let s: String = data[pos..pos + len]
            .iter()
            .take_while(|&&b| b != 0)
            .map(|&b| b as char)
            .collect();

        if s.is_empty() || !s.chars().all(|c| c.is_ascii_graphic() || c == ' ') {
            break;
        }

        names.push(s);
        pos += len;
    }

    names
}

/// Find the name table section index. Heuristic: it's the section where
/// field5's high 16 bits give a plausible first-name length and the data
/// starts with printable ASCII text.
pub fn find_name_table_section_idx(data: &[u8], sections: &[Section]) -> Option<usize> {
    for (i, section) in sections.iter().enumerate() {
        let first_len = (section.field5 >> 16) as usize;
        if !(2..=64).contains(&first_len) {
            continue;
        }
        let start = section.data_offset();
        if start + first_len > data.len() {
            continue;
        }
        let text: String = data[start..start + first_len]
            .iter()
            .take_while(|&&b| b != 0)
            .map(|&b| b as char)
            .collect();
        if text.len() >= 2
            && text
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == ' ' || c == '_')
        {
            return Some(i);
        }
    }
    None
}

/// Find the slot lookup table. This is an array of u32 values (typically one per
/// name) located just before the name table section. The values are byte offsets
/// into section 1 data. For small models they have uniform stride (e.g., 16); for
/// larger models the stride may vary (variable-length slot metadata).
///
/// Returns (file_offset_of_table, values).
pub fn find_slot_table(
    data: &[u8],
    name_table_section: &Section,
    max_name_count: usize,
    section1_data_size: usize,
) -> (usize, Vec<u32>) {
    if max_name_count == 0 {
        return (0, Vec::new());
    }
    let end = name_table_section.file_offset;

    for gap in 0..20 {
        // Prefer the largest structurally valid table for this gap.
        for name_count in (1..=max_name_count).rev() {
            let table_size_bytes = name_count * 4;
            if end < gap + table_size_bytes {
                continue;
            }
            let table_start = end - gap - table_size_bytes;

            let values: Vec<u32> = (0..name_count)
                .map(|i| read_u32(data, table_start + i * 4))
                .collect();

            let mut sorted = values.clone();
            sorted.sort();
            sorted.dedup();

            if sorted.len() != name_count {
                continue;
            }

            let all_valid = sorted
                .iter()
                .all(|&v| v % 4 == 0 && v > 0 && (v as usize) < section1_data_size);
            if !all_valid {
                continue;
            }

            let strides: Vec<u32> = sorted.windows(2).map(|pair| pair[1] - pair[0]).collect();
            let min_stride = strides.iter().copied().min().unwrap_or(0);
            if min_stride >= 4 {
                return (table_start, values);
            }
        }
    }

    (0, Vec::new())
}

/// Find 64-byte variable records in the gap between section data and the
/// slot/name table region.
///
/// Records are identified by first finding a sentinel pair (two consecutive
/// 0xf6800000 values at byte offsets +32 and +36) to establish alignment,
/// then parsing ALL 64-byte blocks at that alignment. Some records lack
/// sentinels (e.g., lookup table entries or subscript elements) but are
/// still valid 64-byte records at the same stride.
pub fn find_records(data: &[u8], search_start: usize, search_end: usize) -> Vec<VdfRecord> {
    if search_start >= search_end {
        return Vec::new();
    }

    // Find first sentinel pair to establish alignment
    let mut first_record_start = None;
    let mut pos = search_start;
    while pos + 40 <= search_end {
        let v0 = read_u32(data, pos);
        let v1 = read_u32(data, pos + 4);
        if v0 == VDF_SENTINEL && v1 == VDF_SENTINEL {
            first_record_start = Some(pos.saturating_sub(32));
            break;
        }
        pos += 4;
    }

    let Some(rec_start) = first_record_start else {
        return Vec::new();
    };

    // Scan backwards to find records before the first sentinel we found,
    // but never before search_start (which marks the end of sec[1] data).
    let mut actual_start = rec_start;
    while actual_start >= RECORD_SIZE {
        let candidate = actual_start - RECORD_SIZE;
        if candidate < search_start {
            break;
        }
        let f0 = read_u32(data, candidate);
        let s0 = read_u32(data, candidate + 32);
        let s1 = read_u32(data, candidate + 36);
        if f0 <= 64 || s0 == VDF_SENTINEL || s1 == VDF_SENTINEL {
            actual_start = candidate;
        } else {
            break;
        }
    }

    let mut records = Vec::new();
    let mut offset = actual_start;
    while offset + RECORD_SIZE <= search_end {
        let mut fields = [0u32; 16];
        for (i, field) in fields.iter_mut().enumerate() {
            *field = read_u32(data, offset + i * 4);
        }
        records.push(VdfRecord {
            file_offset: offset,
            fields,
        });
        offset += RECORD_SIZE;
    }
    records
}

/// Find the first data block (time series). Identified by having u16 count
/// equal to time_point_count, a fully-set bitmap, a plausible first value,
/// and a monotonically increasing sequence (real time series always increase).
pub fn find_first_data_block(
    data: &[u8],
    time_point_count: usize,
    bitmap_size: usize,
) -> Option<usize> {
    let count_bytes = (time_point_count as u16).to_le_bytes();
    let full_block_size = 2 + bitmap_size + time_point_count * 4;
    let search_start = 0x100;
    for pos in search_start..data.len().saturating_sub(full_block_size) {
        if data[pos..pos + 2] != count_bytes {
            continue;
        }
        let bm = &data[pos + 2..pos + 2 + bitmap_size];
        let set_bits: usize = bm.iter().map(|b| b.count_ones() as usize).sum();
        if set_bits != time_point_count {
            continue;
        }
        let data_off = pos + 2 + bitmap_size;
        if data_off + time_point_count * 4 > data.len() {
            continue;
        }
        let first_val = read_f32(data, data_off);
        if !((0.0..2200.0).contains(&first_val)) {
            continue;
        }
        // Verify the sequence is monotonically increasing -- a real time
        // series must be. This eliminates false positives where a non-time
        // block happens to have a plausible first value (e.g. 0.0).
        let mut monotonic = true;
        let mut prev = first_val;
        for i in 1..time_point_count {
            let val = read_f32(data, data_off + i * 4);
            if val <= prev {
                monotonic = false;
                break;
            }
            prev = val;
        }
        if monotonic {
            return Some(pos);
        }
    }
    None
}

/// Find the offset table by scanning backwards from the first data block
/// for a u32 entry equal to first_block_offset (OT entry 0 = time block).
pub fn find_offset_table(data: &[u8], first_block_offset: usize) -> (usize, usize) {
    let target_bytes = (first_block_offset as u32).to_le_bytes();
    let mut pos = first_block_offset;
    while pos >= 4 {
        pos -= 4;
        if data[pos..pos + 4] == target_bytes {
            let count = (first_block_offset - pos) / 4;
            return (pos, count);
        }
    }
    (first_block_offset, 0)
}

/// Walk data blocks from the first block offset, returning (offset, count, block_size)
/// for each block found.
pub fn enumerate_data_blocks(
    data: &[u8],
    first_block_offset: usize,
    bitmap_size: usize,
    max_count: usize,
) -> Vec<(usize, usize, usize)> {
    let mut blocks = Vec::new();
    let mut offset = first_block_offset;
    while offset + 2 + bitmap_size <= data.len() {
        let count = read_u16(data, offset) as usize;
        if count == 0 || count > max_count * 2 {
            break;
        }
        let block_size = 2 + bitmap_size + count * 4;
        if offset + block_size > data.len() {
            break;
        }
        blocks.push((offset, count, block_size));
        offset += block_size;
    }
    blocks
}

// ---- VDF data extraction ----

/// Parsed VDF data before variable name assignment.
pub struct VdfData {
    /// Time values extracted from block 0 (e.g. [1900.0, 1900.5, ..., 2100.0]).
    pub time_values: Vec<f64>,
    /// Each entry is a time series (one f64 per time point) for one variable.
    /// Indexed by offset-table position. Entry 0 is always the time series.
    pub entries: Vec<Vec<f64>>,
}

/// Build an empirical mapping from canonical variable name to OT entry index.
///
/// Uses time-series correlation to match VDF data entries against a
/// reference simulation. Structural priors can pin known-good placements so
/// the matcher only reassigns series when the structural OT clearly disagrees.
pub fn build_empirical_ot_map(
    vdf: &VdfData,
    reference: &Results,
) -> StdResult<HashMap<Ident<Canonical>, usize>, Box<dyn Error>> {
    build_empirical_ot_map_with_prior(vdf, reference, None)
}

fn build_empirical_ot_map_with_prior(
    vdf: &VdfData,
    reference: &Results,
    prior: Option<&HashMap<Ident<Canonical>, usize>>,
) -> StdResult<HashMap<Ident<Canonical>, usize>, Box<dyn Error>> {
    let step_count = vdf.time_values.len();
    if step_count != reference.step_count {
        return Err(format!(
            "VDF has {step_count} time points but simulation has {}",
            reference.step_count
        )
        .into());
    }

    let sample_indices = build_sample_indices(step_count);
    const MATCH_THRESHOLD: f64 = 0.01;
    let time_ident = Ident::<Canonical>::from_str_unchecked("time");

    let mut claimed: Vec<bool> = vec![false; vdf.entries.len()];
    claimed[0] = true; // OT[0] is always the time series

    let mut ot_map = HashMap::new();
    ot_map.insert(time_ident.clone(), 0usize);
    let ordered_reference = sorted_reference_offsets(reference, &time_ident);

    for (ident, ref_off) in ordered_reference {
        let ref_series: Vec<f64> = (0..step_count)
            .map(|step| {
                let row =
                    &reference.data[step * reference.step_size..(step + 1) * reference.step_size];
                row[ref_off]
            })
            .collect();

        if let Some(&prior_ot) = prior.and_then(|map| map.get(&ident))
            && prior_ot < vdf.entries.len()
            && !claimed[prior_ot]
            && compute_match_error(&ref_series, &vdf.entries[prior_ot], &sample_indices)
                < MATCH_THRESHOLD
        {
            claimed[prior_ot] = true;
            ot_map.insert(ident, prior_ot);
            continue;
        }

        let mut best_entry = None;
        let mut best_error = f64::MAX;

        for (ei, entry) in vdf.entries.iter().enumerate() {
            if claimed[ei] {
                continue;
            }
            let error = compute_match_error(&ref_series, entry, &sample_indices);
            if error < best_error {
                best_error = error;
                best_entry = Some(ei);
            }
        }

        if let Some(ei) = best_entry
            && best_error < MATCH_THRESHOLD
        {
            claimed[ei] = true;
            ot_map.insert(ident, ei);
        }
    }

    Ok(ot_map)
}

fn sorted_reference_offsets(
    reference: &Results,
    time_ident: &Ident<Canonical>,
) -> Vec<(Ident<Canonical>, usize)> {
    let mut ordered: Vec<(Ident<Canonical>, usize)> = reference
        .offsets
        .iter()
        .filter(|(id, _)| **id != *time_ident)
        .map(|(id, &off)| (id.clone(), off))
        .collect();
    ordered.sort_by(|(a_id, a_off), (b_id, b_off)| {
        empirical_reference_priority(a_id)
            .cmp(&empirical_reference_priority(b_id))
            .then_with(|| a_off.cmp(b_off))
            .then_with(|| a_id.as_str().cmp(b_id.as_str()))
    });
    ordered
}

fn empirical_reference_priority(id: &Ident<Canonical>) -> u8 {
    let name = id.as_str();
    if name.starts_with('$') {
        2
    } else if name.starts_with('#') {
        1
    } else {
        0
    }
}

fn merge_empirical_ot_map(
    ot_map: &mut HashMap<Ident<Canonical>, usize>,
    empirical_map: HashMap<Ident<Canonical>, usize>,
) {
    let time_ident = Ident::<Canonical>::from_str_unchecked("time");
    let mut ordered_ids: Vec<Ident<Canonical>> = empirical_map
        .keys()
        .filter(|id| **id != time_ident)
        .cloned()
        .collect();
    ordered_ids.sort_by(|a, b| {
        empirical_reference_priority(a)
            .cmp(&empirical_reference_priority(b))
            .then_with(|| a.as_str().cmp(b.as_str()))
    });

    for id in ordered_ids {
        let Some(&emp_ot) = empirical_map.get(&id) else {
            continue;
        };
        if ot_map.get(&id).copied() == Some(emp_ot) {
            continue;
        }
        if empirical_claim_conflicts_with_visible_structure(&id, emp_ot, ot_map, &empirical_map) {
            continue;
        }
        ot_map.insert(id, emp_ot);
    }
}

fn empirical_claim_conflicts_with_visible_structure(
    id: &Ident<Canonical>,
    emp_ot: usize,
    ot_map: &HashMap<Ident<Canonical>, usize>,
    empirical_map: &HashMap<Ident<Canonical>, usize>,
) -> bool {
    ot_map.iter().any(|(other_id, &other_ot)| {
        other_id != id
            && other_ot == emp_ot
            && is_visible_results_ident(other_id)
            && empirical_map
                .get(other_id)
                .copied()
                .is_none_or(|other_emp_ot| other_emp_ot == emp_ot)
    })
}

fn is_visible_results_ident(id: &Ident<Canonical>) -> bool {
    let name = id.as_str();
    name != "time" && !name.starts_with('#') && !name.starts_with('$')
}

/// Build sample indices for time series correlation. Spread samples across the
/// run so long smooth series do not collide on only a few early points.
fn build_sample_indices(step_count: usize) -> Vec<usize> {
    let mut indices = vec![0usize];
    for numerator in 1..8usize {
        let idx = step_count.saturating_mul(numerator) / 8;
        if idx > 0 && idx + 1 < step_count {
            indices.push(idx);
        }
    }
    indices.push(step_count.saturating_sub(1));
    indices.sort_unstable();
    indices.dedup();
    indices
}

/// Compute a match error between a reference series and a VDF entry,
/// sampled at the given indices. Returns f64::MAX if the series lengths
/// don't match.
fn compute_match_error(reference: &[f64], candidate: &[f64], sample_indices: &[usize]) -> f64 {
    if reference.len() != candidate.len() {
        return f64::MAX;
    }

    let mut total_error = 0.0;
    for &idx in sample_indices {
        let r = reference[idx];
        let c = candidate[idx];

        if r.is_nan() || c.is_nan() {
            return f64::MAX;
        }

        let scale = r.abs().max(c.abs()).max(1e-10);
        let rel_err = ((r - c) / scale).abs();
        total_error += rel_err;
    }

    total_error / sample_indices.len() as f64
}

/// Extract the time series values from the first data block (block 0).
fn extract_time_series(
    data: &[u8],
    block_offset: usize,
    time_point_count: usize,
    bitmap_size: usize,
) -> StdResult<Vec<f64>, Box<dyn Error>> {
    let count = u16::from_le_bytes(data[block_offset..block_offset + 2].try_into()?) as usize;
    if count != time_point_count {
        return Err(format!("time block count {count} != expected {time_point_count}").into());
    }
    let data_start = block_offset + 2 + bitmap_size;
    let mut times = Vec::with_capacity(count);
    for i in 0..count {
        let off = data_start + i * 4;
        let val = f32::from_le_bytes(data[off..off + 4].try_into()?) as f64;
        times.push(val);
    }
    Ok(times)
}

/// Extract a full time series from a VDF data block, producing one value
/// per time point. Uses zero-order hold for time points without stored values.
fn extract_block_series(
    data: &[u8],
    block_offset: usize,
    bitmap_size: usize,
    time_values: &[f64],
) -> StdResult<Vec<f64>, Box<dyn Error>> {
    let step_count = time_values.len();
    let count = u16::from_le_bytes(data[block_offset..block_offset + 2].try_into()?) as usize;
    let bm = &data[block_offset + 2..block_offset + 2 + bitmap_size];
    let data_start = block_offset + 2 + bitmap_size;

    let mut series = vec![f64::NAN; step_count];
    let mut data_idx = 0;
    let mut last_val = f64::NAN;

    // time_values is the full evenly-spaced time grid from block 0, so
    // time_idx corresponds directly to the step index.
    for (time_idx, _) in time_values.iter().enumerate() {
        let bit_set = (bm[time_idx / 8] >> (time_idx % 8)) & 1 == 1;
        if bit_set && data_idx < count {
            let off = data_start + data_idx * 4;
            last_val = f32::from_le_bytes(data[off..off + 4].try_into()?) as f64;
            data_idx += 1;
        }

        series[time_idx] = last_val;
    }

    Ok(series)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_u32() {
        let data = [0x01, 0x00, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0xFF];
        assert_eq!(read_u32(&data, 0), 1);
        assert_eq!(read_u32(&data, 4), 0xFFFFFFFF);
    }

    #[test]
    fn test_read_u16() {
        let data = [0x01, 0x00, 0xFF, 0xFF];
        assert_eq!(read_u16(&data, 0), 1);
        assert_eq!(read_u16(&data, 2), 0xFFFF);
    }

    #[test]
    fn test_read_f32() {
        let data = 1.0f32.to_le_bytes();
        assert_eq!(read_f32(&data, 0), 1.0);
    }

    #[test]
    fn test_section_data_offset_and_region() {
        let s = Section {
            file_offset: 100,
            field1: 0,
            region_end: 300,
            field3: 0,
            field4: 0,
            field5: 0,
        };
        assert_eq!(s.data_offset(), 124);
        assert_eq!(s.region_data_size(), 176); // 300 - 124
    }

    #[test]
    fn test_section_degenerate_region() {
        // Section 5 in small models has its next section's header
        // starting before data_offset, yielding region_data_size() == 0.
        let s = Section {
            file_offset: 100,
            field1: 0,
            region_end: 118, // next section starts before data_offset (124)
            field3: 0,
            field4: 0,
            field5: 0,
        };
        assert_eq!(s.data_offset(), 124);
        assert_eq!(s.region_data_size(), 0);
    }

    #[test]
    fn test_find_sections_empty() {
        let data = vec![0u8; 100];
        assert!(find_sections(&data).is_empty());
    }

    #[test]
    fn test_find_sections_with_magic() {
        let mut data = vec![0u8; 100];
        data[10..14].copy_from_slice(&VDF_SECTION_MAGIC);
        // size at offset+4
        data[14..18].copy_from_slice(&10u32.to_le_bytes());
        // size2 at offset+8
        data[18..22].copy_from_slice(&10u32.to_le_bytes());
        // field3 at offset+12
        data[22..26].copy_from_slice(&500u32.to_le_bytes());
        // field4 at offset+16
        data[26..30].copy_from_slice(&2u32.to_le_bytes());
        // field5 at offset+20
        data[30..34].copy_from_slice(&0u32.to_le_bytes());

        let sections = find_sections(&data);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].file_offset, 10);
        assert_eq!(sections[0].field4, 2);
        // Last section's region_end should be the file length
        assert_eq!(sections[0].region_end, data.len());
    }

    #[test]
    fn test_find_sections_multi_contiguous_regions() {
        let mut data = vec![0u8; 200];

        // Section 0 at offset 10
        data[10..14].copy_from_slice(&VDF_SECTION_MAGIC);
        data[14..18].copy_from_slice(&5u32.to_le_bytes());
        data[18..22].copy_from_slice(&5u32.to_le_bytes());

        // Section 1 at offset 60
        data[60..64].copy_from_slice(&VDF_SECTION_MAGIC);
        data[64..68].copy_from_slice(&20u32.to_le_bytes());
        data[68..72].copy_from_slice(&20u32.to_le_bytes());

        // Section 2 at offset 150
        data[150..154].copy_from_slice(&VDF_SECTION_MAGIC);
        data[154..158].copy_from_slice(&8u32.to_le_bytes());
        data[158..162].copy_from_slice(&8u32.to_le_bytes());

        let sections = find_sections(&data);
        assert_eq!(sections.len(), 3);

        // Regions should be contiguous
        assert_eq!(sections[0].region_end, sections[1].file_offset);
        assert_eq!(sections[1].region_end, sections[2].file_offset);
        assert_eq!(sections[2].region_end, data.len());

        // Region data sizes
        assert_eq!(sections[0].region_data_size(), 60 - 34); // 60 - (10 + 24)
        assert_eq!(sections[1].region_data_size(), 150 - 84); // 150 - (60 + 24)
        assert_eq!(sections[2].region_data_size(), 200 - 174); // 200 - (150 + 24)
    }

    #[test]
    fn test_vdf_record_accessors() {
        let mut fields = [0u32; 16];
        fields[11] = 42;
        fields[12] = 100;
        let rec = VdfRecord {
            file_offset: 0,
            fields,
        };
        assert_eq!(rec.ot_index(), 42);
        assert_eq!(rec.slot_ref(), 100);
    }

    #[test]
    fn test_enumerate_data_blocks_empty() {
        let data = vec![0u8; 100];
        let blocks = enumerate_data_blocks(&data, 0, 8, 100);
        assert!(blocks.is_empty());
    }

    #[test]
    fn test_parse_name_table_extended_multiple_names() {
        let mut data = vec![0u8; 256];

        data[0..4].copy_from_slice(&VDF_SECTION_MAGIC);
        data[4..8].copy_from_slice(&20u32.to_le_bytes());
        data[8..12].copy_from_slice(&20u32.to_le_bytes());
        data[12..16].copy_from_slice(&500u32.to_le_bytes());
        data[16..20].copy_from_slice(&0u32.to_le_bytes());
        // field5: first name length = 6 in high 16 bits
        data[20..24].copy_from_slice(&(6u32 << 16).to_le_bytes());

        // Data starts at offset 24
        // First name: "Time\0\0" (6 bytes, no u16 prefix)
        data[24..28].copy_from_slice(b"Time");
        data[28..30].copy_from_slice(&[0, 0]);

        // Second name: u16 len = 14, "hello world\0\0\0"
        data[30..32].copy_from_slice(&14u16.to_le_bytes());
        data[32..43].copy_from_slice(b"hello world");
        data[43..46].copy_from_slice(&[0, 0, 0]);

        // Third name: u16 len = 8, "foo\0\0\0\0\0" at offset 46
        data[46..48].copy_from_slice(&8u16.to_le_bytes());
        data[48..51].copy_from_slice(b"foo");
        data[51..56].copy_from_slice(&[0, 0, 0, 0, 0]);

        let section = Section {
            file_offset: 0,
            field1: 0,
            region_end: 80,
            field3: 500,
            field4: 0,
            field5: 6u32 << 16,
        };

        let names = parse_name_table_extended(&data, &section, 80);
        assert_eq!(names.len(), 3);
        assert_eq!(names[0], "Time");
        assert_eq!(names[1], "hello world");
        assert_eq!(names[2], "foo");
    }

    #[test]
    fn test_parse_name_table_extended_stops_on_invalid_data() {
        let mut data = vec![0u8; 256];

        data[0..4].copy_from_slice(&VDF_SECTION_MAGIC);
        data[4..8].copy_from_slice(&40u32.to_le_bytes());
        data[8..12].copy_from_slice(&40u32.to_le_bytes());
        data[12..16].copy_from_slice(&500u32.to_le_bytes());
        data[16..20].copy_from_slice(&0u32.to_le_bytes());
        data[20..24].copy_from_slice(&(6u32 << 16).to_le_bytes());

        // First name at offset 24: "Time\0\0"
        data[24..28].copy_from_slice(b"Time");
        data[28..30].copy_from_slice(&[0, 0]);

        // Second name at offset 30: u16=10, "test var\0\0"
        data[30..32].copy_from_slice(&10u16.to_le_bytes());
        data[32..40].copy_from_slice(b"test var");
        data[40..42].copy_from_slice(&[0, 0]);

        // After section data ends at 64, put some binary garbage
        // u16 = 500 (> 256 max)
        data[64..66].copy_from_slice(&500u16.to_le_bytes());
        data[66..70].copy_from_slice(&[0xff, 0xff, 0xff, 0xff]);

        let section = Section {
            file_offset: 0,
            field1: 0,
            region_end: 200,
            field3: 500,
            field4: 0,
            field5: 6u32 << 16,
        };

        let names = parse_name_table_extended(&data, &section, 200);
        assert_eq!(names.len(), 2);
        assert_eq!(names[0], "Time");
        assert_eq!(names[1], "test var");
    }

    #[test]
    fn test_parse_name_table_extended_skips_separators() {
        let mut data = vec![0u8; 256];

        data[0..4].copy_from_slice(&VDF_SECTION_MAGIC);
        data[4..8].copy_from_slice(&10u32.to_le_bytes());
        data[8..12].copy_from_slice(&10u32.to_le_bytes());
        data[12..16].copy_from_slice(&500u32.to_le_bytes());
        data[16..20].copy_from_slice(&0u32.to_le_bytes());
        data[20..24].copy_from_slice(&(6u32 << 16).to_le_bytes());

        // First name: "Time\0\0" (6 bytes)
        data[24..28].copy_from_slice(b"Time");
        data[28..30].copy_from_slice(&[0, 0]);

        // u16 separator at offset 30
        data[30..32].copy_from_slice(&0u16.to_le_bytes());
        // u16 separator at offset 32
        data[32..34].copy_from_slice(&0u16.to_le_bytes());

        // Another name: u16=6, "abc\0\0\0"
        data[34..36].copy_from_slice(&6u16.to_le_bytes());
        data[36..39].copy_from_slice(b"abc");
        data[39..42].copy_from_slice(&[0, 0, 0]);

        let section = Section {
            file_offset: 0,
            field1: 0,
            region_end: 80,
            field3: 500,
            field4: 0,
            field5: 6u32 << 16,
        };

        let names = parse_name_table_extended(&data, &section, 80);
        assert_eq!(names.len(), 2);
        assert_eq!(names[0], "Time");
        assert_eq!(names[1], "abc");
    }

    fn vdf_file(path: &str) -> VdfFile {
        let data = std::fs::read(path)
            .unwrap_or_else(|e| panic!("failed to read VDF file {}: {}", path, e));
        VdfFile::parse(data).unwrap_or_else(|e| panic!("failed to parse VDF file {}: {}", path, e))
    }

    fn remove_model_var(project: &mut crate::datamodel::Project, ident: &str) {
        let Some(model) = project.models.first_mut() else {
            panic!("project missing main model");
        };
        let before = model.variables.len();
        model.variables.retain(|var| var.get_ident() != ident);
        assert_ne!(
            model.variables.len(),
            before,
            "expected to remove variable {ident}"
        );
    }

    /// For each possible trailing gap size, find the largest table length
    /// that passes slot-table structural validation.
    fn best_slot_candidate_count(vdf: &VdfFile, gap: usize) -> usize {
        let Some(ns_idx) = vdf.name_section_idx else {
            return 0;
        };
        let sec1_data_size = vdf
            .sections
            .get(1)
            .map(|s| s.region_data_size())
            .unwrap_or(0);
        let end = vdf.sections[ns_idx].file_offset;
        let mut best = 0;

        for n in 1..=vdf.names.len() {
            let table_size_bytes = n * 4;
            if end < gap + table_size_bytes {
                break;
            }
            let table_start = end - gap - table_size_bytes;
            let values: Vec<u32> = (0..n)
                .map(|i| read_u32(&vdf.data, table_start + i * 4))
                .collect();

            let mut sorted = values.clone();
            sorted.sort();
            sorted.dedup();
            if sorted.len() != n {
                continue;
            }

            let all_valid = sorted
                .iter()
                .all(|&v| v % 4 == 0 && v > 0 && (v as usize) < sec1_data_size);
            if !all_valid {
                continue;
            }

            let min_stride = sorted
                .windows(2)
                .map(|pair| pair[1] - pair[0])
                .min()
                .unwrap_or(0);
            if min_stride >= 4 {
                best = n;
            }
        }

        best
    }

    fn normalized_stock_backed_outputs(project: &crate::datamodel::Project) -> HashSet<String> {
        let Some(model) = project.models.first() else {
            return HashSet::new();
        };

        let mut names = HashSet::new();
        for var in &model.variables {
            match var {
                crate::datamodel::Variable::Stock(stock) => {
                    names.insert(normalize_vdf_name(&stock.ident));
                }
                crate::datamodel::Variable::Aux(aux) => {
                    if extract_stdlib_call_info(&aux.equation)
                        .is_some_and(|info| info.output_is_stock())
                    {
                        names.insert(normalize_vdf_name(&aux.ident));
                    }
                }
                crate::datamodel::Variable::Flow(flow) => {
                    if extract_stdlib_call_info(&flow.equation)
                        .is_some_and(|info| info.output_is_stock())
                    {
                        names.insert(normalize_vdf_name(&flow.ident));
                    }
                }
                crate::datamodel::Variable::Module(_) => {}
            }
        }

        names
    }

    #[test]
    fn test_slot_candidate_counts_by_gap() {
        let water = vdf_file("../../test/bobby/vdf/water/Current.vdf");
        let econ = vdf_file("../../test/bobby/vdf/econ/base.vdf");
        let wrld3 = vdf_file("../../test/metasd/WRLD3-03/SCEN01.VDF");

        // Small files: known count is exact at gap=4 (marker 0x00430000).
        assert_eq!(best_slot_candidate_count(&water, 4), water.slot_table.len());

        // Medium/large files now parse the largest valid slot table.
        assert_eq!(best_slot_candidate_count(&econ, 4), econ.slot_table.len());
        assert_eq!(best_slot_candidate_count(&wrld3, 4), wrld3.slot_table.len());
        assert!(
            econ.slot_table.len() > 42,
            "econ should parse more than the old 42-slot truncation"
        );
        assert_eq!(wrld3.slot_table.len(), 404);
    }

    // ---- Section-6 tests (previously in vdf/tests/section6_tests.rs) ----

    #[test]
    fn test_section6_ref_stream_counts() {
        let water = vdf_file("../../test/bobby/vdf/water/Current.vdf");
        let pop = vdf_file("../../test/bobby/vdf/pop/Current.vdf");
        let econ = vdf_file("../../test/bobby/vdf/econ/base.vdf");
        let wrld3 = vdf_file("../../test/metasd/WRLD3-03/SCEN01.VDF");

        let (skip_w, entries_w, _) = water.parse_section6_ref_stream().unwrap();
        let (skip_p, entries_p, _) = pop.parse_section6_ref_stream().unwrap();
        let (skip_e, entries_e, _) = econ.parse_section6_ref_stream().unwrap();
        let (skip_r, entries_r, _) = wrld3.parse_section6_ref_stream().unwrap();

        assert_eq!(skip_w, 0);
        assert_eq!(entries_w.len(), 7);
        assert_eq!(skip_p, 0);
        assert_eq!(entries_p.len(), 8);
        assert_eq!(skip_e, 1);
        assert_eq!(entries_e.len(), 79);
        assert_eq!(skip_r, 1);
        assert_eq!(entries_r.len(), 342);
    }

    #[test]
    fn test_section6_ot_class_codes_have_expected_shape() {
        let water = vdf_file("../../test/bobby/vdf/water/Current.vdf");
        let pop = vdf_file("../../test/bobby/vdf/pop/Current.vdf");
        let econ = vdf_file("../../test/bobby/vdf/econ/base.vdf");
        let wrld3 = vdf_file("../../test/metasd/WRLD3-03/SCEN01.VDF");

        let water_codes = water.section6_ot_class_codes().unwrap();
        let pop_codes = pop.section6_ot_class_codes().unwrap();
        let econ_codes = econ.section6_ot_class_codes().unwrap();
        let wrld3_codes = wrld3.section6_ot_class_codes().unwrap();

        assert_eq!(water_codes.len(), water.offset_table_count);
        assert_eq!(pop_codes.len(), pop.offset_table_count);
        assert_eq!(econ_codes.len(), econ.offset_table_count);
        assert_eq!(wrld3_codes.len(), wrld3.offset_table_count);

        assert_eq!(water_codes[0], VDF_SECTION6_OT_CODE_TIME);
        assert_eq!(pop_codes[0], VDF_SECTION6_OT_CODE_TIME);
        assert_eq!(econ_codes[0], VDF_SECTION6_OT_CODE_TIME);
        assert_eq!(wrld3_codes[0], VDF_SECTION6_OT_CODE_TIME);

        assert_eq!(
            water_codes,
            vec![0x0f, 0x08, 0x17, 0x17, 0x17, 0x11, 0x11, 0x17, 0x11, 0x17]
        );
        assert_eq!(
            pop_codes,
            vec![
                0x0f, 0x08, 0x08, 0x17, 0x11, 0x17, 0x11, 0x17, 0x17, 0x17, 0x11, 0x17, 0x17,
            ]
        );
        assert_eq!(
            econ_codes
                .iter()
                .filter(|&&code| code == VDF_SECTION6_OT_CODE_STOCK)
                .count(),
            11
        );
        assert_eq!(
            wrld3_codes
                .iter()
                .filter(|&&code| code == VDF_SECTION6_OT_CODE_STOCK)
                .count(),
            41
        );
    }

    #[test]
    fn test_section6_final_values_match_extracted_last_values() {
        let models = [
            ("water", "../../test/bobby/vdf/water/Current.vdf"),
            ("pop", "../../test/bobby/vdf/pop/Current.vdf"),
            ("econ", "../../test/bobby/vdf/econ/base.vdf"),
            ("wrld3", "../../test/metasd/WRLD3-03/SCEN01.VDF"),
        ];

        for (label, vdf_path) in models {
            let vdf = vdf_file(vdf_path);
            let final_values = vdf.section6_ot_final_values().unwrap();
            let data = vdf.extract_data().unwrap();

            assert_eq!(
                final_values.len(),
                data.entries.len(),
                "{label}: final-value vector length should match OT/data entries",
            );

            for (ot, (final_value, series)) in
                final_values.iter().zip(data.entries.iter()).enumerate()
            {
                let expected = series.last().copied().unwrap_or(f64::NAN) as f32;
                assert!(
                    (final_value - expected).abs() < 1e-5
                        || (final_value.is_nan() && expected.is_nan()),
                    "{label}: OT[{ot}] final value mismatch: parsed={final_value} expected={expected}",
                );
            }
        }
    }

    #[test]
    fn test_section6_display_record_stream_shape() {
        let water = vdf_file("../../test/bobby/vdf/water/Current.vdf");
        let pop = vdf_file("../../test/bobby/vdf/pop/Current.vdf");
        let econ = vdf_file("../../test/bobby/vdf/econ/base.vdf");
        let wrld3 = vdf_file("../../test/metasd/WRLD3-03/SCEN01.VDF");

        let water_records = water.section6_display_records().unwrap();
        let pop_records = pop.section6_display_records().unwrap();
        let econ_records = econ.section6_display_records().unwrap();
        let wrld3_records = wrld3.section6_display_records().unwrap();

        assert!(
            water_records.is_empty(),
            "water should have no parsed display records"
        );
        assert!(
            pop_records.is_empty(),
            "pop should have no parsed display records"
        );
        assert_eq!(
            econ_records.len(),
            4,
            "econ display-record count should be stable"
        );
        assert_eq!(
            wrld3_records.len(),
            55,
            "WRLD3 display-record count should be stable"
        );

        for (label, vdf, records) in [
            ("econ", &econ, &econ_records),
            ("wrld3", &wrld3, &wrld3_records),
        ] {
            for rec in records {
                assert!(
                    rec.ot_index() < vdf.offset_table_count,
                    "{label}: display record OT {} out of range",
                    rec.ot_index()
                );
                assert_eq!(
                    rec.words[11], 1,
                    "{label}: expected stable display-record flag"
                );
                assert_eq!(rec.words[12], 0, "{label}: expected zero terminator word");
            }
        }
    }

    #[test]
    fn test_to_results_with_model_uses_vdf_visible_names_only() {
        for (mdl_path, vdf_path) in [
            (
                "../../test/bobby/vdf/water/water.mdl",
                "../../test/bobby/vdf/water/Current.vdf",
            ),
            (
                "../../test/bobby/vdf/pop/pop.mdl",
                "../../test/bobby/vdf/pop/Current.vdf",
            ),
        ] {
            let contents = std::fs::read_to_string(mdl_path).unwrap();
            let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
            let project = std::rc::Rc::new(crate::Project::from(datamodel_project));
            let vdf = vdf_file(vdf_path);

            let results = vdf
                .to_results_with_model(&project, "main")
                .unwrap_or_else(|e| panic!("to_results_with_model failed: {e}"));

            assert!(
                results.offsets.keys().all(|id| {
                    let name = id.as_str();
                    name == "time"
                        || (!name.starts_with('#')
                            && !name.starts_with('$')
                            && !is_probable_lookup_table_name(name))
                }),
                "expected Results offsets to expose only visible VDF names"
            );
        }
    }

    #[test]
    fn test_to_results_with_model_succeeds_on_econ() {
        let mdl_path = "../../test/bobby/vdf/econ/mark2.mdl";
        let vdf_path = "../../test/bobby/vdf/econ/base.vdf";

        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let project = std::rc::Rc::new(crate::Project::from(datamodel_project));
        let vdf = vdf_file(vdf_path);

        let results = vdf
            .to_results_with_model(&project, "main")
            .unwrap_or_else(|e| panic!("to_results_with_model should succeed on econ: {e}"));
        assert!(
            results.offsets.len() > 1,
            "econ: expected mapped variable columns"
        );
    }

    fn visible_result_names(results: &crate::Results) -> std::collections::BTreeSet<String> {
        results
            .offsets
            .keys()
            .map(|id| id.as_str().to_owned())
            .collect()
    }

    fn constant_column_value(results: &crate::Results, id: &Ident<Canonical>) -> f64 {
        let offset = results
            .offsets
            .get(id)
            .copied()
            .unwrap_or_else(|| panic!("missing Results column for {}", id.as_str()));
        let first = results.data[offset];
        for step in 1..results.step_count {
            let value = results.data[step * results.step_size + offset];
            assert!(
                (value - first).abs() < 1e-9,
                "{} should be flat across the saved run",
                id.as_str()
            );
        }
        first
    }

    #[test]
    fn test_to_results_with_model_includes_scalar_consts_but_not_lookup_tables() {
        let mdl_path = "../../test/bobby/vdf/consts/consts.mdl";
        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let project = std::rc::Rc::new(crate::Project::from(datamodel_project));

        for (label, vdf_path, expected_stock_final, expected_net_flow) in [
            (
                "consts-b-is-3",
                "../../test/bobby/vdf/consts/b_is_3.vdf",
                617.5,
                6.12,
            ),
            (
                "consts-b-is-4",
                "../../test/bobby/vdf/consts/b_is_4.vdf",
                717.5,
                7.12,
            ),
        ] {
            let vdf = vdf_file(vdf_path);
            let results = vdf
                .to_results_with_model(&project, "main")
                .unwrap_or_else(|e| panic!("{label}: to_results_with_model failed: {e}"));

            let names = visible_result_names(&results);
            let expected = std::collections::BTreeSet::from([
                "time".to_owned(),
                "a".to_owned(),
                "a_stock".to_owned(),
                "b".to_owned(),
                "c".to_owned(),
                "d".to_owned(),
                "net_flow".to_owned(),
            ]);
            assert_eq!(
                names, expected,
                "{label}: Results should expose time plus all non-control, non-lookup model variables",
            );

            assert!(
                !results
                    .offsets
                    .contains_key(&Ident::<Canonical>::new("graphical_function")),
                "{label}: lookup definitions should not become Results columns",
            );

            assert!((constant_column_value(&results, &Ident::new("a")) - 1.0).abs() < 1e-9);

            let b_value = constant_column_value(&results, &Ident::new("b"));
            let c_value = constant_column_value(&results, &Ident::new("c"));
            let mut scalar_pair = [b_value, c_value];
            scalar_pair.sort_by(|lhs, rhs| lhs.partial_cmp(rhs).unwrap());
            assert_eq!(
                scalar_pair,
                [3.0, if label.ends_with("4") { 4.0 } else { 3.0 }],
                "{label}: expected scalar constants to survive in Results",
            );

            let d_value = constant_column_value(&results, &Ident::new("d"));
            assert!(
                d_value.is_finite(),
                "{label}: computed auxiliary constant should remain available"
            );

            assert!(
                (constant_column_value(&results, &Ident::new("net_flow")) - expected_net_flow)
                    .abs()
                    < 1e-6,
                "{label}: expected net_flow constant from the VDF",
            );

            let stock_off = results.offsets[&Ident::new("a_stock")];
            let stock_start = results.data[stock_off];
            let stock_final =
                results.data[(results.step_count - 1) * results.step_size + stock_off];
            assert!(
                (stock_start - 5.5).abs() < 1e-9,
                "{label}: unexpected a_stock initial value"
            );
            assert!(
                (stock_final - expected_stock_final).abs() < 1e-6,
                "{label}: unexpected a_stock final value",
            );
        }
    }

    fn result_series(results: &crate::Results, offset: usize) -> Vec<f64> {
        results.iter().map(|row| row[offset]).collect()
    }

    fn sampled_series_error(
        actual: &crate::Results,
        reference: &crate::Results,
        id: &Ident<Canonical>,
    ) -> Option<f64> {
        let actual_off = *actual.offsets.get(id)?;
        let reference_off = *reference.offsets.get(id)?;
        let sample_indices = build_sample_indices(actual.step_count);
        let actual_series = result_series(actual, actual_off);
        let reference_series = result_series(reference, reference_off);
        Some(compute_match_error(
            &reference_series,
            &actual_series,
            &sample_indices,
        ))
    }

    fn matching_visible_series_count(
        actual: &crate::Results,
        reference: &crate::Results,
    ) -> (usize, usize) {
        let mut shared = 0usize;
        let mut matching = 0usize;

        for id in actual.offsets.keys() {
            let name = id.as_str();
            if name == "time" || name.starts_with('$') || name.starts_with('#') {
                continue;
            }

            let Some(error) = sampled_series_error(actual, reference, id) else {
                continue;
            };
            shared += 1;
            if error < 0.01 {
                matching += 1;
            }
        }

        (shared, matching)
    }

    #[test]
    fn test_to_results_with_model_matches_wrld3_reference_outputs() {
        let mdl_path = "../../test/metasd/WRLD3-03/wrld3-03.mdl";
        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let project = std::rc::Rc::new(crate::Project::from(datamodel_project));
        let reference = crate::interpreter::Simulation::new(&project, "main")
            .unwrap()
            .run_to_end()
            .unwrap();

        for (label, vdf_path) in [
            ("wrld3-scen01", "../../test/metasd/WRLD3-03/SCEN01.VDF"),
            (
                "wrld3-experiment",
                "../../test/metasd/WRLD3-03/experiment.vdf",
            ),
        ] {
            let vdf = vdf_file(vdf_path);
            let results = vdf
                .to_results_with_model(&project, "main")
                .unwrap_or_else(|e| panic!("{label}: to_results_with_model failed: {e}"));

            let (shared, matching) = matching_visible_series_count(&results, &reference);
            assert!(
                shared >= 230,
                "{label}: expected broad visible overlap with the simulation reference, got {shared}",
            );
            assert!(
                matching >= 225,
                "{label}: expected many WRLD3 series to match the simulation reference, got {matching} of {shared}",
            );

            for name in [
                "population",
                "food",
                "industrial_output",
                "persistent_pollution_index",
                "nonrenewable_resources",
            ] {
                let id = Ident::<Canonical>::new(name);
                let error = sampled_series_error(&results, &reference, &id)
                    .unwrap_or_else(|| panic!("{label}: missing sampled comparison for {name}"));
                assert!(
                    error < 0.01,
                    "{label}: expected {name} to match the simulation reference, error={error}",
                );
            }
        }
    }

    #[test]
    fn test_section6_stock_code_matches_small_model_stock_ots() {
        let models = [
            (
                "../../test/bobby/vdf/water/water.mdl",
                "../../test/bobby/vdf/water/Current.vdf",
            ),
            (
                "../../test/bobby/vdf/pop/pop.mdl",
                "../../test/bobby/vdf/pop/Current.vdf",
            ),
        ];

        for (mdl_path, vdf_path) in models {
            let contents = std::fs::read_to_string(mdl_path).unwrap();
            let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
            let stock_backed = normalized_stock_backed_outputs(&datamodel_project);
            let vdf = vdf_file(vdf_path);
            let codes = vdf.section6_ot_class_codes().unwrap();
            let sf_map = vdf.build_stocks_first_ot_map(&datamodel_project).unwrap();

            for (name, ot) in sf_map {
                if name.as_str() == "time" {
                    assert_eq!(codes[ot], VDF_SECTION6_OT_CODE_TIME);
                    continue;
                }

                let is_stock_backed = stock_backed.contains(&normalize_vdf_name(name.as_str()));
                assert_eq!(
                    codes[ot] == VDF_SECTION6_OT_CODE_STOCK,
                    is_stock_backed,
                    "{vdf_path}: expected OT[{ot}] {} to be stock_backed={is_stock_backed}, codes={codes:?}",
                    name.as_str()
                );
            }
        }
    }

    #[test]
    fn test_section6_stock_code_matches_empirical_econ_results() {
        let mdl_path = "../../test/bobby/vdf/econ/mark2.mdl";
        let vdf_path = "../../test/bobby/vdf/econ/base.vdf";

        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let stock_backed = normalized_stock_backed_outputs(&datamodel_project);

        let project = std::rc::Rc::new(crate::Project::from(datamodel_project));
        let sim = crate::interpreter::Simulation::new(&project, "main").unwrap();
        let results = sim.run_to_end().unwrap();

        let vdf = vdf_file(vdf_path);
        let codes = vdf.section6_ot_class_codes().unwrap();
        let vdf_data = vdf.extract_data().unwrap();
        let empirical = build_empirical_ot_map(&vdf_data, &results).unwrap();

        for (name, &ot) in &empirical {
            if name.as_str() == "time" {
                assert_eq!(codes[ot], VDF_SECTION6_OT_CODE_TIME);
                continue;
            }
            let expected_stock = stock_backed.contains(&normalize_vdf_name(name.as_str()));
            assert_eq!(
                codes[ot] == VDF_SECTION6_OT_CODE_STOCK,
                expected_stock,
                "econ: OT[{ot}] {} expected stock_backed={expected_stock}",
                name.as_str()
            );
        }
    }

    #[test]
    fn test_section5_set_stream_counts() {
        let water = vdf_file("../../test/bobby/vdf/water/Current.vdf");
        let econ = vdf_file("../../test/bobby/vdf/econ/base.vdf");
        let wrld3 = vdf_file("../../test/metasd/WRLD3-03/SCEN01.VDF");

        let (_, sets_w, _) = water.parse_section5_set_stream().unwrap();
        let (_, sets_e, _) = econ.parse_section5_set_stream().unwrap();
        let (_, sets_r, _) = wrld3.parse_section5_set_stream().unwrap();

        assert!(sets_w.is_empty());
        assert!(sets_e.is_empty());
        assert!(sets_r.is_empty());
    }

    #[test]
    fn test_record_ot_ranges_partition_selected_files() {
        let water = vdf_file("../../test/bobby/vdf/water/Current.vdf");
        let econ = vdf_file("../../test/bobby/vdf/econ/base.vdf");
        let wrld3 = vdf_file("../../test/metasd/WRLD3-03/SCEN01.VDF");

        for (label, vdf, expected_ranges) in [
            ("water", &water, 8usize),
            ("econ", &econ, 61usize),
            ("wrld3", &wrld3, 234usize),
        ] {
            let ranges = vdf.record_ot_ranges();
            assert_eq!(
                ranges.len(),
                expected_ranges,
                "{label}: unexpected range count"
            );
            assert!(
                !ranges.is_empty(),
                "{label}: expected at least one OT range from records"
            );
            assert_eq!(
                ranges.first().unwrap().start,
                1,
                "{label}: first OT range should start at 1"
            );
            assert_eq!(
                ranges.last().unwrap().end,
                vdf.offset_table_count,
                "{label}: last OT range should end at ot_count"
            );

            let covered: usize = ranges.iter().map(|r| r.len()).sum();
            assert_eq!(
                covered,
                vdf.offset_table_count.saturating_sub(1),
                "{label}: OT ranges should partition 1..ot_count"
            );
            assert!(
                ranges
                    .windows(2)
                    .all(|w| w[0].end == w[1].start && w[0].start < w[0].end)
            );
        }
    }

    #[test]
    #[ignore]
    fn test_record_filter_counts() {
        let water = vdf_file("../../test/bobby/vdf/water/Current.vdf");
        let econ = vdf_file("../../test/bobby/vdf/econ/base.vdf");
        let wrld3 = vdf_file("../../test/metasd/WRLD3-03/SCEN01.VDF");

        for (_label, vdf) in [("water", &water), ("econ", &econ), ("wrld3", &wrld3)] {
            let ot_count = vdf.offset_table_count;
            let c_basic = vdf
                .records
                .iter()
                .filter(|r| {
                    r.fields[0] != 0
                        && r.fields[1] != RECORD_F1_SYSTEM
                        && r.fields[11] > 0
                        && (r.fields[11] as usize) < ot_count
                })
                .count();
            let c_any_ot = vdf
                .records
                .iter()
                .filter(|r| {
                    r.fields[1] != RECORD_F1_SYSTEM
                        && r.fields[11] > 0
                        && (r.fields[11] as usize) < ot_count
                })
                .count();
            let c_f10 = vdf
                .records
                .iter()
                .filter(|r| {
                    r.fields[0] != 0
                        && r.fields[1] != RECORD_F1_SYSTEM
                        && r.fields[11] > 0
                        && (r.fields[11] as usize) < ot_count
                        && r.fields[10] > 0
                })
                .count();
            let c_not_init = vdf
                .records
                .iter()
                .filter(|r| {
                    r.fields[0] != 0
                        && r.fields[1] != RECORD_F1_SYSTEM
                        && r.fields[1] != RECORD_F1_INITIAL_TIME_CONST
                        && r.fields[11] > 0
                        && (r.fields[11] as usize) < ot_count
                })
                .count();
            assert!(vdf.records.len() >= c_any_ot);
            assert!(c_any_ot >= c_basic);
            assert!(c_basic >= c_f10.min(c_not_init));
        }
    }

    #[test]
    #[ignore]
    fn test_ot_reference_coverage() {
        let water = vdf_file("../../test/bobby/vdf/water/Current.vdf");
        let econ = vdf_file("../../test/bobby/vdf/econ/base.vdf");
        let wrld3 = vdf_file("../../test/metasd/WRLD3-03/SCEN01.VDF");

        for (_label, vdf) in [("water", &water), ("econ", &econ), ("wrld3", &wrld3)] {
            let mut referenced = vec![false; vdf.offset_table_count];
            for rec in &vdf.records {
                let idx = rec.fields[11] as usize;
                if idx < vdf.offset_table_count {
                    referenced[idx] = true;
                }
            }

            let mut ref_count = 0usize;
            let mut missing_blocks = 0usize;
            let mut missing_consts = 0usize;
            for (idx, seen) in referenced.iter().enumerate() {
                if *seen {
                    ref_count += 1;
                    continue;
                }
                if idx == 0 {
                    continue;
                }
                if let Some(raw) = vdf.offset_table_entry(idx) {
                    if vdf.is_data_block_offset(raw) {
                        missing_blocks += 1;
                    } else {
                        missing_consts += 1;
                    }
                }
            }
            assert_eq!(
                ref_count + missing_blocks + missing_consts + usize::from(!referenced[0]),
                vdf.offset_table_count
            );
        }
    }

    #[test]
    fn test_econ_base_names_and_slots() {
        let vdf = vdf_file("../../test/bobby/vdf/econ/base.vdf");

        // After widening slot detection, 94 names have slot table entries.
        assert_eq!(vdf.slot_table.len(), 94);
        assert_eq!(vdf.names[0], "Time");
        assert_eq!(vdf.names[93], "ST");

        // Names past the slot table don't have slot table entries
        let unslotted = &vdf.names[vdf.slot_table.len()..];
        assert!(
            !unslotted.is_empty(),
            "expected unslotted names for econ model"
        );
        assert_eq!(
            unslotted[0],
            "#DELAY1(insolvencyrisk,averagetimebeforedefault)#"
        );
        assert_eq!(
            unslotted[1],
            "#LV1<DELAY1(insolvencyrisk,averagetimebeforedefault)#"
        );
        assert_eq!(unslotted[2], "#SMOOTH(realinflationrate,3)#");

        assert_eq!(unslotted.len(), 6);

        assert!(vdf.names.len() >= 92);
    }

    #[test]
    fn test_small_vdf_names_parsed() {
        let vdf = vdf_file("../../test/bobby/vdf/sd202_a2/Current.vdf");

        assert!(
            vdf.names.len() >= 10,
            "expected at least 10 names, got {}",
            vdf.names.len()
        );
        assert!(
            vdf.names.contains(&"Time".to_string()),
            "expected 'Time' in names"
        );
    }

    fn collect_vdf_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut files = Vec::new();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    files.extend(collect_vdf_files(&path));
                } else if path
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("vdf"))
                {
                    files.push(path);
                }
            }
        }
        files
    }

    #[test]
    fn test_all_uib_vdf_files_parse() {
        let vdf_paths = collect_vdf_files(std::path::Path::new("../../test/bobby/vdf"));

        assert!(
            vdf_paths.len() >= 10,
            "expected at least 10 VDF files, found {}",
            vdf_paths.len()
        );

        let mut parsed_count = 0;
        let mut skipped_count = 0;
        for path in &vdf_paths {
            let data = std::fs::read(path)
                .unwrap_or_else(|e| panic!("failed to read {}: {}", path.display(), e));
            let file_len = data.len();
            // Some .vdf files use a different format variant (e.g. magic
            // byte 0x41 instead of 0x52). Skip those rather than failing.
            if data.len() >= 4 && data[0..4] != VDF_FILE_MAGIC {
                skipped_count += 1;
                continue;
            }
            let vdf = VdfFile::parse(data)
                .unwrap_or_else(|e| panic!("failed to parse {}: {}", path.display(), e));
            parsed_count += 1;

            assert!(
                !vdf.names.is_empty(),
                "{}: expected at least one name",
                path.display()
            );
            assert_eq!(
                vdf.names[0],
                "Time",
                "{}: first name should be 'Time'",
                path.display()
            );

            for name in &vdf.names[vdf.slot_table.len()..] {
                assert!(
                    !name.is_empty() && name.chars().all(|c| c.is_ascii_graphic() || c == ' '),
                    "{}: invalid unslotted name: {:?}",
                    path.display(),
                    name
                );
            }

            // Verify section regions are contiguous
            for i in 0..vdf.sections.len() - 1 {
                assert_eq!(
                    vdf.sections[i].region_end,
                    vdf.sections[i + 1].file_offset,
                    "{}: section {} region_end should equal section {} file_offset",
                    path.display(),
                    i,
                    i + 1
                );
            }
            if let Some(last) = vdf.sections.last() {
                assert_eq!(
                    last.region_end,
                    file_len,
                    "{}: last section region_end should equal file length",
                    path.display()
                );
            }
        }
        assert!(
            parsed_count >= 10,
            "expected at least 10 parseable VDF files, only {} succeeded ({} skipped for different magic)",
            parsed_count,
            skipped_count
        );
    }

    #[test]
    fn test_section5_degenerate_in_small_models() {
        // In small/econ models, section 5 has size=6 and its "data"
        // overlaps with section 6's header, making it a zero-content marker.
        let vdf = vdf_file("../../test/bobby/vdf/sd202_a2/Current.vdf");
        assert!(
            vdf.sections.len() >= 6,
            "expected at least 6 sections, got {}",
            vdf.sections.len()
        );

        let sec5 = &vdf.sections[5];
        assert_eq!(
            sec5.region_data_size(),
            0,
            "section 5 should be degenerate (region_data_size == 0)"
        );
    }

    #[test]
    fn test_records_within_section1_region() {
        let vdf = vdf_file("../../test/bobby/vdf/econ/base.vdf");
        if let Some(sec1) = vdf.sections.get(1) {
            for rec in &vdf.records {
                assert!(
                    rec.file_offset >= sec1.data_offset()
                        && rec.file_offset + RECORD_SIZE <= sec1.region_end,
                    "record at 0x{:x} outside section 1 region (0x{:x}..0x{:x})",
                    rec.file_offset,
                    sec1.data_offset(),
                    sec1.region_end,
                );
            }
        }
    }

    #[test]
    fn test_name_table_within_section_region() {
        let vdf = vdf_file("../../test/bobby/vdf/econ/base.vdf");
        if let Some(ns_idx) = vdf.name_section_idx {
            let sec = &vdf.sections[ns_idx];
            assert!(
                !vdf.names.is_empty(),
                "expected names in the name table section"
            );
            assert!(
                vdf.names.len() > vdf.slot_table.len(),
                "econ model should have names past slotted count"
            );
            assert!(
                sec.region_data_size() > 0,
                "name table section should have data"
            );
        }
    }

    #[test]
    fn test_to_results_with_model_small_models() {
        for (label, mdl_path, vdf_path) in [
            (
                "water",
                "../../test/bobby/vdf/water/water.mdl",
                "../../test/bobby/vdf/water/Current.vdf",
            ),
            (
                "pop",
                "../../test/bobby/vdf/pop/pop.mdl",
                "../../test/bobby/vdf/pop/Current.vdf",
            ),
        ] {
            let contents = std::fs::read_to_string(mdl_path).unwrap();
            let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
            let project = std::rc::Rc::new(crate::Project::from(datamodel_project));
            let vdf = vdf_file(vdf_path);
            let results = vdf
                .to_results_with_model(&project, "main")
                .unwrap_or_else(|e| panic!("{label}: to_results_with_model failed: {e}"));

            assert!(
                results.offsets.len() > 1,
                "{label}: expected mapped variable columns"
            );
            assert_eq!(
                results.step_count, vdf.time_point_count,
                "{label}: expected VDF-sized timestep count"
            );
            assert!(
                results.offsets.contains_key(&Ident::new("time")),
                "{label}: expected time column"
            );
            for &v in results.data.iter() {
                assert!(
                    v.is_finite(),
                    "{label}: expected finite mapped series values"
                );
            }
        }
    }

    #[test]
    fn test_lookup_fixture_results_include_inline_lookup_but_not_lookup_definition() {
        let mdl_path = "../../test/bobby/vdf/lookups/lookup_ex.mdl";
        let vdf_path = "../../test/bobby/vdf/lookups/lookup_ex.vdf";

        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let project = std::rc::Rc::new(crate::Project::from(datamodel_project));
        let vdf = vdf_file(vdf_path);

        let results = vdf
            .to_results_with_model(&project, "main")
            .unwrap_or_else(|e| panic!("lookup fixture: to_results_with_model failed: {e}"));

        let inline_lookup = Ident::new("inline_lookup_table");
        let lookup_definition = Ident::new("lookup_table_1");
        let net_change = Ident::new("net_change");

        assert!(
            results.offsets.contains_key(&inline_lookup),
            "inline WITH LOOKUP auxiliary should be present in Results"
        );
        assert!(
            !results.offsets.contains_key(&lookup_definition),
            "standalone lookup-table definitions should not become Results columns"
        );

        let inline_col = results.offsets[&inline_lookup];
        let net_change_col = results.offsets[&net_change];
        let value_at =
            |col: usize, step: usize| -> f64 { results.data[step * results.step_size + col] };

        assert!((value_at(inline_col, 0) - 6.0).abs() < 1e-9);
        assert!((value_at(inline_col, 30) - 5.0).abs() < 1e-9);
        assert!((value_at(inline_col, 100) - 3.0).abs() < 1e-9);

        assert!((value_at(net_change_col, 0) - 10.0).abs() < 1e-9);
        assert!((value_at(net_change_col, 30) - 15.0).abs() < 1e-9);
        assert!((value_at(net_change_col, 100) - 23.0).abs() < 1e-9);
    }

    #[test]
    fn test_lookup_fixture_stocks_first_map_skips_lookup_definition() {
        let mdl_path = "../../test/bobby/vdf/lookups/lookup_ex.mdl";
        let vdf_path = "../../test/bobby/vdf/lookups/lookup_ex.vdf";

        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let vdf = vdf_file(vdf_path);
        let map = vdf.build_stocks_first_ot_map(&datamodel_project).unwrap();
        let data = vdf.extract_data().unwrap();

        let inline_lookup = Ident::new("inline_lookup_table");
        let lookup_definition = Ident::new("lookup_table_1");
        let net_change = Ident::new("net_change");

        assert!(
            map.contains_key(&inline_lookup),
            "inline WITH LOOKUP auxiliary should receive an OT mapping"
        );
        assert!(
            !map.contains_key(&lookup_definition),
            "standalone lookup-table definitions should not receive Results OT mappings"
        );

        let inline_ot = map[&inline_lookup];
        let net_change_ot = map[&net_change];
        assert_ne!(
            inline_ot, net_change_ot,
            "inline lookup and net change should not collapse onto the same OT"
        );

        assert!((data.entries[inline_ot][0] - 6.0).abs() < 1e-9);
        assert!((data.entries[inline_ot][100] - 3.0).abs() < 1e-9);
        assert!((data.entries[net_change_ot][0] - 10.0).abs() < 1e-9);
        assert!((data.entries[net_change_ot][100] - 23.0).abs() < 1e-9);
    }

    #[test]
    fn test_to_results_with_model_keeps_vdf_name_missing_from_model() {
        let mdl_path = "../../test/bobby/vdf/water/water.mdl";
        let vdf_path = "../../test/bobby/vdf/water/Current.vdf";

        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let full_datamodel = crate::compat::open_vensim(&contents).unwrap();

        // Build the full-model map to get the known-good OT for "gap"
        let vdf = vdf_file(vdf_path);
        let full_map = vdf.build_stocks_first_ot_map(&full_datamodel).unwrap();
        let gap_ot = *full_map
            .get(&Ident::new("gap"))
            .expect("full stocks-first map missing gap");

        // Now remove "gap" from the model and verify to_results_with_model
        // still includes it from the VDF name table.
        let mut stripped = full_datamodel;
        remove_model_var(&mut stripped, "gap");
        let project = std::rc::Rc::new(crate::Project::from(stripped));

        let vdf_data = vdf.extract_data().unwrap();
        let results = vdf
            .to_results_with_model(&project, "main")
            .unwrap_or_else(|e| panic!("to_results_with_model failed: {e}"));

        let gap_ident = Ident::new("gap");
        let Some(&gap_col) = results.offsets.get(&gap_ident) else {
            panic!("expected VDF-only variable 'gap' to be preserved in results");
        };
        let expected = &vdf_data.entries[gap_ot];

        for (step, expected_value) in expected.iter().enumerate() {
            let actual = results.data[step * results.step_size + gap_col];
            assert!(
                (actual - *expected_value).abs() < 1e-9,
                "gap mismatch at step {step}: expected {expected_value}, got {actual}"
            );
        }
    }

    #[test]
    #[ignore = "requires third_party/uib_sd fixtures"]
    fn test_to_results_with_model_arrayed_baserun() {
        let mdl_path = "../../third_party/uib_sd/zambaqui/ZamMod1.mdl";
        let vdf_path = "../../third_party/uib_sd/zambaqui/baserun.vdf";
        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let project = std::rc::Rc::new(crate::Project::from(datamodel_project));
        let vdf = vdf_file(vdf_path);

        let results = vdf
            .to_results_with_model(&project, "main")
            .unwrap_or_else(|e| panic!("baserun: to_results_with_model failed: {e}"));
        assert!(
            results.offsets.len() > 1,
            "baserun: expected at least some mapped series"
        );
        assert!(
            results.offsets.keys().any(|id| id.as_str().contains('[')),
            "baserun: expected array element identifiers in mapped outputs"
        );
        assert_eq!(results.step_count, vdf.time_point_count);
    }

    /// The econ model (medium-sized, 42 slotted names, 74 records)
    /// previously failed OT detection because `find_first_data_block`
    /// matched a false-positive block starting with 0.0. With the
    /// monotonicity check, the correct time block (starting at t=1.0)
    /// is found, and the real OT is located just before it.
    #[test]
    fn test_econ_offset_table_found() {
        let vdf = vdf_file("../../test/bobby/vdf/econ/base.vdf");

        assert!(
            vdf.offset_table_count > 0,
            "econ: expected offset_table_count > 0"
        );
        assert!(
            vdf.synthetic_ot.is_none(),
            "econ: should use real OT, not synthetic"
        );

        // Records have nonzero f[10] values
        let nonzero_f10 = vdf.records.iter().filter(|r| r.fields[10] != 0).count();
        assert!(
            nonzero_f10 > 0,
            "econ: records should have nonzero f[10] values"
        );

        // Data extraction should succeed and produce valid entries
        let vdf_data = vdf.extract_data().unwrap();
        assert!(
            vdf_data.entries.len() > 1,
            "econ: expected multiple data entries, got {}",
            vdf_data.entries.len()
        );

        // Time series should start at 1.0 (INITIAL TIME from mark2.mdl)
        assert!(
            (vdf_data.time_values[0] - 1.0).abs() < 0.01,
            "econ: time should start at 1.0, got {}",
            vdf_data.time_values[0]
        );
        let last_time = *vdf_data.time_values.last().unwrap();
        assert!(
            (last_time - 300.0).abs() < 0.01,
            "econ: time should end at 300.0, got {last_time}"
        );
    }

    /// The econ VDF was generated from mark2.mdl (per header).
    /// With the synthetic OT, empirical matching should find matches.
    #[test]
    fn test_econ_vdf_from_mark2_mdl() {
        let vdf = vdf_file("../../test/bobby/vdf/econ/base.vdf");

        let header_text: String = vdf.data[4..0x78]
            .iter()
            .take_while(|&&b| b != 0)
            .map(|&b| b as char)
            .collect();
        assert!(
            header_text.contains("mark2.mdl"),
            "econ: expected header to reference mark2.mdl, got: {header_text}"
        );

        let vdf_data = vdf.extract_data().unwrap();
        assert!(
            vdf_data.entries.len() > 1,
            "econ: expected data entries with synthetic OT, got {}",
            vdf_data.entries.len()
        );

        // Simulate the MDL and check that empirical matching works
        let contents = std::fs::read_to_string("../../test/bobby/vdf/econ/mark2.mdl").unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let project = std::rc::Rc::new(crate::Project::from(datamodel_project));
        let sim = crate::interpreter::Simulation::new(&project, "main").unwrap();
        let results = sim.run_to_end().unwrap();

        assert_eq!(
            results.step_count,
            vdf_data.time_values.len(),
            "econ: step count mismatch between VDF and simulation"
        );
        let emp_map = build_empirical_ot_map(&vdf_data, &results).unwrap();
        let matched = emp_map.len() - 1; // subtract Time
        assert!(
            matched > 0,
            "econ: expected at least some empirical matches, got 0"
        );
    }

    /// Verify extracted time series data is valid: time is monotonically
    /// increasing, all values are finite.
    #[test]
    fn test_extracted_data_validity() {
        let small_vdfs = [
            "../../test/bobby/vdf/bact/Current.vdf",
            "../../test/bobby/vdf/water/Current.vdf",
            "../../test/bobby/vdf/pop/Current.vdf",
        ];

        for vdf_path in &small_vdfs {
            let vdf = vdf_file(vdf_path);
            let vdf_data = vdf
                .extract_data()
                .unwrap_or_else(|e| panic!("{vdf_path}: extract_data failed: {e}"));

            // Time (entry 0) must be monotonically increasing
            for i in 1..vdf_data.time_values.len() {
                assert!(
                    vdf_data.time_values[i] > vdf_data.time_values[i - 1],
                    "{vdf_path}: time not monotonic at index {i}: {} <= {}",
                    vdf_data.time_values[i],
                    vdf_data.time_values[i - 1]
                );
            }

            // All entries should have finite values
            for (ei, entry) in vdf_data.entries.iter().enumerate() {
                assert_eq!(
                    entry.len(),
                    vdf_data.time_values.len(),
                    "{vdf_path}: entry {ei} length mismatch"
                );
                for (ti, &val) in entry.iter().enumerate() {
                    assert!(
                        val.is_finite(),
                        "{vdf_path}: entry {ei} has non-finite value at t[{ti}]: {val}"
                    );
                }
            }
        }
    }

    // ---- Small-file chain analysis (task #1) ----
    //
    // Key findings from tracing name->slot->record->OT on smallest VDFs:
    //
    // 1. f[11] IS the correct OT index for all records where in-range.
    // 2. f[12] groups records into view-level clusters (2-3 distinct values),
    //    NOT 1:1 with variable names. Values match system var slot offsets.
    // 3. f[1]=15 records (INITIAL TIME) exist in some models but not others;
    //    they pass the model-var filter and break the name count.
    // 4. Slot data at a given section 1 offset is positional/structural.
    // 5. The deterministic approach works for models without f[1]=15 anomaly.

    /// Test the stocks-first-alphabetical mapping algorithm against the
    /// econ and WRLD3 models by comparing with empirical correlation matching.
    #[test]
    fn test_stocks_first_mapping_econ() {
        let mdl_path = "../../test/bobby/vdf/econ/mark2.mdl";
        let vdf_path = "../../test/bobby/vdf/econ/base.vdf";

        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();

        let vdf = vdf_file(vdf_path);

        let map = vdf.build_stocks_first_ot_map(&datamodel_project).unwrap();

        eprintln!("\n=== Stocks-First OT Map for Econ ===");
        eprintln!("Total mapped: {}", map.len());

        let mut by_ot: Vec<_> = map.iter().map(|(n, &ot)| (n, ot)).collect();
        by_ot.sort_by_key(|(_, ot)| *ot);

        for (name, ot) in &by_ot {
            let is_internal = name.as_str().starts_with('#');
            let label = if is_internal { " [internal]" } else { "" };
            eprintln!("  OT[{ot:3}] {}{label}", name.as_str());
        }

        // Validate against VDF data initial values for stocks
        let vdf_data = vdf.extract_data().unwrap();
        eprintln!("\n=== Validation: Stock initial values ===");
        for (name, ot) in &by_ot {
            if *ot == 0 || *ot >= vdf_data.entries.len() {
                continue;
            }
            let first = vdf_data.entries[*ot][0];
            eprintln!("  OT[{ot:3}] first={first:.4} ← {}", name.as_str());
        }
    }

    /// Validate generated VDF signatures against actual VDF name table entries.
    /// This directly compares our algorithm's output with what Vensim wrote.
    #[test]
    fn test_vdf_signature_generation_econ() {
        let mdl_path = "../../test/bobby/vdf/econ/mark2.mdl";
        let vdf_path = "../../test/bobby/vdf/econ/base.vdf";

        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();

        let vdf = vdf_file(vdf_path);

        // Get VDF's actual #-prefixed names
        let vdf_sigs: Vec<String> = vdf
            .names
            .iter()
            .filter(|n| n.starts_with('#'))
            .cloned()
            .collect();

        // Generate signatures from MDL
        let model = &datamodel_project.models[0];
        let mut generated_sigs: Vec<String> = Vec::new();
        for var in &model.variables {
            let equation = match var {
                crate::datamodel::Variable::Aux(a) => &a.equation,
                crate::datamodel::Variable::Flow(f) => &f.equation,
                _ => continue,
            };
            if let Some(info) = extract_stdlib_call_info(equation) {
                for (sig, _) in info.vensim_signatures() {
                    generated_sigs.push(sig);
                }
            }
        }

        eprintln!("\n=== Signature Validation (econ) ===");
        eprintln!(
            "VDF has {} # names, we generated {}",
            vdf_sigs.len(),
            generated_sigs.len()
        );

        // Normalize for comparison (lowercase, remove spaces)
        let vdf_normalized: std::collections::HashSet<String> =
            vdf_sigs.iter().map(|s| s.to_lowercase()).collect();
        let gen_normalized: std::collections::HashSet<String> =
            generated_sigs.iter().map(|s| s.to_lowercase()).collect();

        let matched: Vec<_> = gen_normalized.intersection(&vdf_normalized).collect();
        let only_vdf: Vec<_> = vdf_normalized.difference(&gen_normalized).collect();
        let only_gen: Vec<_> = gen_normalized.difference(&vdf_normalized).collect();

        eprintln!("Matched: {}", matched.len());
        eprintln!("Only in VDF (missed): {}", only_vdf.len());
        for s in &only_vdf {
            eprintln!("  VDF: {s}");
        }
        eprintln!("Only in generated (extra): {}", only_gen.len());
        for s in &only_gen {
            eprintln!("  GEN: {s}");
        }

        assert!(
            only_vdf.is_empty(),
            "some VDF signatures not generated by our algorithm"
        );
    }

    #[test]
    fn test_vdf_signature_generation_wrld3() {
        let mdl_path = "../../test/metasd/WRLD3-03/wrld3-03.mdl";
        let vdf_path = "../../test/metasd/WRLD3-03/SCEN01.VDF";

        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();

        let vdf = vdf_file(vdf_path);

        let vdf_sigs: Vec<String> = vdf
            .names
            .iter()
            .filter(|n| n.starts_with('#'))
            .cloned()
            .collect();

        let model = &datamodel_project.models[0];
        let mut generated_sigs: Vec<String> = Vec::new();
        for var in &model.variables {
            let equation = match var {
                crate::datamodel::Variable::Aux(a) => &a.equation,
                crate::datamodel::Variable::Flow(f) => &f.equation,
                _ => continue,
            };
            if let Some(info) = extract_stdlib_call_info(equation) {
                for (sig, _) in info.vensim_signatures() {
                    generated_sigs.push(sig);
                }
            }
        }

        eprintln!("\n=== Signature Validation (WRLD3) ===");
        eprintln!(
            "VDF has {} # names, we generated {}",
            vdf_sigs.len(),
            generated_sigs.len()
        );

        let vdf_normalized: std::collections::HashSet<String> =
            vdf_sigs.iter().map(|s| s.to_lowercase()).collect();
        let gen_normalized: std::collections::HashSet<String> =
            generated_sigs.iter().map(|s| s.to_lowercase()).collect();

        let matched: Vec<_> = gen_normalized.intersection(&vdf_normalized).collect();
        let only_vdf: Vec<_> = vdf_normalized.difference(&gen_normalized).collect();
        let only_gen: Vec<_> = gen_normalized.difference(&vdf_normalized).collect();

        eprintln!("Matched: {}", matched.len());
        eprintln!("Only in VDF (missed): {}", only_vdf.len());
        for s in &only_vdf {
            eprintln!("  VDF: {s}");
        }
        eprintln!("Only in generated (extra): {}", only_gen.len());
        for s in &only_gen {
            eprintln!("  GEN: {s}");
        }
    }

    #[test]
    fn test_stocks_first_mapping_wrld3() {
        let mdl_path = "../../test/metasd/WRLD3-03/wrld3-03.mdl";
        let vdf_path = "../../test/metasd/WRLD3-03/SCEN01.VDF";

        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();

        let vdf = vdf_file(vdf_path);
        let map = vdf.build_stocks_first_ot_map(&datamodel_project).unwrap();

        eprintln!("\n=== Stocks-First OT Map for WRLD3 ===");
        eprintln!(
            "Total mapped: {} (VDF OT entries: {})",
            map.len(),
            vdf.offset_table_count
        );

        // Show first 30 and last 10 entries
        let mut by_ot: Vec<_> = map.iter().map(|(n, &ot)| (n, ot)).collect();
        by_ot.sort_by_key(|(_, ot)| *ot);

        eprintln!("\nFirst 30 entries (should be stocks):");
        for (name, ot) in by_ot.iter().take(30) {
            eprintln!("  OT[{ot:3}] {}", name.as_str());
        }

        if by_ot.len() > 30 {
            eprintln!("\n... ({} total entries) ...", by_ot.len());
            eprintln!("\nLast 10 entries (should be non-stocks):");
            let tail: Vec<_> = by_ot.iter().rev().take(10).collect();
            for (name, ot) in tail.iter().rev() {
                eprintln!("  OT[{ot:3}] {}", name.as_str());
            }
        }

        // Validate: compare with empirical map if simulation works
        // (WRLD3 uses external data files, simulation may fail)
        let project = std::rc::Rc::new(crate::Project::from(datamodel_project.clone()));
        match crate::interpreter::Simulation::new(&project, "main") {
            Ok(sim) => {
                match sim.run_to_end() {
                    Ok(results) => {
                        let vdf_data = vdf.extract_data().unwrap();
                        match build_empirical_ot_map(&vdf_data, &results) {
                            Ok(empirical) => {
                                // Compare: for each empirically matched variable,
                                // check if our predicted OT matches
                                let mut correct = 0;
                                let mut wrong = 0;
                                let mut missing = 0;

                                for (name, &emp_ot) in &empirical {
                                    if name.as_str() == "time" {
                                        continue;
                                    }
                                    if let Some(&pred_ot) = map.get(name) {
                                        if pred_ot == emp_ot {
                                            correct += 1;
                                        } else {
                                            wrong += 1;
                                            eprintln!(
                                                "  MISMATCH: {} predicted OT={pred_ot} actual OT={emp_ot}",
                                                name.as_str()
                                            );
                                        }
                                    } else {
                                        missing += 1;
                                        eprintln!(
                                            "  MISSING: {} not in prediction (empirical OT={emp_ot})",
                                            name.as_str()
                                        );
                                    }
                                }

                                eprintln!(
                                    "\n=== Validation Results ===\n\
                                         Empirical matches: {}\n\
                                         Correct: {correct}\n\
                                         Wrong: {wrong}\n\
                                         Missing from prediction: {missing}\n\
                                         Accuracy: {:.1}%",
                                    empirical.len() - 1,
                                    if correct + wrong > 0 {
                                        100.0 * correct as f64 / (correct + wrong) as f64
                                    } else {
                                        0.0
                                    }
                                );
                            }
                            Err(e) => eprintln!("empirical map failed: {e}"),
                        }
                    }
                    Err(e) => eprintln!("simulation failed: {e}"),
                }
            }
            Err(e) => eprintln!("couldn't create simulation: {e}"),
        }
    }

    #[test]
    fn test_stocks_first_map_keeps_vdf_candidates_when_model_is_missing_name() {
        let mdl_path = "../../test/metasd/WRLD3-03/wrld3-03.mdl";
        let vdf_path = "../../test/metasd/WRLD3-03/SCEN01.VDF";

        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let mut datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        remove_model_var(&mut datamodel_project, "food");

        let vdf = vdf_file(vdf_path);
        let map = vdf.build_stocks_first_ot_map(&datamodel_project).unwrap();
        assert!(
            map.contains_key(&Ident::new("food")),
            "expected VDF candidate 'food' to survive model drift"
        );
    }

    #[test]
    fn test_stocks_first_map_nonrecord_ots_are_stock_backed_on_wrld3() {
        let mdl_path = "../../test/metasd/WRLD3-03/wrld3-03.mdl";
        let vdf_path = "../../test/metasd/WRLD3-03/SCEN01.VDF";

        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let vdf = vdf_file(vdf_path);
        let map = vdf.build_stocks_first_ot_map(&datamodel_project).unwrap();
        let section6_codes = vdf.section6_ot_class_codes().unwrap();

        let mut record_ots: HashSet<usize> = vdf
            .records
            .iter()
            .filter_map(|rec| {
                let ot_idx = rec.fields[11] as usize;
                if rec.fields[0] != 0
                    && rec.fields[1] != RECORD_F1_SYSTEM
                    && rec.fields[1] != RECORD_F1_INITIAL_TIME_CONST
                    && rec.fields[10] > 0
                    && ot_idx > 0
                    && ot_idx < vdf.offset_table_count
                {
                    Some(ot_idx)
                } else {
                    None
                }
            })
            .collect();
        record_ots.insert(0);

        for (name, &ot) in &map {
            if !record_ots.contains(&ot) {
                assert_eq!(
                    section6_codes.get(ot),
                    Some(&VDF_SECTION6_OT_CODE_STOCK),
                    "{name} mapped to OT {ot}, which is neither record-backed nor stock-coded"
                );
            }
        }
    }

    #[test]
    fn test_stocks_first_map_includes_compiled_stdlib_aliases_econ() {
        let mdl_path = "../../test/bobby/vdf/econ/mark2.mdl";
        let vdf_path = "../../test/bobby/vdf/econ/base.vdf";

        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let project = crate::Project::from(datamodel_project.clone());
        let vdf = vdf_file(vdf_path);
        let map = vdf
            .build_stocks_first_ot_map_for_project(&project, "main")
            .unwrap();

        let perceived_hpi = Ident::<Canonical>::new("perceived_hpi");
        let module_output =
            Ident::<Canonical>::from_str_unchecked("$⁚perceived_hpi⁚0⁚smth1.output");
        let module_input = Ident::<Canonical>::from_str_unchecked("$⁚perceived_hpi⁚0⁚smth1.input");
        let indexed_hpi = Ident::<Canonical>::new("indexed_hpi");

        assert_eq!(map.get(&module_output), map.get(&perceived_hpi));
        assert_eq!(map.get(&module_input), map.get(&indexed_hpi));
    }

    #[test]
    fn test_stocks_first_map_includes_compiled_stdlib_state_aliases_wrld3() {
        let mdl_path = "../../test/metasd/WRLD3-03/wrld3-03.mdl";
        let vdf_path = "../../test/metasd/WRLD3-03/SCEN01.VDF";

        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let project = crate::Project::from(datamodel_project.clone());
        let vdf = vdf_file(vdf_path);
        let map = vdf
            .build_stocks_first_ot_map_for_project(&project, "main")
            .unwrap();

        let land_yield_factor_2 = Ident::<Canonical>::new("land_yield_factor_2");
        let ly_output =
            Ident::<Canonical>::from_str_unchecked("$⁚land_yield_factor_2⁚0⁚smth3.output");
        let ly_stock_1 =
            Ident::<Canonical>::from_str_unchecked("$⁚land_yield_factor_2⁚0⁚smth3.stock_1");
        let ly_stock_2 =
            Ident::<Canonical>::from_str_unchecked("$⁚land_yield_factor_2⁚0⁚smth3.stock_2");
        let pollution_output = Ident::<Canonical>::from_str_unchecked(
            "$⁚persistent_pollution_appearance_rate⁚0⁚delay3.output",
        );
        let pollution_stock_3 = Ident::<Canonical>::from_str_unchecked(
            "$⁚persistent_pollution_appearance_rate⁚0⁚delay3.stock_3",
        );

        assert_eq!(map.get(&ly_output), map.get(&land_yield_factor_2));
        assert!(map.contains_key(&ly_stock_1));
        assert!(map.contains_key(&ly_stock_2));
        assert!(map.contains_key(&pollution_output));
        assert!(map.contains_key(&pollution_stock_3));
    }
}
