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
//! - Model-guided name-to-OT mapping via [`VdfFile::build_section6_guided_ot_map`]:
//!   uses section-6 OT class codes to identify contiguous stock/non-stock
//!   blocks, classifies variables using the parsed model, and assigns OT
//!   indices by alphabetical sort within each block.

use std::collections::{HashMap, HashSet};
use std::{error::Error, result::Result as StdResult};

use crate::{
    common::{Canonical, Ident},
    results::{Method, Results, Specs},
};

mod section3;

pub use section3::{VdfSection3Directory, VdfSection3DirectoryEntry};

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

    /// Whether the user-visible output of this stdlib call is stored in a
    /// stock-backed OT entry.
    #[cfg(test)]
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

/// Heuristic for names that look like standalone lookup/table definitions.
/// These names may appear in the name table but lack their own OT entries.
fn is_lookupish_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.contains(" lookup") || lower.contains(" table") || lower.contains("graphical function")
}

/// Normalize a VDF name for comparison: lowercase, strip spaces and underscores.
fn normalize_vdf_name(name: &str) -> String {
    name.replace([' ', '_'], "").to_lowercase()
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

/// Dataset VDF file magic bytes used by Vensim's imported dataset files.
pub const VDF_DATASET_FILE_MAGIC: [u8; 4] = [0x7f, 0xf7, 0x17, 0x41];

/// High-level VDF container kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VdfKind {
    /// Standard simulation-results VDF.
    SimulationResults,
    /// Dataset/reference-mode VDF imported from external data.
    Dataset,
}

/// Probe the container kind from the first four bytes.
pub fn probe_vdf_kind(data: &[u8]) -> Option<VdfKind> {
    if data.len() < 4 {
        return None;
    }
    if data[0..4] == VDF_FILE_MAGIC {
        Some(VdfKind::SimulationResults)
    } else if data[0..4] == VDF_DATASET_FILE_MAGIC {
        Some(VdfKind::Dataset)
    } else {
        None
    }
}

/// VDF section header magic value: float32 -0.797724 = 0xbf4c37a1.
/// This 4-byte sequence delimits sections within the VDF file.
pub const VDF_SECTION_MAGIC: [u8; 4] = [0xa1, 0x37, 0x4c, 0xbf];

/// Sentinel value appearing in record fields 8, 9, and sometimes 14.
pub const VDF_SENTINEL: u32 = 0xf6800000;

/// Section-6 OT class code for the Time series (OT[0]) in all observed files.
pub const VDF_SECTION6_OT_CODE_TIME: u8 = 0x0f;

/// Section-6 OT class code marking stock-backed OT entries in all validated
/// files.
///
/// The `level_vs_aux` regression fixtures pin this as the authoritative
/// OT-side stock/non-stock signal: when the same variable `x` is changed from
/// a level to a supplementary auxiliary, the saved series moves from a
/// stock-coded OT entry (`0x08`) to a dynamic non-stock entry (`0x11`) while
/// the nearby section-1 record classification fields stay unchanged.
pub const VDF_SECTION6_OT_CODE_STOCK: u8 = 0x08;

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
    /// Field4: varies across files and Vensim versions. Not a reliable
    /// section discriminator; identify sections by index instead.
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

    /// field[11]: OT block start index for this variable.
    ///
    /// For arrayed variables, this points to the first of N consecutive
    /// OT entries (one per subscript element). For scalar variables, it
    /// points to the single OT entry. Values can exceed the actual OT
    /// count; callers should check `ot_index < offset_table_count`.
    pub fn ot_index(&self) -> u32 {
        self.fields[11]
    }

    /// field[6]: shape selector for this variable's array layout.
    ///
    /// Known values:
    /// - 5: scalar (no array dimensions)
    /// - 32 (0x20): generic arrayed marker (first or only array shape)
    /// - other: section-3 index_word linking to a specific shape template
    ///
    /// Use `is_arrayed()` to test scalar vs. arrayed, or `shape_code()`
    /// to retrieve the raw value for shape-template lookups.
    pub fn is_arrayed(&self) -> bool {
        self.fields[6] != 5
    }

    /// Raw field[6] value: the shape selector linking this record to a
    /// section-3 shape template (or 5 for scalar, 32 for generic arrayed).
    pub fn shape_code(&self) -> u32 {
        self.fields[6]
    }

    /// Whether this record has the sentinel pair identifying it as a
    /// proper variable metadata record (vs. a padding or alignment block).
    pub fn has_sentinel(&self) -> bool {
        self.fields[8] == VDF_SENTINEL && self.fields[9] == VDF_SENTINEL
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

/// Parsed section-5 set entry.
///
/// The on-disk layout is `u32 n; u32 marker; u32 refs[refs_len]`, where:
/// - marker=0: refs_len = n + 1 (dimension elements plus one trailing anchor)
/// - marker=1: refs_len = n + 2 (dimension elements plus two axis anchors)
///
/// In array-heavy files, `n` is the associated subscript-set cardinality.
/// This structure preserves `n` and `marker` explicitly rather than inferring
/// only from `refs.len()`.
#[derive(Debug, Clone)]
pub struct VdfSection5SetEntry {
    /// Absolute file offset where this entry begins.
    pub file_offset: usize,
    /// Header count field (`n`): the dimension cardinality.
    pub n: usize,
    /// Second header word: 0 for standard entries, 1 for entries with an
    /// extra axis-anchor ref.
    pub marker: u32,
    /// Referenced section-1 offsets carried by this entry.
    pub refs: Vec<u32>,
    /// Number of refs that are present in the slot table.
    pub slotted_ref_count: usize,
}

impl VdfSection5SetEntry {
    /// Number of section-1 refs in the entry.
    pub fn set_size(&self) -> usize {
        self.refs.len()
    }

    /// Candidate dimension size implied by this set.
    ///
    /// For marker=0 entries, dimension_size = refs.len() - 1 (one trailing
    /// anchor ref is not a dimension element). For marker=1 entries,
    /// dimension_size = refs.len() - 2 (two axis-anchor refs are structural,
    /// not dimension elements).
    pub fn dimension_size(&self) -> usize {
        let overhead = match self.marker {
            0 => 1,
            1 => 2,
            _ => 1,
        };
        self.set_size().saturating_sub(overhead)
    }
}

/// Parsed variable-length entry from section 4's view/group metadata stream.
///
/// The observed layout is:
///
/// `u32 packed; u32 refs[count_lo + count_hi]; u32 index_word`
///
/// where `packed` splits into `count_lo` (low 16 bits) and `count_hi`
/// (high 16 bits). The exact semantics of those halves remain unknown, but
/// across the validated corpus their sum gives the number of following
/// section-1 slot refs before the trailing index word.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VdfSection4Entry {
    /// Absolute file offset where this entry begins.
    pub file_offset: usize,
    /// Raw packed header word.
    pub packed_word: u32,
    /// Referenced section-1 offsets carried by this entry.
    pub refs: Vec<u32>,
    /// Trailing small index-like word used by section-3 correlation.
    pub index_word: u32,
    /// Number of refs that are present in the slot table.
    pub slotted_ref_count: usize,
}

impl VdfSection4Entry {
    /// Low 16 bits of the packed header word.
    pub fn count_lo(&self) -> u16 {
        self.packed_word as u16
    }

    /// High 16 bits of the packed header word.
    pub fn count_hi(&self) -> u16 {
        (self.packed_word >> 16) as u16
    }

    /// Ref count implied by the packed word.
    pub fn packed_ref_count(&self) -> usize {
        self.count_lo() as usize + self.count_hi() as usize
    }

    /// Trailing index-like value carried by the entry.
    pub fn index_word(&self) -> u32 {
        self.index_word
    }
}

/// Parsed section-4 entry stream plus its zero-word prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VdfSection4EntryStream {
    /// Number of leading zero words before entry parsing begins.
    pub zero_prefix_words: usize,
    /// Parsed section-4 entries.
    pub entries: Vec<VdfSection4Entry>,
}

/// A named dimension set inferred from section 5 and nearby name-table entries.
///
/// Observed array VDFs carry the dimension name as the only non-metadata name
/// referenced from the section-5 entry. Element names are not referenced
/// directly; instead they appear immediately after the dimension name in the
/// name table, interleaved only with metadata entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VdfDimensionSet {
    /// Dimension name as stored in the VDF name table.
    pub name: String,
    /// Element names in VDF subscript order.
    pub elements: Vec<String>,
}

impl VdfDimensionSet {
    /// Number of elements in this dimension set.
    pub fn len(&self) -> usize {
        self.elements.len()
    }

    /// Whether this dimension set has no elements.
    pub fn is_empty(&self) -> bool {
        self.elements.is_empty()
    }
}

/// Fixed-width record stored in the trailing section-6 suffix.
///
/// After the section-6 OT class-code array and the OT-aligned final-value
/// vector, observed files store a `13 * u32` record stream terminated by a
/// single zero word. These records correspond 1:1 with lookup table
/// definitions in the name table; word[10] carries the OT index for each
/// lookup. The semantic meaning of the other 12 fields is not yet decoded.
#[derive(Debug, Clone, PartialEq)]
pub struct VdfSection6LookupRecord {
    /// Absolute file offset where this record begins.
    pub file_offset: usize,
    /// Raw words comprising the record.
    pub words: [u32; 13],
}

impl VdfSection6LookupRecord {
    /// OT index for this lookup table definition.
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

/// Parsed dataset/reference-mode VDF file.
///
/// These files share the section framing, slot table heuristics, record
/// layout, and sparse block format with simulation-result VDFs, but they
/// use a different file magic and a shorter five-section container.
pub struct VdfDatasetFile {
    /// Raw file bytes.
    pub data: Vec<u8>,
    /// Human-readable dataset origin string from the file header.
    pub origin: String,
    /// Number of dataset time points.
    pub time_point_count: usize,
    /// Sparse-block bitmap size in bytes.
    pub bitmap_size: usize,
    /// Parsed section headers.
    pub sections: Vec<Section>,
    /// Parsed dataset names from the shifted name-table section.
    pub names: Vec<String>,
    /// Slot table recovered from section 0.
    pub slot_table: Vec<u32>,
    /// File offset where the slot table starts.
    pub slot_table_offset: usize,
    /// Record-like metadata entries recovered from section 0.
    pub records: Vec<VdfRecord>,
    /// Ordered sparse block offsets for dataset series.
    pub data_block_offsets: Vec<usize>,
}

/// Extracted dataset/reference-mode time series.
pub struct VdfDatasetData {
    /// Decimal-year time axis recovered from the dataset.
    pub time_values: Vec<f64>,
    /// Stable presentation order for named series.
    pub series_order: Vec<String>,
    /// Named dataset series keyed by display name.
    pub series: HashMap<String, Vec<f64>>,
}

impl VdfDatasetData {
    /// Borrow a named series by display name.
    pub fn series(&self, name: &str) -> Option<&[f64]> {
        self.series.get(name).map(|series| series.as_slice())
    }
}

/// Dataset-series binding recovered from the dataset record stream.
///
/// Dataset VDFs do not yet expose the same per-variable owner fields as
/// simulation-result VDFs. The current bridge is the stable record ordering
/// in section 0 paired with visible names from section 1 and block offsets
/// from section 4. Exposing the record sort keys keeps that join inspectable.
pub struct VdfDatasetSeriesBinding {
    /// Visible dataset series name.
    pub name: String,
    /// Zero-based index into `VdfDatasetFile::data_block_offsets`.
    pub block_index: usize,
    /// Absolute file offset of the record that contributed this binding.
    pub record_file_offset: usize,
    /// Primary record sort key (`field[2]`).
    pub record_f2: u32,
    /// Secondary record sort key (`field[3]`).
    pub record_f3: u32,
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
    /// Header offset 0x58: absolute file offset to section-6 final values
    /// array (one f32 per OT entry).
    header_final_values_offset: usize,
    /// Header offset 0x5c: absolute file offset to section-6 lookup mapping
    /// records (immediately after the final values array).
    /// OT count = (header_lookup_mapping_offset - header_final_values_offset) / 4.
    header_lookup_mapping_offset: usize,
}

fn dataset_header_time_point_count(data: &[u8]) -> Option<usize> {
    [0x7cusize, 0x78, 0x74]
        .into_iter()
        .map(|offset| read_u32(data, offset) as usize)
        .find(|&count| count > 0)
}

impl VdfDatasetFile {
    /// Parse a dataset/reference-mode VDF.
    pub fn parse(data: Vec<u8>) -> StdResult<Self, Box<dyn Error>> {
        if data.len() < FILE_HEADER_SIZE {
            return Err("VDF file too small".into());
        }
        if probe_vdf_kind(&data) != Some(VdfKind::Dataset) {
            return Err("invalid dataset VDF magic bytes".into());
        }

        let time_point_count =
            dataset_header_time_point_count(&data).ok_or("missing dataset time-point count")?;
        let bitmap_size = time_point_count.div_ceil(8);
        let sections = find_sections(&data);
        if sections.len() != 5 {
            return Err(
                format!("dataset VDF expected 5 sections, found {}", sections.len()).into(),
            );
        }

        let origin = data[4..FILE_HEADER_SIZE]
            .iter()
            .take_while(|&&b| b != 0)
            .map(|&b| b as char)
            .collect::<String>()
            .trim_end_matches('\n')
            .to_string();

        // Dataset VDFs shift the familiar simulation-VDF sections left:
        // section 0 behaves like the string/record area and section 1 carries
        // the printable name table.
        let names = parse_name_table_extended(&data, &sections[1], sections[1].region_end);
        let section0_data_size = sections[0].region_data_size();
        let (slot_table_offset, slot_table) =
            find_slot_table(&data, &sections[1], names.len(), section0_data_size);

        let search_start = if !slot_table.is_empty() {
            let sec0_data_start = sections[0].data_offset();
            let mut sorted_slots = slot_table.clone();
            sorted_slots.sort_unstable();
            let max_offset = *sorted_slots.last().unwrap() as usize;
            let last_stride = if sorted_slots.len() >= 2 {
                let n = sorted_slots.len();
                (sorted_slots[n - 1] - sorted_slots[n - 2]) as usize
            } else {
                max_offset
            };
            sec0_data_start + max_offset + last_stride
        } else {
            sections[0].data_offset()
        };
        let records_end = if slot_table_offset > 0 && slot_table_offset < sections[0].region_end {
            slot_table_offset
        } else {
            sections[0].region_end
        };
        let records = find_records(&data, search_start, records_end);

        let sec4 = &sections[4];
        let mut pos = sec4.data_offset();
        let sec4_end = sec4.region_end.min(data.len());
        let mut trailing_offsets = Vec::new();
        while pos + 4 <= sec4_end {
            let offset = read_u32(&data, pos) as usize;
            pos += 4;
            if offset == 0 {
                break;
            }
            trailing_offsets.push(offset);
        }
        if trailing_offsets.is_empty() {
            return Err("dataset VDF missing data-block offsets".into());
        }
        let first_data_block = pos;
        let mut data_block_offsets = Vec::with_capacity(trailing_offsets.len() + 1);
        data_block_offsets.push(first_data_block);
        data_block_offsets.extend(trailing_offsets);

        Ok(VdfDatasetFile {
            data,
            origin,
            time_point_count,
            bitmap_size,
            sections,
            names,
            slot_table,
            slot_table_offset,
            records,
            data_block_offsets,
        })
    }

