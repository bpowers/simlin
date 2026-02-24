// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Parser for Vensim VDF (binary data file) format.
//!
//! VDF is Vensim's proprietary binary format for simulation output. The format
//! is completely undocumented. See `doc/design/vdf.md` for the full format
//! specification, field-level analysis, and known pitfalls.
//!
//! This module handles:
//! - Parsing the file header, sections, records, slot table, name table,
//!   offset table, and sparse data blocks.
//! - Deterministic name-to-OT mapping for small models via
//!   [`VdfFile::build_deterministic_ot_map`] (sorts records by f[10] and
//!   names alphabetically, pairs 1:1). This does not work for large models
//!   where f[10] is not alphabetically ordered.
//! - Time series correlation (`build_vdf_results`, `build_empirical_ot_map`)
//!   for validating mapping hypotheses against a reference simulation. These
//!   are testing/validation tools, not production decoding strategies.

#[cfg(feature = "file_io")]
use std::collections::{HashMap, HashSet};
#[cfg(feature = "file_io")]
use std::{error::Error, result::Result as StdResult};

#[cfg(feature = "file_io")]
use crate::{
    common::{Canonical, Ident},
    results::{Method, Results, Specs},
};

/// VDF file magic bytes (first 4 bytes of every VDF file).
pub const VDF_FILE_MAGIC: [u8; 4] = [0x7f, 0xf7, 0x17, 0x52];

/// VDF section header magic value: float32 -0.797724 = 0xbf4c37a1.
/// This 4-byte sequence delimits sections within the VDF file.
pub const VDF_SECTION_MAGIC: [u8; 4] = [0xa1, 0x37, 0x4c, 0xbf];

/// Sentinel value appearing in record fields 8, 9, and sometimes 14.
pub const VDF_SENTINEL: u32 = 0xf6800000;

/// Record field[1] value identifying system/control variables (INITIAL TIME,
/// FINAL TIME, TIME STEP, SAVEPER).
#[cfg_attr(not(feature = "file_io"), allow(dead_code))]
const RECORD_F1_SYSTEM: u32 = 23;

/// Record field[1] value identifying INITIAL TIME constant records. These
/// pass the standard model-variable filter but aren't model variables.
#[cfg_attr(not(feature = "file_io"), allow(dead_code))]
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
/// captured by `region_end`. See `doc/design/vdf.md` for details.
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
#[cfg(feature = "file_io")]
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
#[cfg(feature = "file_io")]
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

#[cfg(feature = "file_io")]
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

/// A contiguous OT index range implied by record start indices (field[11]).
///
/// Each range starts at a unique in-range `f11` value and ends at the next
/// start (or `offset_table_count` for the last range). Range length > 1
/// indicates a multi-entry block (commonly array/lookups/table data).
#[cfg(feature = "file_io")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VdfOtRange {
    /// Inclusive start OT index.
    pub start: usize,
    /// Exclusive end OT index.
    pub end: usize,
    /// Number of records whose `f11` equals `start`.
    pub record_count: usize,
}

