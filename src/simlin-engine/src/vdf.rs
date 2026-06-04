// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Parser for Vensim VDF (binary data file) format.
//!
//! VDF is Vensim's proprietary binary format for simulation output. The format
//! is completely undocumented. See `docs/design/vdf.md` for the full format
//! specification and field-level analysis.
//!
//! This module handles:
//! - Parsing the file header, sections, records, slot table, name table,
//!   offset table, and sparse data blocks.
//! - Deterministic name-to-OT mapping via [`VdfFile::to_results_via_records`]:
//!   each section-1 record's `f[2]` is a direct key into the section-2 name
//!   table and its `f[11]` is the variable's OT-block start; graphical-function
//!   descriptor records (whose `f[11]` is instead a section-6 lookup-record
//!   index) are peeled off so the remaining owner spans form a clean OT
//!   partition.
//! - The two `#`-signature stdlib-call encodings via
//!   [`VdfFile::output_signatures`] and [`VdfFile::new_style_alias_signatures`].

use std::collections::{HashMap, HashSet};
use std::{error::Error, result::Result as StdResult};

use crate::{
    common::{Canonical, Ident},
    results::{Method, Results, Specs},
};

mod record_results;
mod section3;
mod section5_dims;
mod signatures;

use record_results::build_record_result_columns;
pub use section3::{VdfSection3Directory, VdfSection3DirectoryEntry};

/// Names of stdlib module internal variables that DO consume OT entries.
/// LV1/LV2/LV3/ST are stock-backed; DEL/DL/RT1/RT2 are non-stock. Used by
/// the section-5 dimension recovery to exclude these helper names from the
/// dimension-element catalog.
const STDLIB_PARTICIPANT_HELPERS: [&str; 8] =
    ["DEL", "LV1", "LV2", "LV3", "ST", "RT1", "RT2", "DL"];

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

/// Lexical test for lookup/graphical-function names: a model-free reader's
/// best-effort identification of names that label section-6 lookup-record
/// entries.
///
/// The format does not store an owner-vs-descriptor tag. The decoded forward
/// link is structural: a graphical-function descriptor record's `f[11]` is the
/// zero-based index into the section-6 lookup-record array, and that array is
/// in case-insensitive alphabetical order of the lookup-definition names. A
/// reader that has the model trivially identifies the descriptor records; a
/// model-free reader has to recognise lookup-def names from the name table --
/// this lexical test is the workable approximation. It is correct on every
/// fixture in the corpus except `Ref.vdf` (where descriptor names like
/// `RS N2O` are abbreviations that don't carry the keyword); the
/// `f[10]`-highest fallback in `identify_descriptor_records` covers that case.
pub(super) fn is_lookupish_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.contains(" lookup") || lower.contains(" table") || lower.contains("graphical function")
}

/// Normalize a VDF name for comparison: lowercase, strip spaces and underscores.
fn normalize_vdf_name(name: &str) -> String {
    name.replace([' ', '_'], "").to_lowercase()
}

/// Standard simulation-result VDF file magic bytes.
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

/// Section-6 OT class code for input/data-like blocks. Observed on
/// `risk.vdf`/`risk2.vdf` covering names like `federal funds rate` and
/// `inflation rate`; these slots hold real saved series with the 29-byte
/// bitmap-width data layout.
pub const VDF_SECTION6_OT_CODE_INPUT: u8 = 0x05;

/// Section-6 OT class code marking stock-backed OT entries in all validated
/// files.
///
/// The `level_vs_aux` regression fixtures pin this as the authoritative
/// OT-side stock/non-stock signal: when the same variable `x` is changed from
/// a level to a supplementary auxiliary, the saved series moves from a
/// stock-coded OT entry (`0x08`) to a dynamic non-stock entry (`0x11`) while
/// the nearby section-1 record classification fields stay unchanged.
pub const VDF_SECTION6_OT_CODE_STOCK: u8 = 0x08;

/// Section-6 OT class code marking dynamic (non-stock) OT entries -- the
/// usual classification for auxiliaries and data series stored as
/// per-step blocks.
pub const VDF_SECTION6_OT_CODE_DYNAMIC: u8 = 0x11;

/// Section-6 OT class code marking inline non-stock OT entries observed
/// only in `Ref.vdf`. Treated as a real owner code but with semantics still
/// being characterised; included so the class-code guard in
/// `decoded_record_spans` accepts them.
pub const VDF_SECTION6_OT_CODE_REF_INLINE_LOW: u8 = 0x16;

/// Section-6 OT class code marking constant non-stock OT entries (inline
/// f32 stored in section 6's final-values array rather than a per-step
/// block).
pub const VDF_SECTION6_OT_CODE_CONST: u8 = 0x17;

/// Section-6 OT class code marking inline non-stock OT entries observed
/// only in `Ref.vdf`, complementing `VDF_SECTION6_OT_CODE_REF_INLINE_LOW`.
pub const VDF_SECTION6_OT_CODE_REF_INLINE_HIGH: u8 = 0x18;

/// Whether `code` is one of the section-6 OT class codes that mark a real
/// owner OT slot (i.e. a slot a section-1 record can legitimately point at
/// via `f[11]`).
///
/// `0x0f` (Time) is excluded because it is owned by `OT[0]` only -- normal
/// owner records' `f[11]`s start at 1, and the descriptor-vs-owner
/// reconstruction in `decoded_record_spans` uses this set to reject records
/// whose `f[11]`-as-OT-start would land on a non-owner slot.
pub fn is_owner_ot_class_code(code: u8) -> bool {
    matches!(
        code,
        VDF_SECTION6_OT_CODE_INPUT
            | VDF_SECTION6_OT_CODE_STOCK
            | VDF_SECTION6_OT_CODE_DYNAMIC
            | VDF_SECTION6_OT_CODE_REF_INLINE_LOW
            | VDF_SECTION6_OT_CODE_CONST
            | VDF_SECTION6_OT_CODE_REF_INLINE_HIGH
    )
}