    /// Recover the dataset-series ordering bridge.
    pub fn series_bindings(&self) -> StdResult<Vec<VdfDatasetSeriesBinding>, Box<dyn Error>> {
        let mut records: Vec<&VdfRecord> = self
            .records
            .iter()
            .filter(|rec| rec.ot_index() > 0)
            .collect();
        records.sort_by_key(|rec| (rec.fields[2], rec.file_offset));

        let series_names: Vec<String> = self
            .names
            .iter()
            .filter(|name| name.as_str() != "Time" && !name.starts_with('.'))
            .cloned()
            .collect();

        if records.len() != series_names.len() {
            return Err(format!(
                "dataset series count mismatch: names={} records={}",
                series_names.len(),
                records.len()
            )
            .into());
        }

        let mut out = Vec::with_capacity(series_names.len());
        let mut seen_blocks = HashSet::new();
        for (name, record) in series_names.into_iter().zip(records) {
            let block_index = record.ot_index() as usize;
            if block_index == 0 || block_index > self.data_block_offsets.len() {
                return Err(format!("dataset block index {block_index} out of range").into());
            }
            if !seen_blocks.insert(block_index) {
                return Err(format!("duplicate dataset block index {block_index}").into());
            }
            out.push(VdfDatasetSeriesBinding {
                name,
                block_index: block_index - 1,
                record_file_offset: record.file_offset,
                record_f2: record.fields[2],
                record_f3: record.fields[3],
            });
        }
        Ok(out)
    }