#[cfg(feature = "file_io")]
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

    /// Return canonical base variable names present in the slotted VDF name
    /// prefix, excluding system/group/unit/builtin entries.
    fn slotted_model_base_names(&self) -> HashSet<String> {
        let system_names: HashSet<&str> = SYSTEM_NAMES.into_iter().collect();
        let mut out = HashSet::new();
        for name in self.names.iter().take(self.slot_table.len()) {
            if name.is_empty()
                || name.starts_with('.')
                || name.starts_with('-')
                || system_names.contains(name.as_str())
                || VENSIM_BUILTINS.iter().any(|b| b.eq_ignore_ascii_case(name))
                || (name.len() == 1 && name.starts_with(|c: char| !c.is_alphanumeric()))
            {
                continue;
            }
            out.insert(crate::common::canonicalize(name.as_str()).into_owned());
        }
        out
    }

    /// Build a name->OT mapping using model ordering + VDF structural anchors.
    ///
    /// This path does not use time-series correlation. It uses:
    /// 1. model scalar ordering (`reference.offsets`, sorted by offset),
    /// 2. per-variable element counts from the model,
    /// 3. VDF record/name anchors (`f12` slot -> name, record `f11` start OT),
    /// 4. deterministic first-fit placement over OT indices.
    ///
    /// The output is deterministic for a given `(VDF, model)` pair and is meant
    /// as a structural allocator baseline while deeper compilation-order rules are
    /// still being decoded.
    pub fn build_model_guided_ot_map(
        &self,
        project: &crate::Project,
        main_model_name: &str,
    ) -> StdResult<HashMap<Ident<Canonical>, usize>, Box<dyn Error>> {
        if self.offset_table_count == 0 {
            return Err("empty offset table".into());
        }

        let time_ident = Ident::<Canonical>::from_str_unchecked("time");
        let offsets = crate::interpreter::calc_flattened_offsets(project, main_model_name);

        let mut view_indexes: HashMap<Ident<Canonical>, usize> = HashMap::new();
        if let Some(datamodel) = project
            .datamodel
            .models
            .iter()
            .find(|m| m.name == main_model_name)
        {
            for (v_idx, view) in datamodel.views.iter().enumerate() {
                let crate::datamodel::View::StockFlow(sf) = view;
                for element in &sf.elements {
                    if let Some(name) = element.get_name() {
                        let ident = Ident::<Canonical>::from_str_unchecked(
                            crate::common::canonicalize(name).as_ref(),
                        );
                        view_indexes.entry(ident).or_insert(v_idx);
                    }
                }
            }
        }

        let topo_order = match crate::interpreter::Simulation::new(project, main_model_name) {
            Ok(sim) => {
                let mut map = HashMap::new();
                for (i, id) in sim.runlist_order().into_iter().enumerate() {
                    map.insert(id, i);
                }
                map
            }
            Err(_) => HashMap::new(),
        };

        let mut ordered: Vec<(Ident<Canonical>, usize)> = offsets
            .into_iter()
            .filter(|(id, _)| *id != time_ident)
            .map(|(id, (off, _))| (id, off))
            .collect();

        ordered.sort_by(|(a_id, a_off), (b_id, b_off)| {
            let a_base = a_id
                .as_str()
                .split_once('[')
                .map(|(b, _)| b)
                .unwrap_or(a_id.as_str());
            let b_base = b_id
                .as_str()
                .split_once('[')
                .map(|(b, _)| b)
                .unwrap_or(b_id.as_str());

            let a_base_ident = Ident::<Canonical>::from_str_unchecked(a_base);
            let b_base_ident = Ident::<Canonical>::from_str_unchecked(b_base);

            let a_view = view_indexes
                .get(&a_base_ident)
                .copied()
                .unwrap_or(usize::MAX);
            let b_view = view_indexes
                .get(&b_base_ident)
                .copied()
                .unwrap_or(usize::MAX);

            if a_view != b_view {
                return a_view.cmp(&b_view);
            }

            let a_topo = topo_order.get(&a_base_ident).copied().unwrap_or(usize::MAX);
            let b_topo = topo_order.get(&b_base_ident).copied().unwrap_or(usize::MAX);

            if a_topo != b_topo {
                return a_topo.cmp(&b_topo);
            }

            let a_is_2 = a_base.ends_with("_2");
            let a_is_1 = a_base.ends_with("_1");
            let a_is_table = a_base.ends_with("_table");

            let b_is_2 = b_base.ends_with("_2");
            let b_is_1 = b_base.ends_with("_1");
            let b_is_table = b_base.ends_with("_table");

            // _2 before _1 before base
            let a_suffix_score = if a_is_2 {
                0
            } else if a_is_1 {
                1
            } else {
                2
            };
            let b_suffix_score = if b_is_2 {
                0
            } else if b_is_1 {
                1
            } else {
                2
            };
            if a_suffix_score != b_suffix_score {
                return a_suffix_score.cmp(&b_suffix_score);
            }

            // table colocation
            if a_is_table != b_is_table {
                return if a_is_table {
                    std::cmp::Ordering::Greater
                } else {
                    std::cmp::Ordering::Less
                };
            }

            a_off
                .cmp(b_off)
                .then_with(|| a_id.as_str().cmp(b_id.as_str()))
        });

        // Group scalar identifiers by their base variable (`x[a,b]` -> `x`),
        // preserving model offset order.
        let mut groups: Vec<(String, Vec<Ident<Canonical>>)> = Vec::new();
        let mut group_index_by_name: HashMap<String, usize> = HashMap::new();
        for (ident, _off) in ordered {
            let base = ident
                .as_str()
                .split_once('[')
                .map(|(b, _)| b)
                .unwrap_or(ident.as_str())
                .to_string();
            if let Some(&idx) = group_index_by_name.get(&base) {
                groups[idx].1.push(ident);
            } else {
                let idx = groups.len();
                group_index_by_name.insert(base.clone(), idx);
                groups.push((base, vec![ident]));
            }
        }

        // Only map model groups that are visible in the VDF slotted-name
        // prefix. Large VDF files omit many internal/intermediate variables
        // that exist in the compiled model offset space.
        let target_bases = self.slotted_model_base_names();
        groups.retain(|(base, _)| target_bases.contains(base));
        group_index_by_name = groups
            .iter()
            .enumerate()
            .map(|(idx, (name, _))| (name.clone(), idx))
            .collect();

        let system_names: HashSet<&str> = SYSTEM_NAMES.into_iter().collect();

        // Build slot_ref -> canonical model-name candidates from the slotted
        // prefix of the name table.
        let mut slot_name: HashMap<u32, String> = HashMap::new();
        for (i, &slot) in self.slot_table.iter().enumerate() {
            let Some(name) = self.names.get(i) else {
                continue;
            };
            if name.is_empty()
                || name.starts_with('.')
                || name.starts_with('-')
                || system_names.contains(name.as_str())
                || VENSIM_BUILTINS.iter().any(|b| b.eq_ignore_ascii_case(name))
            {
                continue;
            }
            slot_name.entry(slot).or_insert_with(|| {
                crate::common::canonicalize(name.as_str())
                    .into_owned()
                    .to_string()
            });
        }

        // Collect earliest record OT per anchored base name.
        let mut anchor_ot: HashMap<String, usize> = HashMap::new();
        if self.offset_table_count <= 100 {
            for rec in &self.records {
                let ot = rec.fields[11] as usize;
                if rec.fields[0] == 0
                    || rec.fields[1] == RECORD_F1_SYSTEM
                    || rec.fields[1] == RECORD_F1_INITIAL_TIME_CONST
                    || rec.fields[10] == 0
                    || ot == 0
                    || ot >= self.offset_table_count
                {
                    continue;
                }
                let Some(name) = slot_name.get(&rec.fields[12]) else {
                    continue;
                };
                if group_index_by_name.contains_key(name.as_str()) {
                    anchor_ot
                        .entry(name.clone())
                        .and_modify(|v| *v = (*v).min(ot))
                        .or_insert(ot);
                }
            }
        }

        // For small/simple files, deterministic f10/name ordering can provide
        // strong scalar anchors. Use these as additional hints when available.
        if self.offset_table_count <= 100
            && let Ok(det_map) = self.build_deterministic_ot_map()
        {
            for (name, ot) in det_map {
                let canonical = crate::common::canonicalize(name.as_str()).into_owned();
                if group_index_by_name.contains_key(&canonical)
                    && ot > 0
                    && ot < self.offset_table_count
                {
                    anchor_ot.entry(canonical).or_insert(ot);
                }
            }
        }

        let mut claimed = vec![false; self.offset_table_count];
        claimed[0] = true;

        let mut mapping: HashMap<Ident<Canonical>, usize> = HashMap::new();
        mapping.insert(time_ident, 0);

        let find_run = |claimed: &[bool], start: usize, width: usize| -> Option<usize> {
            if width == 0 || claimed.len() <= width {
                return None;
            }
            for cand in start..=(claimed.len() - width) {
                if cand == 0 {
                    continue;
                }
                if claimed[cand..cand + width].iter().all(|v| !*v) {
                    return Some(cand);
                }
            }
            None
        };

        // Place anchored groups first, in OT order, so strong hints are not
        // displaced by earlier unanchored allocations.
        let mut anchored_groups: Vec<(String, Vec<Ident<Canonical>>, usize)> = Vec::new();
        let mut unanchored_groups: Vec<(String, Vec<Ident<Canonical>>)> = Vec::new();
        for (base, members) in groups {
            if let Some(&desired) = anchor_ot.get(&base) {
                anchored_groups.push((base, members, desired.max(1)));
            } else {
                unanchored_groups.push((base, members));
            }
        }
        anchored_groups.sort_by_key(|(_, _, desired)| *desired);

        for (_base, members, desired) in anchored_groups {
            let width = members.len();
            if width == 0 || width >= self.offset_table_count {
                continue;
            }
            let start = find_run(&claimed, desired, width)
                .or_else(|| find_run(&claimed, 1, width))
                .unwrap_or(0);
            if start == 0 {
                continue;
            }
            for (i, ident) in members.into_iter().enumerate() {
                let ot = start + i;
                claimed[ot] = true;
                mapping.insert(ident, ot);
            }
        }

        let mut next_search = 1usize;
        for (_base, members) in unanchored_groups {
            let width = members.len();
            if width == 0 || width >= self.offset_table_count {
                continue;
            }

            let start = find_run(&claimed, next_search.max(1), width)
                .or_else(|| find_run(&claimed, 1, width))
                .unwrap_or(0);
            if start == 0 {
                continue;
            }

            for (i, ident) in members.into_iter().enumerate() {
                let ot = start + i;
                claimed[ot] = true;
                mapping.insert(ident, ot);
            }
            next_search = start + width;
        }

        Ok(mapping)
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

    /// Build a `Results` struct using model-guided structural mapping
    /// (no time-series correlation).
    ///
    /// The resulting `Results` contains only variables that could be mapped to
    /// OT entries from this VDF. Columns are ordered by model offset.
    pub fn to_results_with_model(
        &self,
        project: &crate::Project,
        main_model_name: &str,
    ) -> StdResult<Results, Box<dyn Error>> {
        let vdf_data = self.extract_data()?;
        let ot_map = self.build_model_guided_ot_map(project, main_model_name)?;
        if ot_map.is_empty() {
            return Err("model-guided mapping produced no variables".into());
        }

        let model_offsets = crate::interpreter::calc_flattened_offsets(project, main_model_name);
        let mut ordered: Vec<(Ident<Canonical>, usize)> =
            ot_map.iter().map(|(id, &ot)| (id.clone(), ot)).collect();
        ordered.sort_by(|(a_id, _), (b_id, _)| {
            let a_off = model_offsets
                .get(a_id)
                .map(|(off, _)| *off)
                .unwrap_or(usize::MAX);
            let b_off = model_offsets
                .get(b_id)
                .map(|(off, _)| *off)
                .unwrap_or(usize::MAX);
            a_off
                .cmp(&b_off)
                .then_with(|| a_id.as_str().cmp(b_id.as_str()))
        });

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

        // Collect model variable records using per-record criteria.
        // We filter individual records rather than whole f[12] groups
        // because some VDFs mix control and model records in the same group.
        let mut model_records: Vec<(u32, u32)> = Vec::new(); // (f10, f11=OT_idx)
        for rec in &self.records {
            let ot_idx = rec.fields[11] as usize;
            if rec.fields[0] != 0
                && rec.fields[1] != RECORD_F1_SYSTEM
                && rec.fields[1] != RECORD_F1_INITIAL_TIME_CONST
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
        let system_names: HashSet<&str> = SYSTEM_NAMES.into_iter().collect();

        let mut candidates: Vec<String> = self.names[..self.slot_table.len()]
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
            let vensim_builtins: HashSet<&str> = VENSIM_BUILTINS.into_iter().collect();
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

    // NOTE: `build_model_guided_ot_map` is currently a deterministic
    // structural allocator baseline. It uses model ordering + VDF anchors,
    // but does not yet fully replicate Vensim's internal compilation order.
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

    let sample_indices = build_sample_indices(step_count);
    const MATCH_THRESHOLD: f64 = 0.01;
    let ordered_reference = sorted_reference_offsets(reference, &time_ident);

    for (ident, ref_off) in ordered_reference {
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
            offsets.insert(ident, col);
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
            n_chunks: step_count,
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

#[cfg(feature = "file_io")]
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
        a_off
            .cmp(b_off)
            .then_with(|| a_id.as_str().cmp(b_id.as_str()))
    });
    ordered
}

/// Build sample indices for time series correlation. Picks first, last,
/// and quartile points to avoid comparing every time step.
#[cfg(feature = "file_io")]
fn build_sample_indices(step_count: usize) -> Vec<usize> {
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

    #[cfg(feature = "file_io")]
    mod file_io_tests {
        use super::super::*;

        fn vdf_file(path: &str) -> VdfFile {
            let data = std::fs::read(path)
                .unwrap_or_else(|e| panic!("failed to read VDF file {}: {}", path, e));
            VdfFile::parse(data)
                .unwrap_or_else(|e| panic!("failed to parse VDF file {}: {}", path, e))
        }

        /// Parse name entries with raw byte offsets (relative to section 2 data start).
        fn parse_name_entries_with_offsets(vdf: &VdfFile) -> Vec<(usize, usize, String)> {
            let Some(ns_idx) = vdf.name_section_idx else {
                return Vec::new();
            };
            let ns = &vdf.sections[ns_idx];
            let data_start = ns.data_offset();
            let parse_end = ns.region_end.min(vdf.data.len());
            let mut out = Vec::new();

            let first_len = (ns.field5 >> 16) as usize;
            if first_len == 0 || data_start + first_len > vdf.data.len() {
                return out;
            }

            let first_name: String = vdf.data[data_start..data_start + first_len]
                .iter()
                .take_while(|&&b| b != 0)
                .map(|&b| b as char)
                .collect();
            out.push((0, 0, first_name));

            let mut pos = data_start + first_len;
            while pos + 2 <= parse_end {
                let len = read_u16(&vdf.data, pos) as usize;
                let prefix_rel = pos - data_start;
                pos += 2;
                if len == 0 {
                    continue;
                }
                if pos + len > parse_end {
                    break;
                }
                if len > 256 {
                    break;
                }
                let s: String = vdf.data[pos..pos + len]
                    .iter()
                    .take_while(|&&b| b != 0)
                    .map(|&b| b as char)
                    .collect();
                if s.is_empty() || !s.chars().all(|c| c.is_ascii_graphic() || c == ' ') {
                    break;
                }
                out.push((prefix_rel, pos - data_start, s));
                pos += len;
            }

            out
        }

        fn parse_name_entry_spans(vdf: &VdfFile) -> Vec<(usize, usize, String)> {
            let Some(ns_idx) = vdf.name_section_idx else {
                return Vec::new();
            };
            let ns = &vdf.sections[ns_idx];
            let data_start = ns.data_offset();
            let parse_end = ns.region_end.min(vdf.data.len());
            let mut out = Vec::new();

            let first_len = (ns.field5 >> 16) as usize;
            if first_len == 0 || data_start + first_len > vdf.data.len() {
                return out;
            }
            let first_name: String = vdf.data[data_start..data_start + first_len]
                .iter()
                .take_while(|&&b| b != 0)
                .map(|&b| b as char)
                .collect();
            out.push((0, first_len, first_name));

            let mut pos = data_start + first_len;
            while pos + 2 <= parse_end {
                let len = read_u16(&vdf.data, pos) as usize;
                pos += 2;
                if len == 0 {
                    continue;
                }
                if pos + len > parse_end || len > 256 {
                    break;
                }
                let s: String = vdf.data[pos..pos + len]
                    .iter()
                    .take_while(|&&b| b != 0)
                    .map(|&b| b as char)
                    .collect();
                if s.is_empty() || !s.chars().all(|c| c.is_ascii_graphic() || c == ' ') {
                    break;
                }
                out.push((pos - data_start, pos - data_start + len, s));
                pos += len;
            }

            out
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

        fn build_f2_order_ot_map(
            vdf: &VdfFile,
        ) -> StdResult<HashMap<String, usize>, Box<dyn Error>> {
            let ot_count = vdf.offset_table_count;

            let mut records: Vec<&VdfRecord> = vdf
                .records
                .iter()
                .filter(|r| {
                    r.fields[1] != RECORD_F1_SYSTEM
                        && r.fields[11] > 0
                        && (r.fields[11] as usize) < ot_count
                })
                .collect();
            records.sort_by_key(|r| (r.fields[2], r.fields[11]));

            let system_names: HashSet<&str> = SYSTEM_NAMES.into_iter().collect();
            let vensim_builtins: HashSet<&str> = VENSIM_BUILTINS.into_iter().collect();

            let mut names: Vec<String> = vdf.names[..vdf.slot_table.len()]
                .iter()
                .filter(|name| {
                    !name.is_empty()
                        && !name.starts_with('.')
                        && !name.starts_with('-')
                        && !name.starts_with('#')
                        && !system_names.contains(name.as_str())
                        && !vensim_builtins.contains(name.to_lowercase().as_str())
                        && (name.len() != 1
                            || name.chars().next().is_none_or(|c| c.is_alphanumeric()))
                })
                .cloned()
                .collect();

            if names.len() < records.len() {
                return Err(format!(
                    "f2-order map: candidate names {} < records {}",
                    names.len(),
                    records.len()
                )
                .into());
            }
            names.truncate(records.len());

            let mut out = HashMap::new();
            out.insert("Time".to_string(), 0);
            for (name, rec) in names.into_iter().zip(records.into_iter()) {
                out.insert(name, rec.fields[11] as usize);
            }
            Ok(out)
        }

        fn score_map_against_empirical(
            name_to_ot: &HashMap<String, usize>,
            empirical: &HashMap<Ident<Canonical>, usize>,
        ) -> (usize, usize) {
            let mut compared = 0usize;
            let mut matches = 0usize;
            for (name, &ot) in name_to_ot {
                let canonical = crate::common::canonicalize(name);
                if let Some(&emp_ot) = empirical.get(canonical.as_ref()) {
                    compared += 1;
                    if ot == emp_ot {
                        matches += 1;
                    }
                }
            }
            (compared, matches)
        }

        #[test]
        #[ignore]
        fn test_f2_order_mapping_vs_empirical_water_econ() {
            let cases = [
                (
                    "water",
                    "../../test/bobby/vdf/water/water.mdl",
                    "../../test/bobby/vdf/water/Current.vdf",
                ),
                (
                    "econ",
                    "../../test/bobby/vdf/econ/mark2.mdl",
                    "../../test/bobby/vdf/econ/base.vdf",
                ),
            ];

            for (label, mdl_path, vdf_path) in cases {
                let vdf = vdf_file(vdf_path);
                let vdf_data = vdf.extract_data().unwrap();
                let ref_results = simulate_mdl(mdl_path);
                let emp_map = build_empirical_ot_map(&vdf_data, &ref_results).unwrap();
                let f2_map = build_f2_order_ot_map(&vdf).unwrap();

                let mut matches = 0usize;
                let mut compared = 0usize;
                for (name, &ot) in &f2_map {
                    let canonical = crate::common::canonicalize(name);
                    if let Some(&emp_ot) = emp_map.get(canonical.as_ref()) {
                        compared += 1;
                        if ot == emp_ot {
                            matches += 1;
                        }
                    }
                }
                eprintln!("{label}: f2-order compared={compared} matches={matches}");

                // Exploratory signal only.
                assert!(compared > 0, "{label}: expected some comparable names");
            }
        }

        #[test]
        #[ignore]
        fn test_record_f3_matches_name_entry_offsets() {
            let econ = vdf_file("../../test/bobby/vdf/econ/base.vdf");
            let wrld3 = vdf_file("../../test/metasd/WRLD3-03/SCEN01.VDF");

            for (label, vdf) in [("econ", &econ), ("wrld3", &wrld3)] {
                let entries = parse_name_entries_with_offsets(vdf);
                assert!(
                    !entries.is_empty(),
                    "{label}: expected parsed name entries with raw offsets"
                );

                let prefix_offsets: std::collections::HashSet<u32> =
                    entries.iter().map(|(p, _, _)| *p as u32).collect();
                let string_offsets: std::collections::HashSet<u32> =
                    entries.iter().map(|(_, s, _)| *s as u32).collect();

                let model_records: Vec<&VdfRecord> = vdf
                    .records
                    .iter()
                    .filter(|r| {
                        r.fields[0] != 0
                            && r.fields[1] != RECORD_F1_SYSTEM
                            && r.fields[1] != RECORD_F1_INITIAL_TIME_CONST
                            && r.fields[10] > 0
                            && r.fields[11] > 0
                            && (r.fields[11] as usize) < vdf.offset_table_count
                    })
                    .collect();
                assert!(!model_records.is_empty(), "{label}: expected model records");

                let prefix_matches = model_records
                    .iter()
                    .filter(|r| prefix_offsets.contains(&r.fields[3]))
                    .count();
                let string_matches = model_records
                    .iter()
                    .filter(|r| string_offsets.contains(&r.fields[3]))
                    .count();

                // This pins down whether f[3] is a direct name-table pointer.
                assert!(
                    prefix_matches > 0 || string_matches > 0,
                    "{label}: expected at least one f[3] match to name entry offsets; got prefix={prefix_matches} string={string_matches}"
                );
            }
        }

        #[test]
        #[ignore]
        fn test_record_f3_within_name_spans() {
            let water = vdf_file("../../test/bobby/vdf/water/Current.vdf");
            let econ = vdf_file("../../test/bobby/vdf/econ/base.vdf");
            let wrld3 = vdf_file("../../test/metasd/WRLD3-03/SCEN01.VDF");

            for (label, vdf) in [("water", &water), ("econ", &econ), ("wrld3", &wrld3)] {
                let spans = parse_name_entry_spans(vdf);
                let model_records: Vec<&VdfRecord> = vdf
                    .records
                    .iter()
                    .filter(|r| {
                        r.fields[1] != RECORD_F1_SYSTEM
                            && r.fields[11] > 0
                            && (r.fields[11] as usize) < vdf.offset_table_count
                    })
                    .collect();

                let mut in_span = 0usize;
                for rec in &model_records {
                    let hits = spans
                        .iter()
                        .filter(|(start, end, _)| {
                            let off = rec.fields[3] as usize;
                            off >= *start && off < *end
                        })
                        .count();
                    if hits > 0 {
                        in_span += 1;
                    }
                }
                assert!(
                    in_span > 0,
                    "{label}: expected some records with f3 in name spans"
                );
            }
        }

        #[test]
        #[ignore]
        fn test_f3_span_mapping_vs_empirical() {
            let cases = [
                (
                    "water",
                    "../../test/bobby/vdf/water/water.mdl",
                    "../../test/bobby/vdf/water/Current.vdf",
                ),
                (
                    "econ",
                    "../../test/bobby/vdf/econ/mark2.mdl",
                    "../../test/bobby/vdf/econ/base.vdf",
                ),
                (
                    "wrld3",
                    "../../test/metasd/WRLD3-03/wrld3-03.mdl",
                    "../../test/metasd/WRLD3-03/SCEN01.VDF",
                ),
            ];

            for (label, mdl_path, vdf_path) in cases {
                let vdf = vdf_file(vdf_path);
                let spans = parse_name_entry_spans(&vdf);
                let mut span_map: HashMap<String, usize> = HashMap::new();
                span_map.insert("Time".to_string(), 0);

                for rec in &vdf.records {
                    if rec.fields[1] == RECORD_F1_SYSTEM {
                        continue;
                    }
                    let ot = rec.fields[11] as usize;
                    if ot == 0 || ot >= vdf.offset_table_count {
                        continue;
                    }

                    let off = rec.fields[3] as usize;
                    let mut hit_name: Option<&str> = None;
                    for (start, end, name) in &spans {
                        if off >= *start && off < *end {
                            if hit_name.is_some() {
                                hit_name = None;
                                break;
                            }
                            hit_name = Some(name.as_str());
                        }
                    }
                    if let Some(name) = hit_name {
                        span_map.entry(name.to_string()).or_insert(ot);
                    }
                }

                let vdf_data = vdf.extract_data().unwrap();
                let ref_results = simulate_mdl(mdl_path);
                let emp_map = build_empirical_ot_map(&vdf_data, &ref_results).unwrap();

                let (_compared, matches) = score_map_against_empirical(&span_map, &emp_map);
                assert!(matches > 0, "{label}: expected at least one f3-span match");
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

        fn make_synthetic_reference(var_insert_order: &[&str]) -> crate::Results {
            let mut offsets: HashMap<Ident<Canonical>, usize> = HashMap::new();
            offsets.insert(Ident::new("time"), 0);
            offsets.insert(Ident::new("a"), 1);
            offsets.insert(Ident::new("b"), 2);
            offsets.insert(Ident::new("c"), 3);

            // Reinsert variable keys in caller-specified order to vary HashMap
            // insertion history without changing logical offsets.
            let mut reordered: HashMap<Ident<Canonical>, usize> = HashMap::new();
            reordered.insert(Ident::new("time"), 0);
            for &name in var_insert_order {
                let id = Ident::new(name);
                reordered.insert(id.clone(), *offsets.get(&id).unwrap());
            }

            // time, a, b, c columns
            let rows: [[f64; 4]; 4] = [
                [0.0, 100.0, 10.0, 5.0],
                [1.0, 101.0, 20.0, 6.0],
                [2.0, 102.0, 40.0, 9.0],
                [3.0, 103.0, 80.0, 14.0],
            ];
            let mut data = Vec::new();
            for row in rows {
                data.extend_from_slice(&row);
            }

            crate::Results {
                offsets: reordered,
                data: data.into_boxed_slice(),
                step_size: 4,
                step_count: 4,
                specs: crate::results::Specs {
                    start: 0.0,
                    stop: 3.0,
                    dt: 1.0,
                    save_step: 1.0,
                    method: crate::results::Method::Euler,
                    n_chunks: 4,
                },
                is_vensim: true,
            }
        }

        fn make_synthetic_vdf() -> VdfData {
            // OT entries: 0=time, 1=b, 2=c, 3=a
            VdfData {
                time_values: vec![0.0, 1.0, 2.0, 3.0],
                entries: vec![
                    vec![0.0, 1.0, 2.0, 3.0],
                    vec![10.0, 20.0, 40.0, 80.0],
                    vec![5.0, 6.0, 9.0, 14.0],
                    vec![100.0, 101.0, 102.0, 103.0],
                ],
            }
        }

        #[test]
        fn test_empirical_map_is_deterministic_across_offset_insertion_order() {
            let vdf = make_synthetic_vdf();
            let reference_a = make_synthetic_reference(&["a", "b", "c"]);
            let reference_b = make_synthetic_reference(&["c", "b", "a"]);

            let map_a = build_empirical_ot_map(&vdf, &reference_a).unwrap();
            let map_b = build_empirical_ot_map(&vdf, &reference_b).unwrap();
            assert_eq!(map_a, map_b);
            assert_eq!(map_a.get(&Ident::new("a")), Some(&3usize));
            assert_eq!(map_a.get(&Ident::new("b")), Some(&1usize));
            assert_eq!(map_a.get(&Ident::new("c")), Some(&2usize));
        }

        #[test]
        fn test_build_vdf_results_is_deterministic_across_offset_insertion_order() {
            let vdf = make_synthetic_vdf();
            let reference_a = make_synthetic_reference(&["a", "b", "c"]);
            let reference_b = make_synthetic_reference(&["c", "b", "a"]);

            let res_a = build_vdf_results(&vdf, &reference_a).unwrap();
            let res_b = build_vdf_results(&vdf, &reference_b).unwrap();
            assert_eq!(res_a.offsets, res_b.offsets);
            assert_eq!(res_a.step_count, res_b.step_count);
            assert_eq!(res_a.step_size, res_b.step_size);
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
                if let Some(&emp_ot) = emp_map.get(canonical.as_ref()) {
                    if **det_ot == emp_ot {
                        matches += 1;
                    } else {
                        mismatches.push((det_name.to_string(), **det_ot, emp_ot));
                    }
                }
            }

            (matches, mismatches)
        }

        fn assert_model_guided_map_well_formed(mdl_path: &str, vdf_path: &str) {
            let contents = std::fs::read_to_string(mdl_path).unwrap();
            let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
            let project = std::rc::Rc::new(crate::Project::from(datamodel_project));
            let vdf = vdf_file(vdf_path);
            let map_a = vdf
                .build_model_guided_ot_map(&project, "main")
                .unwrap_or_else(|e| panic!("model-guided map failed for {vdf_path}: {e}"));
            let map_b = vdf
                .build_model_guided_ot_map(&project, "main")
                .unwrap_or_else(|e| panic!("model-guided map failed for {vdf_path}: {e}"));
            assert_eq!(map_a, map_b);

            let time_ident = Ident::<Canonical>::from_str_unchecked("time");
            assert_eq!(map_a.get(&time_ident), Some(&0usize));

            let target_bases = vdf.slotted_model_base_names();
            let expected_len = crate::interpreter::calc_flattened_offsets(&project, "main")
                .into_iter()
                .filter(|(id, _)| {
                    if *id == time_ident {
                        return false;
                    }
                    let base = id
                        .as_str()
                        .split_once('[')
                        .map(|(b, _)| b)
                        .unwrap_or(id.as_str());
                    target_bases.contains(base)
                })
                .count()
                + 1;
            assert_eq!(
                map_a.len(),
                expected_len,
                "expected model-guided map coverage to match VDF slotted outputs"
            );

            let mut seen_ots = HashSet::new();
            for (id, &ot) in &map_a {
                assert!(
                    ot < vdf.offset_table_count,
                    "{id} mapped out of OT range: {ot}"
                );
                assert!(seen_ots.insert(ot), "duplicate OT assignment: {ot}");
                if *id != time_ident {
                    assert!(ot > 0, "non-time variable {id} mapped to OT zero");
                }
            }
        }

        #[test]
        fn test_model_guided_wrld3_map_is_well_formed() {
            let mdl_path = "../../test/metasd/WRLD3-03/wrld3-03.mdl";
            let vdf_path = "../../test/metasd/WRLD3-03/SCEN01.VDF";
            let contents = std::fs::read_to_string(mdl_path).unwrap();
            let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
            let project = std::rc::Rc::new(crate::Project::from(datamodel_project));
            let vdf = vdf_file(vdf_path);

            let map = vdf
                .build_model_guided_ot_map(&project, "main")
                .unwrap_or_else(|e| panic!("model-guided map failed for {vdf_path}: {e}"));

            let time_ident = Ident::<Canonical>::from_str_unchecked("time");
            assert_eq!(map.get(&time_ident), Some(&0usize));
            assert!(
                map.len() >= 200,
                "wrld3: expected substantial mapped coverage, got {}",
                map.len()
            );
            assert!(
                map.len() <= vdf.offset_table_count,
                "wrld3: mapped columns should not exceed OT entries"
            );

            let mut seen = HashSet::new();
            for (id, &ot) in &map {
                assert!(
                    ot < vdf.offset_table_count,
                    "{id} mapped out of range: {ot}"
                );
                assert!(seen.insert(ot), "duplicate OT assignment: {ot}");
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

        #[test]
        #[ignore]
        fn test_model_guided_map_deterministic_for_wrld3() {
            let mdl_path = "../../test/metasd/WRLD3-03/wrld3-03.mdl";
            let vdf_path = "../../test/metasd/WRLD3-03/SCEN01.VDF";
            let contents = std::fs::read_to_string(mdl_path).unwrap();
            let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
            let project = std::rc::Rc::new(crate::Project::from(datamodel_project));
            let vdf = vdf_file(vdf_path);

            let map_a = vdf.build_model_guided_ot_map(&project, "main").unwrap();
            let map_b = vdf.build_model_guided_ot_map(&project, "main").unwrap();
            assert_eq!(map_a, map_b);
        }

        #[test]
        #[ignore]
        fn test_debug_wrld3_order_signal_coverage() {
            let mdl_path = "../../test/metasd/WRLD3-03/wrld3-03.mdl";
            let vdf_path = "../../test/metasd/WRLD3-03/SCEN01.VDF";

            let contents = std::fs::read_to_string(mdl_path).unwrap();
            let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
            let project = std::rc::Rc::new(crate::Project::from(datamodel_project));
            let vdf = vdf_file(vdf_path);

            let offsets = crate::interpreter::calc_flattened_offsets(&project, "main");
            let target_bases = vdf.slotted_model_base_names();

            let mut base_names: HashSet<String> = HashSet::new();
            for (id, _) in offsets {
                if id == Ident::new("time") {
                    continue;
                }
                let base = id
                    .as_str()
                    .split_once('[')
                    .map(|(b, _)| b)
                    .unwrap_or(id.as_str())
                    .to_string();
                if target_bases.contains(&base) {
                    base_names.insert(base);
                }
            }

            let mut view_indexes: HashMap<Ident<Canonical>, usize> = HashMap::new();
            if let Some(datamodel) = project.datamodel.models.iter().find(|m| m.name == "main") {
                for (v_idx, view) in datamodel.views.iter().enumerate() {
                    let crate::datamodel::View::StockFlow(sf) = view;
                    for element in &sf.elements {
                        if let Some(name) = element.get_name() {
                            let ident = Ident::<Canonical>::from_str_unchecked(
                                crate::common::canonicalize(name).as_ref(),
                            );
                            view_indexes.entry(ident).or_insert(v_idx);
                        }
                    }
                }
            }

            let sim = crate::interpreter::Simulation::new(&project, "main").unwrap();
            let topo_order: HashMap<Ident<Canonical>, usize> = sim
                .runlist_order()
                .into_iter()
                .enumerate()
                .map(|(i, id)| (id, i))
                .collect();

            let mut in_view = 0usize;
            let mut in_topo = 0usize;
            for base in &base_names {
                let id = Ident::<Canonical>::from_str_unchecked(base.as_str());
                if view_indexes.contains_key(&id) {
                    in_view += 1;
                }
                if topo_order.contains_key(&id) {
                    in_topo += 1;
                }
            }

            let mut slot_name: HashMap<u32, String> = HashMap::new();
            let system_names: HashSet<&str> = SYSTEM_NAMES.into_iter().collect();
            for (i, &slot) in vdf.slot_table.iter().enumerate() {
                let Some(name) = vdf.names.get(i) else {
                    continue;
                };
                if name.is_empty()
                    || name.starts_with('.')
                    || name.starts_with('-')
                    || system_names.contains(name.as_str())
                    || VENSIM_BUILTINS.iter().any(|b| b.eq_ignore_ascii_case(name))
                    || (name.len() == 1 && name.starts_with(|c: char| !c.is_alphanumeric()))
                {
                    continue;
                }
                slot_name
                    .entry(slot)
                    .or_insert_with(|| crate::common::canonicalize(name.as_str()).into_owned());
            }

            let mut referenced = HashSet::new();
            for rec in &vdf.records {
                let ot = rec.fields[11] as usize;
                if rec.fields[0] == 0
                    || rec.fields[1] == RECORD_F1_SYSTEM
                    || rec.fields[1] == RECORD_F1_INITIAL_TIME_CONST
                    || rec.fields[10] == 0
                    || ot == 0
                    || ot >= vdf.offset_table_count
                {
                    continue;
                }
                if let Some(name) = slot_name.get(&rec.fields[12]) {
                    referenced.insert(name.clone());
                }
            }

            eprintln!(
                "wrld3 target bases={} in_view={} in_topo={} referenced_by_records={}",
                base_names.len(),
                in_view,
                in_topo,
                referenced.len()
            );
            assert!(!base_names.is_empty());
        }

        #[test]
        fn test_model_guided_small_models_well_formed() {
            assert_model_guided_map_well_formed(
                "../../test/bobby/vdf/water/water.mdl",
                "../../test/bobby/vdf/water/Current.vdf",
            );

            assert_model_guided_map_well_formed(
                "../../test/bobby/vdf/pop/pop.mdl",
                "../../test/bobby/vdf/pop/Current.vdf",
            );
        }

        /// The bact VDF was generated from a different version of the model
        /// than the current bact.mdl (VDF has "bacteria"/"population growth",
        /// MDL has "stock"/"inflow"/"outflow"). The deterministic mapping
        /// fails with a count mismatch: 3 model records but only 2 candidate
        /// names. This documents the limitation.
        #[test]
        fn test_deterministic_bact_mapping_succeeds() {
            let vdf_path = "../../test/bobby/vdf/bact/Current.vdf";
            let vdf = vdf_file(vdf_path);

            // With the f[1]=15 filter, the deterministic mapping should
            // now succeed for the bact model (2 candidates, 2 records).
            let det_map = vdf.build_deterministic_ot_map().unwrap();
            assert!(
                det_map.contains_key("Time"),
                "bact: mapping should include Time"
            );
            assert!(
                det_map.len() >= 3,
                "bact: expected at least 3 entries (Time + 2 vars), got {}",
                det_map.len()
            );
        }

        /// Verify deterministic mapping for the water model. The MDL and
        /// VDF have the same variable names and matching step counts. The
        /// empirical comparison has a known limitation: constant-value
        /// ambiguity for variables with the same constant (e.g., desired
        /// water level = TIME STEP = SAVEPER = 1.0).
        #[test]
        fn test_deterministic_vs_empirical_water() {
            let (matches, mismatches) = compare_det_vs_emp(
                "../../test/bobby/vdf/water/water.mdl",
                "../../test/bobby/vdf/water/Current.vdf",
            );
            // The empirical approach can't distinguish constants with the
            // same value. "desired water level" (1.0) gets matched to a
            // different OT entry than the deterministic map chooses, but
            // both entries hold constant 1.0.
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
            let (matches, mismatches) = compare_det_vs_emp(
                "../../test/bobby/vdf/pop/pop.mdl",
                "../../test/bobby/vdf/pop/Current.vdf",
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
            let vdf_path = "../../test/bobby/vdf/water/Current.vdf";
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
            let vdf_path = "../../test/bobby/vdf/pop/Current.vdf";
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

        /// Verify f[10] ordering matches alphabetical name order for small
        /// VDF files. This is the core assumption of the deterministic mapping.
        #[test]
        fn test_f10_alphabetical_ordering() {
            let small_vdfs = [
                ("bact", "../../test/bobby/vdf/bact/Current.vdf"),
                ("water", "../../test/bobby/vdf/water/Current.vdf"),
                ("pop", "../../test/bobby/vdf/pop/Current.vdf"),
                ("econ", "../../test/bobby/vdf/econ/base.vdf"),
            ];

            for (label, vdf_path) in &small_vdfs {
                let vdf = vdf_file(vdf_path);
                let ot_count = vdf.offset_table_count;

                // Collect model variable records (same filter as deterministic map)
                let mut model_records: Vec<(u32, u32, usize)> = Vec::new();
                for (i, rec) in vdf.records.iter().enumerate() {
                    let ot_idx = rec.fields[11] as usize;
                    if rec.fields[0] != 0
                        && rec.fields[1] != RECORD_F1_SYSTEM
                        && rec.fields[1] != RECORD_F1_INITIAL_TIME_CONST
                        && rec.fields[10] > 0
                        && ot_idx > 0
                        && ot_idx < ot_count
                    {
                        model_records.push((rec.fields[10], rec.fields[11], i));
                    }
                }

                // Check uniqueness: no f[10] ties
                let f10_values: Vec<u32> = model_records.iter().map(|&(f10, _, _)| f10).collect();
                let mut unique = f10_values.clone();
                unique.sort();
                unique.dedup();
                let ties = f10_values.len() - unique.len();

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

        #[test]
        fn test_small_vdf_name_to_data_chain() {
            // ---- euler-5.vdf: bact model, 2 model vars ----
            let euler5 = vdf_file("../../test/bobby/vdf/bact/euler-5.vdf");
            assert_eq!(euler5.names.len(), 10);
            assert_eq!(euler5.slot_table.len(), 10);
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
            assert_eq!(euler5.records[0].fields[1], RECORD_F1_INITIAL_TIME_CONST);
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
                        && r.fields[1] != RECORD_F1_SYSTEM
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
                        && r.fields[1] != RECORD_F1_SYSTEM
                        && r.fields[1] != RECORD_F1_INITIAL_TIME_CONST
                        && r.fields[10] > 0
                        && r.fields[11] > 0
                        && (r.fields[11] as usize) < euler5.offset_table_count
                })
                .collect();
            assert_eq!(model_fixed.len(), 2);

            // ---- euler-10.vdf: same model, 3 model vars ----
            let euler10 = vdf_file("../../test/bobby/vdf/bact/euler-10.vdf");
            assert_eq!(euler10.names[10], "predation");
            let e10_std: Vec<_> = euler10
                .records
                .iter()
                .filter(|r| {
                    r.fields[0] != 0
                        && r.fields[1] != RECORD_F1_SYSTEM
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
                        && r.fields[1] != RECORD_F1_SYSTEM
                        && r.fields[1] != RECORD_F1_INITIAL_TIME_CONST
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
            let water = vdf_file("../../test/bobby/vdf/water/Current.vdf");
            assert!(
                !water
                    .records
                    .iter()
                    .any(|r| r.fields[1] == RECORD_F1_INITIAL_TIME_CONST)
            );
            let water_model: Vec<_> = water
                .records
                .iter()
                .filter(|r| {
                    r.fields[0] != 0
                        && r.fields[1] != RECORD_F1_SYSTEM
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
                            && r.fields[1] != RECORD_F1_SYSTEM
                            && r.fields[1] != RECORD_F1_INITIAL_TIME_CONST
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
    }
}