/// Record classification value (`field[1]`) that marks a view header record.
///
/// View header records sit at sketch-view boundaries in file order; the run of
/// variable records between two consecutive headers belongs to one view. Small
/// and medium fixtures have one header per dot-prefix view marker, but that 1:1
/// relationship is not universal -- edited files can retain orphan headers, and
/// `Ref.vdf`'s nested modules surface sub-group dot names without dedicated
/// header records. Exposed via [`VdfRecord::is_view_header`].
pub const VDF_RECORD_VIEW_HEADER_CLASS: u32 = 138;

/// Size of a VDF section header in bytes (magic + 5 u32 fields).
pub const SECTION_HEADER_SIZE: usize = 24;

/// Size of a VDF variable metadata record in bytes (16 u32 fields).
pub const RECORD_SIZE: usize = 64;

/// Size of the VDF file header in bytes.
pub const FILE_HEADER_SIZE: usize = 0x80;

/// Offset of the first 64-byte variable metadata record within section 1's
/// data (or section 0's data for dataset VDFs).
///
/// Sections store a 12-byte preamble followed by three 64-byte "header"
/// blocks (a string-pool pointer array and misc runtime state) before the
/// real record array begins. The rule "blocks 0..2 are header, blocks 3..
/// are records" is validated across every observed simulation and dataset
/// VDF fixture. Expressed explicitly as `12 + 3 * 64 = 204`.
pub const RECORD_REGION_START_OFFSET: usize = 12 + 3 * RECORD_SIZE;

/// Byte offset, within the header section's data, of the `u32` slot count
/// (`block1[7]`): word 7 of the *second* 64-byte header block -- past the
/// 12-byte preamble and the first 64-byte block. Vensim writes the
/// actively-saved slot count here, and it equals the slot-table entry count on
/// every observed run-file and dataset VDF.
pub const SLOT_COUNT_WORD_OFFSET: usize = 12 + RECORD_SIZE + 7 * 4;