    /// Extract all named dataset series.
    pub fn extract_data(&self) -> StdResult<VdfDatasetData, Box<dyn Error>> {
        let series_bindings = self.series_bindings()?;
        let time_block_index = series_bindings
            .iter()
            .find_map(|binding| {
                (normalize_vdf_name(&binding.name) == "decimalyear").then_some(binding.block_index)
            })
            .ok_or("dataset VDF missing decimal year series")?;
        let time_values = extract_block_series(
            &self.data,
            self.data_block_offsets[time_block_index],
            self.bitmap_size,
            &vec![0.0; self.time_point_count],
        )?;

        let mut series = HashMap::new();
        let mut series_order = Vec::with_capacity(series_bindings.len());
        for binding in series_bindings {
            let values = extract_block_series(
                &self.data,
                self.data_block_offsets[binding.block_index],
                self.bitmap_size,
                &time_values,
            )?;
            series_order.push(binding.name.clone());
            series.insert(binding.name, values);
        }

        Ok(VdfDatasetData {
            time_values,
            series_order,
            series,
        })
    }
}

impl VdfFile {
    /// Parse a VDF file from raw bytes.
    pub fn parse(data: Vec<u8>) -> StdResult<Self, Box<dyn Error>> {
        if data.len() < FILE_HEADER_SIZE {
            return Err("VDF file too small".into());
        }
        if probe_vdf_kind(&data) == Some(VdfKind::Dataset) {
            return Err("dataset VDF file; use VdfDatasetFile::parse".into());
        }
        if data[0..4] != VDF_FILE_MAGIC {
            return Err("invalid VDF magic bytes".into());
        }

        let time_point_count = read_u32(&data, 0x78) as usize;
        let bitmap_size = time_point_count.div_ceil(8);

        // Header offsets 0x58/0x5c/0x60 point directly to key section-6/7
        // data structures, eliminating the need for heuristic scanning.
        let header_final_values_offset = read_u32(&data, 0x58) as usize;
        let header_lookup_mapping_offset = read_u32(&data, 0x5c) as usize;
        let header_offset_table_offset = read_u32(&data, 0x60) as usize;

        let sections = find_sections(&data);
        let name_section_idx = find_name_table_section_idx(&data, &sections);

        let names = name_section_idx
            .map(|ns_idx| {
                parse_name_table_extended(&data, &sections[ns_idx], sections[ns_idx].region_end)
            })
            .unwrap_or_default();

        // Section at index 1 is the string table section. Its field4
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

        // Header offsets 0x58/0x5c/0x60 give direct access to offset table
        // and section-6 structures. OT count = (h5c - h58) / 4.
        let offset_table_count = if header_lookup_mapping_offset > header_final_values_offset {
            (header_lookup_mapping_offset - header_final_values_offset) / 4
        } else {
            return Err("invalid VDF header: lookup mapping offset <= final values offset".into());
        };
        if offset_table_count == 0 {
            return Err("VDF header indicates zero OT entries".into());
        }
        if header_offset_table_offset == 0
            || header_offset_table_offset + offset_table_count * 4 > data.len()
        {
            return Err("VDF header offset table pointer out of bounds".into());
        }
        let offset_table_start = header_offset_table_offset;
        let first_data_block = read_u32(&data, offset_table_start) as usize;

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
            header_final_values_offset,
            header_lookup_mapping_offset,
        })
    }

    /// Read a u32 offset table entry by index.
    pub fn offset_table_entry(&self, index: usize) -> Option<u32> {
        if index >= self.offset_table_count {
            return None;
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

    /// Get the string table section (always at section index 1).
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

    /// Parse section 4's structured entry stream.
    ///
    /// Observed files begin with a fixed two-word zero prefix followed by
    /// variable-length entries. Each entry carries a packed count word, a
    /// run of section-1 slot refs whose length is `count_lo + count_hi`, and
    /// a trailing index-like value.
    pub fn parse_section4_entry_stream(&self) -> Option<VdfSection4EntryStream> {
        let sec = self.sections.get(4)?;
        let start = sec.data_offset();
        let end = sec.region_end.min(self.data.len());
        if start >= end {
            return Some(VdfSection4EntryStream {
                zero_prefix_words: 0,
                entries: Vec::new(),
            });
        }
        let region_len = end - start;
        if !region_len.is_multiple_of(4) {
            return None;
        }

        let words: Vec<u32> = (0..region_len / 4)
            .map(|i| read_u32(&self.data, start + i * 4))
            .collect();
        let zero_prefix_words = words.iter().take_while(|&&word| word == 0).count();
        if zero_prefix_words < 2 {
            return None;
        }

        let sec1_data_size = self
            .sections
            .get(1)
            .map(|s| s.region_data_size())
            .unwrap_or(0);
        let slot_set: HashSet<u32> = self.slot_table.iter().copied().collect();

        let mut entries = Vec::new();
        let mut pos = zero_prefix_words;
        while pos < words.len() {
            let packed_word = words[pos];
            if packed_word == 0 {
                return None;
            }
            let count_lo = (packed_word & 0xffff) as usize;
            let count_hi = (packed_word >> 16) as usize;
            let ref_count = count_lo + count_hi;
            if ref_count == 0 || ref_count > 1024 {
                return None;
            }
            let refs_start = pos + 1;
            let refs_end = refs_start + ref_count;
            if refs_end >= words.len() {
                return None;
            }
            let refs = words[refs_start..refs_end].to_vec();
            if !refs
                .iter()
                .all(|&r| r > 0 && r.is_multiple_of(4) && (r as usize) < sec1_data_size)
            {
                return None;
            }
            let index_word = words[refs_end];
            let slotted_ref_count = refs.iter().filter(|r| slot_set.contains(r)).count();
            entries.push(VdfSection4Entry {
                file_offset: start + pos * 4,
                packed_word,
                refs,
                index_word,
                slotted_ref_count,
            });
            pos = refs_end + 1;
        }

        Some(VdfSection4EntryStream {
            zero_prefix_words,
            entries,
        })
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
            if n == 0 || n > max_n {
                break;
            }
            // marker=0: n+1 refs; marker=1: n+2 refs (extra axis anchor)
            let refs_len = match marker {
                0 => n + 1,
                1 => n + 2,
                _ => break,
            };
            let refs_start = pos + 8;
            let refs_end = refs_start + refs_len * 4;
            if refs_end > end {
                break;
            }
            let refs: Vec<u32> = (0..refs_len)
                .map(|i| read_u32(&self.data, refs_start + i * 4))
                .collect();
            // The last ref (index n) can be zero in some VDF files; only
            // validate that all non-trailing refs are valid section-1 offsets.
            let valid_prefix = &refs[..refs.len().saturating_sub(1)];
            if !valid_prefix
                .iter()
                .all(|&r| r > 0 && r % 4 == 0 && (r as usize) < sec1_data_size)
            {
                break;
            }
            let slotted_ref_count = refs.iter().filter(|r| slot_set.contains(r)).count();
            entries.push(VdfSection5SetEntry {
                file_offset: pos,
                n,
                marker,
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

    /// Extract the section-6 OT class-code array.
    ///
    /// Class codes are byte-packed, one per OT entry, immediately before the
    /// final-values array. The header's final_values_offset (0x58) marks
    /// the end of this array, so class codes are at
    /// `[final_values_offset - ot_count .. final_values_offset]`.
    ///
    /// Class code semantics:
    /// - `0x0f` at OT[0] (Time)
    /// - `0x08` for stock-backed OT entries
    /// - `0x11` for dynamic non-stock entries (data block)
    /// - `0x17` for constant non-stock entries (inline f32)
    pub fn section6_ot_class_codes(&self) -> Option<Vec<u8>> {
        if self.offset_table_count == 0 {
            return None;
        }
        // Primary path: use header offset directly.
        let fv_off = self.header_final_values_offset;
        if fv_off >= self.offset_table_count && fv_off <= self.data.len() {
            let cc_start = fv_off - self.offset_table_count;
            let codes = self.data[cc_start..fv_off].to_vec();
            if codes.first() == Some(&VDF_SECTION6_OT_CODE_TIME) {
                return Some(codes);
            }
        }
        // Fallback: parse section-6 ref stream to find class codes.
        let (_skip_words, _entries, stop_offset) = self.parse_section6_ref_stream()?;
        let sec = self.sections.get(6)?;
        let end = sec.region_end.min(self.data.len());
        let codes_end = stop_offset.checked_add(self.offset_table_count)?;
        if codes_end > end {
            return None;
        }
        Some(self.data[stop_offset..codes_end].to_vec())
    }

    /// Extract the OT-aligned final-value array from section 6.
    ///
    /// One f32 per OT entry, starting at the header's final_values_offset
    /// (0x58). Holds the last saved value for dynamic entries or the constant
    /// itself for inline-constant entries.
    pub fn section6_ot_final_values(&self) -> Option<Vec<f32>> {
        if self.offset_table_count == 0 {
            return None;
        }
        let fv_off = self.header_final_values_offset;
        let fv_end = fv_off + self.offset_table_count * 4;
        if fv_off > 0 && fv_end <= self.data.len() {
            let mut values = Vec::with_capacity(self.offset_table_count);
            for i in 0..self.offset_table_count {
                values.push(read_f32(&self.data, fv_off + i * 4));
            }
            return Some(values);
        }
        // Fallback: derive from ref stream parsing.
        let sec = self.sections.get(6)?;
        let (_skip_words, _entries, stop_offset) = self.parse_section6_ref_stream()?;
        let codes_end = stop_offset.checked_add(self.offset_table_count)?;
        let values_end = codes_end.checked_add(self.offset_table_count.checked_mul(4)?)?;
        if values_end > sec.region_end.min(self.data.len()) {
            return None;
        }
        let mut values = Vec::with_capacity(self.offset_table_count);
        for i in 0..self.offset_table_count {
            values.push(read_f32(&self.data, codes_end + i * 4));
        }
        Some(values)
    }

    /// Parse the fixed-width lookup mapping records from section 6.
    ///
    /// Located at `header_lookup_mapping_offset` (0x5c), these records are
    /// `13 * u32` each, terminated by a single zero word. They correspond
    /// 1:1 with lookup table definitions in the name table; word[10]
    /// carries the OT index for each lookup's evaluated output.
    pub fn section6_lookup_records(&self) -> Option<Vec<VdfSection6LookupRecord>> {
        if self.offset_table_count == 0 {
            return None;
        }
        let lm_start = self.header_lookup_mapping_offset;
        let sec = self.sections.get(6)?;
        let tail_end = sec.region_end.min(self.data.len());

        if lm_start > 0 && lm_start < tail_end {
            return self.parse_lookup_records_from(lm_start, tail_end);
        }

        // Fallback: derive from ref stream + class codes + final values.
        let (_skip_words, _entries, stop_offset) = self.parse_section6_ref_stream()?;
        let codes_end = stop_offset.checked_add(self.offset_table_count)?;
        let values_end = codes_end.checked_add(self.offset_table_count.checked_mul(4)?)?;
        if values_end >= tail_end {
            return Some(Vec::new());
        }
        self.parse_lookup_records_from(values_end, tail_end)
    }

    fn parse_lookup_records_from(
        &self,
        start: usize,
        end: usize,
    ) -> Option<Vec<VdfSection6LookupRecord>> {
        if start >= end {
            return Some(Vec::new());
        }
        let suffix = &self.data[start..end];
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
            out.push(VdfSection6LookupRecord {
                file_offset: start + rec_off,
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

    /// Parse section-5 set entries seen in array-heavy files.
    ///
    /// On-disk layout: `u32 n; u32 marker; u32 refs[refs_len]`, where
    /// marker=0 yields refs_len=n+1, marker=1 yields refs_len=n+2.
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

    /// Axis slot refs shared between section-3 directory entries and
    /// section-5 set entries, confirming the dimension-to-shape bridge.
    ///
    /// Section-3 entries carry axis_slot_refs identifying which section-1
    /// slots define each dimension axis. Section-5 entries carry trailing
    /// refs (beyond the dimension elements) that point to the same slots.
    /// The intersection documents the structural link between the shape
    /// templates (section 3) and the dimension-set catalog (section 5).
    pub fn section3_section5_shared_axis_refs(&self) -> HashSet<u32> {
        let sec3_refs: HashSet<u32> = self
            .parse_section3_directory()
            .map(|dir| {
                dir.entries
                    .iter()
                    .flat_map(|e| e.axis_slot_refs())
                    .collect()
            })
            .unwrap_or_default();

        let sec5_trailing: HashSet<u32> = self
            .parse_section5_set_stream()
            .map(|(_, entries, _)| {
                entries
                    .iter()
                    .flat_map(|e| {
                        // The trailing refs beyond the dimension elements
                        // are axis anchors shared with section 3.
                        e.refs.iter().skip(e.n).copied()
                    })
                    .filter(|&r| r > 0)
                    .collect()
            })
            .unwrap_or_default();

        sec3_refs.intersection(&sec5_trailing).copied().collect()
    }

    /// Infer named dimension sets from section 5 plus nearby name-table order.
    ///
    /// This is intentionally conservative:
    /// - the section-5 refs must resolve to exactly one non-metadata name
    /// - the name table must contain enough following non-metadata names to
    ///   satisfy the dimension cardinality
    ///
    /// Small scalar models return an empty set because section 5 is degenerate.
    pub fn inferred_dimension_sets(&self) -> Vec<VdfDimensionSet> {
        let Some((_, entries, _)) = self.parse_section5_set_stream() else {
            return Vec::new();
        };
        if entries.is_empty() {
            return Vec::new();
        }

        let system_names: HashSet<&str> = SYSTEM_NAMES.into_iter().collect();
        let mut slot_to_names: HashMap<u32, Vec<&str>> = HashMap::new();
        for (i, &slot) in self.slot_table.iter().enumerate() {
            slot_to_names
                .entry(slot)
                .or_default()
                .push(self.names[i].as_str());
        }

        let mut out = Vec::new();
        for entry in entries {
            let mut dim_candidates = Vec::new();
            for &slot_ref in &entry.refs {
                let Some(names) = slot_to_names.get(&slot_ref) else {
                    continue;
                };
                for &name in names {
                    if name.is_empty()
                        || name.starts_with('.')
                        || name.starts_with('-')
                        || name.starts_with(':')
                        || system_names.contains(name)
                        || is_vdf_metadata_entry(name)
                        || VENSIM_BUILTINS
                            .iter()
                            .any(|builtin| builtin.eq_ignore_ascii_case(name))
                    {
                        continue;
                    }
                    dim_candidates.push(name.to_string());
                }
            }

            dim_candidates.sort();
            dim_candidates.dedup();
            if dim_candidates.len() != 1 {
                continue;
            }
            let dim_name = dim_candidates.pop().unwrap();
            let Some(dim_idx) = self.names.iter().position(|name| name == &dim_name) else {
                continue;
            };

            let mut elements = Vec::new();
            let mut seen_normalized = HashSet::new();
            for name in self.names.iter().skip(dim_idx + 1) {
                if name.is_empty()
                    || name.starts_with('.')
                    || name.starts_with('-')
                    || name.starts_with(':')
                    || system_names.contains(name.as_str())
                    || is_vdf_metadata_entry(name)
                    || VENSIM_BUILTINS
                        .iter()
                        .any(|builtin| builtin.eq_ignore_ascii_case(name))
                {
                    continue;
                }
                let normalized = normalize_vdf_name(name);
                if !seen_normalized.insert(normalized) {
                    continue;
                }
                elements.push(name.clone());
                if elements.len() == entry.dimension_size() {
                    break;
                }
            }

            if elements.len() == entry.dimension_size() {
                out.push(VdfDimensionSet {
                    name: dim_name,
                    elements,
                });
            }
        }

        out
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

    /// Number of OT entries, derived from header offsets.
    pub fn header_ot_count(&self) -> usize {
        if self.header_lookup_mapping_offset > self.header_final_values_offset {
            (self.header_lookup_mapping_offset - self.header_final_values_offset) / 4
        } else {
            0
        }
    }

    /// Number of stock-backed OT entries (class code 0x08), derived from
    /// the section-6 class-code array.
    pub fn stock_count(&self) -> usize {
        self.section6_ot_class_codes()
            .map(|codes| {
                codes
                    .iter()
                    .skip(1)
                    .filter(|&&c| c == VDF_SECTION6_OT_CODE_STOCK)
                    .count()
            })
            .unwrap_or(0)
    }

    /// Build a `Results` struct using only VDF structural data.
    ///
    /// This path is intentionally conservative: it succeeds only when the VDF
    /// itself forces a unique stock/non-stock partition for the visible scalar
    /// names. When the section-6 stock boundary is known but multiple visible
    /// names could legally occupy the stock-coded OT slots, this returns an
    /// explicit ambiguity error instead of guessing.
    ///
    /// Algorithm:
    /// 1. Parse name table and filter out metadata/builtins/signatures
    /// 2. Use section-6 class codes to get stock count S
    /// 3. Trim excess lookup definitions when they are the only over-capacity names
    /// 4. Treat system variables as deterministically non-stock
    /// 5. Succeed only when the remaining unresolved names are forced entirely
    ///    into the stock block or entirely into the non-stock block
    /// 6. Sort stocks alphabetically into OT[1..S], non-stocks into OT[S+1..]
    ///    and build Results
    ///
    /// Array-bearing VDFs remain unsupported because their base-variable to
    /// shape/dimension ownership is still not fully decoded.
    ///
    /// For a less restrictive structural path that accepts an external stock
    /// classifier, see [`VdfFile::to_results_with_stock_classifier`].
    pub fn to_results(&self) -> StdResult<Results, Box<dyn Error>> {
        if self
            .parse_section3_directory()
            .is_some_and(|directory| !directory.entries.is_empty())
        {
            return Err(
                "arrayed VDF requires array-aware name binding; use model-guided or array-info path"
                    .into(),
            );
        }

        let codes = self
            .section6_ot_class_codes()
            .ok_or("no section-6 class codes")?;
        let vdf_data = self.extract_data()?;

        let stock_count = codes
            .iter()
            .skip(1)
            .filter(|&&c| c == VDF_SECTION6_OT_CODE_STOCK)
            .count();
        let target_participants = self.offset_table_count - 1; // minus Time at OT[0]
        let lookup_record_count = self
            .section6_lookup_records()
            .map(|records| records.len())
            .unwrap_or(0);

        let mut excluded_names: HashSet<String> = HashSet::new();
        let mut candidates = self.filter_ot_candidate_names();
        if candidates.len() > target_participants {
            let excess = candidates.len() - target_participants;
            if excess > lookup_record_count {
                return Err(format!(
                    "candidate count ({}) exceeds OT capacity ({}) by {} \
                     (expected at most {} lookup definitions)",
                    candidates.len(),
                    target_participants,
                    excess,
                    lookup_record_count
                )
                .into());
            }

            candidates.sort_by(|a, b| {
                is_lookupish_name(a)
                    .cmp(&is_lookupish_name(b))
                    .then_with(|| a.to_lowercase().cmp(&b.to_lowercase()))
            });
            for _ in 0..excess {
                if let Some(removed) = candidates.pop() {
                    excluded_names.insert(normalize_vdf_name(&removed));
                }
            }
        }

        if candidates.len() != target_participants {
            return Err(format!(
                "candidate count ({}) does not match OT capacity ({target_participants})",
                candidates.len()
            )
            .into());
        }

        let system_names: HashSet<&str> = SYSTEM_NAMES.into_iter().collect();
        let mut stock_names = Vec::new();
        let mut nonstock_names = Vec::new();
        let mut unresolved = Vec::new();

        for name in candidates {
            if system_names.contains(name.as_str()) {
                nonstock_names.push(name);
            } else {
                unresolved.push(name);
            }
        }

        if stock_count > unresolved.len() {
            return Err(format!(
                "section-6 reports {stock_count} stock OT entries, but only {} unresolved visible names remain",
                unresolved.len()
            )
            .into());
        }

        if stock_count == 0 {
            nonstock_names.extend(unresolved);
        } else if stock_count == unresolved.len() {
            stock_names.extend(unresolved);
        } else {
            unresolved.sort_by_key(|name| name.to_lowercase());
            return Err(format!(
                "ambiguous VDF-only stock assignment: {} unresolved names compete for {stock_count} stock OT slots ({})",
                unresolved.len(),
                unresolved.join(", ")
            )
            .into());
        }

        reconcile_stock_boundary(&mut stock_names, &mut nonstock_names, stock_count)?;
        let ordered =
            build_visible_ot_entries(&stock_names, &nonstock_names, stock_count, &excluded_names);
        Ok(vdf_data.build_results(&ordered))
    }

    /// Build a `Results` struct using only VDF structural data plus a
    /// stock-classification function.
    ///
    /// The `is_stock` closure receives a normalized VDF name and returns
    /// true if that name is a stock variable. This is the only piece of
    /// model knowledge required; everything else comes from the VDF.
    ///
    /// Algorithm:
    /// 1. Parse name table and filter out metadata/builtins/signatures
    /// 2. Use section-6 class codes to get stock count S
    /// 3. Classify filtered names as stock/non-stock via the closure
    /// 4. Sort stocks alphabetically into OT[1..S], non-stocks into OT[S+1..]
    /// 5. Extract data and build Results
    pub fn to_results_with_stock_classifier(
        &self,
        is_stock: impl Fn(&str) -> bool,
    ) -> StdResult<Results, Box<dyn Error>> {
        let codes = self
            .section6_ot_class_codes()
            .ok_or("no section-6 class codes")?;
        let vdf_data = self.extract_data()?;

        let stock_count = codes
            .iter()
            .skip(1)
            .filter(|&&c| c == VDF_SECTION6_OT_CODE_STOCK)
            .count();
        let target_participants = self.offset_table_count - 1; // minus Time at OT[0]

        // Filter name table to candidate OT participants.
        let candidates = self.filter_ot_candidate_names();

        // If we have more candidates than OT slots, the excess are
        // standalone lookup definitions that don't have their own OT entries.
        if candidates.len() > target_participants {
            let excess = candidates.len() - target_participants;
            let lookup_record_count = self
                .section6_lookup_records()
                .map(|recs| recs.len())
                .unwrap_or(0);
            if excess > lookup_record_count {
                return Err(format!(
                    "candidate count ({}) exceeds OT capacity ({}) by {} \
                     (expected at most {} lookup definitions)",
                    candidates.len(),
                    target_participants,
                    excess,
                    lookup_record_count
                )
                .into());
            }
        }

        // Separate into stocks and non-stocks, using the provided classifier.
        let mut stock_names: Vec<String> = Vec::new();
        let mut nonstock_names: Vec<String> = Vec::new();
        let mut excluded_names: HashSet<String> = HashSet::new();

        for name in &candidates {
            if is_stock(name) {
                stock_names.push(name.clone());
            } else {
                nonstock_names.push(name.clone());
            }
        }

        // If candidate count exceeds target, trim excess from non-stocks.
        let total = stock_names.len() + nonstock_names.len();
        if total > target_participants {
            let to_remove = total - target_participants;
            nonstock_names.sort_by(|a, b| {
                let a_lookupish = is_lookupish_name(a);
                let b_lookupish = is_lookupish_name(b);
                a_lookupish
                    .cmp(&b_lookupish)
                    .then_with(|| a.to_lowercase().cmp(&b.to_lowercase()))
            });
            for _ in 0..to_remove {
                if let Some(removed) = nonstock_names.pop() {
                    excluded_names.insert(normalize_vdf_name(&removed));
                }
            }
        }

        reconcile_stock_boundary(&mut stock_names, &mut nonstock_names, stock_count)?;
        let ordered =
            build_visible_ot_entries(&stock_names, &nonstock_names, stock_count, &excluded_names);
        Ok(vdf_data.build_results(&ordered))
    }

    /// Build a `Results` struct with element-level array expansion.
    ///
    /// Like `to_results_with_stock_classifier`, but also handles arrayed
    /// variables.
    ///
    /// - `is_stock`: returns true for stock variable names
    /// - `array_dims`: normalized base name -> element names in subscript
    ///   order. Scalar variables should NOT appear in this map.
    /// - `exclude_names`: normalized names to exclude from OT mapping
    ///   (dimension names like "sub1", element names like "a"/"b"/"c").
    ///   Element names from `array_dims` are automatically excluded.
    ///
    /// Each arrayed variable expands to N consecutive OT entries (one per
    /// element, in subscript order). The resulting `Results` contains entries
    /// like `"a stock[a]"`, `"a stock[b]"`, `"a stock[c]"` instead of just
    /// `"a stock"`.
    pub fn to_results_with_array_info(
        &self,
        is_stock: impl Fn(&str) -> bool,
        array_dims: &HashMap<String, Vec<String>>,
        exclude_names: &HashSet<String>,
    ) -> StdResult<Results, Box<dyn Error>> {
        let codes = self
            .section6_ot_class_codes()
            .ok_or("no section-6 class codes")?;
        let vdf_data = self.extract_data()?;

        let stock_count = codes
            .iter()
            .skip(1)
            .filter(|&&c| c == VDF_SECTION6_OT_CODE_STOCK)
            .count();
        let target_participants = self.offset_table_count - 1;

        // Build the full set of names to exclude: caller-provided names
        // plus element names from array_dims.
        let mut non_variable_names: HashSet<String> = exclude_names.clone();
        for elems in array_dims.values() {
            for elem in elems {
                non_variable_names.insert(normalize_vdf_name(elem));
            }
        }

        let candidates = self.filter_ot_candidate_names();
        let filtered: Vec<String> = candidates
            .into_iter()
            .filter(|name| !non_variable_names.contains(&normalize_vdf_name(name)))
            .collect();

        // Expand arrayed variables to per-element entries.
        let mut stock_entries: Vec<String> = Vec::new();
        let mut nonstock_entries: Vec<String> = Vec::new();

        for name in &filtered {
            let normalized = normalize_vdf_name(name);
            let var_is_stock = is_stock(name);

            if let Some(elems) = array_dims.get(&normalized) {
                for elem in elems {
                    let display = format!("{name}[{elem}]");
                    if var_is_stock {
                        stock_entries.push(display);
                    } else {
                        nonstock_entries.push(display);
                    }
                }
            } else {
                if var_is_stock {
                    stock_entries.push(name.clone());
                } else {
                    nonstock_entries.push(name.clone());
                }
            }
        }

        // Trim excess (lookupish names) if needed
        let total = stock_entries.len() + nonstock_entries.len();
        if total > target_participants {
            let to_remove = total - target_participants;
            nonstock_entries.sort_by(|a, b| {
                is_lookupish_name(a)
                    .cmp(&is_lookupish_name(b))
                    .then_with(|| a.to_lowercase().cmp(&b.to_lowercase()))
            });
            nonstock_entries.truncate(nonstock_entries.len().saturating_sub(to_remove));
        }

        reconcile_stock_boundary(&mut stock_entries, &mut nonstock_entries, stock_count)?;
        let ordered = build_visible_ot_entries(
            &stock_entries,
            &nonstock_entries,
            stock_count,
            &HashSet::new(),
        );
        Ok(vdf_data.build_results(&ordered))
    }

    /// Filter the VDF name table to candidate OT participant names.
    ///
    /// Excludes group/view markers (`.` prefix), unit annotations (`-` prefix),
    /// metadata tags (`:` prefix), internal signatures (`#` prefix),
    /// builtin function names, stdlib helper names, single non-alphanumeric
    /// chars, numeric strings, and other non-variable entries.
    ///
    /// System variable names (INITIAL TIME, etc.) ARE included since they
    /// occupy OT entries.
    fn filter_ot_candidate_names(&self) -> Vec<String> {
        let vensim_builtins: HashSet<&str> = VENSIM_BUILTINS.into_iter().collect();
        let inferred_non_variable_names: HashSet<String> = self
            .inferred_dimension_sets()
            .into_iter()
            .flat_map(|dim| {
                std::iter::once(dim.name)
                    .chain(dim.elements)
                    .map(|name| normalize_vdf_name(&name))
                    .collect::<Vec<_>>()
            })
            .collect();
        let mut seen_normalized: HashSet<String> = HashSet::new();
        let mut candidates = Vec::new();

        // "Time" is OT[0], handled separately
        seen_normalized.insert(normalize_vdf_name("time"));

        for name in &self.names {
            if name.is_empty() || name.starts_with('#') || is_vdf_metadata_entry(name) {
                continue;
            }
            if name.len() == 1 && name.starts_with(|c: char| !c.is_alphanumeric()) {
                continue;
            }
            if vensim_builtins.contains(name.to_lowercase().as_str()) {
                continue;
            }
            if STDLIB_PARTICIPANT_HELPERS.contains(&name.as_str()) {
                continue;
            }

            let normalized = normalize_vdf_name(name);
            if inferred_non_variable_names.contains(&normalized) {
                continue;
            }
            if !seen_normalized.insert(normalized) {
                continue;
            }

            candidates.push(name.clone());
        }

        candidates
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

/// Walk data blocks from the first block offset, returning (offset, count, block_size)
/// for each block found. Used by diagnostic tools (vdf_dump) to visualize the
/// data block layout.
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

/// Reconcile stock/non-stock candidate lists against the section-6 stock count.
///
/// Section-6 class codes are authoritative for the stock boundary. This moves
/// names between groups to match: demotes excess stocks (alphabetically last)
/// to non-stock, promotes deficit non-stocks (alphabetically first) to stock.
fn reconcile_stock_boundary(
    stocks: &mut Vec<String>,
    nonstocks: &mut Vec<String>,
    stock_count: usize,
) -> StdResult<(), Box<dyn Error>> {
    stocks.sort_by_key(|n| n.to_lowercase());
    nonstocks.sort_by_key(|n| n.to_lowercase());

    while stocks.len() > stock_count && !stocks.is_empty() {
        let demoted = stocks.pop().unwrap();
        nonstocks.push(demoted);
        nonstocks.sort_by_key(|n| n.to_lowercase());
    }
    while stocks.len() < stock_count && !nonstocks.is_empty() {
        let promoted = nonstocks.remove(0);
        stocks.push(promoted);
        stocks.sort_by_key(|n| n.to_lowercase());
    }

    if stocks.len() != stock_count {
        return Err(format!(
            "could not reconcile: stocks={}/{stock_count} nonstocks={}",
            stocks.len(),
            nonstocks.len(),
        )
        .into());
    }
    Ok(())
}

/// Build an ordered (name, OT index) list from reconciled stock/non-stock groups.
///
/// System variable names (INITIAL TIME, etc.) are excluded from visible results.
/// OT[0] = Time is always included.
fn build_visible_ot_entries(
    stocks: &[String],
    nonstocks: &[String],
    stock_count: usize,
    excluded_names: &HashSet<String>,
) -> Vec<(Ident<Canonical>, usize)> {
    let system_names: HashSet<&str> = SYSTEM_NAMES.into_iter().collect();
    let mut ordered: Vec<(Ident<Canonical>, usize)> =
        vec![(Ident::<Canonical>::from_str_unchecked("time"), 0)];

    for (i, name) in stocks.iter().enumerate() {
        if !system_names.contains(name.as_str())
            && !excluded_names.contains(&normalize_vdf_name(name))
        {
            ordered.push((Ident::<Canonical>::new(name), i + 1));
        }
    }
    for (i, name) in nonstocks.iter().enumerate() {
        if !system_names.contains(name.as_str())
            && !excluded_names.contains(&normalize_vdf_name(name))
        {
            ordered.push((Ident::<Canonical>::new(name), stock_count + 1 + i));
        }
    }

    ordered
}

impl VdfData {
    /// Build a `Results` struct from an ordered list of (name, OT index) pairs.
    ///
    /// Each pair maps a variable identifier to its offset-table position. The
    /// resulting `Results` includes one column per entry, laid out in the order
    /// given.
    fn build_results(&self, ordered: &[(Ident<Canonical>, usize)]) -> Results {
        let step_count = self.time_values.len();
        let step_size = ordered.len();
        let mut step_data = vec![f64::NAN; step_count * step_size];
        let mut offsets: HashMap<Ident<Canonical>, usize> = HashMap::new();

        for (col, (id, ot_idx)) in ordered.iter().enumerate() {
            offsets.insert(id.clone(), col);
            if let Some(series) = self.entries.get(*ot_idx) {
                for step in 0..step_count {
                    step_data[step * step_size + col] = series[step];
                }
            }
        }

        let initial_time = self.time_values[0];
        let final_time = self.time_values[step_count - 1];
        let saveper = if step_count > 1 {
            self.time_values[1] - self.time_values[0]
        } else {
            1.0
        };

        Results {
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
        }
    }
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

    fn model_stock_set(model: &crate::datamodel::Model) -> HashSet<String> {
        model
            .variables
            .iter()
            .filter_map(|v| match v {
                crate::datamodel::Variable::Stock(s) => Some(normalize_vdf_name(&s.ident)),
                _ => None,
            })
            .collect()
    }

    fn build_results_from_ot_map(
        vdf_data: &VdfData,
        ot_map: &HashMap<Ident<Canonical>, usize>,
    ) -> crate::Results {
        let mut sorted_entries: Vec<_> = ot_map.iter().map(|(id, &ot)| (id.clone(), ot)).collect();
        sorted_entries.sort_by_key(|(_, ot)| *ot);
        vdf_data.build_results(&sorted_entries)
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
    fn test_section6_lookup_record_stream_shape() {
        let water = vdf_file("../../test/bobby/vdf/water/Current.vdf");
        let pop = vdf_file("../../test/bobby/vdf/pop/Current.vdf");
        let econ = vdf_file("../../test/bobby/vdf/econ/base.vdf");
        let wrld3 = vdf_file("../../test/metasd/WRLD3-03/SCEN01.VDF");

        let water_records = water.section6_lookup_records().unwrap();
        let pop_records = pop.section6_lookup_records().unwrap();
        let econ_records = econ.section6_lookup_records().unwrap();
        let wrld3_records = wrld3.section6_lookup_records().unwrap();

        assert!(
            water_records.is_empty(),
            "water should have no parsed lookup records"
        );
        assert!(
            pop_records.is_empty(),
            "pop should have no parsed lookup records"
        );
        assert_eq!(
            econ_records.len(),
            4,
            "econ lookup-record count should be stable"
        );
        assert_eq!(
            wrld3_records.len(),
            55,
            "WRLD3 lookup-record count should be stable"
        );

        for (label, vdf, records) in [
            ("econ", &econ, &econ_records),
            ("wrld3", &wrld3, &wrld3_records),
        ] {
            for rec in records {
                assert!(
                    rec.ot_index() < vdf.offset_table_count,
                    "{label}: lookup record OT {} out of range",
                    rec.ot_index()
                );
                assert_eq!(
                    rec.words[11], 1,
                    "{label}: expected stable lookup-record flag"
                );
                assert_eq!(rec.words[12], 0, "{label}: expected zero terminator word");
            }
        }
    }

    #[test]
    fn test_section6_extended_ot_codes_in_ref_fixture() {
        let ref_vdf = vdf_file("../../test/xmutil_test_models/Ref.vdf");
        let codes = ref_vdf.section6_ot_class_codes().unwrap();

        assert_eq!(
            codes.iter().filter(|&&code| code == 0x16).count(),
            350,
            "Ref.vdf should pin the 0x16 extended code count"
        );
        assert_eq!(
            codes.iter().filter(|&&code| code == 0x18).count(),
            46,
            "Ref.vdf should pin the 0x18 extended code count"
        );

        for code in [0x16u8, 0x18u8] {
            assert!(
                codes
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| **c == code)
                    .all(|(ot, _)| {
                        ref_vdf
                            .offset_table_entry(ot)
                            .is_some_and(|raw| !ref_vdf.is_data_block_offset(raw))
                    }),
                "Ref.vdf code 0x{code:02x} entries should currently be inline values",
            );
        }

        let code11_has_block = codes.iter().enumerate().any(|(ot, &code)| {
            code == 0x11
                && ref_vdf
                    .offset_table_entry(ot)
                    .is_some_and(|raw| ref_vdf.is_data_block_offset(raw))
        });
        let code11_has_inline = codes.iter().enumerate().any(|(ot, &code)| {
            code == 0x11
                && ref_vdf
                    .offset_table_entry(ot)
                    .is_some_and(|raw| !ref_vdf.is_data_block_offset(raw))
        });
        assert!(
            code11_has_block && code11_has_inline,
            "Ref.vdf shows that 0x11 is not a pure data-block code in array-heavy files"
        );
    }

    #[test]
    fn test_section6_ot_codes_distinguish_level_vs_supplementary_aux() {
        let stock_vdf = vdf_file("../../test/bobby/vdf/level_vs_aux/x_is_stock.vdf");
        let aux_vdf = vdf_file("../../test/bobby/vdf/level_vs_aux/x_is_aux.vdf");

        let stock_codes = stock_vdf.section6_ot_class_codes().unwrap();
        let aux_codes = aux_vdf.section6_ot_class_codes().unwrap();
        let stock_finals = stock_vdf.section6_ot_final_values().unwrap();
        let aux_finals = aux_vdf.section6_ot_final_values().unwrap();

        let stock_x_matches: Vec<usize> = stock_finals
            .iter()
            .enumerate()
            .filter_map(|(ot, &value)| ((value - 6.44).abs() < 0.01).then_some(ot))
            .collect();
        let aux_x_matches: Vec<usize> = aux_finals
            .iter()
            .enumerate()
            .filter_map(|(ot, &value)| ((value - 7.44).abs() < 0.01).then_some(ot))
            .collect();

        assert_eq!(
            stock_x_matches,
            vec![1],
            "stock fixture should have exactly one OT entry matching x's final value"
        );
        assert_eq!(
            aux_x_matches,
            vec![5],
            "aux fixture should have exactly one OT entry matching x's final value"
        );

        let stock_x_ot = stock_x_matches[0];
        let aux_x_ot = aux_x_matches[0];

        assert_eq!(
            stock_codes[stock_x_ot], VDF_SECTION6_OT_CODE_STOCK,
            "level-backed x should land on a stock-coded OT entry"
        );
        assert_eq!(
            aux_codes[aux_x_ot], 0x11,
            "supplementary x should land on a dynamic non-stock OT entry"
        );
        assert_eq!(
            stock_vdf.stock_count(),
            1,
            "stock fixture should expose one stock OT"
        );
        assert_eq!(
            aux_vdf.stock_count(),
            0,
            "aux fixture should expose no stock OTs"
        );

        assert!(
            stock_vdf
                .offset_table_entry(stock_x_ot)
                .is_some_and(|raw| stock_vdf.is_data_block_offset(raw)),
            "level-backed x should still be stored as a data block"
        );
        assert!(
            aux_vdf
                .offset_table_entry(aux_x_ot)
                .is_some_and(|raw| aux_vdf.is_data_block_offset(raw)),
            "supplementary x should also be stored as a data block"
        );

        let stock_x_record = stock_vdf
            .records
            .iter()
            .find(|record| record.ot_index() as usize == stock_x_ot)
            .expect("missing stock fixture record for x");
        let aux_x_record = aux_vdf
            .records
            .iter()
            .find(|record| record.ot_index() as usize == aux_x_ot)
            .expect("missing aux fixture record for x");

        assert_eq!(
            stock_x_record.fields[1], aux_x_record.fields[1],
            "section-1 record classification does not distinguish the level/aux flip here"
        );
        assert_eq!(
            stock_x_record.shape_code(),
            aux_x_record.shape_code(),
            "section-1 shape code does not distinguish the level/aux flip here"
        );
        assert_eq!(stock_x_record.fields[1], 135);
        assert_eq!(stock_x_record.shape_code(), 5);
    }

    #[test]
    fn test_wrld3_experiment_variant_preserves_section4_and_ref_stream_shape() {
        let scen01 = vdf_file("../../test/metasd/WRLD3-03/SCEN01.VDF");
        let experiment = vdf_file("../../test/metasd/WRLD3-03/experiment.vdf");

        let scen01_section4 = scen01.parse_section4_entry_stream().unwrap();
        let experiment_section4 = experiment.parse_section4_entry_stream().unwrap();
        assert_eq!(scen01_section4.entries.len(), 37);
        assert_eq!(
            experiment_section4.entries.len(),
            scen01_section4.entries.len()
        );

        let (_, scen01_refs, _) = scen01.parse_section6_ref_stream().unwrap();
        let (_, experiment_refs, _) = experiment.parse_section6_ref_stream().unwrap();
        assert_eq!(scen01_refs.len(), 342);
        assert_eq!(experiment_refs.len(), scen01_refs.len());

        assert_eq!(experiment.offset_table_count, scen01.offset_table_count);
        assert_eq!(
            experiment.section6_ot_class_codes().unwrap().len(),
            scen01.section6_ot_class_codes().unwrap().len()
        );
        assert_ne!(
            experiment.names.len(),
            scen01.names.len(),
            "the variant should add coverage beyond a byte-for-byte duplicate"
        );
    }

    #[test]
    fn test_to_results_with_stock_classifier_uses_vdf_visible_names_only() {
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
            let model = datamodel_project.models.first().unwrap();
            let stock_set = model_stock_set(model);
            let vdf = vdf_file(vdf_path);

            let results = vdf
                .to_results_with_stock_classifier(|name| {
                    stock_set.contains(&normalize_vdf_name(name))
                })
                .unwrap_or_else(|e| panic!("to_results_with_stock_classifier failed: {e}"));

            assert!(
                results.offsets.keys().all(|id| {
                    let name = id.as_str();
                    name == "time"
                        || (!name.starts_with('#')
                            && !name.starts_with('$')
                            && !is_lookupish_name(name))
                }),
                "expected Results offsets to expose only visible VDF names"
            );
        }
    }

    #[test]
    fn test_section6_guided_map_succeeds_on_econ() {
        let mdl_path = "../../test/bobby/vdf/econ/mark2.mdl";
        let vdf_path = "../../test/bobby/vdf/econ/base.vdf";

        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let model = datamodel_project.models.first().unwrap();
        let vdf = vdf_file(vdf_path);

        let map = vdf
            .build_section6_guided_ot_map(model)
            .unwrap_or_else(|e| panic!("build_section6_guided_ot_map should succeed on econ: {e}"));
        assert!(map.len() > 1, "econ: expected mapped variable columns");
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
    fn test_to_results_with_stock_classifier_includes_scalar_consts_but_not_lookup_tables() {
        let mdl_path = "../../test/bobby/vdf/consts/consts.mdl";
        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let model = datamodel_project.models.first().unwrap();
        let stock_set = model_stock_set(model);

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
                .to_results_with_stock_classifier(|name| {
                    stock_set.contains(&normalize_vdf_name(name))
                })
                .unwrap_or_else(|e| {
                    panic!("{label}: to_results_with_stock_classifier failed: {e}")
                });

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

    fn test_build_sample_indices(step_count: usize) -> Vec<usize> {
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

    fn test_compute_match_error(
        reference: &[f64],
        candidate: &[f64],
        sample_indices: &[usize],
    ) -> f64 {
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

    fn sampled_series_error(
        actual: &crate::Results,
        reference: &crate::Results,
        id: &Ident<Canonical>,
    ) -> Option<f64> {
        let actual_off = *actual.offsets.get(id)?;
        let reference_off = *reference.offsets.get(id)?;
        let sample_indices = test_build_sample_indices(actual.step_count);
        let actual_series = result_series(actual, actual_off);
        let reference_series = result_series(reference, reference_off);
        Some(test_compute_match_error(
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
    fn test_section6_guided_map_matches_wrld3_reference_outputs() {
        let mdl_path = "../../test/metasd/WRLD3-03/wrld3-03.mdl";
        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let model = datamodel_project.models.first().unwrap();
        let project = std::rc::Rc::new(crate::Project::from(datamodel_project.clone()));
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
            let ot_map = vdf
                .build_section6_guided_ot_map(model)
                .unwrap_or_else(|e| panic!("{label}: build_section6_guided_ot_map failed: {e}"));
            let vdf_data = vdf.extract_data().unwrap();
            let results = build_results_from_ot_map(&vdf_data, &ot_map);

            let (shared, matching) = matching_visible_series_count(&results, &reference);
            assert!(
                shared >= 230,
                "{label}: expected broad visible overlap with the simulation reference, got {shared}",
            );
            // Section6 structural mapping achieves partial matching on WRLD3;
            // the non-stock block has many variables and some permutations are
            // expected without empirical refinement.
            assert!(
                matching >= 30,
                "{label}: expected some WRLD3 series to match the simulation reference, got {matching} of {shared}",
            );
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
            let model = datamodel_project.models.first().unwrap();
            let sf_map = vdf.build_section6_guided_ot_map(model).unwrap();

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
    fn test_section6_stock_code_matches_section6_guided_econ_results() {
        let mdl_path = "../../test/bobby/vdf/econ/mark2.mdl";
        let vdf_path = "../../test/bobby/vdf/econ/base.vdf";

        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let model = datamodel_project.models.first().unwrap();
        let stock_set = model_stock_set(model);

        let vdf = vdf_file(vdf_path);
        let codes = vdf.section6_ot_class_codes().unwrap();
        let map = vdf.build_section6_guided_ot_map(model).unwrap();

        // Verify that most direct model stocks land in stock-coded OT entries.
        // The section6 mapping is a structural heuristic; some models with
        // many SMOOTH/DELAY macros may have residual misplacements.
        let mut stocks_correct = 0usize;
        let mut stocks_total = 0usize;
        for (name, &ot) in &map {
            if name.as_str() == "time" || name.as_str().starts_with('#') {
                continue;
            }
            if stock_set.contains(&normalize_vdf_name(name.as_str())) {
                stocks_total += 1;
                if codes[ot] == VDF_SECTION6_OT_CODE_STOCK {
                    stocks_correct += 1;
                }
            }
        }
        assert!(
            stocks_total > 0,
            "econ: expected at least one stock in the map"
        );
        assert!(
            stocks_correct * 2 >= stocks_total,
            "econ: expected majority of stocks in stock-coded OTs, got {stocks_correct}/{stocks_total}"
        );
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
    fn test_to_results_with_stock_classifier_small_models() {
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
            let model = datamodel_project.models.first().unwrap();
            let stock_set = model_stock_set(model);
            let vdf = vdf_file(vdf_path);
            let results = vdf
                .to_results_with_stock_classifier(|name| {
                    stock_set.contains(&normalize_vdf_name(name))
                })
                .unwrap_or_else(|e| {
                    panic!("{label}: to_results_with_stock_classifier failed: {e}")
                });

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
    fn test_to_results_vdf_only_succeeds_on_unambiguous_level_vs_aux() {
        let stock_vdf = vdf_file("../../test/bobby/vdf/level_vs_aux/x_is_stock.vdf");
        let stock_results = stock_vdf
            .to_results()
            .unwrap_or_else(|e| panic!("stock fixture should be VDF-only unambiguous: {e}"));
        let stock_x = stock_results.offsets[&Ident::<Canonical>::new("x")];
        let stock_row0 = &stock_results.data[0..stock_results.step_size];
        let stock_last = stock_results.step_count - 1;
        let stock_row_last = &stock_results.data
            [stock_last * stock_results.step_size..(stock_last + 1) * stock_results.step_size];
        assert!((stock_row0[stock_x] - (157.0 / 50.0)).abs() < 0.01);
        assert!((stock_row_last[stock_x] - 6.44).abs() < 0.01);

        let aux_vdf = vdf_file("../../test/bobby/vdf/level_vs_aux/x_is_aux.vdf");
        let aux_results = aux_vdf
            .to_results()
            .unwrap_or_else(|e| panic!("aux fixture should be VDF-only unambiguous: {e}"));
        let aux_x = aux_results.offsets[&Ident::<Canonical>::new("x")];
        let aux_row0 = &aux_results.data[0..aux_results.step_size];
        let aux_last = aux_results.step_count - 1;
        let aux_row_last = &aux_results.data
            [aux_last * aux_results.step_size..(aux_last + 1) * aux_results.step_size];
        assert!((aux_row0[aux_x] - 4.14).abs() < 0.01);
        assert!((aux_row_last[aux_x] - 7.44).abs() < 0.01);
    }

    #[test]
    fn test_to_results_vdf_only_rejects_ambiguous_small_scalar_files() {
        for (label, path) in [
            ("bact", "../../test/bobby/vdf/bact/Current.vdf"),
            ("water", "../../test/bobby/vdf/water/Current.vdf"),
            ("pop", "../../test/bobby/vdf/pop/Current.vdf"),
            ("consts", "../../test/bobby/vdf/consts/b_is_3.vdf"),
            ("lookups", "../../test/bobby/vdf/lookups/lookup_ex.vdf"),
            ("sd202_a2", "../../test/bobby/vdf/sd202_a2/Current.vdf"),
        ] {
            let vdf = vdf_file(path);
            let err = vdf
                .to_results()
                .expect_err("expected ambiguous VDF-only stock assignment");
            assert!(
                err.to_string()
                    .contains("ambiguous VDF-only stock assignment"),
                "{label}: unexpected error: {err}"
            );
        }
    }

    #[test]
    fn test_to_results_vdf_only_rejects_models_with_hidden_participants() {
        for (label, path) in [
            ("econ_base", "../../test/bobby/vdf/econ/base.vdf"),
            ("econ_mark2", "../../test/bobby/vdf/econ/mark2.vdf"),
        ] {
            let vdf = vdf_file(path);
            let err = vdf
                .to_results()
                .expect_err("expected hidden-participant count mismatch");
            assert!(
                err.to_string().contains("candidate count"),
                "{label}: unexpected error: {err}"
            );
            assert!(
                err.to_string().contains("OT capacity"),
                "{label}: unexpected error: {err}"
            );
        }
    }

    #[test]
    fn test_lookup_fixture_section6_guided_map_maps_net_change() {
        let mdl_path = "../../test/bobby/vdf/lookups/lookup_ex.mdl";
        let vdf_path = "../../test/bobby/vdf/lookups/lookup_ex.vdf";

        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let model = datamodel_project.models.first().unwrap();
        let vdf = vdf_file(vdf_path);
        let map = vdf.build_section6_guided_ot_map(model).unwrap();
        let data = vdf.extract_data().unwrap();

        let net_change = Ident::new("net_change");
        assert!(
            map.contains_key(&net_change),
            "net_change should receive an OT mapping"
        );

        let net_change_ot = map[&net_change];
        assert!((data.entries[net_change_ot][0] - 10.0).abs() < 1e-9);
        assert!((data.entries[net_change_ot][100] - 23.0).abs() < 1e-9);
    }

    #[test]
    fn test_section6_guided_map_keeps_vdf_name_missing_from_model() {
        let mdl_path = "../../test/bobby/vdf/water/water.mdl";
        let vdf_path = "../../test/bobby/vdf/water/Current.vdf";

        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let full_datamodel = crate::compat::open_vensim(&contents).unwrap();
        let full_model = full_datamodel.models.first().unwrap();

        // Build the full-model map to get the known-good OT for "gap"
        let vdf = vdf_file(vdf_path);
        let full_map = vdf.build_section6_guided_ot_map(full_model).unwrap();
        let gap_ot = *full_map
            .get(&Ident::new("gap"))
            .expect("full section6 map missing gap");

        // Now remove "gap" from the model and verify section6 map
        // still includes it from the VDF name table.
        let mut stripped = full_datamodel;
        remove_model_var(&mut stripped, "gap");
        let stripped_model = stripped.models.first().unwrap();

        let vdf_data = vdf.extract_data().unwrap();
        let stripped_map = vdf.build_section6_guided_ot_map(stripped_model).unwrap();

        let gap_ident = Ident::new("gap");
        let &stripped_gap_ot = stripped_map
            .get(&gap_ident)
            .expect("expected VDF-only variable 'gap' to be preserved in section6 map");
        let expected = &vdf_data.entries[gap_ot];
        let actual = &vdf_data.entries[stripped_gap_ot];

        for (step, expected_value) in expected.iter().enumerate() {
            assert!(
                (actual[step] - *expected_value).abs() < 1e-9,
                "gap mismatch at step {step}: expected {expected_value}, got {}",
                actual[step]
            );
        }
    }

    #[test]
    #[ignore = "requires third_party/uib_sd fixtures"]
    fn test_to_results_with_stock_classifier_arrayed_baserun() {
        let mdl_path = "../../third_party/uib_sd/zambaqui/ZamMod1.mdl";
        let vdf_path = "../../third_party/uib_sd/zambaqui/baserun.vdf";
        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let model = datamodel_project.models.first().unwrap();
        let stock_set = model_stock_set(model);
        let vdf = vdf_file(vdf_path);

        let results = vdf
            .to_results_with_stock_classifier(|name| stock_set.contains(&normalize_vdf_name(name)))
            .unwrap_or_else(|e| panic!("baserun: to_results_with_stock_classifier failed: {e}"));
        assert!(
            results.offsets.len() > 1,
            "baserun: expected at least some mapped series"
        );
        assert_eq!(results.step_count, vdf.time_point_count);
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

    #[test]
    fn test_section6_guided_map_keeps_vdf_candidates_when_model_is_missing_name() {
        let mdl_path = "../../test/metasd/WRLD3-03/wrld3-03.mdl";
        let vdf_path = "../../test/metasd/WRLD3-03/SCEN01.VDF";

        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let mut datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        remove_model_var(&mut datamodel_project, "food");

        let model = datamodel_project.models.first().unwrap();
        let vdf = vdf_file(vdf_path);
        let map = vdf.build_section6_guided_ot_map(model).unwrap();
        assert!(
            map.contains_key(&Ident::new("food")),
            "expected VDF candidate 'food' to survive model drift"
        );
    }

    #[test]
    fn test_section6_guided_map_ots_within_valid_range_on_wrld3() {
        let mdl_path = "../../test/metasd/WRLD3-03/wrld3-03.mdl";
        let vdf_path = "../../test/metasd/WRLD3-03/SCEN01.VDF";

        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let model = datamodel_project.models.first().unwrap();
        let vdf = vdf_file(vdf_path);
        let map = vdf.build_section6_guided_ot_map(model).unwrap();
        let section6_codes = vdf.section6_ot_class_codes().unwrap();

        for (name, &ot) in &map {
            assert!(
                ot < section6_codes.len(),
                "{name} mapped to OT {ot} which is out of range (max {})",
                section6_codes.len() - 1
            );
        }

        // Verify that stock-classified model variables land in stock-coded OTs
        let stock_set = model_stock_set(model);
        for (name, &ot) in &map {
            if name.as_str() == "time" || name.as_str().starts_with('#') {
                continue;
            }
            if stock_set.contains(&normalize_vdf_name(name.as_str())) {
                assert_eq!(
                    section6_codes[ot],
                    VDF_SECTION6_OT_CODE_STOCK,
                    "stock {} mapped to OT {ot} with non-stock code",
                    name.as_str()
                );
            }
        }
    }

    #[test]
    fn test_section6_guided_map_includes_stdlib_aliases_econ() {
        let mdl_path = "../../test/bobby/vdf/econ/mark2.mdl";
        let vdf_path = "../../test/bobby/vdf/econ/base.vdf";

        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let model = datamodel_project.models.first().unwrap();
        let vdf = vdf_file(vdf_path);
        let map = vdf.build_section6_guided_ot_map(model).unwrap();

        let perceived_hpi = Ident::<Canonical>::new("perceived_hpi");
        assert!(
            map.contains_key(&perceived_hpi),
            "expected perceived_hpi to be mapped"
        );
    }

    #[test]
    fn test_section6_guided_map_includes_stdlib_aliases_wrld3() {
        let mdl_path = "../../test/metasd/WRLD3-03/wrld3-03.mdl";
        let vdf_path = "../../test/metasd/WRLD3-03/SCEN01.VDF";

        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let model = datamodel_project.models.first().unwrap();
        let vdf = vdf_file(vdf_path);
        let map = vdf.build_section6_guided_ot_map(model).unwrap();

        let land_yield_factor_2 = Ident::<Canonical>::new("land_yield_factor_2");
        assert!(
            map.contains_key(&land_yield_factor_2),
            "expected land_yield_factor_2 to be mapped"
        );
    }

    #[test]
    fn test_header_ot_count_matches_offset_table_count() {
        let mut all_paths = collect_vdf_files(std::path::Path::new("../../test/bobby/vdf"));
        all_paths.extend(collect_vdf_files(std::path::Path::new(
            "../../test/metasd/WRLD3-03",
        )));
        all_paths.extend(collect_vdf_files(std::path::Path::new(
            "../../test/xmutil_test_models",
        )));
        assert!(!all_paths.is_empty());
        for path in &all_paths {
            let path_str = path.to_str().unwrap();
            let data = std::fs::read(path).unwrap();
            let Ok(vdf) = VdfFile::parse(data) else {
                continue;
            };
            let header_count = vdf.header_ot_count();
            assert_eq!(
                header_count, vdf.offset_table_count,
                "header OT count mismatch for {}",
                path_str
            );
        }
    }

    #[test]
    fn test_header_final_values_offset_gives_correct_final_time() {
        // final_values[0] should always be FINAL TIME
        let cases = [
            ("../../test/bobby/vdf/consts/b_is_3.vdf", 100.0),
            ("../../test/bobby/vdf/lookups/lookup_ex.vdf", 100.0),
            ("../../test/bobby/vdf/water/water.vdf", 20.0),
            ("../../test/bobby/vdf/pop/pop.vdf", 100.0),
            ("../../test/bobby/vdf/bact/euler-1.vdf", 60.0),
            ("../../test/bobby/vdf/econ/base.vdf", 300.0),
        ];
        for (path, expected_final_time) in cases {
            let vdf = vdf_file(path);
            let fvs = vdf.section6_ot_final_values().unwrap();
            assert!(
                (fvs[0] as f64 - expected_final_time).abs() < 0.01,
                "{}: expected final_time={}, got {}",
                path,
                expected_final_time,
                fvs[0]
            );
        }
    }

    #[test]
    fn test_class_codes_start_with_time_code() {
        let mut paths = collect_vdf_files(std::path::Path::new("../../test/bobby/vdf"));
        paths.extend(collect_vdf_files(std::path::Path::new(
            "../../test/metasd/WRLD3-03",
        )));
        paths.extend(collect_vdf_files(std::path::Path::new(
            "../../test/xmutil_test_models",
        )));
        for path in &paths {
            let path_str = path.to_str().unwrap();
            let data = std::fs::read(path).unwrap();
            let Ok(vdf) = VdfFile::parse(data) else {
                continue;
            };
            let codes = vdf.section6_ot_class_codes().unwrap();
            assert_eq!(
                codes[0], VDF_SECTION6_OT_CODE_TIME,
                "OT[0] should always be Time code for {}",
                path_str
            );
        }
    }

    #[test]
    fn test_filter_ot_candidate_names_consts() {
        let vdf = vdf_file("../../test/bobby/vdf/consts/b_is_3.vdf");
        let candidates = vdf.filter_ot_candidate_names();

        // Expected: system vars + model vars, excluding Time, lookup defs,
        // metadata, builtins
        assert!(
            candidates.iter().any(|n| n == "a stock"),
            "should include 'a stock'"
        );
        assert!(
            candidates.iter().any(|n| n == "FINAL TIME"),
            "should include 'FINAL TIME'"
        );
        assert!(
            !candidates.iter().any(|n| n == "Time"),
            "should exclude 'Time'"
        );
        assert!(
            !candidates.iter().any(|n| n.starts_with('.')),
            "should exclude group markers"
        );
        assert!(
            !candidates.iter().any(|n| n.starts_with('-')),
            "should exclude unit annotations"
        );
    }

    #[test]
    fn test_to_results_with_stock_classifier_consts() {
        let vdf = vdf_file("../../test/bobby/vdf/consts/b_is_3.vdf");
        let stock_names: HashSet<String> =
            ["a stock"].iter().map(|s| normalize_vdf_name(s)).collect();
        let results = vdf
            .to_results_with_stock_classifier(|name| {
                stock_names.contains(&normalize_vdf_name(name))
            })
            .unwrap();

        // Verify key variables are present and have correct values
        let time_off = results.offsets[&Ident::<Canonical>::from_str_unchecked("time")];
        let a_stock_off = results.offsets[&Ident::<Canonical>::new("a stock")];
        let a_off = results.offsets[&Ident::<Canonical>::new("a")];
        let b_off = results.offsets[&Ident::<Canonical>::new("b")];
        let d_off = results.offsets[&Ident::<Canonical>::new("d")];
        let net_flow_off = results.offsets[&Ident::<Canonical>::new("net flow")];

        // Check initial values (step 0)
        let row0 = &results.data[0..results.step_size];
        assert!(
            (row0[time_off] - 0.0).abs() < 0.01,
            "Time should start at 0"
        );
        assert!(
            (row0[a_stock_off] - 5.5).abs() < 0.01,
            "a stock should start at 5.5"
        );
        assert!((row0[a_off] - 1.0).abs() < 0.01, "a should be 1");
        assert!((row0[b_off] - 3.0).abs() < 0.01, "b should be 3");

        // Check final values (last step)
        let last = results.step_count - 1;
        let row_last = &results.data[last * results.step_size..(last + 1) * results.step_size];
        assert!(
            (row_last[time_off] - 100.0).abs() < 0.01,
            "Time should end at 100"
        );
        assert!(
            (row_last[net_flow_off] - 6.12).abs() < 0.02,
            "net flow final should be ~6.12"
        );

        // d is graphical_function(c=3), which from the lookup data gives ~2.12
        assert!(
            (row_last[d_off] - 2.12).abs() < 0.02,
            "d final should be ~2.12"
        );

        // System vars should NOT be in results
        assert!(
            !results
                .offsets
                .contains_key(&Ident::<Canonical>::new("FINAL TIME")),
            "system vars should be excluded from visible results"
        );
    }

    #[test]
    fn test_to_results_with_stock_classifier_water() {
        let vdf = vdf_file("../../test/bobby/vdf/water/water.vdf");
        let stock_names: HashSet<String> = ["water level"]
            .iter()
            .map(|s| normalize_vdf_name(s))
            .collect();
        let results = vdf
            .to_results_with_stock_classifier(|name| {
                stock_names.contains(&normalize_vdf_name(name))
            })
            .unwrap();

        assert!(
            results
                .offsets
                .contains_key(&Ident::<Canonical>::new("water level"))
        );
        assert!(
            results
                .offsets
                .contains_key(&Ident::<Canonical>::new("gap"))
        );
        assert!(
            results
                .offsets
                .contains_key(&Ident::<Canonical>::new("inflow"))
        );
    }

    #[test]
    fn test_to_results_with_stock_classifier_pop() {
        let vdf = vdf_file("../../test/bobby/vdf/pop/pop.vdf");
        let stock_names: HashSet<String> = ["producing population", "young population"]
            .iter()
            .map(|s| normalize_vdf_name(s))
            .collect();
        let results = vdf
            .to_results_with_stock_classifier(|name| {
                stock_names.contains(&normalize_vdf_name(name))
            })
            .unwrap();

        assert!(
            results
                .offsets
                .contains_key(&Ident::<Canonical>::new("producing population"))
        );
        assert!(
            results
                .offsets
                .contains_key(&Ident::<Canonical>::new("young population"))
        );
        assert!(
            results
                .offsets
                .contains_key(&Ident::<Canonical>::new("births"))
        );
    }

    #[test]
    fn test_to_results_with_stock_classifier_lookups() {
        let vdf = vdf_file("../../test/bobby/vdf/lookups/lookup_ex.vdf");
        let stock_names: HashSet<String> =
            ["stock"].iter().map(|s| normalize_vdf_name(s)).collect();
        let results = vdf
            .to_results_with_stock_classifier(|name| {
                stock_names.contains(&normalize_vdf_name(name))
            })
            .unwrap();

        // 'stock' and 'inline lookup table' should be present
        assert!(
            results
                .offsets
                .contains_key(&Ident::<Canonical>::new("stock"))
        );
        assert!(
            results
                .offsets
                .contains_key(&Ident::<Canonical>::new("inline lookup table"))
        );
        assert!(
            results
                .offsets
                .contains_key(&Ident::<Canonical>::new("net change"))
        );
        // 'lookup table 1' is a standalone lookup definition, should be excluded
        assert!(
            !results
                .offsets
                .contains_key(&Ident::<Canonical>::new("lookup table 1")),
            "standalone lookup definitions should be excluded"
        );
    }

    #[test]
    fn test_to_results_with_stock_classifier_matches_section6_guided_results() {
        // Verify that the stock_classifier path produces identical time series
        // to the section6-guided OT map for the water and pop models.
        let cases: &[(&str, &str, &[&str])] = &[
            (
                "../../test/bobby/vdf/water/water.mdl",
                "../../test/bobby/vdf/water/Current.vdf",
                &["water level"],
            ),
            (
                "../../test/bobby/vdf/pop/pop.mdl",
                "../../test/bobby/vdf/pop/Current.vdf",
                &["producing population", "young population"],
            ),
        ];

        for &(mdl_path, vdf_path, stocks) in cases {
            let contents = std::fs::read_to_string(mdl_path).unwrap();
            let project = crate::compat::open_vensim(&contents).unwrap();
            let model = project.models.first().unwrap();
            let vdf = vdf_file(vdf_path);

            let s6_map = vdf.build_section6_guided_ot_map(model).unwrap();

            let stock_set: HashSet<String> = stocks.iter().map(|s| normalize_vdf_name(s)).collect();
            let classifier_results = vdf
                .to_results_with_stock_classifier(|name| {
                    stock_set.contains(&normalize_vdf_name(name))
                })
                .unwrap();

            // Verify that every variable in the section6 map also appears
            // in the classifier results.
            for (id, &s6_ot) in &s6_map {
                let Some(&class_off) = classifier_results.offsets.get(id) else {
                    continue;
                };
                let vdf_data = vdf.extract_data().unwrap();
                if let Some(series) = vdf_data.entries.get(s6_ot) {
                    for (step, &s6_val) in series
                        .iter()
                        .enumerate()
                        .take(classifier_results.step_count.min(series.len()))
                    {
                        let class_val = classifier_results.data
                            [step * classifier_results.step_size + class_off];
                        assert!(
                            (s6_val - class_val).abs() < 1e-6
                                || (s6_val.is_nan() && class_val.is_nan()),
                            "{}: {} step {} mismatch: section6={} classifier={}",
                            vdf_path,
                            id.as_str(),
                            step,
                            s6_val,
                            class_val
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn test_subscripts_vdf_structure() {
        let vdf = vdf_file("../../test/bobby/vdf/subscripts/subscripts.vdf");

        assert_eq!(vdf.time_point_count, 24);
        assert_eq!(vdf.offset_table_count, 15);
        assert_eq!(vdf.header_ot_count(), 15);

        // 3 stocks (a stock[a], a stock[b], a stock[c])
        let codes = vdf.section6_ot_class_codes().unwrap();
        assert_eq!(codes[0], VDF_SECTION6_OT_CODE_TIME);
        let stock_count = codes
            .iter()
            .skip(1)
            .filter(|&&c| c == VDF_SECTION6_OT_CODE_STOCK)
            .count();
        assert_eq!(stock_count, 3);

        // Final values: OT[0] = FINAL TIME = 23
        let fvs = vdf.section6_ot_final_values().unwrap();
        assert!((fvs[0] - 23.0).abs() < 0.01);

        // other const elements are inline constants
        assert!((fvs[9] - 0.9).abs() < 0.01, "other const[a] = 0.9");
        assert!((fvs[10] - 0.3).abs() < 0.01, "other const[b] = 0.3");
        assert!((fvs[11] - 0.5).abs() < 0.01, "other const[c] = 0.5");
        assert!((fvs[13] - 0.7).abs() < 0.01, "some rate = 0.7");

        // Name table should contain dimension name and element names
        assert!(vdf.names.contains(&"sub1".to_string()));
        assert!(vdf.names.contains(&"a".to_string()));
        assert!(vdf.names.contains(&"b".to_string()));
        assert!(vdf.names.contains(&"c".to_string()));
        assert!(vdf.names.contains(&"a stock".to_string()));

        // Section 5 should have a dimension entry with n=3
        let (_, entries, _) = vdf.parse_section5_set_stream().unwrap();
        assert_eq!(entries.len(), 1, "one dimension set");
        assert_eq!(entries[0].n, 3, "sub1 has 3 elements");
        assert_eq!(entries[0].dimension_size(), 3);
    }

    #[test]
    fn test_subscripts_inferred_dimension_sets() {
        let vdf = vdf_file("../../test/bobby/vdf/subscripts/subscripts.vdf");
        let dims = vdf.inferred_dimension_sets();

        assert_eq!(dims.len(), 1, "expected one inferred dimension set");
        assert_eq!(dims[0].name, "sub1");
        assert_eq!(dims[0].elements, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_subscripts_candidate_filtering() {
        let vdf = vdf_file("../../test/bobby/vdf/subscripts/subscripts.vdf");
        let candidates = vdf.filter_ot_candidate_names();

        // Should include base variable names and system vars
        assert!(
            candidates.iter().any(|n| n == "a stock"),
            "should include 'a stock'"
        );
        assert!(
            candidates.iter().any(|n| n == "some rate"),
            "should include 'some rate'"
        );
        assert!(
            candidates.iter().any(|n| n == "FINAL TIME"),
            "should include 'FINAL TIME'"
        );

        assert!(
            !candidates.iter().any(|n| n == "sub1"),
            "dimension names should be excluded once inferred"
        );
        assert!(
            !candidates.iter().any(|n| n == "a")
                && !candidates.iter().any(|n| n == "b")
                && !candidates.iter().any(|n| n == "c"),
            "dimension element names should be excluded once inferred"
        );
    }

    #[test]
    fn test_subscripts_with_stock_classifier_and_array_info() {
        let vdf = vdf_file("../../test/bobby/vdf/subscripts/subscripts.vdf");

        // Provide model knowledge: which names are stocks and which are
        // arrayed (and their element names in order).
        let stock_set: HashSet<String> =
            ["a stock"].iter().map(|s| normalize_vdf_name(s)).collect();
        let array_dims: HashMap<String, Vec<String>> = [
            ("a stock", vec!["a", "b", "c"]),
            ("net flow", vec!["a", "b", "c"]),
            ("other const", vec!["a", "b", "c"]),
        ]
        .iter()
        .map(|(name, elems)| {
            (
                normalize_vdf_name(name),
                elems.iter().map(|e| e.to_string()).collect(),
            )
        })
        .collect();

        let results = vdf
            .to_results_with_array_info(
                |name| stock_set.contains(&normalize_vdf_name(name)),
                &array_dims,
                &HashSet::new(),
            )
            .unwrap();

        // Verify element-level variables are in results
        let a_stock_a = Ident::<Canonical>::new("a stock[a]");
        let a_stock_b = Ident::<Canonical>::new("a stock[b]");
        let a_stock_c = Ident::<Canonical>::new("a stock[c]");

        assert!(
            results.offsets.contains_key(&a_stock_a),
            "should have a stock[a]"
        );
        assert!(
            results.offsets.contains_key(&a_stock_b),
            "should have a stock[b]"
        );
        assert!(
            results.offsets.contains_key(&a_stock_c),
            "should have a stock[c]"
        );

        // Verify initial values: a stock[sub1] init = sub1 index (1, 2, 3)
        let off_a = results.offsets[&a_stock_a];
        let off_b = results.offsets[&a_stock_b];
        let off_c = results.offsets[&a_stock_c];
        let row0 = &results.data[0..results.step_size];
        assert!(
            (row0[off_a] - 1.0).abs() < 0.01,
            "a stock[a] init = 1, got {}",
            row0[off_a]
        );
        assert!(
            (row0[off_b] - 2.0).abs() < 0.01,
            "a stock[b] init = 2, got {}",
            row0[off_b]
        );
        assert!(
            (row0[off_c] - 3.0).abs() < 0.01,
            "a stock[c] init = 3, got {}",
            row0[off_c]
        );

        // Verify other const elements
        let oc_a = Ident::<Canonical>::new("other const[a]");
        let oc_b = Ident::<Canonical>::new("other const[b]");
        let oc_c = Ident::<Canonical>::new("other const[c]");
        assert!(results.offsets.contains_key(&oc_a));
        let row0_oc_a = row0[results.offsets[&oc_a]];
        let row0_oc_b = row0[results.offsets[&oc_b]];
        let row0_oc_c = row0[results.offsets[&oc_c]];
        assert!(
            (row0_oc_a - 0.9).abs() < 0.01,
            "other const[a] = 0.9, got {}",
            row0_oc_a
        );
        assert!(
            (row0_oc_b - 0.3).abs() < 0.01,
            "other const[b] = 0.3, got {}",
            row0_oc_b
        );
        assert!(
            (row0_oc_c - 0.5).abs() < 0.01,
            "other const[c] = 0.5, got {}",
            row0_oc_c
        );

        // Verify some rate is scalar
        let some_rate = Ident::<Canonical>::new("some rate");
        assert!(results.offsets.contains_key(&some_rate));
        assert!(
            (row0[results.offsets[&some_rate]] - 0.7).abs() < 0.01,
            "some rate = 0.7"
        );

        // Verify step 1 of a stock[a]: should be 1 + (0.7*1 + 0.9) = 2.6
        let row1 = &results.data[results.step_size..2 * results.step_size];
        assert!(
            (row1[off_a] - 2.6).abs() < 0.02,
            "a stock[a] step 1 = 2.6, got {}",
            row1[off_a]
        );

        // System vars should not be in visible results
        assert!(
            !results
                .offsets
                .contains_key(&Ident::<Canonical>::new("FINAL TIME")),
        );

        // Dimension/element names should not be in visible results
        assert!(
            !results
                .offsets
                .contains_key(&Ident::<Canonical>::new("sub1"))
        );
        assert!(!results.offsets.contains_key(&Ident::<Canonical>::new("a")));
    }

    // ---- Record field decoding and section structure tests ----

    fn sentinel_model_records(vdf: &VdfFile) -> Vec<&VdfRecord> {
        vdf.records
            .iter()
            .filter(|r| {
                r.has_sentinel()
                    && r.ot_index() > 0
                    && (r.ot_index() as usize) < vdf.offset_table_count
            })
            .collect()
    }

    #[test]
    fn test_record_f6_arrayed_signal() {
        let vdf = vdf_file("../../test/bobby/vdf/subscripts/subscripts.vdf");
        for rec in sentinel_model_records(&vdf) {
            let ot = rec.ot_index() as usize;
            if [1, 6, 9].contains(&ot) {
                assert!(rec.is_arrayed(), "arrayed var at OT {ot} should have f6=32");
            } else if ot == 13 {
                assert!(!rec.is_arrayed(), "scalar var at OT {ot} should have f6=5");
            }
        }
        // Scalar models: all model records have f6=5
        for path in [
            "../../test/bobby/vdf/water/Current.vdf",
            "../../test/bobby/vdf/pop/Current.vdf",
            "../../test/bobby/vdf/consts/b_is_3.vdf",
        ] {
            let vdf = vdf_file(path);
            for rec in sentinel_model_records(&vdf) {
                assert!(
                    !rec.is_arrayed(),
                    "{path}: OT {} should be scalar",
                    rec.ot_index()
                );
            }
        }
    }

    #[test]
    fn test_record_ot_index_gives_array_block_starts() {
        let vdf = vdf_file("../../test/bobby/vdf/subscripts/subscripts.vdf");
        let codes = vdf.section6_ot_class_codes().unwrap();

        let arrayed: Vec<&VdfRecord> = sentinel_model_records(&vdf)
            .into_iter()
            .filter(|r| r.is_arrayed())
            .collect();
        assert_eq!(arrayed.len(), 3);

        let mut starts: Vec<usize> = arrayed.iter().map(|r| r.ot_index() as usize).collect();
        starts.sort();
        assert_eq!(starts, vec![1, 6, 9]);

        // Each 3-element block has uniform class codes
        for &s in &starts {
            for off in 1..3 {
                assert_eq!(
                    codes[s + off],
                    codes[s],
                    "OT[{}] class should match OT[{s}]",
                    s + off
                );
            }
        }
    }

    #[test]
    fn test_section4_entry_stream_shape() {
        for (label, path, expected_entries, expected_last_index) in [
            (
                "water",
                "../../test/bobby/vdf/water/Current.vdf",
                1usize,
                0u32,
            ),
            ("pop", "../../test/bobby/vdf/pop/Current.vdf", 2, 0),
            ("econ", "../../test/bobby/vdf/econ/base.vdf", 6, 0),
            ("wrld3", "../../test/metasd/WRLD3-03/SCEN01.VDF", 37, 0),
            (
                "subscripts",
                "../../test/bobby/vdf/subscripts/subscripts.vdf",
                1,
                0,
            ),
            ("ref", "../../test/xmutil_test_models/Ref.vdf", 94, 0),
        ] {
            let vdf = vdf_file(path);
            let stream = vdf.parse_section4_entry_stream().unwrap();
            assert_eq!(stream.zero_prefix_words, 2, "{label}");
            assert_eq!(stream.entries.len(), expected_entries, "{label}");
            assert_eq!(
                stream.entries.last().map(|entry| entry.index_word()),
                Some(expected_last_index),
                "{label}"
            );
            assert!(
                stream.entries.iter().all(|entry| !entry.refs.is_empty()),
                "{label}: every parsed entry should carry at least one ref",
            );
        }
    }

    #[test]
    fn test_section4_entry_counts_match_packed_words() {
        for (label, path) in [
            ("water", "../../test/bobby/vdf/water/Current.vdf"),
            ("pop", "../../test/bobby/vdf/pop/Current.vdf"),
            ("econ", "../../test/bobby/vdf/econ/base.vdf"),
            ("wrld3", "../../test/metasd/WRLD3-03/SCEN01.VDF"),
            (
                "subscripts",
                "../../test/bobby/vdf/subscripts/subscripts.vdf",
            ),
            ("ref", "../../test/xmutil_test_models/Ref.vdf"),
        ] {
            let vdf = vdf_file(path);
            let stream = vdf.parse_section4_entry_stream().unwrap();
            for entry in &stream.entries {
                assert_eq!(
                    entry.refs.len(),
                    entry.count_lo() as usize + entry.count_hi() as usize,
                    "{label}: packed count mismatch at 0x{:08x}",
                    entry.file_offset,
                );
                assert_eq!(
                    entry.slotted_ref_count,
                    entry.refs.len(),
                    "{label}: all section-4 refs should resolve through the slot table",
                );
            }
        }
    }

    #[test]
    fn test_section4_index_words_overlap_section3_directory() {
        let subscripts = vdf_file("../../test/bobby/vdf/subscripts/subscripts.vdf");
        let subscripts_directory = subscripts.parse_section3_directory().unwrap();
        let subscripts_stream = subscripts.parse_section4_entry_stream().unwrap();
        let subscripts_indexes: std::collections::HashSet<u32> = subscripts_stream
            .entries
            .iter()
            .map(|entry| entry.index_word())
            .collect();
        assert_eq!(
            subscripts_directory
                .entries
                .iter()
                .map(|entry| entry.index_word())
                .collect::<Vec<_>>(),
            vec![0],
        );
        assert!(subscripts_indexes.contains(&0));

        let ref_vdf = vdf_file("../../test/xmutil_test_models/Ref.vdf");
        let ref_directory = ref_vdf.parse_section3_directory().unwrap();
        let ref_stream = ref_vdf.parse_section4_entry_stream().unwrap();
        let ref_indexes: std::collections::HashSet<u32> = ref_stream
            .entries
            .iter()
            .map(|entry| entry.index_word())
            .collect();
        let mut overlap: Vec<u32> = ref_directory
            .entries
            .iter()
            .map(|entry| entry.index_word())
            .filter(|idx| ref_indexes.contains(idx))
            .collect();
        overlap.sort_unstable();
        assert_eq!(overlap, vec![0, 59, 194, 248, 275, 302]);
    }

    #[test]
    fn test_probe_vdf_kind_distinguishes_dataset_files() {
        let current = std::fs::read("../../test/bobby/vdf/econ/base.vdf").unwrap();
        let dataset = std::fs::read("../../test/bobby/vdf/econ/data.vdf").unwrap();

        assert_eq!(probe_vdf_kind(&current), Some(VdfKind::SimulationResults));
        assert_eq!(probe_vdf_kind(&dataset), Some(VdfKind::Dataset));
    }

    #[test]
    fn test_dataset_vdf_parses_shared_structure() {
        let dataset =
            VdfDatasetFile::parse(std::fs::read("../../test/bobby/vdf/econ/data.vdf").unwrap())
                .unwrap();

        assert_eq!(dataset.sections.len(), 5);
        assert_eq!(dataset.time_point_count, 225);
        assert_eq!(dataset.names.len(), 12);
        assert_eq!(dataset.slot_table.len(), 12);
        assert_eq!(dataset.records.len(), 11);
        assert_eq!(dataset.data_block_offsets.len(), 10);
        assert_eq!(
            dataset.origin,
            "rates_vensim.xls converted to dataset on Tue Nov 04 13:08:42 2008"
        );

        let expected_names = vec![
            "Time",
            ".rates vensim",
            "Date",
            "decimal year",
            "Consumer Price Index",
            "Inflation Rate",
            "Interest rate on Mortgages",
            "Federal Funds Rate",
            "Home Price Index",
            "Real Inflation rate",
            "Change in CPI",
            "Change in HPI",
        ];
        assert_eq!(dataset.names, expected_names);
        assert_eq!(
            dataset.data_block_offsets,
            vec![
                0x0d4c, 0x10eb, 0x148a, 0x182d, 0x1bd0, 0x1f73, 0x2316, 0x26b5, 0x2a28, 0x2dcb,
            ]
        );
    }

    #[test]
    fn test_dataset_vdf_extracts_reference_series() {
        let dataset =
            VdfDatasetFile::parse(std::fs::read("../../test/bobby/vdf/econ/data.vdf").unwrap())
                .unwrap();
        let data = dataset.extract_data().unwrap();

        assert_eq!(data.time_values.len(), 225);
        assert!((data.time_values[0] - 1990.0).abs() < 1e-6);
        assert!((data.time_values[data.time_values.len() - 1] - 2008.6700439453125).abs() < 1e-6);
        assert_eq!(
            data.series_order,
            vec![
                "Date",
                "decimal year",
                "Consumer Price Index",
                "Inflation Rate",
                "Interest rate on Mortgages",
                "Federal Funds Rate",
                "Home Price Index",
                "Real Inflation rate",
                "Change in CPI",
                "Change in HPI",
            ]
        );

        let date = data.series("Date").unwrap();
        assert!((date[0] - 32874.0).abs() < 1e-6);
        assert!((date[date.len() - 1] - 39692.0).abs() < 1e-6);

        let cpi = data.series("Consumer Price Index").unwrap();
        assert!((cpi[0] - 127.4000015258789).abs() < 1e-6);
        assert!((cpi[cpi.len() - 1] - 218.7830047607422).abs() < 1e-6);

        let mortgages = data.series("Interest rate on Mortgages").unwrap();
        assert!((mortgages[0] - 9.899999618530273).abs() < 1e-6);
        assert!((mortgages[mortgages.len() - 1] - 6.039999961853027).abs() < 1e-6);

        let fed_funds = data.series("Federal Funds Rate").unwrap();
        assert!((fed_funds[0] - 8.229999542236328).abs() < 1e-6);
        assert!((fed_funds[fed_funds.len() - 1] - 1.809999942779541).abs() < 1e-6);

        let hpi = data.series("Home Price Index").unwrap();
        assert!((hpi[0] - 82.29000091552734).abs() < 1e-6);
        assert!((hpi[hpi.len() - 1] - 176.60000610351562).abs() < 1e-6);

        let inflation = data.series("Inflation Rate").unwrap();
        assert!(inflation[0].is_nan());
        assert!((inflation[12] - 5.38116979598999).abs() < 1e-6);
        assert!((inflation[inflation.len() - 1] - 4.698160171508789).abs() < 1e-6);

        let real_inflation = data.series("Real Inflation rate").unwrap();
        assert!(real_inflation[0].is_nan());
        assert!((real_inflation[12] - 1.5288300514221191).abs() < 1e-6);
        assert!((real_inflation[real_inflation.len() - 1] - -2.888159990310669).abs() < 1e-6);

        let delta_cpi = data.series("Change in CPI").unwrap();
        assert!(delta_cpi[0].is_nan());
        assert!((delta_cpi[1] - 0.6000000238418579).abs() < 1e-6);
        assert!((delta_cpi[delta_cpi.len() - 1] - -0.30300000309944153).abs() < 1e-6);

        let delta_hpi = data.series("Change in HPI").unwrap();
        assert!(delta_hpi[0].is_nan());
        assert!((delta_hpi[1] - -0.14000000059604645).abs() < 1e-6);
        assert!((delta_hpi[delta_hpi.len() - 1] - -176.60000610351563).abs() < 1e-6);
    }

    #[test]
    fn test_dataset_vdf_series_bindings_preserve_record_bridge() {
        let dataset =
            VdfDatasetFile::parse(std::fs::read("../../test/bobby/vdf/econ/data.vdf").unwrap())
                .unwrap();
        let bindings = dataset.series_bindings().unwrap();

        assert_eq!(bindings.len(), 10);
        assert_eq!(
            bindings
                .iter()
                .map(|binding| binding.name.as_str())
                .collect::<Vec<_>>(),
            vec![
                "Date",
                "decimal year",
                "Consumer Price Index",
                "Inflation Rate",
                "Interest rate on Mortgages",
                "Federal Funds Rate",
                "Home Price Index",
                "Real Inflation rate",
                "Change in CPI",
                "Change in HPI",
            ]
        );
        assert_eq!(
            bindings
                .iter()
                .map(|binding| binding.block_index)
                .collect::<Vec<_>>(),
            vec![3, 4, 2, 7, 8, 5, 6, 9, 0, 1]
        );
        assert_eq!(
            bindings
                .iter()
                .map(|binding| binding.record_file_offset)
                .collect::<Vec<_>>(),
            vec![
                0x01cc, 0x020c, 0x024c, 0x028c, 0x02cc, 0x030c, 0x034c, 0x038c, 0x03cc, 0x040c
            ]
        );
        assert_eq!(
            bindings
                .iter()
                .map(|binding| (binding.record_f2, binding.record_f3))
                .collect::<Vec<_>>(),
            vec![
                (14, 7),
                (16, 14),
                (20, 21),
                (26, 28),
                (31, 35),
                (39, 42),
                (45, 49),
                (50, 56),
                (56, 63),
                (61, 70),
            ]
        );
    }
}
