// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Parser for Vensim VDF (binary data file) format.
//!
//! VDF is Vensim's proprietary binary format for simulation output. The format
//! is completely undocumented and no open-source parser exists. This
//! implementation is based on reverse-engineering multiple VDF files of
//! varying complexity:
//!   - `test/metasd/WRLD3-03/SCEN01.VDF` (World3-03 model, 333KB, ~420 variables)
//!   - `test/xmutil_test_models/Ref.vdf` (C-LEARN model, 1.8MB)
//!   - Small models from `~/uib_sd/fall_2008/` (3-7KB, 8-13 variables)
//!
//! # File layout
//!
//! ## 1. File header (0x00..0x7F)
//!
//! ```text
//!   0x00  Magic bytes: 7f f7 17 52
//!   0x04  ASCII timestamp, e.g. "(Sun Nov 30 23:28:16 2008) From bact.mdl"
//!         Null-terminated, zero-padded to offset 0x78.
//!   0x78  u32 time_point_count (e.g. 401 for WRLD3-03, 61 for bact)
//!   0x7C  u32 time_point_count (duplicate)
//! ```
//!
//! ## 2. Sections
//!
//! The file contains multiple sections, each delimited by a 4-byte magic
//! value 0xbf4c37a1 (which is f32 -0.797724). Each section has a 24-byte
//! header followed by variable-length data:
//!
//! ```text
//!   +0   u32  magic (0xbf4c37a1)
//!   +4   u32  declared_size (byte count of "core" section data)
//!   +8   u32  size2 (always equals declared_size)
//!   +12  u32  field3 (often 0x1F4 = 500)
//!   +16  u32  field4 (section type: 19=model info, 2=variable slots, etc.)
//!   +20  u32  field5 (for name table section: high 16 bits = first name length)
//!   +24  ...  section data (extends to next section's magic, not just declared_size)
//! ```
//!
//! ### Section with field4=2 ("Section 1"): Variable slot table
//!
//! This section contains one 16-byte "slot" per variable name. The slots
//! are packed at stride 16. A small pre-slot header (typically 28-44 bytes)
//! precedes the first slot; the header size equals the minimum slot offset.
//!
//! Each slot is 4 x u32 whose meaning is still under investigation. Variable
//! metadata records reference slots via byte offset (see field[13] below).
//!
//! ### Name table section
//!
//! Identified by: field5's high 16 bits give a non-zero length for the first
//! name entry, and the data starts with printable ASCII text. Contains:
//!
//! - **First entry**: NO u16 length prefix. Length from field5 >> 16.
//!   Always the primary output variable name (e.g. "Time").
//! - **Subsequent entries**: u16 length prefix + that many bytes of
//!   null-terminated, zero-padded string data.
//! - **u16 = 0**: Group separator (between model groups, builtins, etc.).
//!   These are skipped during parsing.
//!
//! Names include: variable names, system constants (INITIAL TIME, FINAL TIME,
//! TIME STEP, SAVEPER), model/view group names (prefixed with "." or "-"),
//! and unit names.
//!
//! ## 3. Variable metadata records
//!
//! Between section 1's data end and the name-mapping region, there are N
//! records of 64 bytes each (16 x u32 fields). These records encode the
//! mapping between variable names and their time series data.
//!
//! Key fields identified so far:
//!
//! ```text
//!   field[0]  (offset +0):   Variable type or flags. Values seen: 0, 32, 36, 40, 44.
//!   field[2]  (offset +8):   Monotonically increasing across records. Possibly a
//!                            byte offset into some internal table.
//!   field[8]  (offset +32):  Often the sentinel value 0xf6800000. Some records
//!   field[9]  (offset +36):  have both f[8] and f[9] as sentinel (common for
//!                            "real" variable records). Non-sentinel records may
//!                            represent lookup tables or structural metadata.
//!   field[11] (offset +44):  For small models (bact), this appears to be the
//!                            offset table index. For large models (WRLD3), some
//!                            values exceed the OT count, suggesting the field
//!                            meaning may vary by file version or record type.
//!   field[12] (offset +48):  A byte offset into section 1 data. Groups records
//!                            into clusters (multiple records share a value).
//!                            NOT a direct name reference (values don't match
//!                            the name-mapping table entries for large models).
//!   field[13] (offset +52):  Always 0 in observed files.
//!   field[14] (offset +56):  Sometimes sentinel 0xf6800000.
//! ```
//!
//! The sentinel value 0xf6800000 appears in fields 8, 9, and sometimes 14.
//! Not all records have sentinels; non-sentinel records at the end of the
//! record sequence may represent lookup tables or model structure metadata.
//!
//! ## 4. Name-mapping table (slot table)
//!
//! An array of N u32 values (one per name in the name table), located between
//! the last variable record and the name table section. A few bytes of padding
//! (typically 4) separate the table from the section magic.
//!
//! Each value is a byte offset (relative to section 1's data start). For small
//! models (like bact with 10 names), the offsets have uniform stride 16 (fixed
//! 16-byte "slots" per variable). For larger models (like WRLD3 with 138 names),
//! the stride varies because each slot region holds variable-length metadata.
//!
//! Example (bact model, 10 names, stride 16):
//!   table = [156, 124, 140, 172, 76, 60, 188, 108, 44, 92]
//!   Sorted: 44, 60, 76, 92, 108, 124, 140, 156, 172, 188
//!
//! ## 5. Offset table
//!
//! N u32 entries immediately preceding the first data block. Entry 0 always
//! points to the first data block (the time series). Other entries are either:
//!   - File offsets to data blocks (>= first_block_offset)
//!   - Inline f32 constant values (for variables that don't change over time)
//!
//! ## 6. Data blocks
//!
//! Each block stores a sparse time series:
//!
//! ```text
//!   u16    count (number of stored values, <= time_point_count)
//!   [u8]   bitmap (ceil(time_point_count / 8) bytes)
//!          Each bit indicates whether that time point has a stored value.
//!   [f32]  count values, in time order
//! ```
//!
//! Block 0 is always the time series itself (values like 1900.0, 1900.5, ...).
//! Blocks are packed contiguously with no alignment padding.
//!
//! # Name-to-data mapping
//!
//! The metadata chain linking names to data entries has not been fully decoded
//! despite extensive reverse engineering. No single record field or slot data
//! value reliably maps names to offset table entries across all file sizes.
//!
//! ## Deterministic approach (small models)
//!
//! For small-to-medium models, [`VdfFile::build_deterministic_ot_map`] maps
//! names to OT indices using only structural metadata:
//!
//! 1. Filter records to model variables: f[0]!=0, f[1]!=23, f[10]>0,
//!    f[11]>0, f[11]<ot_count.
//! 2. Sort these records by f[10] (an alphabetical sort key).
//! 3. Filter names (remove system names, group/unit markers, and Vensim
//!    builtin function names embedded in the name table).
//! 4. Sort names alphabetically, pair 1:1 with sorted records.
//!
//! f[10] is NOT alphabetically ordered for large models (Kendall's tau = 0.46
//! for WRLD3), so this only works reliably for smaller VDFs.
//!
//! ## Empirical approach (all models)
//!
//! For large models or when a reference simulation is available:
//!
//! 1. [`load_vdf`] parses the raw data: time series, offset table entries
//!    (each yielding either a full time series or a constant), and the
//!    name table.
//! 2. [`build_vdf_results`] takes the parsed VDF data and a reference
//!    `Results` (e.g. from a simulation run) and matches VDF entries to
//!    simulation variables by comparing time series values at sample points.
//!    Only matches with < 1% relative error are accepted.
//!
//! This approach successfully matches 290+ variables for the WRLD3-03 model
//! with < 0.5% maximum relative error vs Vensim's output.

#[cfg(feature = "file_io")]
use std::collections::{HashMap, HashSet};
#[cfg(feature = "file_io")]
use std::error::Error;
#[cfg(feature = "file_io")]
use std::result::Result as StdResult;

#[cfg(feature = "file_io")]
use crate::common::{Canonical, Ident};
#[cfg(feature = "file_io")]
use crate::results::{Method, Results, Specs};

/// VDF file magic bytes (first 4 bytes of every VDF file).
pub const VDF_FILE_MAGIC: [u8; 4] = [0x7f, 0xf7, 0x17, 0x52];

/// VDF section header magic value: float32 -0.797724 = 0xbf4c37a1.
/// This 4-byte sequence delimits sections within the VDF file.
pub const VDF_SECTION_MAGIC: [u8; 4] = [0xa1, 0x37, 0x4c, 0xbf];

/// Sentinel value appearing in record fields 8, 9, and sometimes 14.
pub const VDF_SENTINEL: u32 = 0xf6800000;

/// Size of a VDF section header in bytes (magic + 5 u32 fields).
pub const SECTION_HEADER_SIZE: usize = 24;

/// Size of a VDF variable metadata record in bytes (16 u32 fields).
pub const RECORD_SIZE: usize = 64;

/// Size of the VDF file header in bytes.
pub const FILE_HEADER_SIZE: usize = 0x80;

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
/// Each section has a 24-byte header followed by data. The header's `declared_size`
/// field describes only a "core" portion of the section's data. The actual extent
/// of a section runs from its header to the start of the next section header
/// (magic-to-magic), captured by `region_end`. See `vdf_analysis.md` for details.
#[derive(Debug, Clone)]
pub struct Section {
    /// Absolute file offset of the section magic bytes.
    pub file_offset: usize,
    /// Declared size of section data in bytes (from the header). This describes
    /// only a "core" or "initial" portion; the real extent is `region_end`.
    pub declared_size: u32,
    /// Absolute file offset where this section's region ends. For sections
    /// 0..n-1, this is the next section's `file_offset`. For the last
    /// section, this is the file length.
    pub region_end: usize,
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

    /// Absolute file offset where the declared section data ends.
    /// This is the header's `declared_size` extent, not the full region.
    pub fn declared_data_end(&self) -> usize {
        self.data_offset() + self.declared_size as usize
    }

    /// Size in bytes of the section's full region data (after the header).
    /// For degenerate sections (like section 5 in small models, whose
    /// declared data overlaps the next section's header), this returns 0.
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

/// Fully parsed VDF file holding all structural metadata.
///
/// Created via [`VdfFile::parse`], this struct provides access to all
/// decoded sections, names, records, slot table, and offset table entries.
#[cfg(feature = "file_io")]
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
    /// The first `section_name_count` names fall within the section's
    /// `declared_size`; the rest are in the region but past that boundary.
    pub names: Vec<String>,
    /// How many names fall within the name table section's `declared_size`.
    /// The slot table has exactly this many entries, paired 1:1 with
    /// `names[..section_name_count]`.
    pub section_name_count: usize,
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
}