/// Sentinel `u32` written immediately after the slot table, separating it from
/// the name-table section magic. Constant (`0x00430000`) across every observed
/// run-file and dataset VDF; used to cross-check the deterministic slot-table
/// decode.
pub const SLOT_TABLE_TERMINATOR: u32 = 0x0043_0000;

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

    /// field[11] under the owner interpretation: OT block start index.
    ///
    /// For arrayed variables, this points to the first of N consecutive
    /// OT entries (one per subscript element). For scalar variables, it
    /// points to the single OT entry. Lookup/graphical-function descriptor
    /// records can instead use the same word as a section-6 lookup-record
    /// index, so callers must validate the intended interpretation.
    /// Values can exceed the actual OT count; callers should check
    /// `ot_index < offset_table_count` before treating this as an OT.
    pub fn ot_index(&self) -> u32 {
        self.fields[11]
    }

    /// field[6]: shape selector for this variable's array layout.
    ///
    /// Known values:
    /// - 5: scalar (no array dimensions)
    /// - 32 (0x20): generic arrayed marker (first or only array shape)
    /// - other: section-3 shape key; some multi-shape directories use the
    ///   following physical entry as the actual shape payload
    /// - 0: ambiguous padding/dimension/descriptor marker, not a decoded
    ///   arrayed-owner shape by itself
    ///
    /// Use `is_arrayed()` to test scalar vs. arrayed, or `shape_code()`
    /// to retrieve the raw value for shape-template lookups.
    pub fn is_arrayed(&self) -> bool {
        self.fields[6] != 0 && self.fields[6] != 5
    }

    /// Raw field[6] value: the shape selector/key for section-3 shape lookup
    /// (or 5 for scalar, 32 for generic arrayed, 0 for ambiguous metadata).
    pub fn shape_code(&self) -> u32 {
        self.fields[6]
    }

    /// Whether this record has the common sentinel pair seen on many
    /// owner/system/descriptor records.
    ///
    /// This is not the final owner-vs-descriptor discriminator; large fixtures
    /// contain sentinel lookup descriptors and at least one non-sentinel stock
    /// owner.
    pub fn has_sentinel(&self) -> bool {
        self.fields[8] == VDF_SENTINEL && self.fields[9] == VDF_SENTINEL
    }

    /// Whether this record is a **view header** marker (`field[1] == 138`).
    ///
    /// View header records mark boundaries between Vensim sketch views in
    /// file order. The run of records between two consecutive view headers
    /// (or between the file start and the first view header, or between
    /// the last view header and the end of the record region) corresponds
    /// to one view's worth of variable records. See
    /// [`VDF_RECORD_VIEW_HEADER_CLASS`] for the validation evidence.
    pub fn is_view_header(&self) -> bool {
        self.fields[1] == VDF_RECORD_VIEW_HEADER_CLASS
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
/// In simple fixtures `n` can match a subscript-set cardinality, but edited
/// files show it is safer to treat it as a header count for the leading
/// payload refs before the trailing axis anchors. This structure preserves
/// `n` and `marker` explicitly rather than inferring only from `refs.len()`.
#[derive(Debug, Clone)]
pub struct VdfSection5SetEntry {
    /// Absolute file offset where this entry begins.
    pub file_offset: usize,
    /// Header count field (`n`): often cardinality-like, but not a decoded
    /// dimension cardinality in every fixture.
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

    /// Number of non-trailing refs stored in the entry payload.
    ///
    /// The trailing one or two refs are structural axis anchors, not element
    /// payload refs. In observed files this equals `n`, but deriving it from
    /// the stored refs keeps malformed entries bounded.
    pub fn payload_ref_count(&self) -> usize {
        let overhead = match self.marker {
            0 => 1,
            1 => 2,
            _ => 1,
        };
        self.set_size().saturating_sub(overhead)
    }

    /// Candidate dimension size implied by this set.
    ///
    /// For marker=0 entries, dimension_size = refs.len() - 1 (one trailing
    /// anchor ref is not a dimension element). For marker=1 entries,
    /// dimension_size = refs.len() - 2 (two axis-anchor refs are structural,
    /// not dimension elements).
    pub fn dimension_size(&self) -> usize {
        self.payload_ref_count()
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

/// A named dimension set recovered from VDF-local array metadata.
///
/// Recovered by [`VdfFile::recover_dimension_sets_via_sec5`]: section-5
/// entries pair 1:1 (in `f[8]`-ascending order) with the section-1 record
/// `f[8]` dimension-anchor groups, root dimensions list their elements
/// directly, and subrange dimensions project their elements from the parent
/// root's element list via the section-5 payload-subsequence rule. See
/// `docs/design/vdf.md`.
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
/// single zero word. These records describe lookup/graphical-function payloads;
/// word[10] carries an associated evaluated-output OT index. Small files can
/// pair them with lookup definition names by order, but `Ref.vdf` proves that
/// name-table correspondence is not universal. The semantic meaning of the
/// other 12 fields is not yet fully decoded.
#[derive(Debug, Clone, PartialEq)]
pub struct VdfSection6LookupRecord {
    /// Absolute file offset where this record begins.
    pub file_offset: usize,
    /// Raw words comprising the record.
    pub words: [u32; 13],
}

impl VdfSection6LookupRecord {
    /// OT index of the lookup's evaluated-output block (word[10]).
    ///
    /// This is the OT of a *consumer* of the lookup, not the lookup-def name's
    /// own record; it can be shared by several lookup records (see
    /// `docs/design/vdf.md`, "Lookup mapping records").
    pub fn ot_index(&self) -> usize {
        self.words[10] as usize
    }

    /// Width of the evaluated-output block (word[11]): the number of OT slots
    /// the consumer occupies starting at [`Self::ot_index`]. `1` for a scalar
    /// consumer; the element count for an arrayed one.
    pub fn output_width(&self) -> usize {
        self.words[11] as usize
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
        // Dataset VDFs shift the record/header area into section 0, so the
        // slot-table header (`field1` + `block1[7]`) is section 0 while the
        // name table is section 1.
        let (slot_table_offset, slot_table) =
            slot_table_from_header(&data, &sections[0], sections[1].file_offset);

        // Dataset VDFs share the "12-byte preamble + three header blocks,
        // then records" layout with simulation VDFs. The only difference is
        // that the record region sits in section 0 instead of section 1.
        let search_start = sections[0].data_offset() + RECORD_REGION_START_OFFSET;
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

        // The slot table's start pointer and entry count live in the
        // section-1 header (`field1` + `block1[7]`). Section 1 is identified
        // by position because its `field4` varies across VDF versions.
        let (slot_table_offset, slot_table) = match (sections.get(1), name_section_idx) {
            (Some(sec1), Some(ns_idx)) => {
                slot_table_from_header(&data, sec1, sections[ns_idx].file_offset)
            }
            _ => (0, Vec::new()),
        };

        // Find records. The record region lives at a fixed offset within
        // section 1's data: `RECORD_REGION_START_OFFSET` bytes past
        // `data_offset()` (12-byte preamble + three 64-byte header blocks).
        // Full 64-byte variable metadata records start there and continue
        // until just before the slot table. Some files leave a short residual
        // trailer before the slot table; `find_records` ignores any bytes that
        // do not make up a complete record. The slot table does not fence the
        // record region on the low side (that job belongs to the header
        // blocks), so deriving the search start from slot offsets would skip
        // over records in medium+ fixtures; use the fixed offset instead.
        let sec1_data_start = sections
            .get(1)
            .map(|s| s.data_offset())
            .unwrap_or(FILE_HEADER_SIZE);
        let search_start = sec1_data_start + RECORD_REGION_START_OFFSET;
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
    /// 4-byte prefix skip and `stop_offset` is the absolute file offset
    /// where stream parsing stopped.
    ///
    /// The skip is derived deterministically from the section-6 header:
    /// `skip_words = max(0, sec6.field4 - 1)`. `field4 == 1` (the common
    /// case) encodes no prefix; `field4 == 2` encodes a single
    /// section-1-descriptor-offset-shaped prefix word before the ref stream.
    /// The prefix word's semantic binding is not decoded and it is not always
    /// a slot-table entry.
    pub fn parse_section6_ref_stream(&self) -> Option<(usize, Vec<VdfRefListEntry>, usize)> {
        let sec = self.sections.get(6)?;
        let skip = (sec.field4 as usize).saturating_sub(1);
        let (entries, stop) = self.section_ref_stream_with_skip(6, skip, 512);
        Some((skip, entries, stop))
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
    /// `13 * u32` each, terminated by a single zero word. Word[10] carries
    /// an associated evaluated-output OT index. Some fixtures have a simple
    /// 1:1 lookup-name order, but large files can have more lookup records
    /// than obvious lookupish names and descriptor OTs can overlap ordinary
    /// owner spans.
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
    /// Returns `(0, entries, stop_offset)`. Earlier parser revisions scanned
    /// for a best skip, but the model-edit fixtures prove the stream begins
    /// immediately at section 5's data offset.
    pub fn parse_section5_set_stream(&self) -> Option<(usize, Vec<VdfSection5SetEntry>, usize)> {
        if self.sections.len() <= 5 {
            return None;
        }
        let (entries, stop) = self.section5_set_stream_with_skip(0, 4096);
        Some((0, entries, stop))
    }

    /// Decode the section-5 region-end pointer from header field1.
    ///
    /// Field1 is a 1-based word index from section 5's magic word to the
    /// final word before the next section header. This is a framing checksum:
    /// `sec5.file_offset + 4 * (field1 - 1)` should equal
    /// `sec5.region_end - 4` on observed simulation-result VDFs.
    pub fn section5_region_last_word_from_field1(&self) -> Option<usize> {
        let sec = self.sections.get(5)?;
        let field1 = sec.field1.checked_sub(1)? as usize;
        Some(sec.file_offset + field1 * 4)
    }

    /// Recover dim names + elements via the section-5 payload-subsequence rule (see [`section5_dims`]).
    pub fn recover_dimension_sets_via_sec5(&self) -> Vec<VdfDimensionSet> {
        section5_dims::recover_dimension_sets_via_sec5(self)
    }

    /// Map section-3 axis refs to decoded dimension names.
    ///
    /// Section-3 axis words are section-1 word pointers to `field[9]` of a
    /// dimension-anchor record. This exposes that decoded bridge without
    /// leaking the rest of the anchor catalog.
    pub fn section3_axis_ref_dimension_names(&self) -> HashMap<u32, String> {
        section5_dims::section3_axis_ref_to_dimension_name(self)
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

    /// Map record `field[2]` name keys to section-2 name-table indices.
    ///
    /// The key is the 4-byte word offset of the printable name within the
    /// section-2 data region, plus seven words. The first name (`Time`) has no
    /// length prefix; every following name's key points at the first character
    /// after its u16 length prefix.
    fn record_name_key_to_name_index(&self) -> HashMap<u32, usize> {
        let Some(name_section_idx) = self.name_section_idx else {
            return HashMap::new();
        };
        let Some(section) = self.sections.get(name_section_idx) else {
            return HashMap::new();
        };
        if self.names.is_empty() {
            return HashMap::new();
        }

        let data_start = section.data_offset();
        let parse_end = section.region_end.min(self.data.len());
        let first_len = (section.field5 >> 16) as usize;
        if first_len == 0 || data_start + first_len > self.data.len() {
            return HashMap::new();
        }

        let mut out = HashMap::new();
        out.insert(7, 0);

        let mut pos = data_start + first_len;
        let mut name_idx = 1usize;
        while name_idx < self.names.len() && pos + 2 <= parse_end {
            let len = read_u16(&self.data, pos) as usize;
            pos += 2;
            if len == 0 {
                continue;
            }
            if pos + len > parse_end || len > 256 {
                break;
            }

            let start_rel = pos - data_start;
            if start_rel.is_multiple_of(4) {
                out.insert((start_rel / 4 + 7) as u32, name_idx);
            }

            pos += len;
            name_idx += 1;
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

    /// Build a `Results` struct using only VDF structural data, driven by
    /// the deterministic record-to-name correspondence and the decoded
    /// forward link for graphical-function descriptors.
    ///
    /// ### Pipeline
    ///
    /// 1. `decoded_record_spans` produces one `DecodedRecordSpan` per
    ///    section-1 record whose `f[2]` resolves through the section-2
    ///    name-key formula, whose `f[11]` is interpretable as an OT-block
    ///    start, whose `f[6]` shape code yields a structural flat span,
    ///    and whose covered OT slots all carry a real-data class code
    ///    (`is_owner_ot_class_code`).
    /// 2. `identify_descriptor_records` peels off graphical-function
    ///    descriptor records via the decoded forward link: a descriptor's
    ///    `f[11]` is a zero-based index into the section-6 lookup-record
    ///    array (case-insensitive alphabetical order of the lookup-def
    ///    names). The lexical lookup-def name test handles every overlap
    ///    in the corpus except `Ref.vdf`, where descriptor names are
    ///    domain abbreviations; an `f[10]`-highest fallback covers that.
    /// 3. The remaining owner spans + `Time` at OT[0] are emitted as
    ///    `Results`. When two records bind the same name to overlapping
    ///    starts (rare in practice), the lowest-start span wins.
    ///
    /// ### Stability of the direct mapping
    ///
    /// Record `f[2]` is a direct key into the section-2 name table:
    /// `(name_string_start - section2_data_start) / 4 + 7`, where
    /// `name_string_start` points at the first printable byte after any
    /// u16 length prefix. This is stable on edited and compilation-order
    /// files because it is an address-like string-pool word offset, not a
    /// sort rank.
    ///
    /// ### Filtering rules
    ///
    /// - Records with `f[11] == 0` or `f[11] >= offset_table_count` are
    ///   skipped -- OT[0] is Time, not a record slot.
    /// - Records whose `f[6] == 0` are treated as non-shape/padding and
    ///   skipped (per `decoded_record_shape_length`).
    /// - Name-table entries that are empty or whose index is past the end
    ///   of the name table are skipped (no name to assign).
    /// - The class-code guard rejects records whose `f[11]`-as-OT-start
    ///   would land on a non-owner section-6 OT slot. This catches
    ///   descriptor records whose `f[11]`-as-lookup-index numerically
    ///   coincides with an OT slot that does not hold real data.
    /// - Owner/descriptor overlap is resolved by descriptor identification,
    ///   not by an overlap-selection DP.
    ///
    /// Name category is not filtered here: if a record legitimately points
    /// at an owner OT slot, its keyed name is honored even for stdlib
    /// helper, internal signature, metadata, or builtin-looking names.
    /// Callers that want cleaner symbols can filter results-side.
    ///
    /// The method always returns `Results` (possibly an empty one beyond
    /// Time) so callers can chain with other paths.
    pub fn to_results_via_records(&self) -> StdResult<Results, Box<dyn Error>> {
        let n_recs = self.records.len();
        if n_recs == 0 || self.names.is_empty() {
            return Err(format!(
                "record-based mapping requires non-empty records and names: records={n_recs}, names={}",
                self.names.len()
            )
            .into());
        }

        let name_key_to_name_index = self.record_name_key_to_name_index();
        let vdf_data = self.extract_data()?;

        // Look up section-3 shape directory for arrayed spans. Scalar
        // models return an empty directory (section 3 is all zeros), which
        // is fine because scalar records have field[6] == 5 and never hit
        // the directory.
        let section3_directory = self.parse_section3_directory();
        let dimension_elements_by_name: HashMap<String, Vec<String>> = self
            .recover_dimension_sets_via_sec5()
            .into_iter()
            .map(|dim| (dim.name.to_lowercase(), dim.elements))
            .collect();
        let axis_ref_to_dim_name = self.section3_axis_ref_dimension_names();

        let ordered = build_record_result_columns(
            self,
            &name_key_to_name_index,
            section3_directory.as_ref(),
            &dimension_elements_by_name,
            &axis_ref_to_dim_name,
        );

        Ok(vdf_data.build_results(&ordered))
    }

    /// Return new-style stdlib-call signature triples in name-table file
    /// order. New-style signatures have the form `#alias>FUNC#` where
    /// the alias is embedded in the signature prefix before `>`,
    /// allowing pure string-split decoding with no model file. Returns
    /// `(name_idx, alias, function_family)` tuples.
    pub fn new_style_alias_signatures(&self) -> Vec<(usize, String, String)> {
        signatures::new_style_alias_signatures(&self.names)
    }

    /// Return all output-type `#` signature names in name-table file
    /// order. An output sig is a canonical `#...#` signature representing
    /// a stdlib call's output (not internal helper stocks like
    /// `#LV1<DELAY1(...)#`). The classifier requires either `(` for
    /// old-style `#FUNC(args)#` or exactly one top-level `>` for new-style
    /// `#alias>FUNC#`, and rejects sub-part names (`>linear#`, `>rate#`)
    /// and display names lacking both markers.
    pub fn output_signatures(&self) -> Vec<(usize, String)> {
        signatures::output_signatures(&self.names)
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

        // Edited Vensim files leave stale/deleted entries: a valid u16 length
        // prefix over non-printable binary payload. Vensim's reader skips them
        // by the declared length and keeps going -- the table does not end at
        // the first such entry. Only the out-of-bounds and implausible-length
        // guards above stop the table. (See docs/design/vdf.md, section 2.)
        if !s.is_empty() && s.chars().all(|c| c.is_ascii_graphic() || c == ' ') {
            names.push(s);
        }
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

/// Decode the slot table deterministically from the section header.
///
/// The slot table is an array of `u32` byte-offsets into the header section's
/// data, one per slotted name. Vensim records its location exactly, so no
/// backward scan or stride heuristic is needed:
///
/// - **start** is the 1-based word pointer in the header section's `field1`
///   (`header_section.file_offset + 4 * (field1 - 1)`) -- the same field-1
///   convention sections 6 and 7 use for their payload pointers;
/// - **count** is `block1[7]` (`SLOT_COUNT_WORD_OFFSET` into the section's
///   data), the actively-written slot count;
/// - the table is followed by a single [`SLOT_TABLE_TERMINATOR`] word and then
///   the name-table section magic.
///
/// `header_section` is section 1 for simulation-run files and section 0 for
/// dataset files (whose record/header area shifts one section left).
/// `name_table_offset` is the table's upper boundary -- the name-table
/// section's file offset.
///
/// These three facts over-determine each other and were verified to agree on
/// every run-file and dataset VDF in the corpus. The decode therefore
/// cross-checks the layout (`start + (count + 1) * 4 == name_table_offset`,
/// terminator word present) and returns `(0, empty)` if it does not hold,
/// rather than emitting a mis-decoded table.
pub fn slot_table_from_header(
    data: &[u8],
    header_section: &Section,
    name_table_offset: usize,
) -> (usize, Vec<u32>) {
    if header_section.field1 == 0 {
        return (0, Vec::new());
    }
    let slot_start = header_section.file_offset + 4 * (header_section.field1 as usize - 1);
    let count_word_offset = header_section.data_offset() + SLOT_COUNT_WORD_OFFSET;
    if count_word_offset + 4 > data.len() {
        return (0, Vec::new());
    }
    let slot_count = read_u32(data, count_word_offset) as usize;

    // Cross-check the over-determined layout: `slot_count` slot words, then one
    // terminator word, then the name table. Bail out (no slots) if the file
    // does not match, rather than emit a mis-decoded table.
    let terminator_offset = slot_start + slot_count * 4;
    if slot_start >= name_table_offset
        || terminator_offset + 4 != name_table_offset
        || terminator_offset + 4 > data.len()
        || read_u32(data, terminator_offset) != SLOT_TABLE_TERMINATOR
    {
        return (0, Vec::new());
    }

    let slot_table = (0..slot_count)
        .map(|i| read_u32(data, slot_start + i * 4))
        .collect();
    (slot_start, slot_table)
}

/// Find 64-byte variable records between `search_start` (inclusive) and
/// `search_end` (exclusive).
///
/// Callers pass `search_start = sec_data_offset + RECORD_REGION_START_OFFSET`;
/// the observed layout stores full records in 64-byte strides from there
/// until just before the slot table. Some files leave a short non-record
/// trailer before the slot table; the stride walk ignores any residual bytes
/// shorter than a full record. Some records carry the sentinel pair (two
/// consecutive `0xf6800000` values at field offsets 8 and 9), while others
/// (padding records, lookup table metadata, subscript elements) do not.
///
/// As a defensive cross-check, the function still anchors its forward walk
/// to the first sentinel pair it finds, then scans backward through blocks
/// that look recordish (`f[0] <= 64` or either sentinel half set) up to --
/// but never past -- `search_start`. With a correct `search_start` this
/// backward scan is a no-op on well-formed files; on malformed inputs it
/// prevents emitting garbage aligned against random section prefix bytes.
pub fn find_records(data: &[u8], search_start: usize, search_end: usize) -> Vec<VdfRecord> {
    if search_start >= search_end {
        return Vec::new();
    }

    // Find first sentinel pair to anchor the forward walk. If the region
    // contains no sentinels at all (e.g., a corrupt or truncated file), we
    // have no trusted alignment and return no records rather than emitting
    // random 64-byte slices.
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

    // Scan backwards through blocks that look recordish, but never past
    // `search_start`. With the fixed record-region offset this normally
    // drops us straight back to `search_start`; on malformed files it stops
    // before the header blocks.
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

    #[test]
    fn test_parse_name_table_extended_skips_stale_nonprintable_entries() {
        // Edited Vensim files leave stale/deleted name-table entries: a valid
        // u16 length prefix followed by non-printable binary payload (often a
        // 4-byte value plus a fragment of the deleted name). Vensim's reader
        // skips them by the declared byte count and keeps going; the table does
        // not end there. (Real example from `risk2.vdf`: len=10, payload
        // `91 01 00 00 63 79 20 30 00 00`.) Verify the parser skips rather than
        // stops, so the names after the stale entry are still recovered.
        let mut data = vec![0u8; 256];
        data[0..4].copy_from_slice(&VDF_SECTION_MAGIC);
        data[20..24].copy_from_slice(&(6u32 << 16).to_le_bytes());

        // First name at 24: "Time\0\0" (len 6, no u16 prefix).
        data[24..28].copy_from_slice(b"Time");

        // Name 2 at 30: u16=6, "abc".
        data[30..32].copy_from_slice(&6u16.to_le_bytes());
        data[32..35].copy_from_slice(b"abc");

        // STALE entry at 38: u16=10, non-printable payload (in-bounds, len<=256).
        data[38..40].copy_from_slice(&10u16.to_le_bytes());
        data[40..50].copy_from_slice(&[0x91, 0x01, 0x00, 0x00, b'c', b'y', b' ', b'0', 0, 0]);

        // Name 3 at 50: u16=6, "def".
        data[50..52].copy_from_slice(&6u16.to_le_bytes());
        data[52..55].copy_from_slice(b"def");

        let section = Section {
            file_offset: 0,
            field1: 0,
            region_end: 80,
            field3: 500,
            field4: 0,
            field5: 6u32 << 16,
        };

        let names = parse_name_table_extended(&data, &section, 80);
        assert_eq!(names, vec!["Time", "abc", "def"]);
    }

    #[test]
    fn test_slot_table_deterministic_matches_header() {
        // The slot table is decoded deterministically from the section-1
        // header: it starts at the `field1` 1-based word pointer, has
        // `block1[7]` entries, and is followed by a single `0x00430000`
        // terminator word before the name table. The previous heuristic
        // scanner under-counted on edited files (name-parser cap) and
        // over-counted by one elsewhere (a spurious leading word); the
        // deterministic decode matches the header exactly. See the
        // corpus-wide invariant in tests/integration/vdf_structural_invariants.rs.
        for path in [
            "../../test/bobby/vdf/econ/risk2.vdf",
            "../../test/metasd/WRLD3-03/SCEN01.VDF",
            "../../test/metasd/social-network-valuation/optimistic.vdf",
            "../../test/bobby/vdf/econ/risk.vdf",
            "../../test/bobby/vdf/water/Current.vdf",
        ] {
            let vdf = vdf_file(path);
            let sec1 = &vdf.sections[1];
            // block1[7]: 12-byte preamble + 64-byte block0 + 7 words into block1.
            let block1_word7 = read_u32(&vdf.data, sec1.data_offset() + 12 + RECORD_SIZE + 28);
            let field1_start = sec1.file_offset + 4 * (sec1.field1 as usize - 1);
            assert_eq!(
                vdf.slot_table.len() as u32,
                block1_word7,
                "{path}: slot count must equal block1[7]"
            );
            assert_eq!(
                vdf.slot_table_offset, field1_start,
                "{path}: slot table must start at the field1 word pointer"
            );
            // The word immediately after the slot table is the terminator.
            let terminator = read_u32(&vdf.data, field1_start + vdf.slot_table.len() * 4);
            assert_eq!(terminator, 0x0043_0000, "{path}: terminator word");
        }
    }

    fn vdf_file(path: &str) -> VdfFile {
        let data = std::fs::read(path)
            .unwrap_or_else(|e| panic!("failed to read VDF file {}: {}", path, e));
        VdfFile::parse(data).unwrap_or_else(|e| panic!("failed to parse VDF file {}: {}", path, e))
    }

    fn assert_result_column_matches_ot(
        label: &str,
        results: &crate::Results,
        vdf_data: &VdfData,
        name: &str,
        expected_ot: usize,
    ) {
        let ident = Ident::<Canonical>::new(name);
        let col = *results
            .offsets
            .get(&ident)
            .unwrap_or_else(|| panic!("{label}: missing result column {name}"));
        let expected = vdf_data
            .entries
            .get(expected_ot)
            .unwrap_or_else(|| panic!("{label}: missing OT[{expected_ot}]"));
        assert!(
            expected.len() >= results.step_count,
            "{label}: OT[{expected_ot}] has {} values but Results has {} steps",
            expected.len(),
            results.step_count
        );

        for (step, &expected_value) in expected.iter().take(results.step_count).enumerate() {
            let actual = results.data[step * results.step_size + col];
            assert!(
                (actual - expected_value).abs() <= 1e-6,
                "{label}: {name} step {step} mapped to wrong series: \
                 got {actual}, expected OT[{expected_ot}] value {expected_value}"
            );
        }
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
    fn test_record_ot_ranges_partition_selected_files() {
        let water = vdf_file("../../test/bobby/vdf/water/Current.vdf");
        let econ = vdf_file("../../test/bobby/vdf/econ/base.vdf");
        let wrld3 = vdf_file("../../test/metasd/WRLD3-03/SCEN01.VDF");

        // After the record-region start was fixed to skip only the three
        // header blocks (rather than being derived from slot-table offsets),
        // records cover every non-Time OT slot in these fixtures, so the
        // range count is exactly `ot_count - 1` for each.
        for (label, vdf, expected_ranges) in [
            ("water", &water, 9usize),
            ("econ", &econ, 77usize),
            ("wrld3", &wrld3, 296usize),
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

    #[test]
    fn test_to_results_via_records_scalar_stock_fixture() {
        // level_vs_aux/x_is_stock.vdf: a one-variable scalar fixture with
        // `x` as a stock. The direct f[2] string-key mapping should recover
        // the visible variable, not just Time/system records.
        let vdf = vdf_file("../../test/bobby/vdf/level_vs_aux/x_is_stock.vdf");
        let results = vdf
            .to_results_via_records()
            .expect("record-based mapping should succeed on scalar stock fixture");
        let vdf_data = vdf.extract_data().unwrap();

        assert!(
            results
                .offsets
                .contains_key(&Ident::<Canonical>::new("time")),
            "Time must always be present"
        );
        assert_eq!(results.step_count, vdf.time_point_count);
        for col in 0..results.step_size {
            let v = results.data[col];
            assert!(v.is_finite(), "column {col} must be finite: {v}");
        }
        assert_result_column_matches_ot("x_is_stock", &results, &vdf_data, "x", 1);
    }

    #[test]
    fn test_to_results_via_records_scalar_aux_fixture() {
        // level_vs_aux/x_is_aux.vdf: same shape as the stock variant but
        // `x` is a non-stock auxiliary. This fixture has too few records for
        // the old rank-offset approximation; the direct f[2] key recovers it.
        let vdf = vdf_file("../../test/bobby/vdf/level_vs_aux/x_is_aux.vdf");
        let results = vdf
            .to_results_via_records()
            .expect("record-based mapping should succeed on scalar aux fixture");
        let vdf_data = vdf.extract_data().unwrap();

        assert!(
            results
                .offsets
                .contains_key(&Ident::<Canonical>::new("time"))
        );
        assert_eq!(results.step_count, vdf.time_point_count);
        assert_result_column_matches_ot("x_is_aux", &results, &vdf_data, "x", 5);
    }

    #[test]
    fn test_record_name_key_maps_to_section2_string_start() {
        let run8 = vdf_file("../../test/bobby/vdf/model_editing/run_8.vdf");
        let run8_keys = run8.record_name_key_to_name_index();

        assert_eq!(run8.names[*run8_keys.get(&49).unwrap()], "v");
        assert_eq!(run8.names[*run8_keys.get(&51).unwrap()], "constant");
        assert_eq!(run8.names[*run8_keys.get(&54).unwrap()], "stock");
        assert_eq!(run8.names[*run8_keys.get(&57).unwrap()], "flow");

        let lookup = vdf_file("../../test/bobby/vdf/lookups/lookup_ex.vdf");
        let lookup_keys = lookup.record_name_key_to_name_index();

        assert_eq!(
            lookup.names[*lookup_keys.get(&32).unwrap()],
            "lookup table 1"
        );
        assert_eq!(
            lookup.names[*lookup_keys.get(&37).unwrap()],
            "inline lookup table"
        );
        assert_eq!(lookup.names[*lookup_keys.get(&43).unwrap()], "stock");
        assert_eq!(lookup.names[*lookup_keys.get(&46).unwrap()], "net change");
    }

    #[test]
    fn test_to_results_via_records_edited_run_5_recovers_visible_variables() {
        // model_editing/run_5.vdf: an incrementally edited fixture where
        // the original conservative `to_results` returns an ambiguity
        // error. The record-based path instead reads field[11] directly
        // for each proper record and lands visible model variables on
        // their actual OT slots.
        let vdf = vdf_file("../../test/bobby/vdf/model_editing/run_5.vdf");
        let results = vdf
            .to_results_via_records()
            .expect("record-based mapping should resolve model_editing/run_5");

        for name in ["stock", "flow", "constant", "v"] {
            let ident = Ident::<Canonical>::new(name);
            assert!(
                results.offsets.contains_key(&ident),
                "{name} should be mapped by record-based path"
            );
        }

        let stock_col = results.offsets[&Ident::<Canonical>::new("stock")];
        let flow_col = results.offsets[&Ident::<Canonical>::new("flow")];
        let constant_col = results.offsets[&Ident::<Canonical>::new("constant")];
        let row0 = &results.data[0..results.step_size];
        let last = results.step_count - 1;
        let row_last = &results.data[last * results.step_size..(last + 1) * results.step_size];

        // stock starts at 2 and grows; flow starts at 0 and increases;
        // constant is always ~pi. These are the fingerprints that were
        // inspected manually before committing the test.
        assert!(
            (row0[stock_col] - 2.0).abs() < 0.01,
            "stock t0 = {}",
            row0[stock_col]
        );
        assert!(
            row_last[stock_col] > row0[stock_col] + 100.0,
            "stock grows over time: {} -> {}",
            row0[stock_col],
            row_last[stock_col]
        );
        assert!(
            (row0[flow_col] - 0.0).abs() < 0.01,
            "flow t0 = 0, got {}",
            row0[flow_col]
        );
        assert!(
            row_last[flow_col] > 10.0,
            "flow grows, t_last = {}",
            row_last[flow_col]
        );
        assert!(
            (row0[constant_col] - std::f64::consts::PI).abs() < 0.01,
            "constant = pi, got {}",
            row0[constant_col]
        );
    }

    #[test]
    fn test_to_results_via_records_fixture_with_hidden_stdlib_helpers() {
        // model_editing/run_10.vdf: edited fixture containing a SMOOTH
        // invocation (expanded into hidden stock-backed helpers). The
        // direct f[2] key should still succeed and produce a usable `Results`.
        let vdf = vdf_file("../../test/bobby/vdf/model_editing/run_10.vdf");
        let results = vdf
            .to_results_via_records()
            .expect("record-based mapping should resolve model_editing/run_10");

        assert_eq!(results.step_count, vdf.time_point_count);
        assert!(
            !results.offsets.is_empty(),
            "record-based mapping should at minimum emit Time"
        );
        assert!(
            results
                .offsets
                .contains_key(&Ident::<Canonical>::new("time")),
            "Time must always be present"
        );
    }

    #[test]
    fn test_identify_descriptor_records_uses_f10_fallback_on_ref_vdf() {
        // Ref.vdf is the canonical case where the lexical lookup-def name
        // test cannot disambiguate descriptor records: the descriptor names
        // are domain abbreviations (e.g. `RS N2O`) that don't carry the
        // "lookup"/"table"/"graphical function" keywords. The
        // `f[10]`-highest fallback resolves the conflict and the
        // identification result must surface that decision via
        // `used_f10_fallback`.
        //
        // On every other corpus fixture the lexical test is sufficient and
        // the fallback flag stays `false`, which is checked by the small
        // models below as a regression guard.
        use super::record_results::{decoded_record_spans, identify_descriptor_records};

        let ref_vdf = vdf_file("../../test/xmutil_test_models/Ref.vdf");
        let key_map = ref_vdf.record_name_key_to_name_index();
        let dir = ref_vdf.parse_section3_directory();
        let spans = decoded_record_spans(&ref_vdf, &key_map, dir.as_ref());
        let id = identify_descriptor_records(&ref_vdf, &spans);
        assert!(
            id.used_f10_fallback,
            "Ref.vdf descriptor identification must hit the f[10] fallback"
        );
        assert!(
            !id.descriptor_indices.is_empty(),
            "Ref.vdf must have at least one descriptor record"
        );

        for label in ["water", "pop"] {
            let path = format!("../../test/bobby/vdf/{label}/Current.vdf");
            let small = vdf_file(&path);
            let key_map = small.record_name_key_to_name_index();
            let dir = small.parse_section3_directory();
            let spans = decoded_record_spans(&small, &key_map, dir.as_ref());
            let id = identify_descriptor_records(&small, &spans);
            assert!(
                !id.used_f10_fallback,
                "{label}: small model descriptor identification should not need the f[10] fallback"
            );
        }
    }

    #[test]
    fn test_to_results_via_records_lookup_ex_separates_lookup_definition_from_output() {
        let vdf = vdf_file("../../test/bobby/vdf/lookups/lookup_ex.vdf");
        let results = vdf
            .to_results_via_records()
            .expect("record-based mapping should resolve lookup_ex");
        let vdf_data = vdf.extract_data().unwrap();

        for (name, ot) in [("stock", 1), ("inline lookup table", 4), ("net change", 5)] {
            assert_result_column_matches_ot("lookup_ex", &results, &vdf_data, name, ot);
        }

        if let Some(&col) = results
            .offsets
            .get(&Ident::<Canonical>::new("lookup table 1"))
        {
            let ot4 = vdf_data.entries.get(4).expect("lookup_ex missing OT[4]");
            let is_ot4 =
                ot4.iter()
                    .take(results.step_count)
                    .enumerate()
                    .all(|(step, &expected_value)| {
                        (results.data[step * results.step_size + col] - expected_value).abs()
                            <= 1e-6
                    });
            assert!(
                !is_ot4,
                "lookup_ex: lookup table 1 must not steal the inline lookup table OT[4] series"
            );
        }
    }

    #[test]
    fn test_to_results_via_records_covers_all_non_time_ots_on_small_models() {
        // With the fixed record-region start (`sec1.data_offset() + 204`),
        // the record finder returns every metadata record -- including the
        // header-like records that precede the first sentinel pair. For
        // small, non-arrayed fixtures that gives one record per OT slot
        // beyond Time, so the record-based mapping now resolves 100% of
        // those OT slots.
        //
        // This locks in the coverage gain we now expect on well-shaped
        // small models; regressions here would indicate either a
        // search-start drift or a new filtering rule swallowing
        // previously-valid records.
        for (label, path) in [
            ("water", "../../test/bobby/vdf/water/Current.vdf"),
            ("pop", "../../test/bobby/vdf/pop/Current.vdf"),
        ] {
            let vdf = vdf_file(path);
            let results = vdf
                .to_results_via_records()
                .unwrap_or_else(|e| panic!("{label}: record-based mapping should succeed: {e}"));
            let non_time_named = results.offsets.len().saturating_sub(1);
            let non_time_ots = vdf.offset_table_count.saturating_sub(1);
            assert_eq!(
                non_time_named, non_time_ots,
                "{label}: expected record-based coverage to hit every non-Time OT \
                 ({non_time_named}/{non_time_ots} named)"
            );
        }
    }

    #[test]
    fn test_to_results_via_records_decodes_large_ambiguous_fixture() {
        // WRLD3-03/SCEN01.VDF is a large, edited fixture. The direct f[2] key
        // path decodes each record's section-2 name pointer independently of
        // the slot table, keeping substantial (partial) coverage on this
        // still-ambiguous file. (Before the deterministic slot-table decode
        // this fixture reported fewer slots than records -- a slot-under-count
        // artifact of the old name-parser/scanner, not a property the
        // record-based mapping ever relied on.)
        let vdf = vdf_file("../../test/metasd/WRLD3-03/SCEN01.VDF");
        let results = vdf
            .to_results_via_records()
            .expect("WRLD3 SCEN01: record-based mapping should not error");
        let non_time_named = results.offsets.len().saturating_sub(1);
        assert!(
            non_time_named >= 150,
            "WRLD3 SCEN01: expected >=150 non-Time OT mappings to avoid regressing \
             below the pre-fix baseline (~202), got {non_time_named}"
        );
    }

    #[test]
    fn test_to_results_via_records_produces_columns_on_econ_siblings() {
        // Regression guard: the record-based mapping must still produce a
        // non-empty Results on each econ sibling and must not panic when
        // `to_results_via_records` is invoked. Label correctness on these
        // fixtures is NOT asserted here -- they share the duplicate-owner and
        // alias limitations with econ/base (see the matches_guided test for
        // econ/base), even though record f[2] now decodes names directly.
        for (label, vdf_path) in [
            ("econ/mark2", "../../test/bobby/vdf/econ/mark2.vdf"),
            ("econ/policy", "../../test/bobby/vdf/econ/policy.vdf"),
            ("econ/risk", "../../test/bobby/vdf/econ/risk.vdf"),
        ] {
            let vdf = vdf_file(vdf_path);
            let results = vdf
                .to_results_via_records()
                .unwrap_or_else(|e| panic!("{label}: record-based mapping should succeed: {e}"));
            assert_eq!(
                results.step_count, vdf.time_point_count,
                "{label}: step_count"
            );
            assert!(
                results
                    .offsets
                    .contains_key(&Ident::<Canonical>::new("time")),
                "{label}: Time must always be present"
            );
            // At least produce >1 column (Time + something). Lower bound
            // kept deliberately loose: the point is to catch a regression
            // that empties the Results entirely, not to encode a count
            // floor that pretends labels are correct.
            assert!(
                results.offsets.len() > 1,
                "{label}: expected more than just Time"
            );
        }
    }

    #[test]
    fn test_to_results_via_records_admits_stdlib_helper_names_on_econ() {
        // Verify the filter-removal specifically: names that used to be
        // discarded (notably `#` stdlib-call signatures) now appear as result
        // columns when their keyed record has a valid f[6] and f[11].
        // Record-based mapping alone cannot reason about WHICH variable each
        // name aliases -- that is a model-guided concern -- but the column
        // must surface rather than be silently dropped.
        //
        // We assert that at least one such previously-filtered name is
        // present, which is enough to catch a regression re-introducing
        // the filter.
        let vdf = vdf_file("../../test/bobby/vdf/econ/base.vdf");
        let results = vdf
            .to_results_via_records()
            .expect("econ/base: record-based mapping should succeed");
        let has_stdlib_name = results
            .offsets
            .keys()
            .any(|id| id.as_str().starts_with('#'));
        assert!(
            has_stdlib_name,
            "econ/base: expected at least one stdlib-helper signature column \
             after filter relaxation; got columns \
             {:?}",
            results
                .offsets
                .keys()
                .map(|id| id.as_str().to_owned())
                .collect::<Vec<_>>()
        );
    }
}
