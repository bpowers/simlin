// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use super::*;

/// Size of an observed section-3 directory entry in u32 words.
const SECTION3_DIRECTORY_ENTRY_WORDS: usize = 27;

/// Parsed section-3 directory entry from array-bearing VDF files.
///
/// Observed array VDFs store a run of 27-word records after a zero-filled
/// prefix in section 3. The full semantics remain only partially decoded, but
/// the stable signals are:
/// - `words[0]`: an index-like value; in `Ref.vdf` the first ten records form
///   the arithmetic progression `59, 86, 113, ... , 302`
/// - `words[1..=3]`: packed shape words. One-dimensional entries duplicate
///   the flattened size (`[3, 3]`), while composite entries use
///   `[flattened_size, axis_a, axis_b]` (for example `[21, 7, 3]`)
/// - `words[18..=19]`: one section-1 slot ref per encoded axis in the
///   validated fixtures
/// - `words[26]`: the encoded axis count (`1` and `2` in the validated
///   fixtures)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VdfSection3DirectoryEntry {
    /// Absolute file offset where this entry begins.
    pub file_offset: usize,
    /// Raw words comprising the entry.
    pub words: [u32; SECTION3_DIRECTORY_ENTRY_WORDS],
}

impl VdfSection3DirectoryEntry {
    /// Entry-local index-like value carried in word 0.
    pub fn index_word(&self) -> u32 {
        self.words[0]
    }

    /// Non-zero shape words from the leading cardinality fields.
    pub fn shape_words(&self) -> Vec<usize> {
        self.words[1..=3]
            .iter()
            .copied()
            .filter(|&word| word > 0)
            .map(|word| word as usize)
            .collect()
    }

    /// Flattened element count encoded in the leading shape word.
    pub fn flat_size(&self) -> usize {
        self.words[1] as usize
    }

    /// Axis cardinalities encoded by this entry.
    ///
    /// One-dimensional entries duplicate the flattened size (`[3, 3]`), so
    /// this normalizes them to `[3]`. Composite entries preserve the trailing
    /// factor words (`[21, 7, 3] -> [7, 3]`).
    pub fn axis_sizes(&self) -> Vec<usize> {
        let shape = self.shape_words();
        match shape.as_slice() {
            [] => Vec::new(),
            [size] => vec![*size],
            [flat, size] if flat == size => vec![*size],
            [_flat, rest @ ..] => rest.to_vec(),
        }
    }

    /// Section-1 slot refs carried near the end of the record.
    ///
    /// In the validated fixtures there is one ref per encoded axis.
    pub fn axis_slot_refs(&self) -> Vec<u32> {
        self.words[18..=19]
            .iter()
            .copied()
            .filter(|&word| word > 0)
            .collect()
    }

    /// Number of encoded axes implied by the record's normalized shape.
    pub fn axis_count(&self) -> usize {
        self.axis_sizes().len()
    }

    /// Whether this entry encodes multiple axes composed into one shape.
    pub fn is_composite_shape(&self) -> bool {
        self.axis_count() > 1
    }

    /// Small terminal tag carried in the final word.
    pub fn terminal_tag(&self) -> u32 {
        self.words[SECTION3_DIRECTORY_ENTRY_WORDS - 1]
    }
}

/// Parsed section-3 directory layout.
///
/// Scalar files keep section 3 as all zeros. Array-bearing files instead store
/// a zero-filled prefix followed by fixed-width 27-word records and an
/// optional trailing zero word.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VdfSection3Directory {
    /// Absolute file offset where section-3 data begins.
    pub data_offset: usize,
    /// Number of leading zero words before directory entries begin.
    pub zero_prefix_words: usize,
    /// Whether the section ends with a single trailing zero word after the
    /// fixed-width record stream.
    pub has_trailing_zero_word: bool,
    /// Parsed directory entries.
    pub entries: Vec<VdfSection3DirectoryEntry>,
}

impl VdfSection3Directory {
    /// Validate that non-zero index_words form an arithmetic progression
    /// with step = SECTION3_DIRECTORY_ENTRY_WORDS (27).
    ///
    /// Returns `true` when the invariant holds (including trivially when
    /// there are fewer than two non-zero index_words).
    pub fn validate_index_word_progression(&self) -> bool {
        let non_zero: Vec<u32> = self
            .entries
            .iter()
            .map(|e| e.index_word())
            .filter(|&w| w > 0)
            .collect();
        if non_zero.len() < 2 {
            return true;
        }
        let step = SECTION3_DIRECTORY_ENTRY_WORDS as u32;
        non_zero.windows(2).all(|pair| pair[1] == pair[0] + step)
    }
}