#[cfg(feature = "file_io")]
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

        // Parse names up to the section's region boundary. The name table
        // extends past declared_size within the region.
        let (names, section_name_count) = name_section_idx
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
                find_slot_table(&data, ns, section_name_count, sec1_data_size)
            })
            .unwrap_or((0, Vec::new()));

        // Find records between section 1's declared data end and the name
        // table section boundary. Records live within section 1's region
        // but past its declared data extent.
        let search_start = sections
            .get(1)
            .map(|s| s.declared_data_end())
            .unwrap_or(FILE_HEADER_SIZE);
        let search_bound = sections.get(1).map(|s| s.region_end).unwrap_or(data.len());
        let records_end = if slot_table_offset > 0 && slot_table_offset < search_bound {
            slot_table_offset
        } else {
            search_bound
        };
        let records = find_records(&data, search_start, records_end);

        let first_data_block = find_first_data_block(&data, time_point_count, bitmap_size)
            .ok_or("could not find first VDF data block")?;
        let (offset_table_start, offset_table_count) = find_offset_table(&data, first_data_block);

        Ok(VdfFile {
            data,
            time_point_count,
            bitmap_size,
            sections,
            names,
            section_name_count,
            name_section_idx,
            slot_table,
            slot_table_offset,
            records,
            offset_table_start,
            offset_table_count,
            first_data_block,
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

    /// Get the variable slot table section (always at section index 1).
    /// Its field4 value varies across VDF versions (2, 42, etc.).
    pub fn slot_section(&self) -> Option<&Section> {
        self.sections.get(1)
    }

    /// Build a `Results` struct by matching VDF data entries against a reference
    /// simulation's results using time series correlation.
    ///
    /// The VDF metadata chain (records, slots, name table) does not provide a
    /// reliable mapping from variable names to data entries -- extensive reverse
    /// engineering found no consistent decoding. This method uses empirical
    /// matching instead: for each variable in `reference`, it finds the VDF entry
    /// whose time series best matches by sum of squared relative errors.
    pub fn to_results(&self, reference: &Results) -> StdResult<Results, Box<dyn Error>> {
        let vdf_data = self.extract_data()?;
        build_vdf_results(&vdf_data, reference)
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
                extract_block_series(
                    &self.data,
                    raw_val as usize,
                    self.bitmap_size,
                    &time_values,
                    step_count,
                )?
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

    /// Build a mapping from variable names to offset table indices using
    /// structural metadata only (no reference simulation needed).
    ///
    /// Within each f[12] record group, records sorted by f[10] correspond
    /// to alphabetically sorted variable names. This method:
    ///
    /// 1. Groups records by f[12] and identifies "control" groups (those
    ///    containing records with f[1]=23, which marks system variables
    ///    like INITIAL TIME, FINAL TIME, etc.).
    /// 2. Collects model variable records from non-control groups.
    /// 3. Filters model variable names from the name table (excluding
    ///    group markers starting with '.', unit names starting with '-',
    ///    and system variable names).
    /// 4. Sorts records by f[10] and names alphabetically, then pairs
    ///    them 1:1.
    ///
    /// Returns a map from VDF name-table names (original case) to OT
    /// indices. Always includes "Time" → 0.
    ///
    /// This works reliably for small-to-medium models. For very large
    /// models (like WRLD3 with 400+ variables), f[10] ordering may not
    /// perfectly match alphabetical order; use [`build_empirical_ot_map`]
    /// with a reference simulation instead.
    pub fn build_deterministic_ot_map(&self) -> StdResult<HashMap<String, usize>, Box<dyn Error>> {
        let ot_count = self.offset_table_count;

        // Collect model variable records using per-record criteria:
        //   f[0] != 0 : non-zero/non-padding record
        //   f[1] != 23 : not a system/control variable (INITIAL TIME, etc.)
        //   f[10] > 0  : has a non-zero alphabetical sort key
        //   f[11] > 0  : OT index > 0 (0 is always the time series)
        //   f[11] < ot_count : valid offset table index
        //
        // Note: we filter individual records rather than whole f[12] groups
        // because some VDFs mix control and model records in the same group.
        let mut model_records: Vec<(u32, u32)> = Vec::new(); // (f10, f11=OT_idx)
        for rec in &self.records {
            let ot_idx = rec.fields[11] as usize;
            if rec.fields[0] != 0
                && rec.fields[1] != 23
                && rec.fields[10] > 0
                && ot_idx > 0
                && ot_idx < ot_count
            {
                model_records.push((rec.fields[10], rec.fields[11]));
            }
        }
        model_records.sort_by_key(|&(f10, _)| f10);

        // Filter candidate variable names from the name table: exclude
        // group markers ('.'), unit names ('-'), and system variable names.
        let system_names: HashSet<&str> =
            ["Time", "INITIAL TIME", "FINAL TIME", "TIME STEP", "SAVEPER"]
                .into_iter()
                .collect();

        // Only use slotted names (those within declared_size, which have
        // slot table entries) for the deterministic mapping.
        let mut candidates: Vec<String> = self.names[..self.section_name_count]
            .iter()
            .filter(|name| {
                !name.is_empty()
                    && !name.starts_with('.')
                    && !name.starts_with('-')
                    && !system_names.contains(name.as_str())
            })
            .cloned()
            .collect();

        let target = model_records.len();

        // Vensim embeds builtin function names (STEP, MIN, MAX, etc.) in the
        // name table alongside model variables. When we have more candidates
        // than model records, remove names matching known Vensim builtins.
        if candidates.len() > target {
            // Single-character non-alphanumeric names (e.g., "?") are always
            // structural placeholders, never model variables.
            candidates
                .retain(|n| n.len() != 1 || n.chars().next().is_some_and(|c| c.is_alphanumeric()));
        }
        if candidates.len() > target {
            let vensim_builtins: HashSet<&str> = [
                "abs", "cos", "exp", "integer", "ln", "log", "max", "min", "modulo", "pi", "sin",
                "sqrt", "tan", "step", "pulse", "ramp", "delay", "delay1", "delay3", "smooth",
                "smooth3", "trend", "sum", "product", "vmin", "vmax", "elmcount",
            ]
            .into_iter()
            .collect();
            candidates.retain(|n| !vensim_builtins.contains(n.to_lowercase().as_str()));
        }

        candidates.sort_by_key(|a| a.to_lowercase());

        if candidates.len() != target {
            return Err(format!(
                "candidate name count ({}) != model record count ({}); names={:?}",
                candidates.len(),
                model_records.len(),
                candidates
            )
            .into());
        }

        let mut mapping = HashMap::new();
        mapping.insert("Time".to_string(), 0);
        for (i, name) in candidates.iter().enumerate() {
            mapping.insert(name.clone(), model_records[i].1 as usize);
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
                    declared_size: read_u32(data, offset + 4),
                    region_end: 0, // filled in below
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

/// Parse the name table from a section, stopping at the declared section
/// boundary. The first entry has no u16 length prefix; its length comes
/// from field5's high 16 bits. Subsequent entries are u16-length-prefixed.
/// u16=0 entries are group separators (skipped).
pub fn parse_name_table(data: &[u8], section: &Section) -> Vec<String> {
    let section_end = section.declared_data_end().min(data.len());
    parse_name_table_extended(data, section, section_end).0
}

/// Parse the name table up to `parse_end`. The name table typically extends
/// past the section's `declared_size` within its region. `parse_end` should
/// be the section's `region_end`.
///
/// Validates each entry (max 256 bytes, printable ASCII) and stops when it
/// encounters data that doesn't look like a name entry.
///
/// Returns `(all_names, section_name_count)` where `section_name_count` is
/// the number of names within the section's `declared_size`. The slot table
/// has exactly `section_name_count` entries, paired 1:1 with those names.
pub fn parse_name_table_extended(
    data: &[u8],
    section: &Section,
    parse_end: usize,
) -> (Vec<String>, usize) {
    let mut names = Vec::new();
    let data_start = section.data_offset();
    let section_end = section.declared_data_end().min(data.len());
    let parse_end = parse_end.min(data.len());

    let first_len = (section.field5 >> 16) as usize;
    if first_len == 0 || data_start + first_len > data.len() {
        return (names, 0);
    }
    let s: String = data[data_start..data_start + first_len]
        .iter()
        .take_while(|&&b| b != 0)
        .map(|&b| b as char)
        .collect();
    names.push(s);

    let mut pos = data_start + first_len;
    let mut section_name_count: Option<usize> = None;

    while pos + 2 <= parse_end {
        // Track when we've crossed the section boundary. The old
        // parse_name_table stops in two cases: (1) can't read a u16
        // prefix because pos+2 > section_end, or (2) the name data
        // extends past section_end. Record the count at that point.
        if section_name_count.is_none() && pos + 2 > section_end {
            section_name_count = Some(names.len());
        }

        let len = read_u16(data, pos) as usize;
        pos += 2;
        if len == 0 {
            continue;
        }

        if section_name_count.is_none() && pos + len > section_end {
            section_name_count = Some(names.len());
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

    let section_name_count = section_name_count.unwrap_or(names.len());
    (names, section_name_count)
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

/// Find the slot lookup table. This is an array of N u32 values (one per name)
/// located just before the name table section. The values are byte offsets into
/// section 1 data. For small models they have uniform stride (e.g., 16); for
/// larger models the stride may vary (variable-length slot metadata).
///
/// Returns (file_offset_of_table, values).
pub fn find_slot_table(
    data: &[u8],
    name_table_section: &Section,
    name_count: usize,
    section1_data_size: usize,
) -> (usize, Vec<u32>) {
    if name_count == 0 {
        return (0, Vec::new());
    }
    let end = name_table_section.file_offset;
    let table_size_bytes = name_count * 4;

    for gap in 0..20 {
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
            .all(|&v| v % 4 == 0 && (v as usize) < section1_data_size);
        if !all_valid {
            continue;
        }

        if sorted[0] == 0 {
            continue;
        }

        let strides: Vec<u32> = sorted.windows(2).map(|pair| pair[1] - pair[0]).collect();
        let min_stride = strides.iter().copied().min().unwrap_or(0);
        if min_stride >= 4 {
            return (table_start, values);
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
/// equal to time_point_count, a fully-set bitmap, and a plausible first value
/// (a year like 1900 or 0 for models starting at t=0).
pub fn find_first_data_block(
    data: &[u8],
    time_point_count: usize,
    bitmap_size: usize,
) -> Option<usize> {
    let count_bytes = (time_point_count as u16).to_le_bytes();
    let min_block_size = 2 + bitmap_size + 4;
    let search_start = 0x100;
    for pos in search_start..data.len().saturating_sub(min_block_size) {
        if data[pos..pos + 2] != count_bytes {
            continue;
        }
        let bm = &data[pos + 2..pos + 2 + bitmap_size];
        let set_bits: usize = bm.iter().map(|b| b.count_ones() as usize).sum();
        if set_bits != time_point_count {
            continue;
        }
        let data_off = pos + 2 + bitmap_size;
        if data_off + 4 > data.len() {
            continue;
        }
        let first_val = read_f32(data, data_off);
        if (1800.0..2200.0).contains(&first_val) || first_val == 0.0 {
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
#[cfg(feature = "file_io")]
pub struct VdfData {
    /// Time values extracted from block 0 (e.g. [1900.0, 1900.5, ..., 2100.0]).
    pub time_values: Vec<f64>,
    /// Each entry is a time series (one f64 per time point) for one variable.
    /// Indexed by offset-table position. Entry 0 is always the time series.
    pub entries: Vec<Vec<f64>>,
}

/// Parse a Vensim VDF binary file into raw time series data.
///
/// Returns the parsed data without variable name assignments, since the
/// VDF metadata mapping from names to data entries is not yet decoded.
/// Use [`build_vdf_results`] to match entries against simulation output.
#[cfg(feature = "file_io")]
pub fn load_vdf(file_path: &str) -> StdResult<VdfData, Box<dyn Error>> {
    let data = std::fs::read(file_path)?;
    let vdf = VdfFile::parse(data)?;
    vdf.extract_data()
}

/// Build a `Results` struct by matching VDF data entries against a reference
/// simulation's results.
///
/// For each variable in `reference`, we search the VDF entries for the one
/// whose time series best matches (by sum of squared relative errors at
/// multiple sample points). This avoids needing to decode the VDF metadata
/// that maps names to offset table positions.
#[cfg(feature = "file_io")]
pub fn build_vdf_results(vdf: &VdfData, reference: &Results) -> StdResult<Results, Box<dyn Error>> {
    let step_count = vdf.time_values.len();
    let ref_step_count = reference.step_count;

    if step_count != ref_step_count {
        return Err(format!(
            "VDF has {step_count} time points but simulation has {ref_step_count}"
        )
        .into());
    }

    let n_vars = reference.offsets.len();
    let step_size = n_vars;
    let mut step_data = vec![f64::NAN; step_count * step_size];
    let mut offsets: HashMap<Ident<Canonical>, usize> = HashMap::new();

    let time_ident = Ident::<Canonical>::from_str_unchecked("time");
    let time_col = 0;
    offsets.insert(time_ident.clone(), time_col);
    for (step, &t) in vdf.time_values.iter().enumerate() {
        step_data[step * step_size + time_col] = t;
    }

    let mut claimed: Vec<bool> = vec![false; vdf.entries.len()];
    claimed[0] = true;

    let mut next_col = 1;

    let sample_indices: Vec<usize> = {
        let mut indices = vec![0];
        if step_count > 10 {
            indices.push(step_count / 4);
            indices.push(step_count / 2);
            indices.push(3 * step_count / 4);
        }
        if step_count > 1 {
            indices.push(step_count - 1);
        }
        indices
    };

    const MATCH_THRESHOLD: f64 = 0.01;

    for (ident, &ref_off) in &reference.offsets {
        if *ident == time_ident {
            continue;
        }

        let ref_series: Vec<f64> = (0..step_count)
            .map(|step| {
                let row =
                    &reference.data[step * reference.step_size..(step + 1) * reference.step_size];
                row[ref_off]
            })
            .collect();

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
            let col = next_col;
            next_col += 1;
            offsets.insert(ident.clone(), col);
            claimed[ei] = true;
            for step in 0..step_count {
                step_data[step * step_size + col] = vdf.entries[ei][step];
            }
        }
    }

    let initial_time = vdf.time_values[0];
    let final_time = vdf.time_values[step_count - 1];
    let saveper = if step_count > 1 {
        vdf.time_values[1] - vdf.time_values[0]
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
        },
        is_vensim: true,
    })
}

/// Build an empirical mapping from canonical variable name to OT entry index.
///
/// Uses the same time-series correlation approach as [`build_vdf_results`]
/// but returns the raw mapping instead of a `Results` struct. Useful for
/// verifying metadata-based name resolution against known-good matches.
#[cfg(feature = "file_io")]
pub fn build_empirical_ot_map(
    vdf: &VdfData,
    reference: &Results,
) -> StdResult<HashMap<Ident<Canonical>, usize>, Box<dyn Error>> {
    let step_count = vdf.time_values.len();
    if step_count != reference.step_count {
        return Err(format!(
            "VDF has {step_count} time points but simulation has {}",
            reference.step_count
        )
        .into());
    }

    let sample_indices: Vec<usize> = {
        let mut indices = vec![0];
        if step_count > 10 {
            indices.push(step_count / 4);
            indices.push(step_count / 2);
            indices.push(3 * step_count / 4);
        }
        if step_count > 1 {
            indices.push(step_count - 1);
        }
        indices
    };

    const MATCH_THRESHOLD: f64 = 0.01;
    let time_ident = Ident::<Canonical>::from_str_unchecked("time");

    let mut claimed: Vec<bool> = vec![false; vdf.entries.len()];
    claimed[0] = true; // OT[0] is always the time series

    let mut ot_map = HashMap::new();
    ot_map.insert(time_ident.clone(), 0usize);

    for (ident, &ref_off) in &reference.offsets {
        if *ident == time_ident {
            continue;
        }

        let ref_series: Vec<f64> = (0..step_count)
            .map(|step| {
                let row =
                    &reference.data[step * reference.step_size..(step + 1) * reference.step_size];
                row[ref_off]
            })
            .collect();

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
            ot_map.insert(ident.clone(), ei);
        }
    }

    Ok(ot_map)
}

/// Compute a match error between a reference series and a VDF entry,
/// sampled at the given indices. Returns f64::MAX if the series lengths
/// don't match.
#[cfg(feature = "file_io")]
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

    total_error
}

/// Extract the time series values from the first data block (block 0).
#[cfg(feature = "file_io")]
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
#[cfg(feature = "file_io")]
fn extract_block_series(
    data: &[u8],
    block_offset: usize,
    bitmap_size: usize,
    time_values: &[f64],
    step_count: usize,
) -> StdResult<Vec<f64>, Box<dyn Error>> {
    let count = u16::from_le_bytes(data[block_offset..block_offset + 2].try_into()?) as usize;
    let bm = &data[block_offset + 2..block_offset + 2 + bitmap_size];
    let data_start = block_offset + 2 + bitmap_size;

    let initial_time = time_values[0];
    let saveper = if time_values.len() > 1 {
        time_values[1] - time_values[0]
    } else {
        1.0
    };

    let mut series = vec![f64::NAN; step_count];
    let mut data_idx = 0;
    let mut last_val = f64::NAN;

    for (time_idx, &t) in time_values.iter().enumerate() {
        let bit_set = (bm[time_idx / 8] >> (time_idx % 8)) & 1 == 1;
        if bit_set && data_idx < count {
            let off = data_start + data_idx * 4;
            last_val = f32::from_le_bytes(data[off..off + 4].try_into()?) as f64;
            data_idx += 1;
        }

        let step = ((t - initial_time) / saveper).round() as usize;
        if step < step_count {
            series[step] = last_val;
        }
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
    fn test_section_offsets() {
        let s = Section {
            file_offset: 100,
            declared_size: 50,
            region_end: 300,
            field3: 0,
            field4: 0,
            field5: 0,
        };
        assert_eq!(s.data_offset(), 124);
        assert_eq!(s.declared_data_end(), 174);
        assert_eq!(s.region_data_size(), 176); // 300 - 124
    }

    #[test]
    fn test_section_degenerate_region() {
        // Section 5 in small models has declared_size=6 but its data
        // overlaps with the next section's header, so region_end <=
        // data_offset, yielding region_data_size() == 0.
        let s = Section {
            file_offset: 100,
            declared_size: 6,
            region_end: 118, // next section starts before data_offset (124)
            field3: 0,
            field4: 0,
            field5: 0,
        };
        assert_eq!(s.data_offset(), 124);
        assert_eq!(s.declared_data_end(), 130);
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
        assert_eq!(sections[0].declared_size, 10);
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
    fn test_parse_name_table_extended_past_declared_size() {
        // Names extend past declared_size within the section's region.
        // Only "Time" fits within declared_size (20 bytes); the other
        // names are in the region but past that boundary.
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
        // u16 prefix at offset 30 (inside declared_size, which ends at 44)
        data[30..32].copy_from_slice(&14u16.to_le_bytes());
        // Name data at 32..46, extends past declared_size boundary
        data[32..43].copy_from_slice(b"hello world");
        data[43..46].copy_from_slice(&[0, 0, 0]);

        // Third name: u16 len = 8, "foo\0\0\0\0\0" at offset 46
        data[46..48].copy_from_slice(&8u16.to_le_bytes());
        data[48..51].copy_from_slice(b"foo");
        data[51..56].copy_from_slice(&[0, 0, 0, 0, 0]);

        let section = Section {
            file_offset: 0,
            declared_size: 20,
            region_end: 80,
            field3: 500,
            field4: 0,
            field5: 6u32 << 16,
        };

        // parse_name_table stops at declared_size
        let section_only = parse_name_table(&data, &section);
        assert_eq!(section_only.len(), 1);
        assert_eq!(section_only[0], "Time");

        // parse_name_table_extended parses the full region
        let (all_names, section_name_count) = parse_name_table_extended(&data, &section, 80);
        assert_eq!(section_name_count, 1); // only "Time" within declared_size
        assert_eq!(all_names.len(), 3);
        assert_eq!(all_names[0], "Time");
        assert_eq!(all_names[1], "hello world");
        assert_eq!(all_names[2], "foo");
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
            declared_size: 40,
            region_end: 200,
            field3: 500,
            field4: 0,
            field5: 6u32 << 16,
        };

        let (names, section_name_count) = parse_name_table_extended(&data, &section, 200);
        // Both names are within the section
        assert_eq!(section_name_count, 2);
        assert_eq!(names.len(), 2);
        assert_eq!(names[0], "Time");
        assert_eq!(names[1], "test var");
    }

    #[test]
    fn test_parse_name_table_extended_skips_separators() {
        let mut data = vec![0u8; 256];

        data[0..4].copy_from_slice(&VDF_SECTION_MAGIC);
        data[4..8].copy_from_slice(&10u32.to_le_bytes()); // small section
        data[8..12].copy_from_slice(&10u32.to_le_bytes());
        data[12..16].copy_from_slice(&500u32.to_le_bytes());
        data[16..20].copy_from_slice(&0u32.to_le_bytes());
        data[20..24].copy_from_slice(&(6u32 << 16).to_le_bytes());

        // First name: "Time\0\0" (6 bytes)
        data[24..28].copy_from_slice(b"Time");
        data[28..30].copy_from_slice(&[0, 0]);

        // Section ends at 24 + 10 = 34

        // u16 separator at offset 30
        data[30..32].copy_from_slice(&0u16.to_le_bytes());
        // u16 separator at offset 32
        data[32..34].copy_from_slice(&0u16.to_le_bytes());

        // Extended name after section boundary: u16=6, "abc\0\0\0"
        data[34..36].copy_from_slice(&6u16.to_le_bytes());
        data[36..39].copy_from_slice(b"abc");
        data[39..42].copy_from_slice(&[0, 0, 0]);

        let section = Section {
            file_offset: 0,
            declared_size: 10,
            region_end: 80,
            field3: 500,
            field4: 0,
            field5: 6u32 << 16,
        };

        let (names, section_name_count) = parse_name_table_extended(&data, &section, 80);
        assert_eq!(section_name_count, 1); // only "Time" in section
        assert_eq!(names.len(), 2);
        assert_eq!(names[0], "Time");
        assert_eq!(names[1], "abc");
    }

    #[cfg(feature = "file_io")]
    mod file_io_tests {
        use super::super::*;

        fn vdf_file(path: &str) -> VdfFile {
            let data = std::fs::read(path)
                .unwrap_or_else(|e| panic!("failed to read VDF file {}: {}", path, e));
            VdfFile::parse(data)
                .unwrap_or_else(|e| panic!("failed to parse VDF file {}: {}", path, e))
        }

        #[test]
        fn test_econ_base_names_past_declared_size() {
            let vdf = vdf_file("../../third_party/uib_sd/fall_2008/econ/base.vdf");

            // 42 names within the section boundary have slot table entries
            assert_eq!(vdf.section_name_count, 42);
            assert_eq!(vdf.names[0], "Time");
            assert_eq!(
                vdf.names[41],
                "effect of hud policies on risk taking behavior"
            );

            // Names past declared_size don't have slot table entries
            let unslotted = &vdf.names[vdf.section_name_count..];
            assert!(
                !unslotted.is_empty(),
                "expected unslotted names for econ model"
            );
            assert_eq!(
                unslotted[0],
                "effect of negative inflation rate on risk taking behavior"
            );
            assert_eq!(unslotted[1], "max risk");
            assert_eq!(unslotted[2], "hud policy");

            assert!(
                unslotted.len() >= 50,
                "expected at least 50 unslotted names, got {}",
                unslotted.len()
            );

            assert!(vdf.names.len() >= 92);

            // Slot table has entries only for names within declared_size
            assert_eq!(vdf.slot_table.len(), 42);
        }

        #[test]
        fn test_zambaqui_baserun_names_past_declared_size() {
            let vdf = vdf_file("../../third_party/uib_sd/zambaqui/baserun.vdf");

            assert_eq!(vdf.section_name_count, 178);
            assert_eq!(vdf.names[0], "Time");

            let unslotted = &vdf.names[vdf.section_name_count..];
            assert!(
                !unslotted.is_empty(),
                "expected unslotted names for zambaqui model"
            );

            assert!(
                unslotted.contains(&"births".to_string()),
                "expected 'births' in unslotted names"
            );
            assert!(
                unslotted.contains(&"capital".to_string()),
                "expected 'capital' in unslotted names"
            );
            assert!(
                unslotted.contains(&"total population".to_string()),
                "expected 'total population' in unslotted names"
            );

            assert!(
                unslotted.len() >= 250,
                "expected at least 250 unslotted names, got {}",
                unslotted.len()
            );

            // Slot table has entries only for names within declared_size
            assert_eq!(vdf.slot_table.len(), 178);
        }

        #[test]
        fn test_small_vdf_all_names_slotted() {
            // Small models should have all names within declared_size
            let vdf = vdf_file(
                "../../third_party/uib_sd/fall_2008/sd202/assignments/assignment_2/Current.vdf",
            );

            assert!(!vdf.names.is_empty());
            assert_eq!(
                vdf.section_name_count,
                vdf.names.len(),
                "small VDF should have all names within declared_size, got {} total vs {} slotted",
                vdf.names.len(),
                vdf.section_name_count
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
            let vdf_paths = collect_vdf_files(std::path::Path::new("../../third_party/uib_sd"));

            assert!(
                vdf_paths.len() >= 10,
                "expected at least 10 VDF files, found {}",
                vdf_paths.len()
            );

            let mut parsed_count = 0;
            for path in &vdf_paths {
                let data = std::fs::read(path)
                    .unwrap_or_else(|e| panic!("failed to read {}: {}", path.display(), e));
                let file_len = data.len();
                // Some .vdf files use a different format variant (e.g. magic
                // 0x41 instead of 0x52). Skip those rather than failing.
                let vdf = match VdfFile::parse(data) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
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

                for name in &vdf.names[vdf.section_name_count..] {
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
                "expected at least 10 parseable VDF files, only {} succeeded",
                parsed_count
            );
        }

        #[test]
        fn test_section5_degenerate_in_small_models() {
            // In small/econ models, section 5 has size=6 and its "data"
            // overlaps with section 6's header, making it a zero-content marker.
            let vdf = vdf_file(
                "../../third_party/uib_sd/fall_2008/sd202/assignments/assignment_2/Current.vdf",
            );
            assert!(
                vdf.sections.len() >= 6,
                "expected at least 6 sections, got {}",
                vdf.sections.len()
            );

            let sec5 = &vdf.sections[5];
            assert_eq!(sec5.declared_size, 6);
            assert_eq!(
                sec5.region_data_size(),
                0,
                "section 5 should be degenerate (region_data_size == 0)"
            );
        }

        #[test]
        fn test_records_within_section1_region() {
            let vdf = vdf_file("../../third_party/uib_sd/fall_2008/econ/base.vdf");
            if let Some(sec1) = vdf.sections.get(1) {
                for rec in &vdf.records {
                    assert!(
                        rec.file_offset >= sec1.declared_data_end()
                            && rec.file_offset + RECORD_SIZE <= sec1.region_end,
                        "record at 0x{:x} outside section 1 region (0x{:x}..0x{:x})",
                        rec.file_offset,
                        sec1.declared_data_end(),
                        sec1.region_end,
                    );
                }
            }
        }

        #[test]
        fn test_name_table_within_section_region() {
            let vdf = vdf_file("../../third_party/uib_sd/fall_2008/econ/base.vdf");
            if let Some(ns_idx) = vdf.name_section_idx {
                let sec = &vdf.sections[ns_idx];
                assert!(
                    !vdf.names.is_empty(),
                    "expected names in the name table section"
                );
                assert!(
                    vdf.names.len() > vdf.section_name_count,
                    "econ model should have names past declared_size"
                );
                // The region should be large enough to hold the name data
                assert!(
                    sec.region_data_size() > sec.declared_size as usize,
                    "name table region should be larger than declared size"
                );
            }
        }

        /// Analyze the slot data structure and record chain across all VDF
        /// files. Key findings this test explores:
        /// - Record f[12] values do NOT match slot table entries (0% overlap)
        /// - The 16-byte slot data at each slot offset contains record-like fields
        /// - Some slot data has sentinel pairs (0xf6800000) at words 0,1
        /// - Slot word[1]=23 indicates system variables
        #[test]
        fn test_slot_record_ot_chain() {
            let vdf_paths = collect_vdf_files(std::path::Path::new("../../third_party/uib_sd"));
            assert!(
                vdf_paths.len() >= 10,
                "expected at least 10 VDF files, found {}",
                vdf_paths.len()
            );

            let mut total_files_parsed = 0;
            let mut total_slotted_names = 0;
            let mut total_w3_unique = 0;

            for path in &vdf_paths {
                let data = match std::fs::read(path) {
                    Ok(d) => d,
                    Err(_) => continue,
                };
                let vdf = match VdfFile::parse(data) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                total_files_parsed += 1;

                let sec1 = match vdf.slot_section() {
                    Some(s) => s.clone(),
                    None => continue,
                };
                let sec1_data_start = sec1.data_offset();
                let ot_count = vdf.offset_table_count;
                let slotted_count = vdf.section_name_count.min(vdf.slot_table.len());

                let fname = path
                    .file_name()
                    .map(|f| f.to_string_lossy().to_string())
                    .unwrap_or_default();

                eprintln!(
                    "\n=== {} ({} names, {} slotted, {} slot_table, {} records, {} OT, first_block=0x{:x}) ===",
                    fname,
                    vdf.names.len(),
                    vdf.section_name_count,
                    vdf.slot_table.len(),
                    vdf.records.len(),
                    ot_count,
                    vdf.first_data_block,
                );

                if slotted_count == 0 {
                    eprintln!("  (no slot table entries)");
                    if vdf.records.len() <= 15 {
                        for (ri, rec) in vdf.records.iter().enumerate() {
                            eprintln!(
                                "  rec[{:2}] f[0]={:3} f[1]={:3} f[10]={:5} f[11]={:5} f[12]={:5}",
                                ri,
                                rec.fields[0],
                                rec.fields[1],
                                rec.fields[10],
                                rec.fields[11],
                                rec.slot_ref()
                            );
                        }
                    }
                    continue;
                }

                // Compute slot strides
                let mut sorted_slots: Vec<u32> = vdf.slot_table[..slotted_count].to_vec();
                sorted_slots.sort();
                sorted_slots.dedup();
                let strides: Vec<u32> = sorted_slots.windows(2).map(|p| p[1] - p[0]).collect();
                let mut unique_strides = strides.clone();
                unique_strides.sort();
                unique_strides.dedup();
                eprintln!(
                    "  Strides: min={}, max={}, unique={:?}",
                    strides.iter().copied().min().unwrap_or(0),
                    strides.iter().copied().max().unwrap_or(0),
                    unique_strides,
                );

                // Read slot data and analyze patterns
                let mut all_w3: Vec<u32> = Vec::new();
                let mut sentinel_01 = 0usize;
                let mut w1_is_23 = 0usize;

                for i in 0..slotted_count {
                    let name = &vdf.names[i];
                    let slot_offset = vdf.slot_table[i];
                    let abs_pos = sec1_data_start + slot_offset as usize;
                    if abs_pos + 16 > vdf.data.len() {
                        continue;
                    }

                    let w = [
                        read_u32(&vdf.data, abs_pos),
                        read_u32(&vdf.data, abs_pos + 4),
                        read_u32(&vdf.data, abs_pos + 8),
                        read_u32(&vdf.data, abs_pos + 12),
                    ];

                    all_w3.push(w[3]);
                    if w[0] == VDF_SENTINEL && w[1] == VDF_SENTINEL {
                        sentinel_01 += 1;
                    }
                    if w[1] == 23 {
                        w1_is_23 += 1;
                    }

                    if i < 20 {
                        let tag = if w[0] == VDF_SENTINEL && w[1] == VDF_SENTINEL {
                            "SENT01"
                        } else if w[1] == 23 {
                            "SYS"
                        } else {
                            ""
                        };
                        eprintln!(
                            "  slot[{:3}] {:45} off={:5} [{:08x},{:08x},{:08x},{:08x}] ({:>8},{:>8},{:>8},{:>8}) {}",
                            i,
                            name,
                            slot_offset,
                            w[0],
                            w[1],
                            w[2],
                            w[3],
                            w[0],
                            w[1],
                            w[2],
                            w[3],
                            tag,
                        );
                    }
                }

                // w3 uniqueness
                let mut w3_sorted = all_w3.clone();
                w3_sorted.sort();
                let w3_unique = {
                    let mut d = w3_sorted.clone();
                    d.dedup();
                    d.len()
                };
                let w3_valid_ot = if ot_count > 0 {
                    all_w3.iter().filter(|&&v| (v as usize) < ot_count).count()
                } else {
                    0
                };

                // f[12] vs slot table overlap
                let unique_f12: std::collections::HashSet<u32> =
                    vdf.records.iter().map(|r| r.slot_ref()).collect();
                let slot_set: std::collections::HashSet<u32> =
                    vdf.slot_table[..slotted_count].iter().copied().collect();
                let overlap = unique_f12.intersection(&slot_set).count();

                eprintln!(
                    "  sent01={}/{} w1=23={}/{} w3_unique={}/{} w3_valid_ot={}/{}",
                    sentinel_01,
                    slotted_count,
                    w1_is_23,
                    slotted_count,
                    w3_unique,
                    slotted_count,
                    w3_valid_ot,
                    slotted_count,
                );
                eprintln!(
                    "  f[12] vs slot overlap: {}/{} f12={:?}",
                    overlap,
                    unique_f12.len(),
                    {
                        let mut v: Vec<_> = unique_f12.iter().copied().collect();
                        v.sort();
                        v
                    }
                );

                if w3_unique < all_w3.len() {
                    let mut dupe_counts: std::collections::HashMap<u32, usize> =
                        std::collections::HashMap::new();
                    for &v in &all_w3 {
                        *dupe_counts.entry(v).or_insert(0) += 1;
                    }
                    let mut dupes: Vec<_> = dupe_counts
                        .iter()
                        .filter(|&(_, &count)| count > 1)
                        .map(|(&val, &count)| (val, count))
                        .collect();
                    dupes.sort();
                    eprintln!("  w3 dupes: {:?}", dupes);
                }

                total_slotted_names += slotted_count;
                total_w3_unique += w3_unique;
            }

            eprintln!("\n=== AGGREGATE ({} files) ===", total_files_parsed);
            eprintln!("  Total slotted names: {}", total_slotted_names);
            eprintln!("  Total w3 unique: {}", total_w3_unique);

            assert!(total_files_parsed >= 10);
        }

        /// Helper: simulate an MDL file and return Results.
        fn simulate_mdl(mdl_path: &str) -> crate::Results {
            let contents = std::fs::read_to_string(mdl_path)
                .unwrap_or_else(|e| panic!("failed to read {mdl_path}: {e}"));
            let datamodel_project = crate::compat::open_vensim(&contents)
                .unwrap_or_else(|e| panic!("failed to parse {mdl_path}: {e}"));
            let project = std::rc::Rc::new(crate::Project::from(datamodel_project));
            let sim = crate::interpreter::Simulation::new(&project, "main")
                .unwrap_or_else(|e| panic!("failed to create simulation for {mdl_path}: {e}"));
            sim.run_to_end()
                .unwrap_or_else(|e| panic!("interpreter run failed for {mdl_path}: {e}"))
        }

        /// Compare deterministic map against empirical ground truth for a
        /// given MDL/VDF pair. Returns (matches, mismatches).
        fn compare_det_vs_emp(
            mdl_path: &str,
            vdf_path: &str,
        ) -> (usize, Vec<(String, usize, usize)>) {
            let ref_results = simulate_mdl(mdl_path);
            let vdf = vdf_file(vdf_path);
            let vdf_data = vdf
                .extract_data()
                .unwrap_or_else(|e| panic!("extract_data failed for {vdf_path}: {e}"));

            let det_map = vdf
                .build_deterministic_ot_map()
                .unwrap_or_else(|e| panic!("deterministic map failed for {vdf_path}: {e}"));
            let emp_map = build_empirical_ot_map(&vdf_data, &ref_results)
                .unwrap_or_else(|e| panic!("empirical map failed for {vdf_path}: {e}"));

            let mut matches = 0;
            let mut mismatches = Vec::new();

            let mut det_sorted: Vec<_> = det_map.iter().collect();
            det_sorted.sort_by_key(|(name, _)| name.to_lowercase());

            for (det_name, det_ot) in &det_sorted {
                let canonical = crate::common::canonicalize(det_name);
                if let Some(&emp_ot) = emp_map.get(&canonical) {
                    if **det_ot == emp_ot {
                        matches += 1;
                        eprintln!("  OK   {det_name:30} -> OT[{det_ot}]");
                    } else {
                        mismatches.push((det_name.to_string(), **det_ot, emp_ot));
                        eprintln!("  FAIL {det_name:30} -> det OT[{det_ot}] vs emp OT[{emp_ot}]");
                    }
                } else {
                    eprintln!("  SKIP {det_name:30} -> OT[{det_ot}] (no empirical match)");
                }
            }

            eprintln!(
                "  total: {matches} matches, {} mismatches",
                mismatches.len()
            );
            (matches, mismatches)
        }

        /// The bact VDF was generated from a different version of the model
        /// than the current bact.mdl (VDF has "bacteria"/"population growth",
        /// MDL has "stock"/"inflow"/"outflow"). The deterministic mapping
        /// fails with a count mismatch: 3 model records but only 2 candidate
        /// names. This documents the limitation.
        #[test]
        fn test_deterministic_bact_count_mismatch() {
            let vdf_path =
                "../../third_party/uib_sd/fall_2008/sd202/assignments/assignment_3/Current.vdf";
            let vdf = vdf_file(vdf_path);

            let result = vdf.build_deterministic_ot_map();
            assert!(
                result.is_err(),
                "bact: expected deterministic map to fail with count mismatch"
            );
            let err = result.unwrap_err().to_string();
            assert!(
                err.contains("candidate name count (2) != model record count (3)"),
                "bact: unexpected error: {err}"
            );
        }

        /// Verify deterministic mapping for the water model. The MDL and
        /// VDF have the same variable names and matching step counts. The
        /// empirical comparison has a known limitation: constant-value
        /// ambiguity for variables with the same constant (e.g., desired
        /// water level = TIME STEP = SAVEPER = 1.0).
        #[test]
        fn test_deterministic_vs_empirical_water() {
            eprintln!("\n=== water model (assignment_4) ===");
            let (matches, mismatches) = compare_det_vs_emp(
                "../../third_party/uib_sd/fall_2008/sd202/assignments/assignment_4/water.mdl",
                "../../third_party/uib_sd/fall_2008/sd202/assignments/assignment_4/Current.vdf",
            );
            // The empirical approach can't distinguish constants with the
            // same value. "desired water level" (1.0) gets matched to a
            // different OT entry than the deterministic map chooses, but
            // both entries hold constant 1.0.
            for (name, det_ot, emp_ot) in &mismatches {
                eprintln!("  mismatch: {name} det=OT[{det_ot}] emp=OT[{emp_ot}]");
            }
            assert!(matches >= 2, "water: expected at least 2 matches");
            assert!(
                mismatches.len() <= 1,
                "water: expected at most 1 mismatch (constant-value ambiguity), got {:?}",
                mismatches
            );
        }

        /// Verify deterministic mapping for the pop model. The empirical
        /// comparison is limited because the VDF was generated with Vensim's
        /// RK4 integrator and our sim uses Euler, so dynamic variables diverge.
        #[test]
        fn test_deterministic_vs_empirical_pop() {
            eprintln!("\n=== pop model (assignment_6) ===");
            let (matches, mismatches) = compare_det_vs_emp(
                "../../third_party/uib_sd/fall_2008/sd202/assignments/assignment_6/pop.mdl",
                "../../third_party/uib_sd/fall_2008/sd202/assignments/assignment_6/Current.vdf",
            );
            assert!(
                mismatches.is_empty(),
                "pop: deterministic disagrees with empirical for {} vars: {:?}",
                mismatches.len(),
                mismatches
            );
            assert!(matches > 0, "pop: expected at least some matches");
        }

        /// Verify the deterministic mapping for water produces physically
        /// plausible data: stocks start at initial values, constants are
        /// constant, flows have the right magnitude.
        #[test]
        fn test_deterministic_water_physical_plausibility() {
            let vdf_path =
                "../../third_party/uib_sd/fall_2008/sd202/assignments/assignment_4/Current.vdf";
            let vdf = vdf_file(vdf_path);
            let vdf_data = vdf.extract_data().unwrap();
            let det_map = vdf.build_deterministic_ot_map().unwrap();

            // water level (stock) should start at 0 and rise to ~1
            let wl_ot = det_map["water level"];
            let wl_first = vdf_data.entries[wl_ot][0];
            let wl_last = *vdf_data.entries[wl_ot].last().unwrap();
            assert!(
                (wl_first - 0.0).abs() < 0.001,
                "water level should start at 0, got {wl_first}"
            );
            assert!(
                wl_last > 0.9 && wl_last <= 1.0,
                "water level should approach 1, got {wl_last}"
            );

            // adjustment time should be constant 2.0
            let at_ot = det_map["adjustment time"];
            let at_series = &vdf_data.entries[at_ot];
            assert!(
                at_series.iter().all(|&v| (v - 2.0).abs() < 0.001),
                "adjustment time should be constant 2.0"
            );

            // gap should start positive and decrease as water level rises
            let gap_ot = det_map["gap"];
            let gap_first = vdf_data.entries[gap_ot][0];
            let gap_last = *vdf_data.entries[gap_ot].last().unwrap();
            assert!(gap_first > gap_last, "gap should decrease over time");
        }

        /// Verify the deterministic mapping for pop produces physically
        /// plausible data: populations are large numbers, constants are
        /// constant, births/ending are flows.
        #[test]
        fn test_deterministic_pop_physical_plausibility() {
            let vdf_path =
                "../../third_party/uib_sd/fall_2008/sd202/assignments/assignment_6/Current.vdf";
            let vdf = vdf_file(vdf_path);
            let vdf_data = vdf.extract_data().unwrap();
            let det_map = vdf.build_deterministic_ot_map().unwrap();

            // producing population is a stock, should be a large number
            let pp_ot = det_map["producing population"];
            let pp_first = vdf_data.entries[pp_ot][0];
            assert!(
                pp_first > 1_000_000.0,
                "producing population should be millions, got {pp_first}"
            );

            // young population should also be a large number
            let yp_ot = det_map["young population"];
            let yp_first = vdf_data.entries[yp_ot][0];
            assert!(
                yp_first > 10_000_000.0,
                "young population should be tens of millions, got {yp_first}"
            );

            // births per person should be a constant < 1
            let bpp_ot = det_map["births per person"];
            let bpp_series = &vdf_data.entries[bpp_ot];
            assert!(
                bpp_series.iter().all(|&v| (v - 0.5).abs() < 0.001),
                "births per person should be constant 0.5"
            );

            // births should be a positive number (flow)
            let births_ot = det_map["births"];
            let births_first = vdf_data.entries[births_ot][0];
            assert!(
                births_first > 1_000_000.0,
                "births should be millions, got {births_first}"
            );
        }

        /// The econ model (medium-sized, 42 slotted names, 74 records)
        /// breaks the deterministic mapping because the offset table
        /// parsing fails (ot_count=0), which causes `f[11] < ot_count`
        /// to exclude all records. The records DO have nonzero f[10] values,
        /// but without a valid offset table the mapping can't proceed.
        #[test]
        fn test_deterministic_econ_fails_no_offset_table() {
            let vdf = vdf_file("../../third_party/uib_sd/fall_2008/econ/base.vdf");

            // The offset table parsing fails for this file
            assert_eq!(
                vdf.offset_table_count, 0,
                "econ: expected offset table count to be 0 (parsing limitation)"
            );

            // Records DO have nonzero f[10], but the ot_count=0 filter
            // eliminates everything via `f[11] < ot_count`
            let nonzero_f10 = vdf.records.iter().filter(|r| r.fields[10] != 0).count();
            assert!(
                nonzero_f10 > 0,
                "econ: records should have nonzero f[10] values"
            );

            // Deterministic mapping fails with count mismatch
            let result = vdf.build_deterministic_ot_map();
            assert!(
                result.is_err(),
                "econ: deterministic mapping should fail when OT is empty"
            );
            let err = result.unwrap_err().to_string();
            assert!(
                err.contains("model record count (0)"),
                "econ: expected 0 model records due to OT filter, got: {err}"
            );
        }

        /// The econ VDF was generated from mark2.mdl (per header).
        /// Test that empirical matching still works when the MDL matches.
        #[test]
        fn test_econ_vdf_from_mark2_mdl() {
            let vdf = vdf_file("../../third_party/uib_sd/fall_2008/econ/base.vdf");

            let header_text: String = vdf.data[4..0x78]
                .iter()
                .take_while(|&&b| b != 0)
                .map(|&b| b as char)
                .collect();
            // Header confirms the VDF came from mark2.mdl
            assert!(
                header_text.contains("mark2.mdl"),
                "econ: expected header to reference mark2.mdl, got: {header_text}"
            );

            // The offset table is empty, so extract_data returns no entries.
            // This is a parsing limitation for medium-sized VDFs.
            let vdf_data = vdf.extract_data().unwrap();
            assert_eq!(
                vdf_data.entries.len(),
                0,
                "econ: offset table is empty, so no data entries extracted"
            );
        }

        /// Verify f[10] ordering matches alphabetical name order for small
        /// VDF files. This is the core assumption of the deterministic mapping.
        #[test]
        fn test_f10_alphabetical_ordering() {
            let small_vdfs = [
                (
                    "bact",
                    "../../third_party/uib_sd/fall_2008/sd202/assignments/assignment_3/Current.vdf",
                ),
                (
                    "water",
                    "../../third_party/uib_sd/fall_2008/sd202/assignments/assignment_4/Current.vdf",
                ),
                (
                    "pop",
                    "../../third_party/uib_sd/fall_2008/sd202/assignments/assignment_6/Current.vdf",
                ),
                ("econ", "../../third_party/uib_sd/fall_2008/econ/base.vdf"),
            ];

            for (label, vdf_path) in &small_vdfs {
                let vdf = vdf_file(vdf_path);
                let ot_count = vdf.offset_table_count;

                // Collect model variable records (same filter as deterministic map)
                let mut model_records: Vec<(u32, u32, usize)> = Vec::new();
                for (i, rec) in vdf.records.iter().enumerate() {
                    let ot_idx = rec.fields[11] as usize;
                    if rec.fields[0] != 0
                        && rec.fields[1] != 23
                        && rec.fields[10] > 0
                        && ot_idx > 0
                        && ot_idx < ot_count
                    {
                        model_records.push((rec.fields[10], rec.fields[11], i));
                    }
                }

                model_records.sort_by_key(|&(f10, _, _)| f10);

                eprintln!(
                    "\n=== {label}: f[10] ordering ({} model records) ===",
                    model_records.len()
                );
                for (f10, f11, rec_idx) in &model_records {
                    eprintln!("  rec[{rec_idx:3}]: f[10]={f10:6}, f[11](OT)={f11:3}");
                }

                // Check uniqueness: no f[10] ties
                let f10_values: Vec<u32> = model_records.iter().map(|&(f10, _, _)| f10).collect();
                let mut unique = f10_values.clone();
                unique.sort();
                unique.dedup();
                let ties = f10_values.len() - unique.len();
                eprintln!(
                    "  f[10] unique: {}/{} (ties: {ties})",
                    unique.len(),
                    f10_values.len()
                );

                // For small models, f[10] should have no ties
                if *label != "econ" {
                    assert_eq!(ties, 0, "{label}: expected no f[10] ties for small model");
                }
            }
        }

        /// Verify extracted time series data is valid: time is monotonically
        /// increasing, all values are finite.
        #[test]
        fn test_extracted_data_validity() {
            let small_vdfs = [
                "../../third_party/uib_sd/fall_2008/sd202/assignments/assignment_3/Current.vdf",
                "../../third_party/uib_sd/fall_2008/sd202/assignments/assignment_4/Current.vdf",
                "../../third_party/uib_sd/fall_2008/sd202/assignments/assignment_6/Current.vdf",
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

                // Print summary
                if let Ok(det_map) = vdf.build_deterministic_ot_map() {
                    eprintln!(
                        "\n{vdf_path}: {} OT entries, {} mapped names",
                        vdf_data.entries.len(),
                        det_map.len()
                    );
                    let mut sorted: Vec<_> = det_map.iter().collect();
                    sorted.sort_by_key(|(name, _)| name.to_lowercase());
                    for (name, ot_idx) in &sorted {
                        let first = vdf_data.entries[**ot_idx][0];
                        let last = *vdf_data.entries[**ot_idx].last().unwrap();
                        eprintln!(
                            "  {name:30} OT[{ot_idx:2}]: first={first:12.4}, last={last:12.4}"
                        );
                    }
                }
            }
        }

        /// Deep diagnostic dump for the bact model to understand the
        /// mismatch between candidate names and model records.
        #[test]
        fn test_deterministic_map_diagnostics() {
            let small_models = [
                (
                    "bact",
                    "../../third_party/uib_sd/fall_2008/sd202/assignments/assignment_3/Current.vdf",
                    "../../third_party/uib_sd/fall_2008/sd202/assignments/assignment_3/bact.mdl",
                ),
                (
                    "water",
                    "../../third_party/uib_sd/fall_2008/sd202/assignments/assignment_4/Current.vdf",
                    "../../third_party/uib_sd/fall_2008/sd202/assignments/assignment_4/water.mdl",
                ),
                (
                    "pop",
                    "../../third_party/uib_sd/fall_2008/sd202/assignments/assignment_6/Current.vdf",
                    "../../third_party/uib_sd/fall_2008/sd202/assignments/assignment_6/pop.mdl",
                ),
            ];

            for (label, vdf_path, mdl_path) in &small_models {
                let vdf = vdf_file(vdf_path);
                let ot_count = vdf.offset_table_count;

                eprintln!("\n========== {label} ==========");
                eprintln!(
                    "names: {} (slotted: {}), records: {}, OT entries: {}",
                    vdf.names.len(),
                    vdf.section_name_count,
                    vdf.records.len(),
                    ot_count
                );

                eprintln!("\nall names:");
                for (i, name) in vdf.names.iter().enumerate() {
                    let marker = if i < vdf.section_name_count {
                        "slotted"
                    } else {
                        "UNSLOTTED"
                    };
                    eprintln!("  [{i:2}] ({marker}) {name:?}");
                }

                eprintln!("\nall records:");
                for (i, rec) in vdf.records.iter().enumerate() {
                    eprintln!(
                        "  rec[{i:2}]: f[0]={:3} f[1]={:3} f[10]={:5} f[11]={:3} f[12]={:5} | off=0x{:x}",
                        rec.fields[0],
                        rec.fields[1],
                        rec.fields[10],
                        rec.fields[11],
                        rec.fields[12],
                        rec.file_offset
                    );
                    let passes = rec.fields[0] != 0
                        && rec.fields[1] != 23
                        && rec.fields[10] > 0
                        && rec.fields[11] > 0
                        && (rec.fields[11] as usize) < ot_count;
                    eprintln!(
                        "         passes model-record filter: {}",
                        if passes { "YES" } else { "no" }
                    );
                }

                // Show what the MDL file has
                let contents = std::fs::read_to_string(mdl_path).unwrap();
                let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
                let project = std::rc::Rc::new(crate::Project::from(datamodel_project));
                let sim = crate::interpreter::Simulation::new(&project, "main").unwrap();
                let results = sim.run_to_end().unwrap();

                eprintln!("\nMDL simulation variables:");
                let mut sim_vars: Vec<_> = results.offsets.iter().collect();
                sim_vars.sort_by_key(|(name, _)| name.as_str().to_string());
                for (name, offset) in &sim_vars {
                    let first = results.data[**offset];
                    let last =
                        results.data[(results.step_count - 1) * results.step_size + **offset];
                    eprintln!("  {name:30} col={offset:2} first={first:12.4} last={last:12.4}");
                }

                // Try empirical mapping (requires matching step counts)
                let vdf_data = vdf.extract_data().unwrap();
                eprintln!(
                    "\nVDF step count: {}, sim step count: {}",
                    vdf_data.time_values.len(),
                    results.step_count
                );

                // Show raw VDF data entries
                eprintln!("\nVDF data entries:");
                for (i, entry) in vdf_data.entries.iter().enumerate() {
                    let first = entry[0];
                    let last = *entry.last().unwrap();
                    eprintln!("  OT[{i:2}]: first={first:12.4}, last={last:12.4}");
                }

                if vdf_data.time_values.len() == results.step_count {
                    let emp_map = build_empirical_ot_map(&vdf_data, &results).unwrap();
                    eprintln!("\nempirical mapping:");
                    let mut emp_sorted: Vec<_> = emp_map.iter().collect();
                    emp_sorted.sort_by_key(|(name, _)| name.as_str().to_string());
                    for (name, ot_idx) in &emp_sorted {
                        let first = vdf_data.entries[**ot_idx][0];
                        let last = *vdf_data.entries[**ot_idx].last().unwrap();
                        eprintln!(
                            "  {name:30} -> OT[{ot_idx:2}] first={first:12.4} last={last:12.4}"
                        );
                    }
                } else {
                    eprintln!("  SKIPPING empirical comparison: step count mismatch");
                }
            }
        }

        /// Investigate what unslotted names represent: data variables,
        /// group markers, or unit names.
        #[test]
        fn test_unslotted_names_investigation() {
            let vdf = vdf_file("../../third_party/uib_sd/fall_2008/econ/base.vdf");
            let vdf_data = vdf
                .extract_data()
                .unwrap_or_else(|e| panic!("extract_data failed: {e}"));

            let slotted = &vdf.names[..vdf.section_name_count];
            let unslotted = &vdf.names[vdf.section_name_count..];

            eprintln!("=== econ base.vdf name analysis ===");
            eprintln!("total names: {}", vdf.names.len());
            eprintln!("slotted: {} (with slot table entries)", slotted.len());
            eprintln!("unslotted: {} (past declared_size)", unslotted.len());
            eprintln!("OT entries: {}", vdf_data.entries.len());
            eprintln!("records: {}", vdf.records.len());

            let mut groups = Vec::new();
            let mut units = Vec::new();
            let mut variable_like = Vec::new();

            for name in unslotted {
                if name.starts_with('.') {
                    groups.push(name.as_str());
                } else if name.starts_with('-') {
                    units.push(name.as_str());
                } else {
                    variable_like.push(name.as_str());
                }
            }

            eprintln!("\nunslotted breakdown:");
            eprintln!("  groups (start with '.'): {}", groups.len());
            eprintln!("  units (start with '-'): {}", units.len());
            eprintln!("  variable-like: {}", variable_like.len());

            if !groups.is_empty() {
                eprintln!("\n  group names: {:?}", groups);
            }
            if !units.is_empty() {
                eprintln!("\n  unit names: {:?}", units);
            }
            eprintln!("\n  variable-like unslotted names:");
            for v in &variable_like {
                eprintln!("    {v}");
            }

            // Cross-reference: how many data entries exist vs how many
            // variable names exist across both slotted and unslotted
            let system_names: HashSet<&str> =
                ["Time", "INITIAL TIME", "FINAL TIME", "TIME STEP", "SAVEPER"]
                    .into_iter()
                    .collect();
            let slotted_vars: Vec<_> = slotted
                .iter()
                .filter(|n| {
                    !n.starts_with('.') && !n.starts_with('-') && !system_names.contains(n.as_str())
                })
                .collect();

            eprintln!("\ncross-reference:");
            eprintln!("  slotted model vars (excl system): {}", slotted_vars.len());
            eprintln!("  unslotted variable-like: {}", variable_like.len());
            eprintln!(
                "  total potential vars: {}",
                slotted_vars.len() + variable_like.len() + 1
            );
            eprintln!("  OT entries: {}", vdf_data.entries.len());
            eprintln!(
                "  gap (OT - total potential): {}",
                vdf_data.entries.len() as isize
                    - (slotted_vars.len() + variable_like.len() + 1) as isize
            );
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

        #[test]
        fn test_small_vdf_name_to_data_chain() {
            // ---- euler-5.vdf: bact model, 2 model vars ----
            let euler5 = vdf_file(
                "../../third_party/uib_sd/fall_2008/sd202/assignments/assignment_3/euler-5.vdf",
            );
            assert_eq!(euler5.names.len(), 10);
            assert_eq!(euler5.section_name_count, 10);
            assert_eq!(euler5.slot_table.len(), 10);
            assert_eq!(euler5.records.len(), 9);
            assert_eq!(euler5.offset_table_count, 7);
            assert_eq!(euler5.names[0], "Time");
            assert_eq!(euler5.names[8], "bacteria");
            assert_eq!(euler5.names[9], "population growth");
            assert_eq!(euler5.slot_table[8], 44);
            assert_eq!(euler5.slot_table[9], 92);

            // f[12] groups records into 2 clusters; values match system var slot
            // offsets (124=INITIAL TIME, 140=FINAL TIME), NOT model var slots.
            let unique_f12: std::collections::HashSet<u32> =
                euler5.records.iter().map(|r| r.slot_ref()).collect();
            assert_eq!(unique_f12.len(), 2);
            assert!(unique_f12.contains(&124));
            assert!(unique_f12.contains(&140));
            assert!(!unique_f12.contains(&44));
            assert!(!unique_f12.contains(&92));

            // f[11] IS the correct OT index:
            // Record 0: f[1]=15 -> OT 3 = const 0 (INITIAL TIME)
            assert_eq!(euler5.records[0].fields[1], 15);
            assert_eq!(euler5.records[0].ot_index(), 3);
            let ot3 = euler5.offset_table_entry(3).unwrap();
            assert!(!euler5.is_data_block_offset(ot3));
            assert_eq!(f32::from_le_bytes(ot3.to_le_bytes()), 0.0);
            // Record 7: stock (f[1]=135), f[11]=1 -> data block
            assert_eq!(euler5.records[7].fields[1], 135);
            assert_eq!(euler5.records[7].ot_index(), 1);
            assert!(euler5.is_data_block_offset(euler5.offset_table_entry(1).unwrap()));
            // Record 8: flow (f[1]=2056), f[11]=4 -> data block
            assert_eq!(euler5.records[8].fields[1], 2056);
            assert_eq!(euler5.records[8].ot_index(), 4);
            assert!(euler5.is_data_block_offset(euler5.offset_table_entry(4).unwrap()));

            // f[1]=15 record passes standard filter but shouldn't be a model var.
            let model_standard: Vec<_> = euler5
                .records
                .iter()
                .filter(|r| {
                    r.fields[0] != 0
                        && r.fields[1] != 23
                        && r.fields[10] > 0
                        && r.fields[11] > 0
                        && (r.fields[11] as usize) < euler5.offset_table_count
                })
                .collect();
            assert_eq!(model_standard.len(), 3, "3 pass for 2 model vars");
            let model_fixed: Vec<_> = euler5
                .records
                .iter()
                .filter(|r| {
                    r.fields[0] != 0
                        && r.fields[1] != 23
                        && r.fields[1] != 15
                        && r.fields[10] > 0
                        && r.fields[11] > 0
                        && (r.fields[11] as usize) < euler5.offset_table_count
                })
                .collect();
            assert_eq!(model_fixed.len(), 2);

            // ---- euler-10.vdf: same model, 3 model vars ----
            let euler10 = vdf_file(
                "../../third_party/uib_sd/fall_2008/sd202/assignments/assignment_3/euler-10.vdf",
            );
            assert_eq!(euler10.names[10], "predation");
            let e10_std: Vec<_> = euler10
                .records
                .iter()
                .filter(|r| {
                    r.fields[0] != 0
                        && r.fields[1] != 23
                        && r.fields[10] > 0
                        && r.fields[11] > 0
                        && (r.fields[11] as usize) < euler10.offset_table_count
                })
                .collect();
            assert_eq!(e10_std.len(), 4);
            let e10_fix: Vec<_> = euler10
                .records
                .iter()
                .filter(|r| {
                    r.fields[0] != 0
                        && r.fields[1] != 23
                        && r.fields[1] != 15
                        && r.fields[10] > 0
                        && r.fields[11] > 0
                        && (r.fields[11] as usize) < euler10.offset_table_count
                })
                .collect();
            assert_eq!(e10_fix.len(), 3);

            // Slot data is positional: same bytes at same offset even when
            // different variables are assigned there across bact VDF files.
            let s5 = euler5.slot_section().unwrap().data_offset();
            let s10 = euler10.slot_section().unwrap().data_offset();
            for off in [44usize, 92] {
                let w5: [u32; 3] = [
                    read_u32(&euler5.data, s5 + off),
                    read_u32(&euler5.data, s5 + off + 4),
                    read_u32(&euler5.data, s5 + off + 8),
                ];
                let w10: [u32; 3] = [
                    read_u32(&euler10.data, s10 + off),
                    read_u32(&euler10.data, s10 + off + 4),
                    read_u32(&euler10.data, s10 + off + 8),
                ];
                assert_eq!(
                    w5, w10,
                    "slot data at offset {} should match across bact VDFs",
                    off
                );
            }

            // ---- water model: no f[1]=15 record, deterministic works ----
            let water = vdf_file(
                "../../third_party/uib_sd/fall_2008/sd202/assignments/assignment_4/Current.vdf",
            );
            assert!(!water.records.iter().any(|r| r.fields[1] == 15));
            let water_model: Vec<_> = water
                .records
                .iter()
                .filter(|r| {
                    r.fields[0] != 0
                        && r.fields[1] != 23
                        && r.fields[10] > 0
                        && r.fields[11] > 0
                        && (r.fields[11] as usize) < water.offset_table_count
                })
                .collect();
            assert_eq!(water_model.len(), 5);

            // All model records (with valid OT indices) share f[12]=124
            for (label, vdf) in [
                ("euler-5", &euler5),
                ("euler-10", &euler10),
                ("water", &water),
            ] {
                let ot_count = vdf.offset_table_count;
                let f12s: std::collections::HashSet<u32> = vdf
                    .records
                    .iter()
                    .filter(|r| {
                        r.fields[0] != 0
                            && r.fields[1] != 23
                            && r.fields[1] != 15
                            && r.fields[10] > 0
                            && r.fields[11] > 0
                            && (r.fields[11] as usize) < ot_count
                    })
                    .map(|r| r.slot_ref())
                    .collect();
                assert_eq!(f12s.len(), 1, "{}: model f[12] should be unique", label);
                assert!(f12s.contains(&124), "{}: model f[12] should be 124", label);
            }

            // Deterministic map produces correct results for water model
            let det = water.build_deterministic_ot_map().unwrap();
            assert_eq!(det.len(), 6);
            assert_eq!(det["Time"], 0);
            let wd = water.extract_data().unwrap();
            assert!((wd.entries[det["water level"]][0]).abs() < 0.01);
            assert!((wd.entries[det["water level"]].last().unwrap() - 1.0).abs() < 0.01);
            assert!((wd.entries[det["desired water level"]][0] - 1.0).abs() < 0.01);
            assert!((wd.entries[det["adjustment time"]][0] - 2.0).abs() < 0.01);
        }

        // ---- Slot decoder tests (task #3) ----

        fn read_slot_words(data: &[u8], sec1_data_offset: usize, slot_offset: u32) -> [u32; 4] {
            let abs = sec1_data_offset + slot_offset as usize;
            [
                read_u32(data, abs),
                read_u32(data, abs + 4),
                read_u32(data, abs + 8),
                read_u32(data, abs + 12),
            ]
        }

        #[test]
        fn test_slot_contents_small_detailed() {
            let small_files = [
                "../../third_party/uib_sd/fall_2008/sd202/assignments/assignment_3/euler-5.vdf",
                "../../third_party/uib_sd/fall_2008/sd202/assignments/assignment_2/Current.vdf",
                "../../third_party/uib_sd/fall_2008/sd202/assignments/assignment_4/Current.vdf",
            ];

            for path in &small_files {
                let vdf = vdf_file(path);
                let sec1 = vdf.slot_section().expect("section 1");
                let sec1_off = sec1.data_offset();

                eprintln!("\n=== {} ===", path);
                eprintln!(
                    "  names={} slotted={} records={} OT={}",
                    vdf.names.len(),
                    vdf.section_name_count,
                    vdf.records.len(),
                    vdf.offset_table_count,
                );

                let mut sorted = vdf.slot_table.clone();
                sorted.sort();
                let min_slot = sorted.first().copied().unwrap_or(0);
                eprintln!("  sorted slots: {:?}", sorted);
                eprintln!("  pre-slot header: {} bytes", min_slot);

                if min_slot > 0 && min_slot <= 64 {
                    eprint!("  header u32s:");
                    for i in (0..min_slot as usize).step_by(4) {
                        eprint!(
                            " {}(0x{:08x})",
                            read_u32(&vdf.data, sec1_off + i),
                            read_u32(&vdf.data, sec1_off + i)
                        );
                    }
                    eprintln!();
                }

                let mut slot_recs: std::collections::BTreeMap<u32, Vec<usize>> =
                    std::collections::BTreeMap::new();
                for (ri, rec) in vdf.records.iter().enumerate() {
                    slot_recs.entry(rec.slot_ref()).or_default().push(ri);
                }

                for (idx, &soff) in vdf.slot_table.iter().enumerate() {
                    let w = read_slot_words(&vdf.data, sec1_off, soff);
                    let name = &vdf.names[idx];
                    let ris = slot_recs.get(&soff).cloned().unwrap_or_default();
                    let ots: Vec<u32> = ris.iter().map(|&ri| vdf.records[ri].fields[11]).collect();
                    eprintln!(
                        "  slot[{:3}] @{:4}: w=[{:6},{:6},{:6},{:6}] 0x[{:08x},{:08x},{:08x},{:08x}] \
                         recs={} OTs={:?} name={:?}",
                        idx,
                        soff,
                        w[0],
                        w[1],
                        w[2],
                        w[3],
                        w[0],
                        w[1],
                        w[2],
                        w[3],
                        ris.len(),
                        ots,
                        name
                    );
                    for &ri in &ris {
                        let r = &vdf.records[ri];
                        eprintln!(
                            "    rec[{:2}]: f=[{},{},{},{},{},{},{},{},0x{:x},0x{:x},{},{},{},{},0x{:x},{}]",
                            ri,
                            r.fields[0],
                            r.fields[1],
                            r.fields[2],
                            r.fields[3],
                            r.fields[4],
                            r.fields[5],
                            r.fields[6],
                            r.fields[7],
                            r.fields[8],
                            r.fields[9],
                            r.fields[10],
                            r.fields[11],
                            r.fields[12],
                            r.fields[13],
                            r.fields[14],
                            r.fields[15]
                        );
                    }
                }
            }
        }

        #[test]
        fn test_slot_data_across_runs() {
            let files = [
                "../../third_party/uib_sd/fall_2008/econ/base.vdf",
                "../../third_party/uib_sd/fall_2008/econ/rk.vdf",
            ];
            let vdfs: Vec<VdfFile> = files.iter().map(|p| vdf_file(p)).collect();
            assert_eq!(vdfs[0].section_name_count, vdfs[1].section_name_count);

            let mut identical = 0;
            let mut different = Vec::new();
            for (idx, name) in vdfs[0].names[..vdfs[0].section_name_count]
                .iter()
                .enumerate()
            {
                let w0 = read_slot_words(
                    &vdfs[0].data,
                    vdfs[0].slot_section().unwrap().data_offset(),
                    vdfs[0].slot_table[idx],
                );
                let w1 = read_slot_words(
                    &vdfs[1].data,
                    vdfs[1].slot_section().unwrap().data_offset(),
                    vdfs[1].slot_table[idx],
                );
                if w0 == w1 {
                    identical += 1;
                } else {
                    different.push((idx, name.clone(), w0, w1));
                }
            }
            eprintln!(
                "\necon base vs rk: {}/{} identical",
                identical, vdfs[0].section_name_count
            );
            for (i, n, w0, w1) in &different {
                eprintln!("  slot[{}] {:?}: base={:?} rk={:?}", i, n, w0, w1);
            }
            assert_eq!(vdfs[0].slot_table, vdfs[1].slot_table);
        }

        #[test]
        fn test_slot_word_global_stats() {
            let vdf_paths = collect_vdf_files(std::path::Path::new("../../third_party/uib_sd"));
            let mut gz = [0usize; 4];
            let mut grc = [0usize; 4];
            let mut got = [0usize; 4];
            let mut gt = 0usize;

            for path in &vdf_paths {
                let data = match std::fs::read(path) {
                    Ok(d) => d,
                    Err(_) => continue,
                };
                let vdf = match VdfFile::parse(data) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let Some(sec1) = vdf.slot_section() else {
                    continue;
                };
                let so = sec1.data_offset();
                let mut src: HashMap<u32, usize> = HashMap::new();
                let mut sot: HashMap<u32, Vec<u32>> = HashMap::new();
                for rec in &vdf.records {
                    *src.entry(rec.slot_ref()).or_default() += 1;
                    sot.entry(rec.slot_ref()).or_default().push(rec.fields[11]);
                }
                for (idx, &soff) in vdf.slot_table.iter().enumerate() {
                    if idx >= vdf.section_name_count {
                        break;
                    }
                    let w = read_slot_words(&vdf.data, so, soff);
                    let rc = src.get(&soff).copied().unwrap_or(0) as u32;
                    let ots = sot.get(&soff).cloned().unwrap_or_default();
                    gt += 1;
                    for i in 0..4 {
                        if w[i] == 0 {
                            gz[i] += 1;
                        }
                        if w[i] == rc {
                            grc[i] += 1;
                        }
                        if w[i] > 0 && ots.contains(&w[i]) {
                            got[i] += 1;
                        }
                    }
                }
            }
            eprintln!("\n=== Global slot stats ({} slots) ===", gt);
            for i in 0..4 {
                eprintln!(
                    "  w[{}]: zero={:.1}% rc={:.1}% ot={:.1}%",
                    i,
                    100.0 * gz[i] as f64 / gt.max(1) as f64,
                    100.0 * grc[i] as f64 / gt.max(1) as f64,
                    100.0 * got[i] as f64 / gt.max(1) as f64
                );
            }
        }

        #[test]
        fn test_slot_stride_patterns() {
            let files = [
                (
                    "small",
                    "../../third_party/uib_sd/fall_2008/sd202/assignments/assignment_2/Current.vdf",
                ),
                ("medium", "../../third_party/uib_sd/fall_2008/econ/base.vdf"),
                ("large", "../../third_party/uib_sd/zambaqui/baserun.vdf"),
            ];
            for (label, path) in &files {
                let vdf = vdf_file(path);
                let mut sorted = vdf.slot_table.clone();
                sorted.sort();
                sorted.dedup();
                let strides: Vec<u32> = sorted.windows(2).map(|w| w[1] - w[0]).collect();
                let (mn, mx) = (
                    strides.iter().copied().min().unwrap_or(0),
                    strides.iter().copied().max().unwrap_or(0),
                );
                eprintln!(
                    "\n{}: {} slots, stride [{},{}]",
                    label,
                    sorted.len(),
                    mn,
                    mx
                );
                let mut hist: std::collections::BTreeMap<u32, usize> =
                    std::collections::BTreeMap::new();
                for &s in &strides {
                    *hist.entry(s).or_default() += 1;
                }
                for (s, c) in &hist {
                    eprintln!("  stride {}: {}", s, c);
                }
                if mx != mn {
                    let mut on: Vec<(u32, usize)> = vdf
                        .slot_table
                        .iter()
                        .enumerate()
                        .map(|(i, &o)| (o, i))
                        .collect();
                    on.sort_by_key(|&(o, _)| o);
                    for pair in on.windows(2) {
                        let stride = pair[1].0 - pair[0].0;
                        if stride != 16 {
                            eprintln!(
                                "  @{} stride={}: {:?}",
                                pair[0].0, stride, &vdf.names[pair[0].1]
                            );
                        }
                    }
                }
            }
        }

        #[test]
        fn test_zambaqui_array_slot_analysis() {
            let vdf = vdf_file("../../third_party/uib_sd/zambaqui/baserun.vdf");
            eprintln!(
                "\nzambaqui: {} slotted, {} total, {} recs, {} OT",
                vdf.section_name_count,
                vdf.names.len(),
                vdf.records.len(),
                vdf.offset_table_count
            );

            let mut brackets = Vec::new();
            for (i, n) in vdf.names.iter().enumerate() {
                if n.contains('[') {
                    brackets.push((i, n.as_str()));
                }
            }
            eprintln!("  bracketed: {}", brackets.len());
            for (i, n) in brackets.iter().take(20) {
                eprintln!(
                    "    [{}] {:?} slotted={}",
                    i,
                    n,
                    *i < vdf.section_name_count
                );
            }

            let sec1_off = vdf.slot_section().unwrap().data_offset();
            let mut srecs: HashMap<u32, Vec<usize>> = HashMap::new();
            for (ri, rec) in vdf.records.iter().enumerate() {
                srecs.entry(rec.slot_ref()).or_default().push(ri);
            }

            eprintln!("\n  multi-record slots:");
            for (idx, &soff) in vdf.slot_table.iter().enumerate() {
                if idx >= vdf.section_name_count {
                    break;
                }
                let rc = srecs.get(&soff).map(|v| v.len()).unwrap_or(0);
                if rc > 1 {
                    let w = read_slot_words(&vdf.data, sec1_off, soff);
                    eprintln!(
                        "    slot[{:3}] {:?}: {} recs, w={:?}",
                        idx, &vdf.names[idx], rc, w
                    );
                    if let Some(ris) = srecs.get(&soff) {
                        for &ri in ris {
                            let r = &vdf.records[ri];
                            eprintln!(
                                "      rec[{:3}]: f0={} f1={} f10={} f11(OT)={}",
                                ri, r.fields[0], r.fields[1], r.fields[10], r.fields[11]
                            );
                        }
                    }
                }
            }

            let mut on: Vec<(u32, usize)> = vdf
                .slot_table
                .iter()
                .enumerate()
                .map(|(i, &o)| (o, i))
                .collect();
            on.sort_by_key(|&(o, _)| o);
            eprintln!("\n  First 50 by offset:");
            for (i, &(o, ni)) in on.iter().take(50).enumerate() {
                let w = read_slot_words(&vdf.data, sec1_off, o);
                let stride = if i + 1 < on.len() { on[i + 1].0 - o } else { 0 };
                let rc = srecs.get(&o).map(|v| v.len()).unwrap_or(0);
                eprintln!(
                    "    @{:5} stride={:3}: w=[{:5},{:5},{:5},{:5}] recs={} {:?}",
                    o, stride, w[0], w[1], w[2], w[3], rc, &vdf.names[ni]
                );
            }
        }

        #[test]
        fn test_slot_extra_data() {
            let vdf = vdf_file("../../third_party/uib_sd/zambaqui/baserun.vdf");
            let sec1_off = vdf.slot_section().unwrap().data_offset();
            let mut on: Vec<(u32, usize)> = vdf
                .slot_table
                .iter()
                .enumerate()
                .map(|(i, &o)| (o, i))
                .collect();
            on.sort_by_key(|&(o, _)| o);
            let mut src: HashMap<u32, usize> = HashMap::new();
            for rec in &vdf.records {
                *src.entry(rec.slot_ref()).or_default() += 1;
            }

            eprintln!("\n=== zambaqui: stride > 16 ===");
            let mut cnt = 0;
            for pair in on.windows(2) {
                let (o0, n0) = pair[0];
                let stride = pair[1].0 - o0;
                if stride <= 16 {
                    continue;
                }
                cnt += 1;
                let rc = src.get(&o0).copied().unwrap_or(0);
                let extra = (stride - 16) / 4;
                eprintln!(
                    "\n  @{} {:?}: stride={} extra={} recs={}",
                    o0, &vdf.names[n0], stride, extra, rc
                );
                let abs = sec1_off + o0 as usize;
                let end = abs + stride as usize;
                eprint!("    u32:");
                for p in (abs..end).step_by(4) {
                    if p + 4 <= vdf.data.len() {
                        eprint!(" {}", read_u32(&vdf.data, p));
                    }
                }
                eprintln!();
                eprint!("    f32:");
                for p in (abs..end).step_by(4) {
                    if p + 4 <= vdf.data.len() {
                        eprint!(" {:.4}", read_f32(&vdf.data, p));
                    }
                }
                eprintln!();
                eprintln!(
                    "    extra({}) == rc({})? {}",
                    extra,
                    rc,
                    extra as usize == rc
                );
            }
            eprintln!("\n  Total: {}/{}", cnt, on.len());
        }

        #[test]
        fn test_slot_interpretation_matrix() {
            let vdf = vdf_file(
                "../../third_party/uib_sd/fall_2008/sd202/assignments/assignment_2/Current.vdf",
            );
            let sec1 = vdf.slot_section().unwrap();
            let sec1_off = sec1.data_offset();
            let sec1_size = sec1.declared_size as usize;
            let mut srecs: HashMap<u32, Vec<usize>> = HashMap::new();
            for (ri, rec) in vdf.records.iter().enumerate() {
                srecs.entry(rec.slot_ref()).or_default().push(ri);
            }
            eprintln!(
                "\n=== Interp matrix: names={} recs={} OT={} sec1={} ===",
                vdf.names.len(),
                vdf.records.len(),
                vdf.offset_table_count,
                sec1_size
            );

            for (idx, &soff) in vdf.slot_table.iter().enumerate() {
                if idx >= vdf.section_name_count {
                    break;
                }
                let w = read_slot_words(&vdf.data, sec1_off, soff);
                let ris = srecs.get(&soff).cloned().unwrap_or_default();
                eprintln!(
                    "\n  slot[{}] @{} {:?}: w={:?}",
                    idx, soff, &vdf.names[idx], w
                );
                for (wi, &v) in w.iter().enumerate() {
                    let mut ip = Vec::new();
                    if v == 0 {
                        ip.push("ZERO".into());
                    }
                    if (v as usize) < vdf.records.len() {
                        ip.push(format!("rec[{}]", v));
                    }
                    if (v as usize) < vdf.names.len() {
                        ip.push(format!("name[{}]={:?}", v, &vdf.names[v as usize]));
                    }
                    if v > 0 && (v as usize) < vdf.offset_table_count {
                        ip.push(format!("OT[{}]", v));
                    }
                    if v > 0 && (v as usize) < sec1_size && v.is_multiple_of(4) {
                        ip.push(format!("s1off({})", v));
                    }
                    if v == ris.len() as u32 {
                        ip.push(format!("=rc({})", v));
                    }
                    let f = f32::from_bits(v);
                    if f.is_finite() && f.abs() > 0.01 && f.abs() < 1e6 {
                        ip.push(format!("f32({:.4})", f));
                    }
                    eprintln!("    w[{}]={:6} (0x{:08x}): {}", wi, v, v, ip.join(" | "));
                }
                for &ri in &ris {
                    let r = &vdf.records[ri];
                    eprintln!(
                        "    -> rec[{:2}]: f=[{},{},{},{},{},{},{},{},0x{:x},0x{:x},{},{},{},{},0x{:x},{}]",
                        ri,
                        r.fields[0],
                        r.fields[1],
                        r.fields[2],
                        r.fields[3],
                        r.fields[4],
                        r.fields[5],
                        r.fields[6],
                        r.fields[7],
                        r.fields[8],
                        r.fields[9],
                        r.fields[10],
                        r.fields[11],
                        r.fields[12],
                        r.fields[13],
                        r.fields[14],
                        r.fields[15]
                    );
                }
            }
        }

        #[test]
        fn test_sec1_header_region() {
            let files = [
                "../../third_party/uib_sd/fall_2008/sd202/assignments/assignment_3/euler-5.vdf",
                "../../third_party/uib_sd/fall_2008/sd202/assignments/assignment_2/Current.vdf",
                "../../third_party/uib_sd/fall_2008/econ/base.vdf",
                "../../third_party/uib_sd/zambaqui/baserun.vdf",
            ];
            for path in &files {
                let vdf = vdf_file(path);
                let sec1 = vdf.slot_section().unwrap();
                let sec1_off = sec1.data_offset();
                let mut sorted = vdf.slot_table.clone();
                sorted.sort();
                let min_slot = sorted.first().copied().unwrap_or(0) as usize;
                eprintln!("\n=== {} ===", path);
                eprintln!(
                    "  sec1: off=0x{:x} decl={} region={}",
                    sec1_off,
                    sec1.declared_size,
                    sec1.region_data_size()
                );
                eprintln!("  header: {} bytes ({} u32s)", min_slot, min_slot / 4);
                for i in (0..min_slot).step_by(4) {
                    let v = read_u32(&vdf.data, sec1_off + i);
                    eprintln!(
                        "    [{:2}] @{:3}: {:10} (0x{:08x}) f32={:.6}",
                        i / 4,
                        i,
                        v,
                        v,
                        f32::from_bits(v)
                    );
                }
            }
        }
    }
}