impl VdfFile {
    /// Parse section 3's array directory when present.
    ///
    /// The observed layout for array-bearing VDFs is:
    ///
    /// `u32 zero_prefix[p]; u32 entry[n][27]; u32 0?`
    ///
    /// where `p` is the leading zero-word count, `entry` is a fixed-width
    /// record stream, and the trailing zero word is optional. Scalar files
    /// return an empty directory.
    pub fn parse_section3_directory(&self) -> Option<VdfSection3Directory> {
        let sec = self.sections.get(3)?;
        let data_offset = sec.data_offset();
        let end = sec.region_end.min(self.data.len());
        if data_offset >= end {
            return Some(VdfSection3Directory {
                data_offset,
                zero_prefix_words: 0,
                has_trailing_zero_word: false,
                entries: Vec::new(),
            });
        }

        let data_len = end - data_offset;
        if !data_len.is_multiple_of(4) {
            return None;
        }

        let words: Vec<u32> = (0..data_len / 4)
            .map(|i| read_u32(&self.data, data_offset + i * 4))
            .collect();
        let leading_zero_words = words.iter().take_while(|&&word| word == 0).count();

        let mut best: Option<(usize, bool, Vec<VdfSection3DirectoryEntry>)> = None;
        for zero_prefix_words in 0..=leading_zero_words {
            let trailing_candidates: &[usize] = if words.last() == Some(&0) {
                &[1, 0]
            } else {
                &[0]
            };
            for &trailing_words in trailing_candidates {
                let remaining_words = words
                    .len()
                    .saturating_sub(zero_prefix_words)
                    .saturating_sub(trailing_words);
                if remaining_words == 0
                    || !remaining_words.is_multiple_of(SECTION3_DIRECTORY_ENTRY_WORDS)
                {
                    continue;
                }

                let mut entries =
                    Vec::with_capacity(remaining_words / SECTION3_DIRECTORY_ENTRY_WORDS);
                let mut valid = true;
                for entry_idx in 0..remaining_words / SECTION3_DIRECTORY_ENTRY_WORDS {
                    let start_word = zero_prefix_words + entry_idx * SECTION3_DIRECTORY_ENTRY_WORDS;
                    let end_word = start_word + SECTION3_DIRECTORY_ENTRY_WORDS;
                    let mut entry_words = [0u32; SECTION3_DIRECTORY_ENTRY_WORDS];
                    entry_words.copy_from_slice(&words[start_word..end_word]);

                    if entry_words[1] == 0 && entry_words[2] == 0 && entry_words[18] == 0 {
                        valid = false;
                        break;
                    }

                    entries.push(VdfSection3DirectoryEntry {
                        file_offset: data_offset + start_word * 4,
                        words: entry_words,
                    });
                }
                if !valid {
                    continue;
                }

                if best
                    .as_ref()
                    .is_none_or(|(_, _, best_entries)| entries.len() > best_entries.len())
                {
                    best = Some((zero_prefix_words, trailing_words == 1, entries));
                }
            }
        }

        let (zero_prefix_words, has_trailing_zero_word, entries) =
            best.unwrap_or((leading_zero_words, false, Vec::new()));
        Some(VdfSection3Directory {
            data_offset,
            zero_prefix_words,
            has_trailing_zero_word,
            entries,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    fn vdf_file(path: &str) -> VdfFile {
        let data = std::fs::read(path)
            .unwrap_or_else(|e| panic!("failed to read VDF file {}: {}", path, e));
        VdfFile::parse(data).unwrap_or_else(|e| panic!("failed to parse VDF file {}: {}", path, e))
    }

    #[test]
    fn test_section3_scalar_vs_array() {
        for path in [
            "../../test/bobby/vdf/water/Current.vdf",
            "../../test/bobby/vdf/pop/Current.vdf",
            "../../test/bobby/vdf/consts/b_is_3.vdf",
            "../../test/bobby/vdf/econ/base.vdf",
            "../../test/metasd/WRLD3-03/SCEN01.VDF",
        ] {
            let vdf = vdf_file(path);
            let sec = &vdf.sections[3];
            assert_eq!(sec.region_data_size(), 104, "{path}");
            assert_eq!(sec.field4, 0, "{path}");
            let start = sec.data_offset();
            for i in 0..26 {
                assert_eq!(read_u32(&vdf.data, start + i * 4), 0, "{path} word {i}");
            }
            let directory = vdf.parse_section3_directory().unwrap();
            assert!(
                directory.entries.is_empty(),
                "{path}: scalar section 3 should not parse entries"
            );
        }

        let vdf = vdf_file("../../test/bobby/vdf/subscripts/subscripts.vdf");
        let sec3 = &vdf.sections[3];
        assert!(sec3.region_data_size() > 104);
        assert_eq!(sec3.field4, 32);
        let directory = vdf.parse_section3_directory().unwrap();
        assert_eq!(directory.zero_prefix_words, 25);
        assert!(directory.has_trailing_zero_word);
        assert_eq!(directory.entries.len(), 1);
        assert_eq!(directory.entries[0].index_word(), 0);
        assert_eq!(directory.entries[0].shape_words(), vec![3, 3]);
    }

    #[test]
    fn test_section3_directory_parses_subscripts_record() {
        let vdf = vdf_file("../../test/bobby/vdf/subscripts/subscripts.vdf");
        let directory = vdf.parse_section3_directory().unwrap();

        assert_eq!(directory.zero_prefix_words, 25);
        assert!(directory.has_trailing_zero_word);
        assert_eq!(directory.entries.len(), 1);

        let entry = &directory.entries[0];
        assert_eq!(entry.index_word(), 0);
        assert_eq!(entry.shape_words(), vec![3, 3]);
        assert_eq!(entry.flat_size(), 3);
        assert_eq!(entry.axis_sizes(), vec![3]);
        assert_eq!(entry.axis_count(), 1);
        assert!(!entry.is_composite_shape());
        assert_eq!(entry.words[10], 1);
        assert_eq!(entry.axis_slot_refs(), vec![172]);
        assert_eq!(entry.terminal_tag(), 1);

        let slot_set: HashSet<u32> = vdf.slot_table.iter().copied().collect();
        for slot_ref in entry.axis_slot_refs() {
            assert!(
                slot_set.contains(&slot_ref),
                "subscripts: missing slot ref {slot_ref}"
            );
        }
    }

    #[test]
    fn test_section3_directory_parses_ref_records() {
        let vdf = vdf_file("../../test/xmutil_test_models/Ref.vdf");
        let directory = vdf.parse_section3_directory().unwrap();

        assert_eq!(directory.zero_prefix_words, 25);
        assert!(directory.has_trailing_zero_word);
        assert_eq!(directory.entries.len(), 11);

        assert_eq!(directory.entries[0].index_word(), 59);
        assert_eq!(directory.entries[0].shape_words(), vec![3, 3]);
        assert_eq!(directory.entries[0].flat_size(), 3);
        assert_eq!(directory.entries[0].axis_sizes(), vec![3]);
        assert_eq!(directory.entries[0].words[10], 1);
        assert_eq!(directory.entries[0].axis_slot_refs(), vec![2412]);
        assert_eq!(directory.entries[0].terminal_tag(), 1);

        assert_eq!(directory.entries[2].index_word(), 113);
        assert_eq!(directory.entries[2].shape_words(), vec![7, 7]);
        assert_eq!(directory.entries[2].flat_size(), 7);
        assert_eq!(directory.entries[2].axis_sizes(), vec![7]);
        assert_eq!(directory.entries[2].axis_slot_refs(), vec![636]);
        assert_eq!(directory.entries[2].terminal_tag(), 1);

        assert_eq!(directory.entries[3].index_word(), 140);
        assert_eq!(directory.entries[3].shape_words(), vec![21, 7, 3]);
        assert_eq!(directory.entries[3].flat_size(), 21);
        assert_eq!(directory.entries[3].axis_sizes(), vec![7, 3]);
        assert_eq!(directory.entries[3].axis_count(), 2);
        assert!(directory.entries[3].is_composite_shape());
        assert_eq!(directory.entries[3].words[10], 3);
        assert_eq!(directory.entries[3].words[11], 1);
        assert_eq!(directory.entries[3].axis_slot_refs(), vec![636, 7036]);
        assert_eq!(directory.entries[3].terminal_tag(), 2);
    }

    #[test]
    fn test_section3_directory_ref_indices_and_slot_refs() {
        let vdf = vdf_file("../../test/xmutil_test_models/Ref.vdf");
        let directory = vdf.parse_section3_directory().unwrap();
        let slot_set: HashSet<u32> = vdf.slot_table.iter().copied().collect();

        let indices: Vec<u32> = directory
            .entries
            .iter()
            .map(|entry| entry.index_word())
            .collect();
        assert_eq!(
            indices,
            vec![59, 86, 113, 140, 167, 194, 221, 248, 275, 302, 0]
        );

        for entry in &directory.entries {
            for slot_ref in entry.axis_slot_refs() {
                assert!(
                    slot_set.contains(&slot_ref),
                    "Ref.vdf: section-3 slot ref {slot_ref} should resolve through the slot table"
                );
            }
        }
    }

    #[test]
    fn test_section3_directory_entry_axis_invariants() {
        for path in [
            "../../test/bobby/vdf/subscripts/subscripts.vdf",
            "../../test/xmutil_test_models/Ref.vdf",
        ] {
            let vdf = vdf_file(path);
            let directory = vdf.parse_section3_directory().unwrap();

            for entry in &directory.entries {
                let axis_sizes = entry.axis_sizes();
                let flat_size = entry.flat_size();

                assert!(
                    !axis_sizes.is_empty(),
                    "{path}: section-3 entry should encode at least one axis"
                );
                assert_eq!(
                    axis_sizes.iter().product::<usize>(),
                    flat_size,
                    "{path}: flattened size should match axis product"
                );
                assert_eq!(
                    entry.axis_slot_refs().len(),
                    axis_sizes.len(),
                    "{path}: one slot ref per encoded axis"
                );
                assert_eq!(
                    entry.terminal_tag() as usize,
                    axis_sizes.len(),
                    "{path}: terminal tag should equal axis count"
                );
                let expected_w10 = if axis_sizes.len() == 1 {
                    1
                } else {
                    *axis_sizes.last().unwrap() as u32
                };
                assert_eq!(
                    entry.words[10], expected_w10,
                    "{path}: word[10] should match the validated packing hint"
                );
            }
        }
    }

    #[test]
    fn test_section3_flat_sizes_reappear_in_record_ranges() {
        for path in [
            "../../test/bobby/vdf/subscripts/subscripts.vdf",
            "../../test/xmutil_test_models/Ref.vdf",
        ] {
            let vdf = vdf_file(path);
            let directory = vdf.parse_section3_directory().unwrap();
            let range_lengths: HashSet<usize> =
                vdf.record_ot_ranges().iter().map(|r| r.len()).collect();

            for entry in &directory.entries {
                assert!(
                    range_lengths.contains(&entry.flat_size()),
                    "{path}: expected an OT range with flat size {}",
                    entry.flat_size()
                );
            }
        }
    }

    #[test]
    fn test_section3_index_word_arithmetic_progression() {
        for path in [
            "../../test/bobby/vdf/subscripts/subscripts.vdf",
            "../../test/xmutil_test_models/Ref.vdf",
        ] {
            let vdf = vdf_file(path);
            let directory = vdf.parse_section3_directory().unwrap();
            assert!(
                directory.validate_index_word_progression(),
                "{path}: non-zero index_words should form an arithmetic progression with step 27"
            );
        }
    }

    #[test]
    fn test_section3_section5_shared_axis_refs() {
        // Scalar files have no shared refs.
        let vdf = vdf_file("../../test/bobby/vdf/water/Current.vdf");
        assert!(
            vdf.section3_section5_shared_axis_refs().is_empty(),
            "scalar file should have no shared axis refs"
        );

        // Simple 1D array files may have zero trailing refs in section 5,
        // so the bridge set can be empty even with section-3 entries.
        let vdf = vdf_file("../../test/bobby/vdf/subscripts/subscripts.vdf");
        let _ = vdf.section3_section5_shared_axis_refs();

        // Multi-dimensional Ref.vdf should have non-zero shared axis refs
        // for entries with multiple axes.
        let vdf = vdf_file("../../test/xmutil_test_models/Ref.vdf");
        let shared = vdf.section3_section5_shared_axis_refs();
        let directory = vdf.parse_section3_directory().unwrap();
        assert!(!directory.entries.is_empty());
        assert!(
            !shared.is_empty(),
            "Ref.vdf should have shared axis refs between section 3 and section 5"
        );
        // Every shared ref should be a valid slot table entry.
        let slot_set: HashSet<u32> = vdf.slot_table.iter().copied().collect();
        for &r in &shared {
            assert!(
                slot_set.contains(&r),
                "shared axis ref {r} should be in slot table"
            );
        }
    }
}
