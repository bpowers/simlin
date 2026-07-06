#!/usr/bin/env python3
"""
VDF X-Ray: structural inspector for Vensim VDF (binary data file) files.

Shares its decoding with the Rust parser (src/simlin-engine/src/vdf.rs) and
the CLI dump tool (src/simlin-cli/src/vdf_dump.rs). See docs/design/vdf.md
for the format specification.

Usage:
    python tools/vdf_xray.py <path.vdf> [--names] [--records] [--sec3..6]
                                        [--ot] [--blocks] [--data] [--all]
                                        [--extract] [--validate]
                                        [--compare OTHER.vdf] [--raw-section N] [--json]
"""

from __future__ import annotations

import argparse
import bisect
from itertools import product
import json
import math
import re
import struct
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Optional

# ---- Constants ----

# The file header spans 0x00..0xA7 (168 bytes) and is followed by Section 0
# magic at 0xA8. Bytes 0x00..0x7F hold the documented fixed-layout header
# (magic, timestamp, OT/lookup/offset-table offsets, time-point count); bytes
# 0x80..0xA7 are an undocumented trailer of zero padding plus one
# runtime-state residue word and a constant `00 00 43 00` tail (see
# docs/design/vdf.md "File header"). Parsers locate Section 0 by scanning for
# the section magic starting at 0x80, so this constant is the minimum file
# length needed before the scan is safe -- not the full documented header
# size.
FILE_HEADER_SIZE = 0x80
FILE_HEADER_DOCUMENTED_END = 0xA8
SECTION_HEADER_SIZE = 24
RECORD_SIZE = 64
SECTION3_ENTRY_WORDS = 27

# Section 1 (or section 0 in dataset VDFs) begins with a 12-byte preamble
# followed by three 64-byte "header" blocks (string-pool pointer array and
# misc runtime state). Real 64-byte variable metadata records start at
# data_offset + 12 + 3*64 = data_offset + 204. Validated across 40 fixtures.
RECORD_PREAMBLE_BYTES = 12
RECORD_HEADER_BLOCKS = 3
RECORD_REGION_START_OFFSET = RECORD_PREAMBLE_BYTES + RECORD_HEADER_BLOCKS * RECORD_SIZE

# The slot table is followed by a single terminator word before the name-table
# section magic. Constant (0x00430000) across every observed run-file and
# dataset VDF; see docs/design/vdf.md "Slot table".
SLOT_TABLE_TERMINATOR = 0x00430000
# block1[7]: 12-byte preamble + 64-byte block0 + 7 words into block1. This is
# the actively-written slot count on every corpus file (vdf.md "Header region").
SLOT_COUNT_WORD_OFFSET = RECORD_PREAMBLE_BYTES + RECORD_SIZE + 7 * 4

VDF_FILE_MAGIC = bytes([0x7F, 0xF7, 0x17, 0x52])
# Sensitivity/optimization runs (zambaqui fixtures). The ordinary
# eight-section result structures parse like 0x52 files, but bytes after the
# normal sparse-block run contain additional payload we have not decoded.
# Both this tool and the Rust reader (`VdfFile::parse`, which calls it
# `VDF_SENSITIVITY_FILE_MAGIC`) accept the magic and parse these files with
# the 0x52 rules, ignoring the undecoded tail.
VDF_ALT_RESULT_MAGIC = bytes([0x7F, 0xF7, 0x17, 0x53])
VDF_DATASET_MAGIC = bytes([0x7F, 0xF7, 0x17, 0x41])
VDF_SECTION_MAGIC = bytes([0xA1, 0x37, 0x4C, 0xBF])
VDF_SENTINEL = 0xF6800000

OT_CODE_TIME = 0x0F
OT_CODE_STOCK = 0x08
OT_CODE_DYNAMIC = 0x11
OT_CODE_CONST = 0x17

SYSTEM_NAMES = {"Time", "INITIAL TIME", "FINAL TIME", "TIME STEP", "SAVEPER"}

VENSIM_BUILTINS = {
    "abs", "cos", "exp", "integer", "ln", "log", "max", "min", "modulo",
    "pi", "sin", "sqrt", "tan", "step", "pulse", "ramp", "delay", "delay1",
    "delay3", "smooth", "smooth3", "trend", "sum", "prod", "product",
    "vmin", "vmax", "elmcount",
}

NUMERIC_ARRAY_LABEL_RE = re.compile(r"\[(?:\d+)(?:,\d+)*\]$")

SECTION_ROLES = [
    "simulation command",
    "string table + metadata",
    "name table",
    "array directory / zeros",
    "view/group metadata",
    "dimension sets",
    "OT metadata",
    "lookup + OT + data",
]


# ---- Low-level readers ----

def u32(data: bytes, offset: int) -> int:
    return struct.unpack_from("<I", data, offset)[0]


def u16(data: bytes, offset: int) -> int:
    return struct.unpack_from("<H", data, offset)[0]


def f32(data: bytes, offset: int) -> float:
    return struct.unpack_from("<f", data, offset)[0]


def u32_as_f32(val: int) -> float:
    return struct.unpack("<f", struct.pack("<I", val))[0]


# ---- Parsed structures ----

@dataclass
class Section:
    file_offset: int
    region_end: int
    field1: int
    field3: int
    field4: int
    field5: int

    def data_offset(self) -> int:
        return self.file_offset + SECTION_HEADER_SIZE

    def region_data_size(self) -> int:
        return max(0, self.region_end - self.data_offset())


@dataclass
class VdfRecord:
    file_offset: int
    fields: list[int]  # 16 x u32

    def slot_ref(self) -> int:
        return self.fields[12]

    def ot_index(self) -> int:
        """field[11] under the owner-record interpretation."""
        return self.fields[11]

    def is_arrayed(self) -> bool:
        return self.fields[6] not in (0, 5)

    def has_sentinel(self) -> bool:
        return self.fields[8] == VDF_SENTINEL and self.fields[9] == VDF_SENTINEL

    def shape_code(self) -> int:
        """field[6]: 5=scalar; nonzero values can select section-3 shapes."""
        return self.fields[6]


@dataclass
class Section3Entry:
    file_offset: int
    words: list[int]  # 27 x u32

    def index_word(self) -> int:
        return self.words[0]

    def shape_words(self) -> list[int]:
        return [w for w in self.words[1:4] if w > 0]

    def flat_size(self) -> int:
        return self.words[1]

    def axis_sizes(self) -> list[int]:
        shape = self.shape_words()
        if len(shape) == 0:
            return []
        if len(shape) == 1:
            return [shape[0]]
        if len(shape) == 2 and shape[0] == shape[1]:
            return [shape[0]]
        return shape[1:]

    def axis_slot_refs(self) -> list[int]:
        return [w for w in self.words[18:20] if w > 0]

    def terminal_tag(self) -> int:
        return self.words[SECTION3_ENTRY_WORDS - 1]


@dataclass
class Section3Directory:
    data_offset: int
    zero_prefix_words: int
    has_trailing_zero: bool
    entries: list[Section3Entry]


@dataclass
class Section4Entry:
    file_offset: int
    packed_word: int
    refs: list[int]
    index_word: int
    slotted_ref_count: int

    def count_lo(self) -> int:
        return self.packed_word & 0xFFFF

    def count_hi(self) -> int:
        return (self.packed_word >> 16) & 0xFFFF


@dataclass
class Section5SetEntry:
    file_offset: int
    n: int
    marker: int
    refs: list[int]
    slotted_ref_count: int

    def payload_ref_count(self) -> int:
        """
        Count of the non-trailing refs stored in the entry payload.

        The model-edit fixtures show that this field should not be treated as
        a decoded dimension cardinality. It is simply the length of the
        leading ref payload before the trailing axis-anchor refs.
        """
        trailing = 1 + self.marker
        return max(0, len(self.refs) - trailing)


@dataclass
class RefListEntry:
    file_offset: int
    refs: list[int]
    slotted_ref_count: int


@dataclass
class LookupRecord:
    file_offset: int
    words: list[int]  # 13 x u32

    def ot_index(self) -> int:
        return self.words[10]

    def output_width(self) -> int:
        return self.words[11]

    def dependency_ref_word(self) -> int:
        return self.words[12]


@dataclass
class Section6PostRefRecord:
    file_offset: int
    words: list[int]  # 4 x u32

    def maybe_ot_index(self) -> int:
        return self.words[1]

    def maybe_block_width(self) -> int:
        return self.words[2]

    def next_ref_word(self) -> int:
        return self.words[3]


@dataclass
class Section6PostRefChain:
    lookup_record_index: int
    root_ref_word: int
    records: list[Section6PostRefRecord]


@dataclass
class SlotReferenceInfo:
    slot_ref: int
    heuristic_names: list[str]
    signature: Optional[list[int]]
    uses: list[str] = field(default_factory=list)


@dataclass
class SlotTableLayout:
    base: int
    max_offset: int
    distinct_strides: list[int]
    irregular_stride_count: int
    missing_16_slots: int
    contiguous_16: bool


@dataclass
class Section5BridgeMatches:
    exact: list[int] = field(default_factory=list)
    partial: list[int] = field(default_factory=list)
    null_trailing: list[int] = field(default_factory=list)


@dataclass
class RecoveredDimensionSet:
    name: str
    elements: list[str]
    sec5_index: Optional[int]
    source: str = "sec5"


@dataclass
class RecordDimensionAnchor:
    """
    Direct record-field[8] dimension-anchor fact.

    For f[6]=0 dimension metadata records, f[11] is observed as a compact
    dimension/subscript identifier, not an OT start. Element records in the
    same f[8] group use f[11] as their zero-based element index.
    """
    name: str
    group_id: int
    record_index: int
    dimension_id: int
    elements: list[tuple[int, int, str]]
    status: str


@dataclass(frozen=True)
class NameTableEntry:
    name: str
    string_offset: int


@dataclass
class OtRange:
    start: int
    end: int
    record_count: int

    def length(self) -> int:
        return self.end - self.start


@dataclass
class DecodedRecordSpan:
    rec_idx: int
    name_idx: int
    name: str
    start: int
    end: int
    shape_code: int
    sort_key: int
    slot_ref: int
    group_id: int
    has_sentinel: bool
    ot_codes: list[int]

    def length(self) -> int:
        return self.end - self.start


# ---- VDF File ----

@dataclass
class VdfFile:
    data: bytes
    time_point_count: int
    bitmap_size: int
    block_time_point_count: int
    block_bitmap_size: int
    # Header 0x74: the external data file's time-point count ("data grid"),
    # 0 when the model loaded no external data. Exogenous-data blocks (class
    # codes 0x05/0x06/0x0c) carry bitmaps over THIS grid (GH #842).
    data_time_point_count: int
    data_bitmap_size: int
    sections: list[Section]
    names: list[str]
    name_section_idx: Optional[int]
    slot_table: list[int]
    slot_table_offset: int
    records: list[VdfRecord]
    offset_table_start: int
    offset_table_count: int
    first_data_block: int
    header_final_values_offset: int
    header_lookup_mapping_offset: int

    # ---- Offset table ----

    def offset_table_entry(self, index: int) -> Optional[int]:
        if index >= self.offset_table_count:
            return None
        off = self.offset_table_start + index * 4
        if off + 4 > len(self.data):
            return None
        return u32(self.data, off)

    def is_data_block_offset(self, raw: int) -> bool:
        return raw >= self.first_data_block and raw < len(self.data)

    # ---- Section 3: array directory ----

    def parse_section3_directory(self) -> Optional[Section3Directory]:
        if len(self.sections) <= 3:
            return None
        sec = self.sections[3]
        data_off = sec.data_offset()
        end = min(sec.region_end, len(self.data))
        if data_off >= end:
            return Section3Directory(data_off, 0, False, [])

        data_len = end - data_off
        if data_len % 4 != 0:
            return None

        words = [u32(self.data, data_off + i * 4) for i in range(data_len // 4)]
        leading_zeros = 0
        for w in words:
            if w != 0:
                break
            leading_zeros += 1

        best = None
        for zp in range(leading_zeros + 1):
            trailing_candidates = [1, 0] if words and words[-1] == 0 else [0]
            for tw in trailing_candidates:
                remaining = len(words) - zp - tw
                if remaining <= 0 or remaining % SECTION3_ENTRY_WORDS != 0:
                    continue
                n_entries = remaining // SECTION3_ENTRY_WORDS
                entries = []
                valid = True
                for ei in range(n_entries):
                    sw = zp + ei * SECTION3_ENTRY_WORDS
                    ew = sw + SECTION3_ENTRY_WORDS
                    ew_list = words[sw:ew]
                    if ew_list[1] == 0 and ew_list[2] == 0 and ew_list[18] == 0:
                        valid = False
                        break
                    entries.append(Section3Entry(
                        file_offset=data_off + sw * 4,
                        words=ew_list,
                    ))
                if not valid:
                    continue
                if best is None or len(entries) > len(best[2]):
                    best = (zp, tw == 1, entries)

        if best is None:
            return Section3Directory(data_off, leading_zeros, False, [])
        return Section3Directory(data_off, best[0], best[1], best[2])

    # ---- Section 4: view/group entries ----

    def parse_section4_entries(self) -> Optional[list[Section4Entry]]:
        if len(self.sections) <= 4:
            return None
        sec = self.sections[4]
        start = sec.data_offset()
        end = min(sec.region_end, len(self.data))
        if start >= end:
            return []
        region_len = end - start
        if region_len % 4 != 0:
            return None

        words = [u32(self.data, start + i * 4) for i in range(region_len // 4)]
        zero_prefix = 0
        for w in words:
            if w != 0:
                break
            zero_prefix += 1
        if zero_prefix < 2:
            return None

        sec1_data_size = self.sections[1].region_data_size() if len(self.sections) > 1 else 0
        slot_set = set(self.slot_table)

        entries = []
        pos = zero_prefix
        while pos < len(words):
            packed = words[pos]
            if packed == 0:
                break
            lo = packed & 0xFFFF
            hi = (packed >> 16) & 0xFFFF
            ref_count = lo + hi
            if ref_count == 0 or ref_count > 1024:
                break
            refs_start = pos + 1
            refs_end = refs_start + ref_count
            if refs_end >= len(words):
                break
            refs = words[refs_start:refs_end]
            if not all(r > 0 and r % 4 == 0 and r < sec1_data_size for r in refs):
                break
            idx_word = words[refs_end]
            slotted = sum(1 for r in refs if r in slot_set)
            entries.append(Section4Entry(
                file_offset=start + pos * 4,
                packed_word=packed,
                refs=refs,
                index_word=idx_word,
                slotted_ref_count=slotted,
            ))
            pos = refs_end + 1
        return entries

    # ---- Section 5: dimension sets ----

    def parse_section5_sets(self) -> Optional[list[Section5SetEntry]]:
        if len(self.sections) <= 5:
            return None

        sec1_data_size = self.sections[1].region_data_size() if len(self.sections) > 1 else 0
        slot_set = set(self.slot_table)
        sec = self.sections[5]
        start = sec.data_offset()
        end = min(sec.region_end, len(self.data))
        if start >= end:
            return []

        entries = []
        pos = start
        while pos + 8 <= end:
            n = u32(self.data, pos)
            marker = u32(self.data, pos + 4)
            if n == 0 or n > 4096:
                break
            if marker == 0:
                refs_len = n + 1
            elif marker == 1:
                refs_len = n + 2
            else:
                break
            refs_start = pos + 8
            refs_end = refs_start + refs_len * 4
            if refs_end > end:
                break
            refs = [u32(self.data, refs_start + i * 4) for i in range(refs_len)]
            valid_prefix = refs[:-1] if refs else []
            if not all(r > 0 and r % 4 == 0 and r < sec1_data_size for r in valid_prefix):
                break
            slotted = sum(1 for r in refs if r in slot_set)
            entries.append(Section5SetEntry(
                file_offset=pos, n=n, marker=marker, refs=refs,
                slotted_ref_count=slotted,
            ))
            pos = refs_end
        return entries

    def section5_region_last_word_from_field1(self) -> Optional[int]:
        """
        Decode the section-5 region-end pointer from the section header.

        For observed simulation-result fixtures, section 5 header field1 is a
        1-based word index from the section magic to the final word before the
        next section header: `sec5.file_offset + 4 * (field1 - 1)`.
        Degenerate scalar-model section 5 has no data words, so the pointer
        lands on the section header's last word (`field5`).
        """
        if len(self.sections) <= 5:
            return None
        sec = self.sections[5]
        if sec.field1 == 0:
            return None
        return sec.file_offset + 4 * (sec.field1 - 1)

    # ---- Section 6: OT metadata ----

    def _section6_ref_stream_with_skip(self, skip: int) -> tuple[list[RefListEntry], int]:
        sec = self.sections[6]
        start = sec.data_offset() + skip * 4
        end = min(sec.region_end, len(self.data))
        if start >= end:
            return [], start

        sec1_data_size = self.sections[1].region_data_size() if len(self.sections) > 1 else 0
        slot_set = set(self.slot_table)

        entries = []
        pos = start
        while pos + 4 <= end:
            n_refs = u32(self.data, pos)
            if n_refs == 0 or n_refs > 512:
                break
            refs_end = pos + 4 + n_refs * 4
            if refs_end > end:
                break
            refs = [u32(self.data, pos + 4 + i * 4) for i in range(n_refs)]
            if not all(r > 0 and r % 4 == 0 and r < sec1_data_size for r in refs):
                break
            slotted = sum(1 for r in refs if r in slot_set)
            entries.append(RefListEntry(file_offset=pos, refs=refs, slotted_ref_count=slotted))
            pos = refs_end
        return entries, pos

    def parse_section6_ref_stream(self) -> Optional[tuple[int, list[RefListEntry], int]]:
        if len(self.sections) <= 6:
            return None
        sec = self.sections[6]
        skip = max(0, sec.field4 - 1)
        entries, stop = self._section6_ref_stream_with_skip(skip)
        return skip, entries, stop

    def section6_class_code_start(self) -> Optional[int]:
        if self.offset_table_count == 0:
            return None
        fv_off = self.header_final_values_offset
        if fv_off < self.offset_table_count or fv_off > len(self.data):
            return None
        return fv_off - self.offset_table_count

    def section6_class_code_start_from_field1(self) -> Optional[int]:
        """
        Decode the section-6 class-code start from the section header.

        For simulation-result fixtures, section 6 header field1 is a 1-based
        word index from the section magic to the OT class-code byte array:
        `sec6.file_offset + 4 * (field1 - 1)`.
        """
        if len(self.sections) <= 6:
            return None
        sec = self.sections[6]
        if sec.field1 == 0:
            return None
        return sec.file_offset + 4 * (sec.field1 - 1)

    def section7_offset_table_start_from_field1(self) -> Optional[int]:
        """
        Decode the section-7 offset-table start from the section header.

        For simulation-result fixtures, section 7 header field1 is a 1-based
        word index from the section magic to the offset table:
        `sec7.file_offset + 4 * (field1 - 1)`.
        """
        if len(self.sections) <= 7:
            return None
        sec = self.sections[7]
        if sec.field1 == 0:
            return None
        return sec.file_offset + 4 * (sec.field1 - 1)

    def section1_slot_area_offset_from_field1(self) -> Optional[int]:
        """
        Decode the section-1 slot-table start from the section header.

        Section 1's field1 is a 1-based word pointer to the slot table --
        the same field-1 convention sections 6 and 7 use for their payload
        pointers. `slot_table_from_header` builds the table from this pointer
        plus the `block1[7]` count and the terminator cross-check.
        """
        if len(self.sections) <= 1:
            return None
        sec = self.sections[1]
        if sec.field1 == 0:
            return None
        return sec.file_offset + 4 * (sec.field1 - 1)

    def parse_section6_post_ref_records(self) -> Optional[list[Section6PostRefRecord]]:
        """
        Parse the 16-byte record stream between the section-6 ref stream and
        the OT class-code array.

        Observed records are four little-endian u32 words. In Ref.vdf this
        stream is a linked-list node pool rooted from section-6 lookup records:
        word[1] is an OT start, word[2] is a width, and word[3] is a 1-based
        section-6 word pointer to the next node, or zero.
        """
        result = self.parse_section6_ref_stream()
        cc_start = self.section6_class_code_start()
        if result is None or cc_start is None:
            return None

        start = result[2]
        if start > cc_start:
            return None
        byte_len = cc_start - start
        if byte_len == 0:
            return []
        if byte_len % 16 != 0:
            return None

        records: list[Section6PostRefRecord] = []
        for offset in range(start, cc_start, 16):
            records.append(Section6PostRefRecord(
                file_offset=offset,
                words=[u32(self.data, offset + i * 4) for i in range(4)],
            ))
        return records

    def section6_word_ref_to_offset(self, ref_word: int) -> Optional[int]:
        """
        Decode a 1-based section-6 word reference.

        Ref.vdf uses this pointer form for lookup-record dependency-list roots
        and post-ref node `next` links: `sec6.file_offset + 4 * (ref_word - 1)`.
        """
        if len(self.sections) <= 6 or ref_word == 0:
            return None
        sec = self.sections[6]
        offset = sec.file_offset + 4 * (ref_word - 1)
        if offset < sec.file_offset or offset >= sec.region_end:
            return None
        return offset

    def section6_offset_to_word_ref(self, offset: int) -> Optional[int]:
        if len(self.sections) <= 6:
            return None
        sec = self.sections[6]
        rel = offset - sec.file_offset
        if rel < 0 or rel % 4 != 0 or offset >= sec.region_end:
            return None
        return rel // 4 + 1

    def parse_section6_post_ref_chains(self) -> Optional[list[Section6PostRefChain]]:
        """
        Decode post-ref records as lookup-rooted linked lists when possible.

        Each section-6 lookup record's word[12] is either zero or a 1-based
        section-6 word pointer to a post-ref record. Each post-ref record's
        word[3] is the next pointer in that list. This returns None if a
        nonzero pointer targets outside the parsed post-ref record pool or if
        a cycle is encountered.
        """
        records = self.parse_section6_post_ref_records()
        lookup_records = self.section6_lookup_records()
        if records is None or lookup_records is None:
            return None

        by_ref_word: dict[int, Section6PostRefRecord] = {}
        for record in records:
            ref_word = self.section6_offset_to_word_ref(record.file_offset)
            if ref_word is None:
                return None
            by_ref_word[ref_word] = record

        chains: list[Section6PostRefChain] = []
        for lookup_idx, lookup_record in enumerate(lookup_records):
            root = lookup_record.dependency_ref_word()
            if root == 0:
                continue

            chain: list[Section6PostRefRecord] = []
            seen: set[int] = set()
            ref_word = root
            while ref_word != 0:
                if ref_word in seen:
                    return None
                record = by_ref_word.get(ref_word)
                if record is None:
                    return None
                seen.add(ref_word)
                chain.append(record)
                ref_word = record.next_ref_word()

            chains.append(Section6PostRefChain(
                lookup_record_index=lookup_idx,
                root_ref_word=root,
                records=chain,
            ))

        return chains

    def section6_ot_class_codes(self) -> Optional[list[int]]:
        if self.offset_table_count == 0:
            return None
        fv_off = self.header_final_values_offset
        if fv_off >= self.offset_table_count and fv_off <= len(self.data):
            cc_start = fv_off - self.offset_table_count
            codes = list(self.data[cc_start:fv_off])
            if codes and codes[0] == OT_CODE_TIME:
                return codes
        # Fallback via ref stream
        result = self.parse_section6_ref_stream()
        if result is None:
            return None
        _, _, stop = result
        sec = self.sections[6]
        end = min(sec.region_end, len(self.data))
        codes_end = stop + self.offset_table_count
        if codes_end > end:
            return None
        return list(self.data[stop:codes_end])

    def section6_final_values(self) -> Optional[list[float]]:
        if self.offset_table_count == 0:
            return None
        fv_off = self.header_final_values_offset
        fv_end = fv_off + self.offset_table_count * 4
        if fv_off > 0 and fv_end <= len(self.data):
            return [f32(self.data, fv_off + i * 4) for i in range(self.offset_table_count)]
        return None

    def section6_lookup_records(self) -> Optional[list[LookupRecord]]:
        if self.offset_table_count == 0:
            return None
        lm_start = self.header_lookup_mapping_offset
        sec = self.sections[6]
        tail_end = min(sec.region_end, len(self.data))
        if lm_start > 0 and lm_start < tail_end:
            return self._parse_lookup_records(lm_start, tail_end)
        return None

    def _parse_lookup_records(self, start: int, end: int) -> Optional[list[LookupRecord]]:
        if start >= end:
            return []
        suffix = self.data[start:end]
        if len(suffix) < 4 or len(suffix) % 4 != 0:
            return None
        word_count = len(suffix) // 4
        if u32(suffix, len(suffix) - 4) != 0:
            return None
        if (word_count - 1) % 13 != 0:
            return None
        record_count = (word_count - 1) // 13
        out = []
        for i in range(record_count):
            rec_off = i * 13 * 4
            words = [u32(suffix, rec_off + j * 4) for j in range(13)]
            out.append(LookupRecord(file_offset=start + rec_off, words=words))
        return out

    # ---- Record OT ranges ----

    def record_ot_ranges(self) -> list[OtRange]:
        if self.offset_table_count <= 1:
            return []
        start_counts: dict[int, int] = {}
        starts: list[int] = []
        for rec in self.records:
            s = rec.fields[11]
            if s == 0 or s >= self.offset_table_count:
                continue
            if s not in start_counts:
                starts.append(s)
            start_counts[s] = start_counts.get(s, 0) + 1
        starts.sort()
        out = []
        for i, s in enumerate(starts):
            e = starts[i + 1] if i + 1 < len(starts) else self.offset_table_count
            if e > s:
                out.append(OtRange(start=s, end=e, record_count=start_counts.get(s, 0)))
        return out

    # ---- Data extraction ----

    def extract_time_values(self) -> Optional[list[float]]:
        if self.first_data_block + 2 + self.bitmap_size > len(self.data):
            return None
        count = u16(self.data, self.first_data_block)
        if count != self.time_point_count:
            return None
        # A zero-count Time block (header 0x78 and the block's u16 both
        # zeroed by corruption) has no time axis to extract; mirror the Rust
        # reader, which rejects it in extract_time_series.
        if count == 0:
            return None
        data_start = self.first_data_block + 2 + self.bitmap_size
        if data_start + count * 4 > len(self.data):
            return None
        return [f32(self.data, data_start + i * 4) for i in range(count)]

    def _block_positions_for_time_values(
        self, time_values: list[float], grid_count: int,
    ) -> list[int]:
        if not time_values:
            return []
        if grid_count == len(time_values):
            return list(range(len(time_values)))
        if len(time_values) == 1:
            return [0]

        step = time_values[1] - time_values[0]
        if abs(step) < 1e-12:
            return list(range(len(time_values)))
        if any(abs((time_values[i] - time_values[i - 1]) - step) > 1e-5
               for i in range(1, len(time_values))):
            return list(range(len(time_values)))

        # Some files save only a suffix of the full output grid. The variable
        # block bitmaps still cover the full grid, while the Time block stores
        # only the selected save points. Derive the grid origin from the final
        # saved time so extraction samples the same absolute time positions.
        origin = time_values[-1] - (grid_count - 1) * step
        positions: list[int] = []
        for value in time_values:
            pos = int(round((value - origin) / step))
            positions.append(pos)
        return positions

    @staticmethod
    def _data_grid_positions(time_values: list[float], grid_count: int) -> list[int]:
        """
        Map saved Time values onto positions of a `grid_count`-point DATA
        grid (header 0x74; the external data file's time axis).

        The run file stores only the grid's point count -- its time values
        live in the external data file and are not recoverable from the run
        file alone (negative result, GH #842). Approximation, mirrored
        bit-for-bit by the Rust reader's
        `data_grid_positions_for_time_values`: the grid is assumed to span
        the saved run uniformly, anchored at the first saved time, with
        floor semantics (zero-order hold). Exact when the data file's axis
        spans the run horizon (zambaqui "old runs"); values are correct but
        interior placement dilated when the data ends early (zambaqui
        baserun's gdp deflator). First/last placement is exact either way.
        """
        if not time_values:
            return []
        t_first = time_values[0]
        t_last = time_values[-1]
        if grid_count <= 1 or t_last <= t_first:
            return [0] * len(time_values)
        scale = (grid_count - 1) / (t_last - t_first)
        return [math.floor((value - t_first) * scale + 1e-6) for value in time_values]

    def _block_bitmap_layout(self, block_offset: int, count: int) -> tuple[int, int, Optional[str]]:
        """
        Decode the bitmap width for a data block from its declared value count.

        Files mix blocks on up to three time grids: the saved grid (header
        0x78), the block grid (0x7C; wider on saved-suffix runs), and the
        data grid (0x74; the external data file's axis, carried by
        exogenous-data blocks -- GH #842). The C-readable invariant is that
        the block's u16 count equals the popcount of the bitmap bytes that
        precede its f32 payload; candidates are tried in that order and the
        first match wins. The ordering is empirical: saved-before-block is
        narrower-first, but the data width is usually the NARROWEST
        candidate and still must come LAST -- over a thousand ordinary
        saved-grid blocks across the corpora coincidentally popcount-match
        it, while zero exogenous blocks match a wider distinct width.
        Residual risk: an exogenous block with a small count and zero-heavy
        leading payload could match the wider saved width and silently
        mis-place values (no corpus file does); the section-6 class code
        (0x05/0x06/0x0c) is available as a future discriminator. Duplicate
        widths keep the earlier candidate: when ceil(0x74/8) ==
        ceil(0x78/8) with different counts (31 zambaqui old-runs files),
        exogenous blocks reconcile as "saved" and decode with identity
        positions -- exact on that corpus, but a consequence of the dedup,
        not a separate rule.

        Returns `(bitmap_size, grid_count, kind)` with kind one of
        "saved"/"block"/"data", or `None` when no width reconciles -- the
        caller must NaN-fill rather than decode garbage.
        """
        candidates: list[tuple[int, int, str]] = [
            (self.bitmap_size, self.time_point_count, "saved"),
            (self.block_bitmap_size, self.block_time_point_count, "block"),
            (self.data_bitmap_size, self.data_time_point_count, "data"),
        ]
        seen_sizes: set[int] = set()
        for bitmap_size, grid_count, kind in candidates:
            if grid_count == 0 or bitmap_size in seen_sizes:
                continue
            seen_sizes.add(bitmap_size)
            bm_start = block_offset + 2
            bm_end = bm_start + bitmap_size
            if bm_end > len(self.data):
                continue
            bit_count = sum(byte.bit_count() for byte in self.data[bm_start:bm_end])
            if bit_count == count:
                return bitmap_size, grid_count, kind
        return self.block_bitmap_size, self.block_time_point_count, None

    def unreconciled_data_blocks(self) -> list[int]:
        """
        OT indices of data blocks whose bitmap reconciles with NO known time
        grid (saved, block, or data). Extraction NaN-fills these series;
        mirrors the Rust reader's `VdfFile::unreconciled_data_blocks`.
        """
        out: list[int] = []
        for ot in range(self.offset_table_count):
            raw = self.offset_table_entry(ot)
            if raw is None or not self.is_data_block_offset(raw):
                continue
            if raw + 2 > len(self.data):
                continue
            count = u16(self.data, raw)
            _bm, _grid, kind = self._block_bitmap_layout(raw, count)
            if kind is None:
                out.append(ot)
        return out

    def extract_block_series(self, block_offset: int, time_values: list[float]) -> list[float]:
        if block_offset == self.first_data_block:
            extracted = self.extract_time_values()
            if extracted is None:
                return [float("nan")] * len(time_values)
            return extracted

        count = u16(self.data, block_offset)
        bitmap_size, step_count, kind = self._block_bitmap_layout(block_offset, count)
        if kind is None:
            # No known grid width popcounts to the declared count: the block
            # is undecodable; NaN-fill (visibly missing) rather than decode
            # garbage under an assumed width (GH #842).
            return [float("nan")] * len(time_values)
        bm_start = block_offset + 2
        data_start = bm_start + bitmap_size
        if data_start > len(self.data):
            return [float("nan")] * len(time_values)

        series = [float("nan")] * step_count
        data_idx = 0
        last_val = float("nan")
        for time_idx in range(step_count):
            byte_idx = time_idx // 8
            bit_idx = time_idx % 8
            bit_set = (self.data[bm_start + byte_idx] >> bit_idx) & 1 == 1
            if bit_set and data_idx < count:
                val_off = data_start + data_idx * 4
                if val_off + 4 > len(self.data):
                    break
                last_val = f32(self.data, val_off)
                data_idx += 1
            series[time_idx] = last_val

        if step_count == len(time_values):
            positions = list(range(len(time_values)))
        elif kind == "data":
            positions = self._data_grid_positions(time_values, step_count)
        else:
            positions = self._block_positions_for_time_values(time_values, step_count)
        return [
            series[pos] if 0 <= pos < len(series) else float("nan")
            for pos in positions
        ]

    def extract_ot_series(self, ot_idx: int, time_values: list[float],
                          codes: Optional[list[int]] = None,
                          final_values: Optional[list[float]] = None) -> Optional[list[float]]:
        raw = self.offset_table_entry(ot_idx)
        if raw is None:
            return None
        if self.is_data_block_offset(raw):
            return self.extract_block_series(raw, time_values)

        code = codes[ot_idx] if codes is not None and ot_idx < len(codes) else None
        final = final_values[ot_idx] if final_values is not None and ot_idx < len(final_values) else None
        if raw == 0 and code == OT_CODE_DYNAMIC and final is not None and final != 0.0:
            return [float("nan")] * len(time_values)
        const_val = u32_as_f32(raw)
        return [const_val] * len(time_values)


# ---- Slot-to-name helpers ----

def build_direct_slot_to_names(vdf: VdfFile) -> dict[int, list[str]]:
    """
    Direct slot-table pairing: slot_table[i] belongs to names[i].

    This is the structural mapping used for format claims. Edited files can
    retain a stale leading slot entry (see run_9), shifting the naive pairing;
    the extraction path does not depend on this pairing.
    """
    out: dict[int, list[str]] = {}
    for i, slot in enumerate(vdf.slot_table):
        if i < len(vdf.names):
            out.setdefault(slot, []).append(vdf.names[i])
    return out


def resolve_slot_ref(slot_ref: int, slot_to_names: dict[int, list[str]]) -> str:
    names = slot_to_names.get(slot_ref)
    if names:
        return f"{slot_ref}:?{'/'.join(names)}"
    return f"{slot_ref}:?"


def analyze_slot_table_offsets(values: list[int]) -> Optional[SlotTableLayout]:
    if not values:
        return None

    sorted_vals = sorted(set(values))
    base = sorted_vals[0]
    max_offset = sorted_vals[-1]
    strides = [sorted_vals[i + 1] - sorted_vals[i] for i in range(len(sorted_vals) - 1)]
    distinct_strides = sorted(set(strides))

    irregular_stride_count = 0
    missing_16_slots = 0
    for stride in strides:
        if stride != 16:
            irregular_stride_count += 1
        if stride > 16 and stride % 16 == 0:
            missing_16_slots += (stride // 16) - 1
        elif stride > 16:
            missing_16_slots += 1

    contiguous_16 = len(sorted_vals) <= 1 or all(stride == 16 for stride in strides)
    return SlotTableLayout(
        base=base,
        max_offset=max_offset,
        distinct_strides=distinct_strides,
        irregular_stride_count=irregular_stride_count,
        missing_16_slots=missing_16_slots,
        contiguous_16=contiguous_16,
    )


def format_slot_table_layout(layout: Optional[SlotTableLayout]) -> str:
    if layout is None:
        return "(empty)"
    stride_str = ",".join(str(s) for s in layout.distinct_strides) if layout.distinct_strides else "-"
    return (f"base={layout.base} max={layout.max_offset} strides=[{stride_str}] "
            f"contiguous16={layout.contiguous_16} missing16={layout.missing_16_slots}")


def section5_trailing_refs(entry: Section5SetEntry) -> list[int]:
    trailing_count = 1 + entry.marker
    if len(entry.refs) < trailing_count:
        return []
    return entry.refs[-trailing_count:]


def section5_payload_refs(entry: Section5SetEntry) -> list[int]:
    trailing_count = 1 + entry.marker
    if len(entry.refs) < trailing_count:
        return entry.refs.copy()
    return entry.refs[:-trailing_count]


def classify_section5_bridge_matches(sec3: Section3Entry,
                                     sec5_entries: list[Section5SetEntry]) -> Section5BridgeMatches:
    axis_refs = [r for r in sec3.axis_slot_refs() if r > 0]
    axis_set = set(axis_refs)
    matches = Section5BridgeMatches()

    if not axis_refs:
        return matches

    for idx, sec5 in enumerate(sec5_entries):
        trailing = section5_trailing_refs(sec5)
        trailing_pos = [r for r in trailing if r > 0]
        trailing_set = set(trailing_pos)

        if trailing_pos and trailing_pos == axis_refs:
            matches.exact.append(idx)
        elif trailing_pos and len(trailing_pos) == len(axis_refs) and trailing_set == axis_set:
            matches.exact.append(idx)
        elif trailing_pos and trailing_set & axis_set:
            matches.partial.append(idx)
        elif trailing and not trailing_pos:
            matches.null_trailing.append(idx)
    return matches


def classify_section5_shape_matches(sec5: Section5SetEntry,
                                    sec3_entries: list[Section3Entry]) -> Section5BridgeMatches:
    trailing = section5_trailing_refs(sec5)
    trailing_pos = [r for r in trailing if r > 0]
    trailing_set = set(trailing_pos)
    matches = Section5BridgeMatches()

    if not trailing:
        return matches
    if not trailing_pos:
        matches.null_trailing.append(0)
        return matches

    for idx, sec3 in enumerate(sec3_entries):
        axis_refs = [r for r in sec3.axis_slot_refs() if r > 0]
        axis_set = set(axis_refs)

        if axis_refs and trailing_pos == axis_refs:
            matches.exact.append(idx)
        elif axis_refs and len(trailing_pos) == len(axis_refs) and trailing_set == axis_set:
            matches.exact.append(idx)
        elif axis_refs and trailing_set & axis_set:
            matches.partial.append(idx)
    return matches


def section5_exact_axis_sizes(sec5: Section5SetEntry,
                              sec3_entries: list[Section3Entry]) -> list[list[int]]:
    matches = classify_section5_shape_matches(sec5, sec3_entries)
    return [sec3_entries[idx].axis_sizes() for idx in matches.exact]


MDL_LHS_RE = re.compile(r"^(?P<name>[^\[]+?)(?:\[(?P<dims>[^\]]+)\])?$")
MDL_NUMERIC_LITERAL_RE = re.compile(
    r"^[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?$"
)
MDL_SKETCH_VAR_RE = re.compile(r"^10,\d+,([^,]+),")


def build_sec3_index_to_entry(vdf: VdfFile) -> dict[int, Section3Entry]:
    directory = vdf.parse_section3_directory()
    if directory is None:
        return {}
    return {entry.index_word(): entry for entry in directory.entries}


def _section3_uses_predecessor_shape_codes(entries: list[Section3Entry]) -> bool:
    """
    Return true for the Ref-style multi-shape directory layout.

    In that layout, record field[6] values equal the index_word of the
    previous 27-word section-3 entry, while the following physical entry holds
    the actual shape. This looks like a pointer-to-struct-field artifact:
    field[6] stores a self-positional word offset, and the payload of interest
    starts one entry later. Small single-shape files use the generic field[6]
    value 32 and do not exercise this path.
    """
    if len(entries) < 3 or entries[-1].index_word() != 0:
        return False
    index_words = [entry.index_word() for entry in entries[:-1]]
    return all(
        index_words[i + 1] - index_words[i] == SECTION3_ENTRY_WORDS
        for i in range(len(index_words) - 1)
    )


def section3_entry_for_record_shape_code(vdf: VdfFile, shape_code: int) -> Optional[Section3Entry]:
    directory = vdf.parse_section3_directory()
    if directory is None:
        return None

    entries = directory.entries
    if _section3_uses_predecessor_shape_codes(entries):
        for idx, entry in enumerate(entries[:-1]):
            if entry.index_word() == shape_code:
                candidate = entries[idx + 1]
                if candidate.flat_size() > 0:
                    return candidate

    for entry in entries:
        if entry.index_word() == shape_code and entry.flat_size() > 0:
            return entry
    return None


def decoded_record_shape_length(vdf: VdfFile, rec: VdfRecord) -> Optional[int]:
    """
    Fact-only shape span for direct record reports.

    Decoded rules:
    - `5` always means scalar (len=1)
    - an active sec3 `index_word` gives its flat size
    - Ref-style multi-shape directories bind explicit field[6] codes to the
      following physical sec3 entry
    - `32` is the generic array marker and resolves only when section 3 exposes
      a single active flat size

    Records with `f[6]=0` are excluded. They can coincide with an active
    section-3 `index_word=0` in some files, but Ref.vdf shows many such records
    are dimension anchors, dimension elements, builtins, or descriptors rather
    than emitted series owners.
    """
    code = rec.shape_code()
    if code == 0:
        return None
    if code == 5:
        return 1

    entry = section3_entry_for_record_shape_code(vdf, code)
    if entry is not None and entry.flat_size() > 0:
        return entry.flat_size()

    if code == 32:
        idx_to_entry = build_sec3_index_to_entry(vdf)
        active_sizes = sorted({e.flat_size() for e in idx_to_entry.values() if e.flat_size() > 0})
        if len(active_sizes) == 1:
            return active_sizes[0]
    return None


def system_record_name_keys(vdf: VdfFile) -> set[int]:
    """
    Return record f[2] keys whose decoded names are Vensim system variables.

    The f[2] key is a string-pool word offset plus seven, so the numeric keys
    move when a file stores builtin/function names before `Time`. Small files
    often use 9/13/17/21 for INITIAL/FINAL/TIME STEP/SAVEPER, but WRLD3
    SCEN01 shifts them to 17/21/25/29. Treating the numeric values as canonical
    is a rank-style mistake; decode the key to a name first.
    """
    key_to_name_idx = build_record_name_key_to_name_index(vdf)
    return {
        key
        for key, name_idx in key_to_name_idx.items()
        if vdf.names[name_idx] in SYSTEM_NAMES and vdf.names[name_idx] != "Time"
    }


def system_ot_indices_from_records(vdf: VdfFile) -> dict[str, int]:
    """
    Direct system-name -> OT mapping from decoded section-1 records.

    System values are ordinary scalar records in the VDF record table (except
    `Time`, which is always OT[0]). Use those direct records before falling
    back to any ordering/gap reconstruction.
    """
    key_to_name_idx = build_record_name_key_to_name_index(vdf)
    out: dict[str, int] = {}
    for rec in vdf.records:
        name_idx = key_to_name_idx.get(rec.fields[2])
        if name_idx is None:
            continue
        name = vdf.names[name_idx]
        if name == "Time" or name not in SYSTEM_NAMES:
            continue
        ot_idx = rec.ot_index()
        if 0 < ot_idx < vdf.offset_table_count:
            out.setdefault(name, ot_idx)
    return out


# ---- Name classification helpers ----

# Names excluded from the visible user-variable candidate set. Some stdlib
# helper names can still own runtime OT entries and are handled by record-based
# paths; they are not user-facing model variables.
VENSIM_MODULE_NAMES = {"IN", "INI", "OUTPUT"}
VENSIM_STDLIB_HELPERS = {"DEL", "LV1", "LV2", "LV3", "ST", "RT1", "RT2", "DL"}


def _is_visible_model_name(name: str) -> bool:
    if classify_name(name):
        return False
    if name in VENSIM_MODULE_NAMES or name in VENSIM_STDLIB_HELPERS:
        return False
    return True


def _valid_record_dimension_group_id(group_id: int) -> bool:
    return group_id not in (0, VDF_SENTINEL)


def decoded_record_dimension_anchors(vdf: VdfFile) -> list[RecordDimensionAnchor]:
    """
    Return direct dimension-anchor facts from record field[8] grouping.

    In observed array fixtures, dimension anchors and their element records
    share record field[8]. Element records carry field[12]=124, field[10]=0,
    zero-based field[11] element indices, and do not carry the field[14]
    sentinel used by dimension anchors. This is deliberately stricter than
    "same group id" so view/unit/helper groups do not become dimensions.

    Ref.vdf's `scenario` dimension stores two late element records in a
    compact alternate layout: field[12] is the dimension group id, field[15]
    is the zero-based element index, and field[6] is the direct name key.
    Those records are still pointed to by section 5 and participate in the
    same group-id catalog.

    A returned anchor is a fact even when its element catalog is incomplete.
    `status == "complete"` is the narrower condition used for labeling array
    elements without guessing.
    """
    key_to_name_idx = build_record_name_key_to_name_index(vdf)
    if not key_to_name_idx:
        return []

    candidate_dims: dict[int, list[tuple[int, str, int]]] = {}
    element_groups: dict[int, list[tuple[int, int, str]]] = {}

    for rec_idx, rec in enumerate(vdf.records):
        if rec.fields[6] == 0:
            group_id = rec.fields[8]
            if not _valid_record_dimension_group_id(group_id):
                continue
            name_idx = key_to_name_idx.get(rec.fields[2])
            if name_idx is None:
                continue
            name = vdf.names[name_idx]
            if not _is_visible_model_name(name):
                continue

            if (
                rec.fields[12] == 124
                and rec.fields[10] == 0
                and rec.fields[14] != VDF_SENTINEL
                and rec.fields[11] < 4096
            ):
                element_groups.setdefault(group_id, []).append(
                    (rec.fields[11], rec_idx, name)
                )
                continue

            if rec.fields[14] == VDF_SENTINEL:
                candidate_dims.setdefault(group_id, []).append((rec_idx, name, rec.fields[11]))
            continue

        alt_group_id = rec.fields[12]
        alt_name_idx = key_to_name_idx.get(rec.fields[6])
        if (
            _valid_record_dimension_group_id(alt_group_id)
            and alt_name_idx is not None
            and rec.fields[10] == 0
            and rec.fields[11] == 0
            and rec.fields[13] == VDF_SENTINEL
            and rec.fields[14] != VDF_SENTINEL
            and rec.fields[15] < 4096
        ):
            name = vdf.names[alt_name_idx]
            if _is_visible_model_name(name):
                element_groups.setdefault(alt_group_id, []).append(
                    (rec.fields[15], rec_idx, name)
                )

    anchors: list[RecordDimensionAnchor] = []
    for group_id in sorted(set(candidate_dims) | set(element_groups)):
        raw_elements = element_groups.get(group_id, [])
        by_index: dict[int, str] = {}
        by_record: dict[int, int] = {}
        duplicate_index = False
        for element_idx, _, name in raw_elements:
            previous = by_index.get(element_idx)
            if previous is not None and previous.lower() != name.lower():
                duplicate_index = True
            else:
                by_index[element_idx] = name
        for element_idx, rec_idx, _ in raw_elements:
            by_record.setdefault(element_idx, rec_idx)

        ordered_indices = sorted(by_index)
        ordered_elements = [
            (element_idx, by_record[element_idx], by_index[element_idx])
            for element_idx in ordered_indices
        ]

        candidates: list[tuple[int, str, int]] = []
        seen_candidates: set[str] = set()
        for rec_idx, name, dimension_id in sorted(candidate_dims.get(group_id, [])):
            key = name.lower()
            if key in seen_candidates:
                continue
            seen_candidates.add(key)
            candidates.append((rec_idx, name, dimension_id))

        if len(candidates) != 1:
            status = "ambiguous-anchor"
        elif duplicate_index:
            status = "duplicate-element-index"
        elif not ordered_indices:
            status = "no-elements"
        elif ordered_indices != list(range(len(ordered_indices))):
            status = "noncontiguous-elements"
        elif len(ordered_indices) < 2:
            status = "partial-single-element"
        else:
            status = "complete"

        for rec_idx, name, dimension_id in candidates:
            anchors.append(RecordDimensionAnchor(
                name=name,
                group_id=group_id,
                record_index=rec_idx,
                dimension_id=dimension_id,
                elements=ordered_elements,
                status=status,
            ))

    return anchors


def section3_axis_ref_to_dimension_anchor(vdf: VdfFile) -> dict[int, RecordDimensionAnchor]:
    """
    Map section-3 axis refs to decoded dimension anchors.

    The "axis slot ref" name is historical. In observed array fixtures these
    words are section-1 word pointers to field[9] of the dimension-anchor
    record, not slot-table refs:

        sec1.data_offset + 4 * axis_ref == anchor_record.file_offset + 9 * 4

    That is a direct C-struct pointer shape and resolves same-cardinality
    axes without guessing from cardinality.
    """
    if len(vdf.sections) <= 1:
        return {}

    sec1_data_offset = vdf.sections[1].data_offset()
    out: dict[int, RecordDimensionAnchor] = {}
    for anchor in decoded_record_dimension_anchors(vdf):
        if anchor.record_index >= len(vdf.records):
            continue
        record = vdf.records[anchor.record_index]
        field9_offset = record.file_offset + 9 * 4
        rel = field9_offset - sec1_data_offset
        if rel < 0 or rel % 4 != 0:
            continue
        axis_ref = rel // 4
        out.setdefault(axis_ref, anchor)
    return out


def sec5_anchor_binding(
    vdf: VdfFile,
) -> list[tuple[RecordDimensionAnchor, Section5SetEntry, int]]:
    """
    Pair section-5 entries with record dimension anchors by shared f[8] order.

    Validated across Ref.vdf, subscripts.vdf, and run_7/8/9/10.vdf: sorting
    record dimension anchors by record field[8] ascending produces a sequence
    whose length and cardinalities line up with the section-5 entries in file
    order. The random-match probability on Ref.vdf alone (18 dims with an
    identifiable cardinality multiset) is about 2e-10, so we treat this as
    structural, not coincidental.

    Returns one `(anchor, sec5_entry, rank)` tuple per paired dim, where
    `rank` is the anchor's position under f[8]-ascending ordering (also the
    section-5 file-order index). Returns an empty list when the anchor and
    section-5 counts disagree; the count mismatch is itself the diagnostic.
    """
    anchors = sorted(
        decoded_record_dimension_anchors(vdf),
        key=lambda a: a.group_id,
    )
    sec5_entries = vdf.parse_section5_sets() or []
    if len(anchors) != len(sec5_entries):
        return []
    return [(anchor, entry, rank) for rank, (anchor, entry) in enumerate(zip(anchors, sec5_entries))]


def _subsequence_positions(needle: tuple[int, ...], haystack: tuple[int, ...]) -> Optional[list[int]]:
    """
    Return the in-order subsequence positions of `needle` within `haystack`,
    or None if `needle` is not an in-order subsequence.

    Treats zero tokens as inert: a zero in `needle` never binds to a non-zero
    haystack entry. All observed Ref.vdf subrange payloads are strictly
    positive slot-ref tokens, so this behavior is conservative.
    """
    i = 0
    positions: list[int] = []
    for j, token in enumerate(haystack):
        if i < len(needle) and needle[i] == token:
            positions.append(j)
            i += 1
    return positions if i == len(needle) else None


def recover_all_dimension_elements(vdf: VdfFile) -> dict[str, list[str]]:
    """
    Recover every decoded dimension's element list, including subranges.

    Combines two structural signals documented in
    `/docs/design/vdf.md`:

    1. Root dimensions with complete element-record groups: the element
       records (field[8]-matched, f[6]=0, f[14]!=sentinel) carry zero-based
       element indices in f[11], so sorting by f[11] yields the canonical
       element list directly.
    2. Subrange dimensions: their section-5 payload is an in-order
       subsequence of a root dimension's section-5 payload. The subsequence
       positions are the root-relative element indices, which we then use to
       project the root's element list down to the subrange.

    Root detection uses the "no other strict-longer dim has this dim's
    payload as a subseq" rule; when a subrange could bind to either a root
    or another subrange, we prefer the root. Incomplete anchors (partial or
    ambiguous) keep whatever elements we do have; callers can inspect
    `decoded_record_dimension_anchors` for status details.
    """
    pairings = sec5_anchor_binding(vdf)
    if not pairings:
        return {}

    payloads: list[tuple[int, ...]] = [
        tuple(section5_payload_refs(entry)) for _, entry, _ in pairings
    ]
    anchors = [anchor for anchor, _, _ in pairings]

    # An anchor is a root iff no strictly longer anchor's payload contains it
    # as an in-order subsequence. That pattern matches Ref.vdf exactly: seven
    # roots (one per MDL-declared root dim) and eleven subranges. When two
    # same-length payloads are equal, neither is considered a subsequence of
    # the other, so equal-length twins stay roots.
    is_root = [True] * len(anchors)
    for i in range(len(anchors)):
        for j in range(len(anchors)):
            if i == j:
                continue
            if len(payloads[i]) >= len(payloads[j]):
                continue
            if _subsequence_positions(payloads[i], payloads[j]) is not None:
                is_root[i] = False
                break

    results: dict[str, list[str]] = {}

    # Step 1: resolve roots. Complete anchors use their decoded element
    # records. Partial roots keep whatever element facts are present so xray
    # can still expose the partial catalog in diagnostics.
    root_elements: dict[int, list[str]] = {}
    for idx, anchor in enumerate(anchors):
        if not is_root[idx]:
            continue
        elements = [name for _, _, name in anchor.elements]
        card = payloads[idx] and len(payloads[idx]) or 0
        if anchor.status == "complete" and len(elements) == card:
            root_elements[idx] = elements
            results[anchor.name] = elements
        elif len(elements) > 0:
            # Preserve partial roots; they are still decoded facts.
            root_elements[idx] = elements
            results[anchor.name] = elements

    # Step 2: resolve subranges. Prefer root parents over subrange parents;
    # the "bottom" vs "lower+layers" tie in Ref.vdf is the canonical test.
    for idx, anchor in enumerate(anchors):
        if is_root[idx]:
            continue
        parent_idx: Optional[int] = None
        parent_positions: Optional[list[int]] = None
        # Pass 1: look only at roots.
        for root_i, root_payload in enumerate(payloads):
            if not is_root[root_i]:
                continue
            if root_i not in root_elements:
                continue
            positions = _subsequence_positions(payloads[idx], root_payload)
            if positions is None:
                continue
            if parent_idx is None:
                parent_idx = root_i
                parent_positions = positions
            else:
                # Multiple root parents: keep the shorter one (more specific)
                # to stay closer to the MDL's declared parent.
                if len(root_payload) < len(payloads[parent_idx]):
                    parent_idx = root_i
                    parent_positions = positions
        if parent_idx is None:
            # Pass 2: fall back to subrange parents.
            for other_i, other_payload in enumerate(payloads):
                if other_i == idx or is_root[other_i]:
                    continue
                if anchors[other_i].name not in results:
                    continue
                positions = _subsequence_positions(payloads[idx], other_payload)
                if positions is None:
                    continue
                if parent_idx is None or len(other_payload) < len(payloads[parent_idx]):
                    parent_idx = other_i
                    parent_positions = positions

        if parent_idx is None or parent_positions is None:
            continue
        parent_name = anchors[parent_idx].name
        parent_list = results.get(parent_name)
        if parent_list is None:
            continue
        if any(pos >= len(parent_list) for pos in parent_positions):
            continue
        results[anchor.name] = [parent_list[pos] for pos in parent_positions]

    return results


def _recover_record_dimension_sets(vdf: VdfFile) -> list[RecoveredDimensionSet]:
    """
    Recover dimension element lists from record field[8] grouping plus the
    sec5-payload subsequence subrange rule.

    Step 1 consumes `decoded_record_dimension_anchors(..., status=complete)`:
    root dimensions with a complete element-record set yield their labels
    directly. Step 2 adds subrange dims via `recover_all_dimension_elements`
    (see that function's docstring for the subsequence rule). Incomplete
    anchors that cannot be resolved through either path are left out so
    callers can still see the unresolved anchor facts.
    """
    dims: list[RecoveredDimensionSet] = []
    seen: set[str] = set()
    for anchor in decoded_record_dimension_anchors(vdf):
        if anchor.status != "complete":
            continue
        by_index = {element_idx: name for element_idx, _, name in anchor.elements}
        ordered_indices = sorted(by_index)
        if ordered_indices != list(range(len(ordered_indices))):
            continue
        if len(ordered_indices) < 2:
            continue
        # Deduplicate same-named complete anchors, keeping the first (a
        # dimension appears once in the recovered set). This mirrors the Rust
        # reader's `section5_dims.rs::complete_root_dimension_sets`; without it
        # a duplicate-named complete anchor would leave two same-name entries
        # here, and the cardinality-match label fallback below
        # (`len(matches) == 1`) would then see two matches and drop to numeric
        # labels while the deduped Rust reader emitted real ones -- a parity
        # divergence. No corpus file has such duplicates (both readers stay in
        # lockstep), so this is defense-in-depth that keeps them aligned.
        if anchor.name.lower() in seen:
            continue

        dims.append(RecoveredDimensionSet(
            name=anchor.name,
            elements=[by_index[i] for i in ordered_indices],
            sec5_index=None,
            source="record-field8",
        ))
        seen.add(anchor.name.lower())

    # Layer subrange recovery over the top. This fills in dims whose element
    # records are not present in the VDF (Ref.vdf's 11 subrange dims).
    subrange_elements = recover_all_dimension_elements(vdf)
    for name, elements in subrange_elements.items():
        if name.lower() in seen:
            continue
        if len(elements) < 1:
            continue
        dims.append(RecoveredDimensionSet(
            name=name,
            elements=elements,
            sec5_index=None,
            source="sec5-subsequence",
        ))
        seen.add(name.lower())
    return dims


def _recover_sec5_dimension_sets(vdf: VdfFile) -> list[RecoveredDimensionSet]:
    """RECONSTRUCTION HEURISTIC: recover the original single-section-5
    dimension layout.

    The straightforward case is the old single-dimension layout: one sec5
    entry, one non-metadata payload ref naming the dimension, and the next `n`
    simple visible names after that anchor providing the element labels.

    Edited models can leave multiple sec5 entries with stale/stuttering refs.
    Those are left unresolved instead of guessed through. The authoritative
    dimension-element path is `_recover_record_dimension_sets` (record
    field[8] grouping); this function remains only as a fallback for the
    simple single-entry layout.
    """
    sec5_entries = vdf.parse_section5_sets() or []
    if len(sec5_entries) != 1:
        return []

    slot_to_names = build_direct_slot_to_names(vdf)
    entry = sec5_entries[0]
    payload_names: list[str] = []
    seen_payload: set[str] = set()
    for slot_ref in section5_payload_refs(entry):
        for name in slot_to_names.get(slot_ref, []):
            if not _is_visible_model_name(name):
                continue
            key = name.lower()
            if key in seen_payload:
                continue
            seen_payload.add(key)
            payload_names.append(name)

    if len(payload_names) != 1:
        return []

    anchor = payload_names[0]
    try:
        anchor_idx = vdf.names.index(anchor)
    except ValueError:
        return []

    elements: list[str] = []
    seen_elements: set[str] = set()
    for name in vdf.names[anchor_idx + 1:]:
        if not _is_visible_model_name(name):
            continue
        if " " in name:
            break
        key = name.lower()
        if key in seen_elements or key == anchor.lower():
            continue
        seen_elements.add(key)
        elements.append(name)
        if len(elements) == entry.n:
            break

    if len(elements) != entry.n:
        return []

    return [RecoveredDimensionSet(name=anchor, elements=elements, sec5_index=0)]


def _recover_dimension_sets(vdf: VdfFile) -> list[RecoveredDimensionSet]:
    """
    Recover dimension names and element labels through decoded structural paths.

    Record field[8] grouping is preferred because it directly pairs dimension
    anchors with zero-based element records in both `subscripts.vdf` and
    `Ref.vdf`. The older single-section-5 path remains as a fallback for
    fixtures where the record grouping is absent.
    """
    dims = _recover_record_dimension_sets(vdf)
    seen = {dim.name.lower() for dim in dims}
    for dim in _recover_sec5_dimension_sets(vdf):
        if dim.name.lower() in seen:
            continue
        dims.append(dim)
        seen.add(dim.name.lower())
    return dims


def _vensim_sort_key(name: str) -> str:
    """Vensim sorts names case-insensitively."""
    return name.lower()


def _name_looks_lookupish(name: str) -> bool:
    """
    Lexical test for lookup/graphical-function names.

    Matches any name containing the space-prefixed substrings " lookup" or
    " table", or the phrase "graphical function" (case-insensitive) --
    exactly mirroring Rust `vdf::is_lookupish_name`. The space prefix keeps
    ordinary words that merely embed the letters from matching ("stable
    population" is not a table). This is the model-free reader's best-effort
    identification of names that label section-6 lookup-record entries.

    The format itself does not store an owner-vs-descriptor tag (see
    `docs/design/vdf.md` "Appendix: the owner/descriptor discriminator").
    The decoded forward link is structural:
    a graphical-function descriptor record's `f[11]` is the zero-based
    index into the section-6 lookup-record array, and that array is in
    case-insensitive alphabetical order of the lookup-definition names. A
    reader with the model trivially identifies the descriptor records;
    a model-free reader has to recognize lookup-def names from the name
    table -- this lexical test is the workable approximation. It is
    correct on every fixture except `Ref.vdf` (where descriptor names like
    `RS N2O` are abbreviations that don't carry the keyword); the
    `f[10]`-highest fallback in `identify_descriptor_records` covers that.
    """
    lower = name.lower()
    return " lookup" in lower or " table" in lower or "graphical function" in lower


@dataclass
class ResidualOverlapComponent:
    """
    A residual OT-overlap component: two or more DIFFERENTLY-named decoded
    spans that still claim a shared OT slot after descriptor peeling and the
    standalone lookup-only drop. Stage 1 drops every span in the component
    from emission (honest missing data over a silent alphabetical first-claim
    win); the component is retained here so a later stage can re-resolve it.

    Mirrors Rust `record_results.rs::ResidualOverlapComponent`.
    """
    span_indices: list[int]
    contested_ots: list[int]


@dataclass
class DescriptorIdentification:
    """
    Result of identifying graphical-function descriptor records.

    `standalone_drop_veto_fired` / `standalone_drop_vetoed_candidates` record
    when the standalone lookup-only drop's per-file coherence veto withheld
    candidates (SimService/Base.vdf is the canonical case) -- a silent veto
    would let a future file quietly resurrect ghost columns.

    `residual_overlap_components` records the still-conflicted spans dropped by
    the residual-overlap floor: every span in each component is added to
    `descriptor_indices` and therefore NOT emitted, so no OT slot is resolved
    by alphabetical emission order. Retained as data so a later re-resolution
    stage can narrow the drop before falling back to this floor.
    """
    descriptor_indices: set[int]
    used_f10_fallback: bool
    standalone_drop_veto_fired: bool = False
    standalone_drop_vetoed_candidates: int = 0
    residual_overlap_components: list[ResidualOverlapComponent] = field(default_factory=list)


@dataclass
class StandaloneDropOutcome:
    """
    Outcome of the standalone lookup-only descriptor detection: the records
    to drop, plus the per-file coherence veto diagnostics (`veto_fired` with
    the number of physically-gated candidates the veto withheld).
    """
    dropped: set[int]
    veto_fired: bool = False
    vetoed_candidates: int = 0


def identify_descriptor_records(
    vdf: VdfFile,
    spans: list[DecodedRecordSpan],
) -> DescriptorIdentification:
    """
    Identify graphical-function descriptor records from `decoded_record_spans`.

    Background. Vensim's writer stores graphical-function definitions
    ("descriptor" records) and their consuming variables ("owner" records)
    side-by-side in section 1, with `f[11]` as an *untagged* union: for
    owners it is the OT-block start, for descriptors it is the zero-based
    index into the section-6 lookup-record array (and that array is in
    case-insensitive alphabetical order of the lookup-def names). The reader
    is expected to already know which records are descriptors because
    Vensim has the compiled model. The on-disk format does not store the
    discriminator -- a field-by-field analysis (see `vdf.md` "Appendix:
    the owner/descriptor discriminator") confirms no byte, bit, or
    `(f0, f1)` combination distinguishes the two.

    Algorithm. For each `f[11] == k` group (`k` in `[1, lookup_count)`)
    with two or more spans:
    1. **Lookup-def name test.** If exactly one span's name is lexically
       lookupish (matches `_name_looks_lookupish`) it is the descriptor.
       This catches every overlap in the corpus *except* on `Ref.vdf`,
       where descriptor names are model-domain abbreviations.
    2. **Highest-`f[10]` fallback.** When the lookup-def name test is
       ambiguous, the span with the highest `f[10]` (sort-key) is treated
       as the descriptor. Exact on `lookup_ex`, all `econ`, and `Ref.vdf`
       (35/35 Ref pairs); imprecise on WRLD3 SCEN01 (13/55 conflict pairs)
       because `f[10]` is view-local. Sets the `used_f10_fallback` flag.

    Once a record is identified as a descriptor, its true binding is the
    *decoded forward link*: `lookup_record[f[11]].word[10]` is the
    evaluated-output OT, `word[5..6]` are the section-7 x/y array offsets,
    `word[12]` is the optional dependency-chain root.
    """
    n_lookups = len(vdf.section6_lookup_records() or [])
    if n_lookups == 0:
        # No lookup records to forward-link, so the overlap-path descriptor peel
        # and the standalone lookup-only drop are no-ops -- but the residual
        # pass is name/OT-based and MUST still run: a lookup-free file can carry
        # the stale-f[11] owner-vs-owner conflict this guards against, and
        # skipping it would emit duplicate columns for a contested slot (GH
        # #844). phase-1 descriptors are empty here, so nothing is un-peeled and
        # `descriptor_indices` is exactly the re-resolution's drop set -- the
        # same result the Rust reader's unconditional residual pass produces.
        residual_components = residual_overlap_components(spans, set())
        resolution = resolve_residual_components(
            spans, residual_components, set(), RESIDUAL_ORDERING_GATE,
            RESIDUAL_ORDERING_MIN_PAIRS,
        )
        return DescriptorIdentification(
            descriptor_indices=set(resolution.dropped),
            used_f10_fallback=False,
            standalone_drop_veto_fired=False,
            standalone_drop_vetoed_candidates=0,
            residual_overlap_components=resolution.unresolved_components,
        )

    # Build OT-slot -> spans-claiming-it. Spans that overlap (share any OT slot
    # with another span) are descriptor-pair candidates. Note: descriptors
    # sometimes have arrayed shapes that *cross* owner ranges, so a descriptor
    # at `f[11]==k` may not literally share `f[11]` with its colliding owners
    # (e.g. Ref.vdf `RS PFC` at f[11]=120 with shape len 7 spans OT[120..127)
    # which overlaps `C in Biomass` at f[11]=119 / shape len 3 spanning
    # OT[119..122) on OT[120..122)). Span-level overlap detection catches this;
    # an `f[11]`-only check does not.
    by_slot: dict[int, list[DecodedRecordSpan]] = {}
    for span in spans:
        for ot in range(span.start, span.end):
            by_slot.setdefault(ot, []).append(span)

    # Connected components of overlapping spans (union-find on span indices).
    span_index = {id(span): i for i, span in enumerate(spans)}
    parent = list(range(len(spans)))

    def find(x: int) -> int:
        while parent[x] != x:
            parent[x] = parent[parent[x]]
            x = parent[x]
        return x

    def union(x: int, y: int) -> None:
        px, py = find(x), find(y)
        if px != py:
            parent[px] = py

    for slot_spans in by_slot.values():
        if len(slot_spans) >= 2:
            base_idx = span_index[id(slot_spans[0])]
            for span in slot_spans[1:]:
                union(base_idx, span_index[id(span)])

    # A span participates in overlap iff some OT in its range has 2+ claimants
    # (equivalently, iff its union-find root is shared with another span).
    overlapping_span_ids = {
        id(span)
        for slot_spans in by_slot.values() if len(slot_spans) >= 2
        for span in slot_spans
    }
    components: dict[int, list[DecodedRecordSpan]] = {}
    for i, span in enumerate(spans):
        if id(span) in overlapping_span_ids:
            components.setdefault(find(i), []).append(span)

    descriptor_indices: set[int] = set()
    used_f10_fallback = False

    for component in components.values():
        # Iteratively peel off descriptor records until the component is
        # internally non-overlapping. The decoded forward link constrains
        # candidates to records whose `f[11]` is in `[0, lookup_count)`; each
        # iteration picks the lookup-def-name match (lexical test, 1-of-N) or,
        # failing that, the highest-`f[10]` candidate (Ref.vdf-style fallback).
        active = list(component)
        while True:
            # Recompute slot coverage of remaining active spans and find which
            # ones still participate in an overlap.
            comp_by_slot: dict[int, list[DecodedRecordSpan]] = {}
            for span in active:
                for ot in range(span.start, span.end):
                    comp_by_slot.setdefault(ot, []).append(span)
            still_overlapping = {
                id(span)
                for slot_spans in comp_by_slot.values() if len(slot_spans) >= 2
                for span in slot_spans
            }
            if not still_overlapping:
                break
            candidates = [
                span for span in active
                if id(span) in still_overlapping
                and 0 <= vdf.records[span.rec_idx].fields[11] < n_lookups
            ]
            if not candidates:
                # Owner-only overlap with no descriptor candidate: leave the
                # component alone. The precision report will surface the
                # residual `record-span-overlap` blocker.
                break
            lookupish = [span for span in candidates if _name_looks_lookupish(span.name)]
            if len(lookupish) == 1:
                desc = lookupish[0]
            else:
                desc = max(candidates, key=lambda s: (s.sort_key, s.rec_idx))
                used_f10_fallback = True
            descriptor_indices.add(desc.rec_idx)
            active = [span for span in active if span.rec_idx != desc.rec_idx]

    # Standalone (non-overlapping) descriptors: a lookup-only variable Vensim
    # saves only as a descriptor record (a graphical-function definition). The
    # overlap path above never sees it (it collides with nothing), so it would
    # otherwise decode at its spurious f[11]-as-OT-start ghost slot. A bare
    # lookup is a table, not a time series, so recognise it and DROP it -- its
    # values, where they matter, are carried by the consumer variables that
    # call it (separately-emitted owners). Mirrors the Rust reference
    # (record_results.rs `standalone_lookup_only_descriptors`).
    lookup_records = vdf.section6_lookup_records() or []
    overlapping_indices = {
        i for i, span in enumerate(spans) if id(span) in overlapping_span_ids
    }
    standalone = standalone_lookup_only_descriptors(
        spans=spans,
        f11_by_span=[vdf.records[span.rec_idx].fields[11] for span in spans],
        overlapping=overlapping_indices,
        peeled_descriptors=set(descriptor_indices),
        n_lookups=n_lookups,
        lookup_word10=[rec.ot_index() for rec in lookup_records],
        lookup_word11=[rec.output_width() for rec in lookup_records],
        class_codes=vdf.section6_ot_class_codes() or [],
        ot_count=vdf.offset_table_count,
    )
    descriptor_indices.update(standalone.dropped)

    # Residual-overlap re-resolution. After peeling overlap-path descriptors and
    # dropping standalone lookup-only tables, some differently-named spans can
    # STILL claim a shared OT slot -- an owner-vs-owner conflict no structural
    # signal resolved. Compute those components as data, then re-resolve each
    # from scratch (lexical peel of table names + the alphabetical-ordering
    # oracle), recovering the real owners and dropping only the ghosts. Whatever
    # the oracle cannot adjudicate is honest-dropped and surfaced on the
    # diagnostics. Mirrors the Rust reference (record_results.rs
    # `residual_overlap_components` + `resolve_residual_components`).
    phase1_descriptors = set(descriptor_indices)
    residual_components = residual_overlap_components(spans, descriptor_indices)
    resolution = resolve_residual_components(
        spans, residual_components, phase1_descriptors, RESIDUAL_ORDERING_GATE,
        RESIDUAL_ORDERING_MIN_PAIRS,
    )
    descriptor_indices -= resolution.readmitted
    descriptor_indices |= resolution.dropped

    return DescriptorIdentification(
        descriptor_indices=descriptor_indices,
        used_f10_fallback=used_f10_fallback,
        standalone_drop_veto_fired=standalone.veto_fired,
        standalone_drop_vetoed_candidates=standalone.vetoed_candidates,
        residual_overlap_components=resolution.unresolved_components,
    )


def standalone_lookup_only_descriptors(
    spans: list[DecodedRecordSpan],
    f11_by_span: list[int],
    overlapping: set[int],
    peeled_descriptors: set[int],
    n_lookups: int,
    lookup_word10: list[int],
    lookup_word11: list[int],
    class_codes: list[int],
    ot_count: int,
) -> StandaloneDropOutcome:
    """
    Identify *standalone* (non-overlapping) graphical-function descriptor
    records to DROP; returns a `StandaloneDropOutcome` with their `rec_idx`
    set plus the coherence-veto diagnostics.

    A bare graphical function is a table, not a time series: Vensim saves no
    series for it, only a descriptor record whose f[11] is a section-6
    lookup-record index (not an OT start). `identify_descriptor_records` only
    peels descriptors that sit in an overlapping OT component; a lookup-only
    variable saved only as a descriptor collides with nothing, so it would
    otherwise decode at its spurious f[11]-as-OT-start ghost slot (a stock
    slot holding 0/garbage; see docs/design/vdf.md "Standalone
    graphical-function descriptors").

    This pure function (functional core) detects the descriptor conservatively
    to avoid dropping a real owner:
    - the span is NOT in `overlapping` (the connected-component peeling path
      owns the overlapping case);
    - its f[11] is a valid section-6 lookup-record index (< n_lookups) -- the
      structural pre-condition for the forward link;
    - every f[11]-as-OT-start ghost slot carries the STOCK class code (0x08).
      A graphical function is never a stock, so landing on stock slots is the
      spurious-owner telltale; a legitimate non-stock owner whose f[11] is
      coincidentally < n_lookups carries a 0x11 (dynamic) etc. code and is
      left untouched;
    - the forward link `lookup_record[f[11]].word[10]` is a valid data OT
      (1 <= ot < ot_count) with owner class codes across
      [word[10], word[10] + span_len), and for an arrayed descriptor
      (span_len > 1) the forward width (word[11]) equals the element count.
      These confirm f[11] really indexes this variable's graphical function
      rather than coincidentally landing in the lookup-index range;
    - consumer corroboration: the forward link must be the exact START of a
      decoded span of exactly the descriptor's length that can actually be
      an emitted owner: spans that are themselves standalone candidates and
      spans already peeled as overlap-path descriptors (`peeled_descriptors`)
      are excluded as corroborators -- a dropped ghost cannot vouch for
      another drop, and two real stocks must not mutually corroborate each
      other's (wrong) drop. A lookup's consumer is by definition a real
      saved variable, so a genuine bare-lookup descriptor's forward link
      resolves to a decoded consumer span (10/10 on Ref.vdf). A real stock
      whose own OT start is coincidentally < n_lookups points, through the
      unrelated lookup it accidentally indexes, at an arbitrary OT that is
      usually not a span start (3 of the 4 SimService/Base.vdf
      false-positive stocks fail here);
    - per-file coherence: if ANY candidate that passes the physical gates
      above fails consumer corroboration, NO standalone drop happens at all.
      A writer that emits standalone bare-lookup descriptors does so
      coherently (every one corroborates), while a file whose standalone
      f[11] < n_lookups population is stocks-by-coincidence (SimService: the
      leading alphabetical stock block collides with the lookup-index range
      because both OT stock allocation and the lookup array are alphabetical)
      will contain uncorroborated candidates, vetoing the set. The veto is
      NOT silent: the outcome carries `veto_fired` and the withheld-candidate
      count, surfaced on `DescriptorIdentification` and
      `NamedResultsDiagnostics`.

    Record-field discrimination is impossible here: the SimService
    false-positive stocks carry f[14] == 0xf6800000 just like descriptors
    (they are lookup-associated stocks), while two genuine Ref.vdf
    descriptors (Ozone precursor forcings, "OC, BC, and bio aerosol
    forcings") carry graph-metadata floats in f[8]/f[9]/f[14] instead of
    sentinels -- no field separates the populations (see vdf.md "Appendix:
    the owner/descriptor discriminator").

    Not matched here (the model-free reader cannot safely distinguish them
    from real owners, so they are left as-is): the `rs_hfc*` family, whose
    forward link is a wider shared 2-D consumer (word[11] != span_len), and a
    descriptor whose forward link is Time/0.
    """
    # Corroboration index: span start -> [(span index, length)].
    span_starts: dict[int, list[tuple[int, int]]] = {}
    for i, span in enumerate(spans):
        span_starts.setdefault(span.start, []).append((i, span.length()))

    # Pass 1: candidates passing the physical gates, with their forward OT.
    candidates: list[tuple[int, int, int]] = []  # (span idx, rec_idx, fwd)
    for i, span in enumerate(spans):
        if i in overlapping:
            continue
        span_len = span.length()
        if span_len == 0 or i >= len(f11_by_span):
            continue
        f11 = f11_by_span[i]
        if f11 >= n_lookups:
            continue
        ghost_all_stock = all(
            ot < len(class_codes) and class_codes[ot] == OT_CODE_STOCK
            for ot in range(span.start, span.end)
        )
        if not ghost_all_stock:
            continue
        if f11 >= len(lookup_word10):
            continue
        fwd = lookup_word10[f11]
        if fwd == 0 or fwd >= ot_count:
            continue
        if span_len > 1:
            if f11 >= len(lookup_word11) or lookup_word11[f11] != span_len:
                continue
        if fwd + span_len > ot_count:
            continue
        fwd_block_all_owner = all(
            ot < len(class_codes) and class_codes[ot] in _OWNER_OT_CLASS_CODES
            for ot in range(fwd, fwd + span_len)
        )
        if not fwd_block_all_owner:
            continue
        candidates.append((i, span.rec_idx, fwd))

    # Pass 2: consumer corroboration. Corroborators are restricted to spans
    # that can actually be emitted owners: a standalone candidate (which
    # might itself be dropped) and an overlap-peeled descriptor are excluded,
    # so a dropped ghost never vouches for a drop and two candidates never
    # mutually corroborate.
    candidate_span_idxs = {i for i, _, _ in candidates}
    dropped: set[int] = set()
    any_uncorroborated = False
    for i, rec_idx, fwd in candidates:
        span_len = spans[i].length()
        corroborated = any(
            j != i
            and length == span_len
            and j not in candidate_span_idxs
            and spans[j].rec_idx not in peeled_descriptors
            for j, length in span_starts.get(fwd, [])
        )
        if corroborated:
            dropped.add(rec_idx)
        else:
            any_uncorroborated = True

    # Per-file coherence: one uncorroborated candidate means the file's
    # standalone f[11] < n_lookups population is owners-by-coincidence,
    # so nothing is dropped. Refusing to drop is the safe failure mode --
    # and the veto is reported, not silent.
    if any_uncorroborated:
        return StandaloneDropOutcome(
            dropped=set(),
            veto_fired=True,
            vetoed_candidates=len(candidates),
        )
    return StandaloneDropOutcome(dropped=dropped)


def residual_overlap_components(
    spans: list[DecodedRecordSpan],
    dropped: set[int],
) -> list[ResidualOverlapComponent]:
    """
    Detect residual OT-overlap among the decoded spans that survive descriptor
    peeling and the standalone lookup-only drop.

    `identify_descriptor_records` resolves the owner/descriptor f[11] union for
    the overlaps a structural signal can adjudicate (the lexical lookup-def
    name test, the section-6 forward link). On the 2007-era SimService writer
    that is not enough: it emits section-1 records for model variables that
    were NOT saved in the run (data variables, lookup/table definitions,
    supplementary vars), each carrying a stale f[11] (plausibly the variable's
    slot in the full runtime array, while the file's OT holds only saved
    variables). The stale f[11]-as-OT-start spans land on arbitrary saved
    slots; most fail the `f11 < n_lookups` candidacy gate, so the overlap peel
    can neither remove them nor detect that it failed, and the component exits
    still conflicted (see docs/design/vdf.md "Residual OT-overlap").

    Emitting both spans would let alphabetical column order silently pick the
    OT owner and scatter one variable's data under another variable's name.
    This returns the residual components; the caller drops every span in them.

    Conflict is DIFFERENT-name only: two spans of the same name that overlap
    are the ordinary same-variable duplicate the emitter's per-name dedup
    already resolves (lowest start wins), not a cross-variable ownership
    conflict. Every span sharing a genuinely-contested slot (one carrying two
    distinct names) is pulled into the component and dropped, since the whole
    slot is ambiguous. Mirrors Rust
    `record_results.rs::residual_overlap_components`.
    """
    # slot -> surviving span indices claiming it.
    by_slot: dict[int, list[int]] = {}
    for i, span in enumerate(spans):
        if span.rec_idx in dropped:
            continue
        for ot in range(span.start, span.end):
            by_slot.setdefault(ot, []).append(i)

    parent = list(range(len(spans)))

    def find(x: int) -> int:
        while parent[x] != x:
            parent[x] = parent[parent[x]]
            x = parent[x]
        return x

    def union(x: int, y: int) -> None:
        px, py = find(x), find(y)
        if px != py:
            parent[px] = py

    conflicted: set[int] = set()
    contested_slot_reps: list[tuple[int, int]] = []  # (ot, rep span idx)
    for ot, slot_spans in by_slot.items():
        distinct_names = any(
            spans[slot_spans[a]].name != spans[slot_spans[b]].name
            for a in range(len(slot_spans))
            for b in range(a + 1, len(slot_spans))
        )
        if not distinct_names:
            continue
        base = slot_spans[0]
        for other in slot_spans:
            union(base, other)
            conflicted.add(other)
        contested_slot_reps.append((ot, base))

    if not conflicted:
        return []

    spans_by_root: dict[int, list[int]] = {}
    for i in conflicted:
        spans_by_root.setdefault(find(i), []).append(i)
    ots_by_root: dict[int, list[int]] = {}
    for ot, rep in contested_slot_reps:
        ots_by_root.setdefault(find(rep), []).append(ot)

    components: list[ResidualOverlapComponent] = []
    for root, span_indices in spans_by_root.items():
        span_indices.sort()
        contested_ots = sorted(set(ots_by_root.get(root, [])))
        components.append(ResidualOverlapComponent(
            span_indices=span_indices,
            contested_ots=contested_ots,
        ))
    # Deterministic order (dict iteration is insertion-ordered but the roots
    # are not meaningful): by first contested OT, then by first span index.
    components.sort(key=lambda c: (
        c.contested_ots[0] if c.contested_ots else math.inf,
        c.span_indices[0] if c.span_indices else math.inf,
    ))
    return components


# Minimum fraction of adjacent uncontested-owner pairs (sorted by OT) that must
# be alphabetically ordered for the ordering oracle to run on a file. Vensim
# allocates OT slots in case-insensitive alphabetical order within a run, so a
# genuine run file's uncontested owners are overwhelmingly ordered (the four
# probed corpus files measure 98.6-99.6%; the few percent of breaks are run
# boundaries). A file below this bar does not exhibit the invariant, so the
# oracle abstains and every residual span is honest-dropped (Stage 1 semantics).
# The bar is a principled "overwhelming majority", not tuned to any one file.
RESIDUAL_ORDERING_GATE = 0.95

# Minimum number of adjacent uncontested-owner pairs a file must have for the
# ordering oracle to run. Below it the oracle abstains (the component
# honest-drops with diagnostics), because a ratio measured over one or two pairs
# carries no real evidence -- with fewer than two owners the ratio is vacuously
# 1.0 and would pass the gate with zero measured support. The exact value is NOT
# corpus-supported: any floor between 2 and ~800 is indistinguishable, since the
# only component-bearing files (the two SimService Base.vdf) measure ~840 pairs.
# It is cheap fail-safe insurance -- abstention costs only recovery, never
# correctness -- and below ~20 pairs the 0.95 ratio already demands near-perfect
# ordering; this floor just forbids the degenerate zero/one-pair case outright.
# Mirrors Rust `record_results.rs::RESIDUAL_ORDERING_MIN_PAIRS`.
RESIDUAL_ORDERING_MIN_PAIRS = 8


@dataclass
class ResidualResolution:
    """
    Outcome of re-resolving the residual-overlap components.

    `dropped` are record indices the re-resolution drops (ghosts + honest-drop
    fallback). `readmitted` are record indices that phase 1 peeled but the
    re-resolution recovered as real owners (e.g. `c Identified Oil Reserve`).
    `unresolved_components` are the residual components' honest-dropped
    remainder (the spans the oracle could not adjudicate), surfaced on the
    diagnostics; empty when every component fully resolves. Mirrors Rust
    `record_results.rs::ResidualResolution`.
    """
    dropped: set[int]
    readmitted: set[int]
    unresolved_components: list[ResidualOverlapComponent]


def _residual_alphabetical_consistency(uncontested_by_ot: list[DecodedRecordSpan]) -> float:
    """Fraction of adjacent OT-sorted uncontested-owner pairs that are
    name-ordered -- the measured strength of Vensim's alphabetical OT
    allocation on this file. 1.0 when there are fewer than two owners."""
    if len(uncontested_by_ot) < 2:
        return 1.0
    ok = sum(
        1 for a, b in zip(uncontested_by_ot, uncontested_by_ot[1:])
        if _vensim_sort_key(a.name) <= _vensim_sort_key(b.name)
    )
    return ok / (len(uncontested_by_ot) - 1)


def _ordering_ok(prev_key: Optional[str], next_key: Optional[str], name_key: str) -> bool:
    """
    Ordering verdict for a span given its prev/next uncontested-owner anchor
    bracket (all args are case-insensitive sort keys).

    An INVERTED bracket (`prev_key > next_key`) means a run boundary sits
    between the two anchors, so the next side is unreliable and only the more
    reliable prev side is tested (the cluster-A case, where the next anchor
    `agricultural land in use` sorts before the real `c *` owners). Otherwise
    the name must fall within `[prev, next]`.
    """
    if prev_key is not None and next_key is not None and prev_key > next_key:
        return prev_key <= name_key
    return (prev_key is None or prev_key <= name_key) and (
        next_key is None or name_key <= next_key
    )


def _anchor_prev(starts: list[int], anchors: list[DecodedRecordSpan],
                 start: int) -> Optional[DecodedRecordSpan]:
    """Nearest anchor with OT start strictly before `start` (`anchors` sorted by
    start, `starts` their start list)."""
    j = bisect.bisect_left(starts, start) - 1
    return anchors[j] if j >= 0 else None


def _anchor_next(starts: list[int], anchors: list[DecodedRecordSpan],
                 end: int) -> Optional[DecodedRecordSpan]:
    """Nearest anchor with OT start at or after `end`."""
    j = bisect.bisect_left(starts, end)
    return anchors[j] if j < len(anchors) else None


def _residual_span_is_owner(
    span: DecodedRecordSpan,
    recovered: list[DecodedRecordSpan],
    recovered_starts: list[int],
    uncontested: list[DecodedRecordSpan],
    uncontested_starts: list[int],
) -> bool:
    """
    Ordering-oracle verdict: does `span` sit where Vensim's alphabetical OT
    allocation would put a real owner?

    Anchors come in two tiers. When a RECOVERED real owner (a residual-component
    span already confirmed this pass) brackets the span on BOTH sides, that
    tier wins: recovered reals share the span's interleaved run, so they are the
    reliable same-run evidence (this is what adjudicates `indicated per capita
    fish demand` vs `China future GDP growth rate` at OT 127 -- the recovered
    `Indicated China GDP`@123 / `indicated row Coal demand`@128 bracket, not the
    nearest uncontested owner `cafe history`@124, which is a different run).
    Otherwise the file's uncontested owners are the anchors.
    """
    name_key = _vensim_sort_key(span.name)
    rp = _anchor_prev(recovered_starts, recovered, span.start)
    rn = _anchor_next(recovered_starts, recovered, span.end)
    if rp is not None and rn is not None:
        return _ordering_ok(_vensim_sort_key(rp.name), _vensim_sort_key(rn.name), name_key)
    up = _anchor_prev(uncontested_starts, uncontested, span.start)
    un = _anchor_next(uncontested_starts, uncontested, span.end)
    up_key = _vensim_sort_key(up.name) if up is not None else None
    un_key = _vensim_sort_key(un.name) if un is not None else None
    return _ordering_ok(up_key, un_key, name_key)


def _spans_overlap(a: DecodedRecordSpan, b: DecodedRecordSpan) -> bool:
    return not (a.end <= b.start or b.end <= a.start)


def _spans_conflict(a: DecodedRecordSpan, b: DecodedRecordSpan) -> bool:
    """Genuine residual CONFLICT: overlapping extents AND different names.
    Same-name overlaps are ordinary duplicate records for one variable that the
    per-name emission dedup owns (keep-lowest-start) -- the same rule
    `residual_overlap_components` uses to build the components. A name-blind
    predicate here would honest-drop a same-name duplicate pair that a
    differently-named ghost dragged into a component, losing the variable
    entirely (GH #844). Mirrors Rust `record_results.rs::spans_conflict`."""
    return _spans_overlap(a, b) and a.name != b.name


def resolve_residual_components(
    spans: list[DecodedRecordSpan],
    components: list[ResidualOverlapComponent],
    phase1_descriptors: set[int],
    gate_threshold: float,
    min_pairs: int,
) -> ResidualResolution:
    """
    Re-resolve each residual-overlap component from scratch, recovering the real
    owners and dropping only the ghosts (stale-`f[11]` unsaved-variable records).

    Per component (see docs/design/vdf.md "Residual OT-overlap"):
    (a) discard the component's phase-1 overlap peels that spatially belong to
        it (un-peel: any phase-1 descriptor overlapping a component span, e.g.
        the wrongly f10-peeled `c Identified Oil Reserve`), then
    (b) LEXICALLY peel spans whose names are lookupish (` lookup`/` table`/...),
        WITHOUT the `f11 < n_lookups` gate -- a lookup definition is a table,
        not a series, so its stale `f[11]` cannot forward-link, then
    (c) adjudicate the remaining conflicts with the alphabetical-ordering oracle
        (`_residual_span_is_owner`), iterating a fixpoint so a span confirmed as
        an owner becomes an anchor for its neighbours, then
    (d) honest-drop anything still in conflict (surfaced on the diagnostics).

    The whole procedure is gated per file: the oracle runs only when the
    uncontested owners supply at least `min_pairs` adjacent pairs AND exhibit the
    alphabetical-allocation invariant (ratio >= `gate_threshold`). Otherwise it
    abstains and every residual span is honest-dropped (Stage 1 semantics), so a
    file that does not follow the invariant -- or offers too little evidence to
    tell -- is never mis-adjudicated. (Production passes `RESIDUAL_ORDERING_GATE`
    / `RESIDUAL_ORDERING_MIN_PAIRS`; oracle-mechanics unit tests pass 0.0 / 0 to
    open the gate, exactly as they already open the ratio side.)

    Mirrors Rust `record_results.rs::resolve_residual_components`.
    """
    dropped: set[int] = set()
    readmitted: set[int] = set()
    if not components:
        return ResidualResolution(dropped, readmitted, [])

    comp_span_idx = {i for c in components for i in c.span_indices}
    # Uncontested owners: the clean, non-conflicted owner partition -- the
    # alphabetical reference.
    uncontested = sorted(
        (s for i, s in enumerate(spans)
         if s.rec_idx not in phase1_descriptors and i not in comp_span_idx),
        key=lambda s: s.start,
    )
    uncontested_starts = [s.start for s in uncontested]

    # Gate: abstain (honest-drop all) when the file offers too few
    # uncontested-owner pairs to measure, OR does not exhibit the alphabetical
    # invariant. The pair-count check is FIRST so the vacuous <2-owner case
    # (where the ratio is 1.0) can never pass on zero evidence.
    n_pairs = max(len(uncontested) - 1, 0)
    if n_pairs < min_pairs or _residual_alphabetical_consistency(uncontested) < gate_threshold:
        for c in components:
            for i in c.span_indices:
                dropped.add(spans[i].rec_idx)
        return ResidualResolution(dropped, readmitted, list(components))

    # Per-component active sets: component spans + un-peeled phase-1 descriptors
    # overlapping any component span. A phase-1 descriptor is un-peeled (given a
    # second chance) exactly when it collides with a component's spans.
    comp_active: list[list[DecodedRecordSpan]] = []
    unpeeled_recidx: set[int] = set()
    for c in components:
        active = [spans[i] for i in c.span_indices]
        # Test un-peel candidacy against a SNAPSHOT of the original component
        # spans, not the growing `active` list: a phase-1 descriptor is
        # un-peeled iff it cross-name-conflicts with an ORIGINAL component span,
        # never merely with a previously-un-peeled descriptor (mirrors the Rust
        # `component_spans` snapshot; keeps the two readers bit-identical on a
        # chained-descriptor residual region). A same-name overlap is not a
        # conflict, so it stays dropped -- its owner twin already represents it.
        component_spans = list(active)
        for i, s in enumerate(spans):
            if s.rec_idx in phase1_descriptors and any(
                _spans_conflict(s, cs) for cs in component_spans
            ):
                active.append(s)
                unpeeled_recidx.add(s.rec_idx)
        comp_active.append(active)

    # (a) lexical peel (drop lookupish table names).
    for active in comp_active:
        keep = []
        for s in active:
            if _name_looks_lookupish(s.name):
                dropped.add(s.rec_idx)
            else:
                keep.append(s)
        active[:] = keep

    # (b)+(c) fixpoint: confirm non-overlapping spans as recovered owners, then
    # drop the decisive ghosts, until no component changes.
    recovered: list[DecodedRecordSpan] = []
    changed = True
    while changed:
        changed = False
        # Confirm any span no longer CONFLICTING with another active span (a
        # same-name overlap is not a conflict, so a same-name duplicate pair
        # confirms and survives once its differently-named ghost is dropped).
        for active in comp_active:
            still = []
            for s in active:
                if any(t is not s and _spans_conflict(s, t) for t in active):
                    still.append(s)
                else:
                    recovered.append(s)
                    changed = True
            active[:] = still
        recovered.sort(key=lambda s: s.start)
        recovered_starts = [s.start for s in recovered]
        # Drop decisive ghosts: in a still-conflicting group with at least one
        # ordering-consistent owner, drop the ordering-inconsistent spans.
        for active in comp_active:
            overl = [s for s in active if any(t is not s and _spans_conflict(s, t) for t in active)]
            if not overl:
                continue
            owners = [s for s in overl
                      if _residual_span_is_owner(s, recovered, recovered_starts,
                                                 uncontested, uncontested_starts)]
            ghosts = [s for s in overl if s not in owners]
            if owners and ghosts:
                for s in ghosts:
                    dropped.add(s.rec_idx)
                active[:] = [s for s in active if s not in ghosts]
                changed = True

    # (d) honest-drop the still-conflicted remainder; report it on diagnostics.
    unresolved: list[ResidualOverlapComponent] = []
    for c, active in zip(components, comp_active):
        leftover = [s for s in active if any(t is not s and _spans_conflict(s, t) for t in active)]
        if leftover:
            leftover_idx = {id(s) for s in leftover}
            span_indices = sorted(i for i in c.span_indices if id(spans[i]) in leftover_idx)
            for s in leftover:
                dropped.add(s.rec_idx)
            contested = sorted({ot for a in leftover for b in leftover
                                if a is not b and _spans_conflict(a, b)
                                for ot in range(max(a.start, b.start), min(a.end, b.end))})
            unresolved.append(ResidualOverlapComponent(
                span_indices=span_indices, contested_ots=contested))

    # Un-peeled phase-1 descriptors that were KEPT are readmitted as owners.
    kept_recidx = {s.rec_idx for active in comp_active for s in active} | {s.rec_idx for s in recovered}
    readmitted = {r for r in unpeeled_recidx if r in kept_recidx and r not in dropped}
    return ResidualResolution(dropped, readmitted, unresolved)


def _array_element_labels_for_span(
    vdf: VdfFile,
    span: DecodedRecordSpan,
    dimension_sets: list[RecoveredDimensionSet],
) -> Optional[list[str]]:
    """
    Recover element labels for an arrayed decoded record span, trying in
    order: an attached dimension-anchor record (same-cardinality
    tie-breaker), the section-3 axis-ref -> dimension-anchor binding,
    a unique-cardinality dimension match, and finally the multi-axis
    product of unique-cardinality axis matches.
    """
    if span.length() <= 1:
        return None

    anchor_labels = _array_element_labels_from_dimension_anchor_in_span(
        vdf, span, dimension_sets)
    if anchor_labels is not None:
        return anchor_labels

    axis_ref_labels = _array_element_labels_from_section3_axis_refs(
        vdf,
        span,
        dimension_sets,
    )
    if axis_ref_labels is not None:
        return axis_ref_labels

    matches = [dim.elements for dim in dimension_sets if len(dim.elements) == span.length()]
    if len(matches) == 1:
        return matches[0]

    entry = section3_entry_for_record_shape_code(vdf, span.shape_code)
    if entry is None or entry.flat_size() != span.length():
        return None

    axis_sizes = entry.axis_sizes()
    if len(axis_sizes) <= 1 or math.prod(axis_sizes) != span.length():
        return None

    axes: list[list[str]] = []
    for axis_size in axis_sizes:
        axis_matches = [dim.elements for dim in dimension_sets if len(dim.elements) == axis_size]
        if len(axis_matches) != 1:
            return None
        axes.append(axis_matches[0])

    return [",".join(coords) for coords in product(*axes)]


def _array_element_labels_from_section3_axis_refs(
    vdf: VdfFile,
    span: DecodedRecordSpan,
    dimension_sets: list[RecoveredDimensionSet],
) -> Optional[list[str]]:
    """
    Label an array span using section-3 axis refs bound to dimension anchors.

    This is the direct structural path for same-cardinality axes. Cardinality
    matching alone cannot distinguish `scenario`, `Target`, `lower`, `upper`,
    or `Aggregated Regions` in Ref.vdf, but the section-3 axis words point to
    the corresponding dimension-anchor records.
    """
    entry = _shape_template_entry_for_span(vdf, span)
    if entry is None or entry.flat_size() != span.length():
        return None

    axis_sizes = entry.axis_sizes()
    axis_refs = [ref for ref in entry.axis_slot_refs() if ref > 0]
    if not axis_sizes or len(axis_sizes) != len(axis_refs):
        return None
    if math.prod(axis_sizes) != span.length():
        return None

    anchors_by_axis_ref = section3_axis_ref_to_dimension_anchor(vdf)
    dims_by_name = {dim.name.lower(): dim for dim in dimension_sets}

    axes: list[list[str]] = []
    for axis_size, axis_ref in zip(axis_sizes, axis_refs):
        anchor = anchors_by_axis_ref.get(axis_ref)
        if anchor is None:
            return None
        dim = dims_by_name.get(anchor.name.lower())
        if dim is None or len(dim.elements) != axis_size:
            return None
        axes.append(dim.elements)

    if len(axes) == 1:
        return axes[0]
    return [",".join(coords) for coords in product(*axes)]


def _shape_template_entry_for_span(
    vdf: VdfFile,
    span: DecodedRecordSpan,
) -> Optional[Section3Entry]:
    """
    Resolve the section-3 shape template used by a decoded record span.

    An explicit shape code points directly through the decoded section-3
    bridge. The generic `32` array marker needs the same conservative handling
    as span decoding: it resolves only when exactly one active section-3 entry
    has the span's flat size.
    """
    entry = section3_entry_for_record_shape_code(vdf, span.shape_code)
    if entry is not None and entry.flat_size() == span.length():
        return entry

    if span.shape_code == 32:
        directory = vdf.parse_section3_directory()
        if directory is not None:
            active = [
                candidate
                for candidate in directory.entries
                if candidate.flat_size() == span.length() and candidate.flat_size() > 0
            ]
            if len(active) == 1:
                return active[0]

    return None


def _array_element_labels_from_dimension_anchor_in_span(
    vdf: VdfFile,
    span: DecodedRecordSpan,
    dimension_sets: list[RecoveredDimensionSet],
) -> Optional[list[str]]:
    """
    Use a dimension-anchor record landing inside the span as a
    same-cardinality tie-breaker.

    In the model-edit fixtures, stock records can have sort_key=0 and borrow
    their visible sort anchor from the dimension record whose elements define
    the stock array. That is a structural relation: the anchor record's
    f[11] lands inside the stock's OT span, it has field[6]=0 and a positive
    sort key, carries the dimension-anchor sentinel in field[14] (but not the
    owner sentinel pair in fields 8/9), and its name is one of the decoded
    dimension sets.
    """
    dims_by_name = {
        dim.name.lower(): dim
        for dim in dimension_sets
        if len(dim.elements) == span.length()
    }
    if not dims_by_name:
        return None

    key_to_name_idx = build_record_name_key_to_name_index(vdf)
    matches: list[RecoveredDimensionSet] = []
    for rec in vdf.records:
        if rec.has_sentinel() or rec.fields[10] <= 0:
            continue
        if not span.start <= rec.ot_index() < span.end:
            continue
        if rec.fields[6] != 0 or rec.fields[14] != VDF_SENTINEL:
            continue
        if not _valid_record_dimension_group_id(rec.fields[8]):
            continue
        name_idx = key_to_name_idx.get(rec.fields[2])
        if name_idx is None:
            continue
        dim = dims_by_name.get(vdf.names[name_idx].lower())
        if dim is not None and dim not in matches:
            matches.append(dim)

    if len(matches) != 1:
        return None
    return matches[0].elements


def _recover_dimension_element_names(vdf: VdfFile) -> set[str]:
    """
    Return element labels from structurally decoded dimension sets.

    This intentionally stays conservative: record field[8] groups and the
    simple single-entry section-5 fallback are decoded, but ambiguous same-size
    dimension ownership is handled later by the block-labeling path.
    """
    elements: set[str] = set()
    for dim in _recover_dimension_sets(vdf):
        elements.update(dim.elements)
    return elements


def build_record_name_key_to_name_index(vdf: VdfFile) -> dict[int, int]:
    """
    Map record field[2] values to name-table indices.

    In the observed simulation corpus, record f[2] is the section-2 string-pool
    word offset of the name's first character, plus seven words. Name starts
    are 4-byte aligned in every decoded simulation fixture so far.
    """
    if vdf.name_section_idx is None:
        return {}
    sec = vdf.sections[vdf.name_section_idx]
    data_start = sec.data_offset()
    parse_end = min(sec.region_end, len(vdf.data))
    if not vdf.names:
        return {}

    out: dict[int, int] = {}
    for name_idx, entry in enumerate(_parse_name_table_entries(vdf.data, sec, parse_end)):
        if name_idx >= len(vdf.names):
            break
        start_rel = entry.string_offset - data_start
        if start_rel % 4 == 0:
            out[start_rel // 4 + 7] = name_idx

    return out


_OWNER_OT_CLASS_CODES: frozenset[int] = frozenset({
    0x05,             # input/data-like blocks (29-byte bitmap width on
                      # `risk.vdf`/`risk2.vdf`'s `federal funds rate`,
                      # `inflation rate`)
    OT_CODE_STOCK,    # 0x08 stock-backed
    OT_CODE_DYNAMIC,  # 0x11 dynamic / data-block
    0x16,             # Ref.vdf inline (semantics unresolved, but still real-data)
    OT_CODE_CONST,    # 0x17 constant / inline f32
    0x18,             # Ref.vdf inline
})


def decoded_record_spans(vdf: VdfFile) -> list[DecodedRecordSpan]:
    """
    Return direct record -> name -> OT span facts, without owner selection.

    This deliberately avoids descriptor pruning, name-category filtering,
    and array label guessing. A span here means a record carries:
    - f[2] resolving through the decoded section-2 name key formula;
    - an in-range f[11] under the owner OT-start interpretation;
    - a non-zero f[6] shape code whose span is structurally decoded;
    - and (class-code guard) the OT slot at f[11] holds real saved data
      (class code in `_OWNER_OT_CLASS_CODES`). The guard rejects records
      whose f[11], interpreted as an OT, would land on a class-`0x0f` Time
      slot (only OT[0]) or any unknown future code; on the current corpus
      this is a no-op for the 31 `exact-by-xray` fixtures (their owner-record
      f[11]s already point only to owner-coded slots) and is forward-defensive
      for files where a graphical-function descriptor's `f[11]`-as-lookup-index
      happens to numerically land on a non-owner OT slot.

    Whether that record is an emitted user-facing series owner remains a
    separate question when spans overlap (the field[11] owner/descriptor union;
    see vdf.md "Appendix: the owner/descriptor discriminator"). The owner
    spans on every fixture form a clean,
    no-overlap, all-OT-slots-covered partition once descriptor records are set
    aside (`identify_descriptor_records`).
    """
    key_to_name_idx = build_record_name_key_to_name_index(vdf)
    codes = vdf.section6_ot_class_codes() or []
    spans: list[DecodedRecordSpan] = []
    for rec_idx, rec in enumerate(vdf.records):
        name_idx = key_to_name_idx.get(rec.fields[2])
        if name_idx is None:
            continue
        start = rec.ot_index()
        if start <= 0 or start >= vdf.offset_table_count:
            continue
        length = decoded_record_shape_length(vdf, rec)
        if length is None or length <= 0:
            continue
        end = start + length
        if end > vdf.offset_table_count:
            continue
        # Class-code guard: every OT slot in the span must be a real-data slot.
        # Time (0x0f) is never spanned (start>=1 guards OT[0]); any other code
        # outside the owner set indicates a stale or descriptor-reinterpreted
        # f[11], not a real owner span.
        if codes and any(
            ot_idx < len(codes) and codes[ot_idx] not in _OWNER_OT_CLASS_CODES
            for ot_idx in range(start, end)
        ):
            continue
        spans.append(DecodedRecordSpan(
            rec_idx=rec_idx,
            name_idx=name_idx,
            name=vdf.names[name_idx],
            start=start,
            end=end,
            shape_code=rec.shape_code(),
            sort_key=rec.fields[10],
            slot_ref=rec.slot_ref(),
            group_id=rec.fields[8],
            has_sentinel=rec.has_sentinel(),
            ot_codes=codes[start:end],
        ))
    return spans


@dataclass
class NamedResult:
    """A single named time series from a VDF file."""
    name: str
    ot_index: int
    values: list[float]


@dataclass
class NamedResultsDiagnostics:
    """
    Side-channel diagnostics from `extract_named_results_with_diagnostics`.

    The `used_*` flags record when extraction fell back onto a reconstruction
    step (a step not directly decoded from the file). On the tracked corpus
    `used_descriptor_f10_fallback` fires only on `Ref.vdf` and the
    `standalone_drop_veto_*` fields fire only on the SimService third_party
    fixtures.
    """
    system_variable_record_missing: bool = False
    # Set when graphical-function descriptor identification falls back to the
    # highest-`f[10]` tie-break because the lexical lookup-def-name test was
    # ambiguous on a conflict pair (`Ref.vdf` is the canonical case; the file
    # genuinely does not store a discriminator, see vdf.md "Appendix: the
    # owner/descriptor discriminator").
    used_descriptor_f10_fallback: bool = False
    # Set when the standalone lookup-only drop's per-file coherence veto
    # withheld physically-gated candidates (SimService/Base.vdf is the
    # canonical case). Mirrors DescriptorIdentification; surfaced here so a
    # veto is observable at the extraction surface rather than silent.
    standalone_drop_veto_fired: bool = False
    standalone_drop_vetoed_candidates: int = 0
    # OT indices of data blocks whose bitmap reconciled with NO known time
    # grid (saved 0x78 / block 0x7C / data 0x74): their series are NaN-filled
    # (visibly missing) rather than decoded under a wrong width. Mirrors the
    # Rust reader's `VdfData::unreconciled_ots`; empty on every tracked
    # corpus file (GH #842).
    bitmap_unreconciled_ots: list[int] = field(default_factory=list)
    # Residual OT-overlap: differently-named decoded spans that still claim a
    # shared OT slot after descriptor peeling, the standalone lookup-only drop,
    # AND the residual-overlap re-resolution -- i.e. the conflicts the ordering
    # oracle could not adjudicate and therefore honest-dropped (honest missing
    # data over a silent alphabetical first-claim win). Empty across the whole
    # tracked corpus, including the two SimService Base.vdf files whose
    # stale-f[11] conflicts the oracle now fully recovers; non-empty only when
    # the per-file gate fails or the oracle leaves a conflict unadjudicated.
    # Mirrors the Rust reader's
    # `DescriptorIdentification.residual_overlap_components` (span-index based;
    # the name-resolved analogue is Rust `VdfFile::residual_overlap_diagnostics`).
    residual_overlap: list[ResidualOverlapComponent] = field(default_factory=list)


def extract_named_results_with_diagnostics(
    vdf: VdfFile,
) -> tuple[Optional[list[NamedResult]], NamedResultsDiagnostics]:
    """
    Extract named time series via the **direct record map** plus the decoded
    forward link for graphical-function descriptors.

    Pipeline (no heuristics on the primary path; `f[10]` fallback flagged
    when it fires):

    1. `decoded_record_spans` (record `f[2]`->name, `f[11]`->OT-start,
       `f[6]`->shape, plus class-code guard) yields the structural owner-
       shaped spans -- the writer's direct map.
    2. `identify_descriptor_records` removes graphical-function descriptor
       records via the decoded forward link: a descriptor's `f[11]` is the
       index into the section-6 lookup-record array (alphabetical name
       order), not an OT start. The `f[10]`-highest tie-break flags
       `used_descriptor_f10_fallback` on `Ref.vdf`-style cases where the
       lookup-def names are abbreviations that don't carry the
       "lookup"/"table" keyword.
    3. The remaining owner spans + `Time` at `OT[0]` are the result set.
       System variables are partitioned out and emitted last, matching the
       previous output ordering.

    `#`-signature internal-helper variables (`#alias>SMOOTH#`, `#LV1<...#`,
    `#FUNCNAME(args)#`, ...) own real OT slots and are emitted under their
    decoded names -- consumers wanting a "user-facing" symbol table can strip
    `#`-prefixed names themselves.

    `system_variable_record_missing` flag: set when any system variable
    (`INITIAL TIME`, `FINAL TIME`, `SAVEPER`, `TIME STEP`) lacks a direct
    record in the file, in which case that variable is silently dropped
    from the result set. Dead on every tracked fixture; defensive only.
    """
    diagnostics = NamedResultsDiagnostics()

    time_values = vdf.extract_time_values()
    if time_values is None:
        return None, diagnostics

    spans = decoded_record_spans(vdf)
    desc_id = identify_descriptor_records(vdf, spans)
    if desc_id.used_f10_fallback:
        diagnostics.used_descriptor_f10_fallback = True
    diagnostics.standalone_drop_veto_fired = desc_id.standalone_drop_veto_fired
    diagnostics.standalone_drop_vetoed_candidates = (
        desc_id.standalone_drop_vetoed_candidates
    )
    diagnostics.bitmap_unreconciled_ots = vdf.unreconciled_data_blocks()
    diagnostics.residual_overlap = desc_id.residual_overlap_components

    owner_spans = [s for s in spans if s.rec_idx not in desc_id.descriptor_indices]

    # Partition owner spans into model vs system. If the same name appears more
    # than once (shouldn't happen on the corpus), keep the lowest-start span.
    model_spans: dict[str, DecodedRecordSpan] = {}
    system_spans: dict[str, DecodedRecordSpan] = {}
    for span in owner_spans:
        if span.name == "Time":
            continue
        target = system_spans if span.name in SYSTEM_NAMES else model_spans
        prev = target.get(span.name)
        if prev is None or span.start < prev.start:
            target[span.name] = span

    codes = vdf.section6_ot_class_codes()
    final_values = vdf.section6_final_values()
    dimension_sets = _recover_dimension_sets(vdf)

    results: list[NamedResult] = [
        NamedResult(name="Time", ot_index=0, values=time_values)
    ]

    def emit_span(name: str, span: DecodedRecordSpan) -> None:
        if span.length() == 1:
            series = vdf.extract_ot_series(span.start, time_values, codes, final_values)
            if series is None:
                return
            results.append(NamedResult(name=name, ot_index=span.start, values=series))
            return
        element_labels = _array_element_labels_for_span(vdf, span, dimension_sets)
        for elem_offset in range(span.length()):
            ot_idx = span.start + elem_offset
            series = vdf.extract_ot_series(ot_idx, time_values, codes, final_values)
            if series is None:
                continue
            if element_labels is not None and elem_offset < len(element_labels):
                elem_name = f"{name}[{element_labels[elem_offset]}]"
            else:
                elem_name = f"{name}[{elem_offset}]"
            results.append(NamedResult(name=elem_name, ot_index=ot_idx, values=series))

    # Emit model vars first, alphabetically (Vensim case-insensitive sort).
    for name in sorted(model_spans.keys(), key=_vensim_sort_key):
        emit_span(name, model_spans[name])

    # Emit system vars last, alphabetically. Dead-flag the (corpus-empty)
    # case where a system variable lacks a direct record.
    for name in sorted((n for n in SYSTEM_NAMES if n != "Time"), key=_vensim_sort_key):
        span = system_spans.get(name)
        if span is None:
            diagnostics.system_variable_record_missing = True
            continue
        emit_span(name, span)

    return results, diagnostics


def extract_named_results(vdf: VdfFile) -> Optional[list[NamedResult]]:
    """
    Extract named time series via the direct record map.

    Returns a list of NamedResult for each mapped variable (scalar variables
    get one entry, arrayed variables get one entry per element). System
    variables (FINAL TIME, INITIAL TIME, SAVEPER, TIME STEP) are also included.

    This is a thin wrapper over `extract_named_results_with_diagnostics`;
    callers that need to see fallback flags should use that function
    directly.
    """
    results, _ = extract_named_results_with_diagnostics(vdf)
    return results


def _encode_series_value(value: float):
    """
    Encode one series value for strict JSON output.

    JSON has no NaN or Infinity literals, so:
    - NaN encodes as null (the Rust parity harness decodes null back to NaN);
    - +/- infinity encode as the strings "Infinity" / "-Infinity".
    Finite values pass through as JSON numbers; Python emits the shortest
    round-trip decimal form, so a reader recovers the exact f64 bits.
    """
    if math.isnan(value):
        return None
    if math.isinf(value):
        return "Infinity" if value > 0 else "-Infinity"
    return value


def extract_results_json_payload(paths: list[Path]) -> dict[str, list[dict]]:
    """
    Machine-readable extraction over multiple files: map each path string (as
    given on the command line) to its `extract_named_results` output as
    `[{name, ot_index, values}]`. Consumed by the Rust differential parity
    harness (`src/simlin-engine/tests/integration/vdf_parity.rs`); accepting
    many paths lets one interpreter launch cover a whole corpus. Raises on
    dataset files or extraction failure rather than skipping -- the caller
    treats a nonzero exit as fatal.
    """
    payload: dict[str, list[dict]] = {}
    for path in paths:
        data = path.read_bytes()
        if data[:4] == VDF_DATASET_MAGIC:
            raise ValueError(f"{path}: dataset VDF is not supported by --extract-json")
        vdf = parse_vdf(data)
        results = extract_named_results(vdf)
        if results is None:
            raise ValueError(f"{path}: named-result extraction failed")
        payload[str(path)] = [
            {
                "name": r.name,
                "ot_index": r.ot_index,
                "values": [_encode_series_value(v) for v in r.values],
            }
            for r in results
        ]
    return payload


# ---- Parsing ----

def find_sections(data: bytes) -> list[Section]:
    sections = []
    pos = 0
    while pos + SECTION_HEADER_SIZE <= len(data):
        idx = data.find(VDF_SECTION_MAGIC, pos)
        if idx < 0:
            break
        if idx + SECTION_HEADER_SIZE <= len(data):
            sections.append(Section(
                file_offset=idx,
                region_end=0,
                field1=u32(data, idx + 4),
                field3=u32(data, idx + 12),
                field4=u32(data, idx + 16),
                field5=u32(data, idx + 20),
            ))
        pos = idx + 1

    for i in range(len(sections)):
        sections[i].region_end = sections[i + 1].file_offset if i + 1 < len(sections) else len(data)
    return sections


def find_name_table_section_idx(data: bytes, sections: list[Section]) -> Optional[int]:
    for i, sec in enumerate(sections):
        first_len = (sec.field5 >> 16) & 0xFFFF
        if not (2 <= first_len <= 64):
            continue
        start = sec.data_offset()
        if start + first_len > len(data):
            continue
        text = ""
        for b in data[start:start + first_len]:
            if b == 0:
                break
            text += chr(b)
        if len(text) >= 2 and all(c.isalnum() or c in " _" for c in text):
            return i
    return None


def _decode_visible_name_bytes(raw: bytes) -> Optional[str]:
    text = raw.split(b"\0", 1)[0]
    if not text or not all(0x20 <= b < 0x7f for b in text):
        return None
    return text.decode("ascii")


def _parse_name_table_entries(data: bytes, sec: Section, parse_end: int) -> list[NameTableEntry]:
    entries: list[NameTableEntry] = []
    data_start = sec.data_offset()
    parse_end = min(parse_end, len(data))

    first_len = (sec.field5 >> 16) & 0xFFFF
    if first_len == 0 or data_start + first_len > len(data):
        return entries

    first_name = _decode_visible_name_bytes(data[data_start:data_start + first_len])
    if first_name is None:
        return entries
    entries.append(NameTableEntry(name=first_name, string_offset=data_start))

    pos = data_start + first_len
    while pos + 2 <= parse_end:
        length = u16(data, pos)
        pos += 2
        if length == 0:
            continue
        if pos + length > parse_end:
            break
        if length > 256:
            break
        name = _decode_visible_name_bytes(data[pos:pos + length])
        if name is not None:
            entries.append(NameTableEntry(name=name, string_offset=pos))
        pos += length
    return entries


def parse_name_table(data: bytes, sec: Section, parse_end: int) -> list[str]:
    return [entry.name for entry in _parse_name_table_entries(data, sec, parse_end)]


def slot_table_from_header(data: bytes, header_section: Section,
                           name_table_offset: int) -> tuple[int, list[int]]:
    """
    Decode the slot table deterministically from the section header.

    Mirrors `vdf::slot_table_from_header` in the Rust decoder (the behavioral
    reference; see docs/design/vdf.md "Slot table"):

    - start = the header section's `field1` 1-based word pointer
      (`header_section.file_offset + 4 * (field1 - 1)`);
    - count = `block1[7]` (the u32 at `data_offset() + SLOT_COUNT_WORD_OFFSET`);
    - the table is followed by a single `SLOT_TABLE_TERMINATOR` word and then
      the name-table section magic.

    `header_section` is section 1 for simulation-run files (and would be
    section 0 for dataset files, which this tool does not yet parse).
    These three facts over-determine each other, so the decode cross-checks
    the layout (`start + (count + 1) * 4 == name_table_offset`, terminator
    word present) and returns `(0, [])` on mismatch rather than emitting a
    mis-decoded table.
    """
    if header_section.field1 == 0:
        return 0, []
    slot_start = header_section.file_offset + 4 * (header_section.field1 - 1)
    count_word_offset = header_section.data_offset() + SLOT_COUNT_WORD_OFFSET
    if count_word_offset + 4 > len(data):
        return 0, []
    slot_count = u32(data, count_word_offset)

    terminator_offset = slot_start + slot_count * 4
    if (
        slot_start >= name_table_offset
        or terminator_offset + 4 != name_table_offset
        or terminator_offset + 4 > len(data)
        or u32(data, terminator_offset) != SLOT_TABLE_TERMINATOR
    ):
        return 0, []

    return slot_start, [u32(data, slot_start + i * 4) for i in range(slot_count)]


def find_records(data: bytes, search_start: int, search_end: int) -> list[VdfRecord]:
    """
    Enumerate 64-byte variable metadata records between `search_start`
    (inclusive) and `search_end` (exclusive).

    Callers pass `search_start = sec_data_offset + RECORD_REGION_START_OFFSET`;
    the observed layout stores full records in 64-byte strides from there
    until just before the slot table. Some files leave a short non-record
    trailer before the slot table; the stride walk ignores any residual bytes
    shorter than a full record. Some records carry the sentinel pair
    (0xf6800000 at fields 8 and 9); others -- padding, lookup table metadata,
    subscript elements -- do not.

    The function still anchors the forward walk to the first sentinel pair
    it finds as a cross-check, and scans backward through recordish blocks
    up to (but never past) `search_start`. On well-formed files the fixed
    record-region offset makes the backward scan a no-op; on malformed
    input it prevents emitting garbage aligned against random prefix bytes.
    """
    if search_start >= search_end:
        return []

    # Find first sentinel pair
    first_record_start = None
    pos = search_start
    while pos + 40 <= search_end:
        v0 = u32(data, pos)
        v1 = u32(data, pos + 4)
        if v0 == VDF_SENTINEL and v1 == VDF_SENTINEL:
            first_record_start = max(0, pos - 32)
            break
        pos += 4

    if first_record_start is None:
        return []

    # Scan backwards
    actual_start = first_record_start
    while actual_start >= RECORD_SIZE:
        candidate = actual_start - RECORD_SIZE
        if candidate < search_start:
            break
        f0 = u32(data, candidate)
        s0 = u32(data, candidate + 32)
        s1 = u32(data, candidate + 36)
        if f0 <= 64 or s0 == VDF_SENTINEL or s1 == VDF_SENTINEL:
            actual_start = candidate
        else:
            break

    records = []
    offset = actual_start
    while offset + RECORD_SIZE <= search_end:
        fields = [u32(data, offset + i * 4) for i in range(16)]
        records.append(VdfRecord(file_offset=offset, fields=fields))
        offset += RECORD_SIZE
    return records


def parse_vdf(data: bytes) -> VdfFile:
    if len(data) < FILE_HEADER_SIZE:
        raise ValueError("VDF file too small")
    if data[:4] not in (VDF_FILE_MAGIC, VDF_ALT_RESULT_MAGIC):
        raise ValueError(f"invalid VDF magic: {data[:4].hex()}")

    time_point_count = u32(data, 0x78)
    bitmap_size = math.ceil(time_point_count / 8)
    block_time_point_count = u32(data, 0x7C) if len(data) >= 0x80 else 0
    if block_time_point_count < time_point_count:
        block_time_point_count = time_point_count
    block_bitmap_size = math.ceil(block_time_point_count / 8)
    # Header 0x74: the loaded data file's time-point count (0 when no
    # external data). Exogenous-data blocks live on this grid (GH #842).
    data_time_point_count = u32(data, 0x74) if len(data) >= 0x80 else 0
    data_bitmap_size = math.ceil(data_time_point_count / 8)

    header_fv_off = u32(data, 0x58)
    header_lm_off = u32(data, 0x5C)
    header_ot_off = u32(data, 0x60)

    sections = find_sections(data)
    name_section_idx = find_name_table_section_idx(data, sections)

    names = []
    if name_section_idx is not None:
        ns = sections[name_section_idx]
        names = parse_name_table(data, ns, ns.region_end)

    slot_table_offset, slot_table = (0, [])
    if name_section_idx is not None and len(sections) > 1:
        slot_table_offset, slot_table = slot_table_from_header(
            data, sections[1], sections[name_section_idx].file_offset)

    # Find records. The record region lives at a fixed offset within section
    # 1's data: the first 12 bytes are a preamble and the next three 64-byte
    # blocks are header blocks (string-pool pointer array and misc state).
    # Full 64-byte variable metadata records start at
    # `sec1.data_offset() + 204`; a short residual trailer may sit between the
    # last complete record and `slot_table_offset`.
    sec1_data_start = sections[1].data_offset() if len(sections) > 1 else FILE_HEADER_SIZE
    search_start = sec1_data_start + RECORD_REGION_START_OFFSET

    search_bound = sections[1].region_end if len(sections) > 1 else len(data)
    records_end = slot_table_offset if 0 < slot_table_offset < search_bound else search_bound
    records = find_records(data, search_start, records_end)

    # OT count from header
    if header_lm_off <= header_fv_off:
        raise ValueError("invalid VDF header: lookup mapping offset <= final values offset")
    ot_count = (header_lm_off - header_fv_off) // 4
    if ot_count == 0:
        raise ValueError("VDF header indicates zero OT entries")
    if header_ot_off == 0 or header_ot_off + ot_count * 4 > len(data):
        raise ValueError("VDF header offset table pointer out of bounds")

    first_data_block = u32(data, header_ot_off)

    return VdfFile(
        data=data,
        time_point_count=time_point_count,
        bitmap_size=bitmap_size,
        block_time_point_count=block_time_point_count,
        block_bitmap_size=block_bitmap_size,
        data_time_point_count=data_time_point_count,
        data_bitmap_size=data_bitmap_size,
        sections=sections,
        names=names,
        name_section_idx=name_section_idx,
        slot_table=slot_table,
        slot_table_offset=slot_table_offset,
        records=records,
        offset_table_start=header_ot_off,
        offset_table_count=ot_count,
        first_data_block=first_data_block,
        header_final_values_offset=header_fv_off,
        header_lookup_mapping_offset=header_lm_off,
    )


# ---- Name classification ----

def classify_name(name: str) -> str:
    if name in SYSTEM_NAMES:
        return "system"
    if name.startswith("."):
        return "group"
    if name.startswith("-"):
        return "unit"
    if name.startswith(":"):
        return "meta"
    if name.startswith("#"):
        return "signature"
    if name.startswith('"'):
        return "quoted"
    if len(name) == 1 and not name[0].isalnum():
        return "builtin?"
    if name.lower() in VENSIM_BUILTINS:
        return "builtin?"
    if name.isdigit():
        return "numeric"
    return ""


def ot_code_label(code: int) -> str:
    labels = {
        0x0F: "time",
        0x08: "stock",
        0x11: "dynamic",
        0x16: "0x16",
        0x17: "const",
        0x18: "0x18",
    }
    return labels.get(code, f"0x{code:02x}")


# ---- Display functions ----

def hexdump(data: bytes, base_offset: int, max_bytes: int = 256) -> None:
    show = min(len(data), max_bytes)
    for start in range(0, show, 16):
        end = min(start + 16, show)
        chunk = data[start:end]
        hex_str = " ".join(f"{b:02x}" for b in chunk[:8])
        if len(chunk) > 8:
            hex_str += "  " + " ".join(f"{b:02x}" for b in chunk[8:])
        hex_str = hex_str.ljust(49)
        ascii_str = "".join(chr(b) if 0x20 <= b < 0x7F else "." for b in chunk)
        print(f"  {base_offset + start:08x}: {hex_str}  |{ascii_str}|")
    if len(data) > max_bytes:
        print(f"  ... ({len(data) - max_bytes} more bytes)")


def print_header(vdf: VdfFile, path: str) -> None:
    ts_bytes = vdf.data[4:0x78]
    ts_end = ts_bytes.find(b"\x00")
    if ts_end < 0:
        ts_end = len(ts_bytes)
    timestamp = ts_bytes[:ts_end].decode("ascii", errors="replace")

    print(f"=== VDF File: {path} ===")
    print(f"File size:    {len(vdf.data)} bytes")
    print(f"Magic:        {vdf.data[:4].hex()}")
    print(f"Timestamp:    {timestamp}")
    print(f"Time points:  {vdf.time_point_count}")
    print(f"Bitmap size:  {vdf.bitmap_size} bytes")
    if vdf.block_time_point_count != vdf.time_point_count:
        print(f"Block grid:   {vdf.block_time_point_count} points ({vdf.block_bitmap_size} bitmap bytes)")
    print()

    print("=== Header Offsets ===")
    print(f"  0x58 final_values_offset:    0x{vdf.header_final_values_offset:08x}")
    print(f"  0x5c lookup_mapping_offset:  0x{vdf.header_lookup_mapping_offset:08x}")
    print(f"  0x60 offset_table_offset:    0x{vdf.offset_table_start:08x}")
    print(f"  0x68 extra/result-tail ptr?:  0x{u32(vdf.data, 0x68):08x}")
    print(f"  0x6c save/run marker?:       0x{u32(vdf.data, 0x6c):08x}")
    print(f"  0x70 lookup point pairs:     {u32(vdf.data, 0x70)}")
    print(f"  0x74 data grid points:       {vdf.data_time_point_count}")
    print(f"  0x78 saved time points:      {vdf.time_point_count}")
    print(f"  0x7c block grid points:      {vdf.block_time_point_count}")
    print(f"  OT count (derived):          {vdf.offset_table_count}")
    print(f"  First data block:            0x{vdf.first_data_block:08x}")
    print()


def print_layout(vdf: VdfFile) -> None:
    entries = [(0, f"File header ({FILE_HEADER_DOCUMENTED_END} bytes)")]
    for i, sec in enumerate(vdf.sections):
        role = SECTION_ROLES[i] if i < len(SECTION_ROLES) else "unknown"
        region_size = sec.region_end - sec.file_offset
        entries.append((sec.file_offset, f"Section {i}: {role} (region {region_size}B)"))
    if vdf.records:
        start = vdf.records[0].file_offset
        entries.append((start, f"Records ({len(vdf.records)}, {len(vdf.records) * RECORD_SIZE} bytes)"))
    if vdf.slot_table_offset > 0:
        entries.append((vdf.slot_table_offset,
                        f"Slot table ({len(vdf.slot_table)} entries, {len(vdf.slot_table) * 4} bytes)"))
    entries.append((vdf.offset_table_start,
                    f"Offset table ({vdf.offset_table_count} entries, {vdf.offset_table_count * 4} bytes)"))
    entries.append((vdf.first_data_block, "Data blocks start"))
    entries.append((len(vdf.data), "End of file"))
    entries.sort(key=lambda x: x[0])

    print("=== File Layout ===")
    for off, desc in entries:
        print(f"  0x{off:08x}  {desc}")
    print()


def print_sections(vdf: VdfFile) -> None:
    print(f"=== Sections ({len(vdf.sections)}) ===")
    for i, sec in enumerate(vdf.sections):
        role = SECTION_ROLES[i] if i < len(SECTION_ROLES) else "unknown"
        print(f"\nSection {i} @ 0x{sec.file_offset:08x}  [{role}]")
        print(f"  field1={sec.field1}  field3={sec.field3}  field4={sec.field4}  field5=0x{sec.field5:08x}")
        print(f"  region: 0x{sec.data_offset():08x}..0x{sec.region_end:08x} ({sec.region_data_size()}B data)")

        data_start = sec.data_offset()
        region_end = min(sec.region_end, len(vdf.data))
        if data_start >= region_end:
            print("  (no data / degenerate section)")
            continue
        if vdf.name_section_idx == i:
            print("  (name table -- shown separately)")
        else:
            hexdump(vdf.data[data_start:region_end], data_start)
    print()


def print_names(vdf: VdfFile) -> None:
    slotted = len(vdf.slot_table)
    unslotted = len(vdf.names) - slotted
    print(f"=== Name Table ({len(vdf.names)} names: {slotted} with slots, {unslotted} without) ===")
    for i, name in enumerate(vdf.names):
        cls = classify_name(name)
        if i == slotted and slotted < len(vdf.names):
            print("  --- names without slot table entries ---")
        suffix = f"  ({cls})" if cls else ""
        print(f"  {i:>3}  \"{name}\"{suffix}")
    print()


def print_slots(vdf: VdfFile) -> None:
    if not vdf.slot_table:
        print("=== Slot Table ===\n  (empty)\n")
        return
    sec1_data_start = vdf.sections[1].data_offset() if len(vdf.sections) > 1 else 0
    print(f"=== Slot Table ({len(vdf.slot_table)} entries @ 0x{vdf.slot_table_offset:08x}) ===")
    print(f"  layout: {format_slot_table_layout(analyze_slot_table_offsets(vdf.slot_table))}")
    print(f"  {'Idx':>3}  {'Sec1Off':>7}  {'Name':<36}  {'w[0]':>8} {'w[1]':>8} {'w[2]':>8} {'w[3]':>8}")
    for i, offset in enumerate(vdf.slot_table):
        name = vdf.names[i] if i < len(vdf.names) else "<unnamed>"
        abs_off = sec1_data_start + offset
        if abs_off + 16 <= len(vdf.data):
            w = [u32(vdf.data, abs_off + j * 4) for j in range(4)]
            print(f"  {i:>3}  {offset:>7}  \"{name}\"{'':>{max(0, 34 - len(name))}}  "
                  f"{w[0]:08x} {w[1]:08x} {w[2]:08x} {w[3]:08x}")
        else:
            print(f"  {i:>3}  {offset:>7}  \"{name}\"  (out of bounds)")
    print()


def print_records(vdf: VdfFile) -> None:
    print(f"=== Variable Metadata Records ({len(vdf.records)}) ===")
    if not vdf.records:
        print("  (none)\n")
        return

    slot_to_name: dict[int, str] = {}
    for slot, names in build_direct_slot_to_names(vdf).items():
        if names:
            slot_to_name[slot] = names[0]

    print(f"  SENT = sentinel 0x{VDF_SENTINEL:08x}")
    print("  Known: f[0]=type f[1]=class f[6]=shape f[10]=sort "
          "f[11]=raw owner/lookup union f[12]=slot_ref")
    print()

    # Header
    hdr = f"  {'#':>3} {'offset':>10}"
    for i in range(16):
        hdr += f" {'f'+str(i):>6}"
    hdr += "  class slot"
    print(hdr)

    ot_count = vdf.offset_table_count
    lookup_records = vdf.section6_lookup_records() or []
    for i, rec in enumerate(vdf.records):
        f = rec.fields
        tags: list[str] = []
        if f[0] == 0:
            tags.append("zero")
        if f[1] == 23:
            tags.append("system?")

        shape_length = decoded_record_shape_length(vdf, rec)
        if (
            shape_length is not None
            and shape_length > 0
            and 0 < f[11] < ot_count
            and f[11] + shape_length <= ot_count
        ):
            tags.append(f"owner?={f[11]}")
        elif f[11] > 0 and f[11] >= ot_count:
            tags.append(f"owner-oob?={f[11]}")

        if f[11] < len(lookup_records):
            tags.append(f"lookup?={f[11]}")
        if f[10] > 0:
            tags.append(f"sort={f[10]}")
        cls = " ".join(tags)

        line = f"  {i:>3} 0x{rec.file_offset:08x}"
        for val in f:
            if val == VDF_SENTINEL:
                line += "   SENT"
            else:
                line += f" {val:>6}"
        slot_name = slot_to_name.get(f[12], "")
        if slot_name:
            line += f"  {cls} slot=\"{slot_name}\""
        else:
            line += f"  {cls}"
        print(line)
    print()


def _format_ot_code_span(codes: list[int]) -> str:
    if not codes:
        return "[]"
    counts: dict[int, int] = {}
    for code in codes:
        counts[code] = counts.get(code, 0) + 1
    if len(counts) == 1:
        code = codes[0]
        return f"{len(codes)}x0x{code:02x}/{ot_code_label(code)}"
    return "[" + ", ".join(f"0x{code:02x}/{ot_code_label(code)}" for code in codes) + "]"


def print_section3(vdf: VdfFile) -> None:
    print("=== Section 3 Directory ===")
    directory = vdf.parse_section3_directory()
    if directory is None:
        print("  (unparseable)\n")
        return

    print(f"  zero_prefix_words={directory.zero_prefix_words} entries={len(directory.entries)} "
          f"trailing_zero={directory.has_trailing_zero}")
    if not directory.entries:
        print()
        return

    slot_to_names = build_direct_slot_to_names(vdf)
    axis_ref_anchors = section3_axis_ref_to_dimension_anchor(vdf)
    sec4_entries = vdf.parse_section4_entries()
    sec4_idx_words = set()
    if sec4_entries:
        sec4_idx_words = {e.index_word for e in sec4_entries}

    def axis_ref_label(ref: int) -> str:
        anchor = axis_ref_anchors.get(ref)
        if anchor is not None:
            return f"{ref}:dim:{anchor.name}"
        return resolve_slot_ref(ref, slot_to_names)

    for i, entry in enumerate(directory.entries):
        axis_refs = [axis_ref_label(sr) for sr in entry.axis_slot_refs()]
        sec4_hit = entry.index_word() in sec4_idx_words
        state = "placeholder" if entry.flat_size() == 0 else "active"
        print(f"  {i:>3} @0x{entry.file_offset:08x} idx={entry.index_word()} sec4_hit={sec4_hit} "
              f"state={state} flat={entry.flat_size()} axes={entry.axis_sizes()} "
              f"w10={entry.words[10]} w11={entry.words[11]} "
              f"shape_words={entry.words[:4]} "
              f"axis_refs={axis_refs} raw_axis_refs={entry.axis_slot_refs()} "
              f"tail={entry.terminal_tag()}")
    print()


def print_section4(vdf: VdfFile) -> None:
    print("=== Section 4 Entries ===")
    entries = vdf.parse_section4_entries()
    if entries is None:
        print("  (unparseable)\n")
        return
    print(f"  entries={len(entries)}")
    if not entries:
        print()
        return

    slot_to_names = build_direct_slot_to_names(vdf)
    for i, e in enumerate(entries[:30]):
        refs = [resolve_slot_ref(r, slot_to_names) for r in e.refs]
        print(f"  {i:>3} @0x{e.file_offset:08x} packed=0x{e.packed_word:08x} "
              f"lo={e.count_lo()} hi={e.count_hi()} refs={len(e.refs)} "
              f"idx={e.index_word} slotted={e.slotted_ref_count} refs={refs}")
    if len(entries) > 30:
        print(f"  ... ({len(entries) - 30} more entries)")
    print()


def print_section5(vdf: VdfFile) -> None:
    print("=== Section 5 Sets ===")
    entries = vdf.parse_section5_sets()
    if entries is None:
        print("  (unparseable)\n")
        return
    sec = vdf.sections[5] if len(vdf.sections) > 5 else None
    last_word = vdf.section5_region_last_word_from_field1()
    if sec is not None and last_word is not None:
        print(f"  sets={len(entries)} stream_start=0x{sec.data_offset():08x} "
              f"field1_last_word=0x{last_word:08x} region_end=0x{sec.region_end:08x}")
    else:
        print(f"  sets={len(entries)}")
    if not entries:
        print()
        return

    slot_to_names = build_direct_slot_to_names(vdf)
    directory = vdf.parse_section3_directory()
    sec3_entries = directory.entries if directory else []
    for i, e in enumerate(entries[:16]):
        refs = [resolve_slot_ref(r, slot_to_names) for r in e.refs[:8]]
        payload = section5_payload_refs(e)
        payload_refs = [resolve_slot_ref(r, slot_to_names) for r in payload[:8]]
        trailing_count = 1 + e.marker
        trailing_refs = e.refs[-trailing_count:] if len(e.refs) >= trailing_count else []
        trailing_ref_names = [resolve_slot_ref(r, slot_to_names) for r in trailing_refs]
        exact_axes = section5_exact_axis_sizes(e, sec3_entries) if sec3_entries else []
        sec3_matches = classify_section5_shape_matches(e, sec3_entries) if sec3_entries else None
        print(f"  {i:>3} @0x{e.file_offset:08x} n={e.n} marker={e.marker} "
              f"size={len(e.refs)} payload_refs={e.payload_ref_count()} "
              f"slotted={e.slotted_ref_count} refs(head)={refs} "
              f"payload={payload_refs} raw_refs={e.refs} "
              f"trailing={trailing_ref_names} raw_trailing={trailing_refs} "
              f"sec3_exact={sec3_matches.exact if sec3_matches else []} "
              f"exact_axes={exact_axes}")
    if len(entries) > 16:
        print(f"  ... ({len(entries) - 16} more sets)")
    print()


def print_section6_ref_stream(vdf: VdfFile) -> None:
    print("=== Section 6 Ref Stream ===")
    result = vdf.parse_section6_ref_stream()
    if result is None or not result[1]:
        if result is None:
            print("  (none)\n")
        else:
            print(f"  skip_words={result[0]} entries=0 stop=0x{result[2]:08x}\n")
        return

    skip, entries, stop = result
    slot_to_names = build_direct_slot_to_names(vdf)
    slot_to_name_flat: dict[int, str] = {}
    for s, names in slot_to_names.items():
        slot_to_name_flat[s] = names[0] if names else ""

    all_slotted = sum(1 for e in entries if e.slotted_ref_count == len(e.refs))
    none_slotted = sum(1 for e in entries if e.slotted_ref_count == 0)
    print(f"  skip_words={skip} entries={len(entries)} stop=0x{stop:08x} "
          f"slotted(all/none)={all_slotted}/{none_slotted}")
    for i, e in enumerate(entries[:12]):
        refs = [resolve_slot_ref(r, slot_to_names) for r in e.refs]
        print(f"  {i:>3} @0x{e.file_offset:08x} n={len(e.refs)} "
              f"slot_hits={e.slotted_ref_count} raw_refs={e.refs} refs={refs}")
    if len(entries) > 12:
        print(f"  ... ({len(entries) - 12} more entries)")
    print()


def print_section6_post_ref_records(vdf: VdfFile, *, max_records: int = 48) -> None:
    print("=== Section 6 Post-Ref 16-Byte Records ===")
    records = vdf.parse_section6_post_ref_records()
    result = vdf.parse_section6_ref_stream()
    cc_start = vdf.section6_class_code_start()
    if records is None or result is None or cc_start is None:
        print("  (unparseable)\n")
        return

    start = result[2]
    print(f"  region=0x{start:08x}..0x{cc_start:08x} bytes={cc_start - start} records={len(records)}")
    print("  layout: w1=OT start, w2=width; w3 is a 1-based section-6 word ref to the next node")
    if not records:
        print()
        return

    codes = vdf.section6_ot_class_codes() or []
    # Owner hints come from the direct record map, with graphical-function
    # descriptor records set aside so a descriptor's f[11]-as-OT ghost span
    # never labels a node.
    owner_hints: dict[tuple[int, int], list[str]] = {}
    spans = decoded_record_spans(vdf)
    descriptor_indices = identify_descriptor_records(vdf, spans).descriptor_indices
    for span in spans:
        if span.rec_idx in descriptor_indices:
            continue
        owner_hints.setdefault((span.start, span.length()), []).append(span.name)

    group_counts: dict[tuple[int, int], int] = {}
    for rec in records:
        key = (rec.words[1], rec.words[2])
        group_counts[key] = group_counts.get(key, 0) + 1
    repeated = sorted(
        ((count, key) for key, count in group_counts.items() if count > 1),
        reverse=True,
    )
    if repeated:
        print("  repeated (w1,w2) groups:")
        for count, key in repeated[:12]:
            w1, w2 = key
            names = owner_hints.get((w1, w2), [])
            label = f" owner?={names[:3]}" if names else ""
            print(f"    {count:>3}x w1=0x{w1:08x}({w1}) w2={w2}{label}")
        if len(repeated) > 12:
            print(f"    ... ({len(repeated) - 12} more repeated groups)")

    chains = vdf.parse_section6_post_ref_chains()
    if chains is not None:
        length_counts: dict[int, int] = {}
        linked_records = 0
        for chain in chains:
            chain_len = len(chain.records)
            linked_records += chain_len
            length_counts[chain_len] = length_counts.get(chain_len, 0) + 1
        distribution = ", ".join(
            f"{count}x len={length}" for length, count in sorted(length_counts.items())
        )
        print(f"  lookup dependency chains={len(chains)} linked_records={linked_records} "
              f"lengths=[{distribution}]")

    print("  records:")
    for idx, rec in enumerate(records[:max_records]):
        w0, w1, w2, w3 = rec.words
        ot_label = ""
        if 0 <= w1 < vdf.offset_table_count:
            code = codes[w1] if w1 < len(codes) else None
            code_label = f" code=0x{code:02x}/{ot_code_label(code)}" if code is not None else ""
            ot_label = f" OT?={w1}{code_label}"
        names = owner_hints.get((w1, w2), [])
        owner_label = f" owner?={names[:3]}" if names else ""
        next_label = ""
        next_offset = vdf.section6_word_ref_to_offset(w3)
        if next_offset is not None:
            next_label = f" next=0x{next_offset:08x}"
        print(f"  {idx:>3} @0x{rec.file_offset:08x} "
              f"w=[{w0:08x} {w1:08x} {w2:08x} {w3:08x}]"
              f"{ot_label} width={w2}{next_label}{owner_label}")
    if len(records) > max_records:
        print(f"  ... ({len(records) - max_records} more records)")
    print()


def print_ot_codes(vdf: VdfFile) -> None:
    print("=== Section 6 OT Class Codes ===")
    codes = vdf.section6_ot_class_codes()
    if codes is None:
        print("  (none)\n")
        return

    stock_count = sum(1 for c in codes[1:] if c == OT_CODE_STOCK)
    first_non_stock = None
    for i, c in enumerate(codes[1:], 1):
        if c != OT_CODE_STOCK:
            first_non_stock = i
            break

    counts: dict[int, int] = {}
    for c in codes:
        counts[c] = counts.get(c, 0) + 1

    print(f"  codes={len(codes)} stocks={stock_count} "
          f"first_non_stock={'OT[' + str(first_non_stock) + ']' if first_non_stock else 'none'}")
    for code in sorted(counts):
        print(f"  code=0x{code:02x}  count={counts[code]:>3}  label={ot_code_label(code)}")

    print("  First 40 codes:")
    for i, code in enumerate(codes[:40]):
        print(f"    OT[{i:>3}]  0x{code:02x}  {ot_code_label(code)}")
    if len(codes) > 40:
        print(f"  ... ({len(codes) - 40} more codes)")
    print()


def print_section6_tail(vdf: VdfFile) -> None:
    print("=== Section 6 Tail ===")
    values = vdf.section6_final_values()
    if values:
        print(f"  OT final values: {len(values)}")
        for ot, val in enumerate(values[:16]):
            print(f"    OT[{ot:>3}] final={val}")
        if len(values) > 16:
            print(f"    ... ({len(values) - 16} more)")
    else:
        print("  OT final values: (none)")

    records = vdf.section6_lookup_records()
    if records:
        print(f"  lookup records: {len(records)}")
        for i, rec in enumerate(records[:16]):
            w = rec.words
            print(f"    {i:>3} @0x{rec.file_offset:08x} ot={rec.ot_index()} "
                  f"words=[{' '.join(f'{x:08x}' for x in w)}]")
        if len(records) > 16:
            print(f"    ... ({len(records) - 16} more)")
    elif records is not None:
        print(f"  lookup records: 0")
    else:
        print("  lookup records: (unparsed)")
    print()


def print_ot_ranges(vdf: VdfFile) -> None:
    print("=== Record-Derived OT Ranges ===")
    ranges = vdf.record_ot_ranges()
    if not ranges:
        print("  (none)\n")
        return

    covered = sum(r.length() for r in ranges)
    multi = sum(1 for r in ranges if r.length() > 1)
    print(f"  ranges={len(ranges)} covered={covered} of {vdf.offset_table_count - 1} "
          f"(excluding OT[0]) multi_entry_ranges={multi}")
    for i, r in enumerate(ranges[:24]):
        print(f"  {i:>3}  [{r.start}..{r.end}) len={r.length()} records@start={r.record_count}")
    if len(ranges) > 24:
        print(f"  ... ({len(ranges) - 24} more)")
    print()


def print_offset_table(vdf: VdfFile) -> None:
    print(f"=== Offset Table ({vdf.offset_table_count} entries @ 0x{vdf.offset_table_start:08x}) ===")
    codes = vdf.section6_ot_class_codes()
    for i in range(vdf.offset_table_count):
        raw = vdf.offset_table_entry(i)
        if raw is None:
            continue
        code_suffix = ""
        if codes and i < len(codes):
            code_suffix = f"  code=0x{codes[i]:02x} ({ot_code_label(codes[i])})"
        if vdf.is_data_block_offset(raw):
            print(f"  {i:>3}  0x{raw:08x}  block{code_suffix}")
        else:
            fval = u32_as_f32(raw)
            print(f"  {i:>3}  0x{raw:08x}  const = {fval}{code_suffix}")
    print()


def print_data_blocks(vdf: VdfFile) -> None:
    block_offsets = set()
    for i in range(vdf.offset_table_count):
        raw = vdf.offset_table_entry(i)
        if raw is not None and vdf.is_data_block_offset(raw):
            block_offsets.add(raw)
    block_offsets = sorted(block_offsets)

    print(f"=== Data Blocks ({len(block_offsets)}) ===")
    for idx, offset in enumerate(block_offsets):
        count = u16(vdf.data, offset) if offset + 2 <= len(vdf.data) else 0
        if offset == vdf.first_data_block:
            bitmap_size = vdf.bitmap_size
            grid_count = vdf.time_point_count
            kind = "saved"
        else:
            bitmap_size, grid_count, kind = vdf._block_bitmap_layout(offset, count)
        if offset + 2 + bitmap_size > len(vdf.data):
            print(f"  {idx:>3}  0x{offset:08x}  (truncated)")
            continue
        block_size = 2 + bitmap_size + count * 4
        density = (count / grid_count * 100) if grid_count > 0 else 0

        data_start = offset + 2 + bitmap_size
        first_val = f32(vdf.data, data_start) if count > 0 and data_start + 4 <= len(vdf.data) else float("nan")
        last_val = f32(vdf.data, data_start + (count - 1) * 4) if count > 1 and data_start + count * 4 <= len(vdf.data) else first_val

        if offset == vdf.first_data_block:
            label = "  [TIME]"
        elif kind == "data":
            label = "  [DATA-GRID]"
        elif kind is None:
            label = "  [UNRECONCILED]"
        else:
            label = ""
        print(f"  {idx:>3}  0x{offset:08x}  {count}/{grid_count} "
              f"({density:.0f}%)  {block_size}B  first={first_val} last={last_val}{label}")
    print()


def print_data_series(vdf: VdfFile) -> None:
    """Extract and print first/last values for every OT entry."""
    print("=== Data Series (first/last values per OT) ===")
    # Get time values first
    time_values = vdf.extract_time_values()
    if time_values is None:
        print("  (time block unavailable)\n")
        return

    codes = vdf.section6_ot_class_codes()
    final_values = vdf.section6_final_values()
    for i in range(vdf.offset_table_count):
        raw = vdf.offset_table_entry(i)
        if raw is None:
            continue
        code_str = ""
        if codes and i < len(codes):
            code_str = f" ({ot_code_label(codes[i])})"

        if vdf.is_data_block_offset(raw):
            series = vdf.extract_ot_series(i, time_values, codes, final_values)
            if series is None:
                continue
            first = series[0] if series else float("nan")
            last = series[-1] if series else float("nan")
            print(f"  OT[{i:>3}]{code_str}  first={first}  last={last}")
        else:
            series = vdf.extract_ot_series(i, time_values, codes, final_values)
            if series is not None and series and math.isnan(series[0]):
                print(f"  OT[{i:>3}]{code_str}  missing")
            else:
                fval = u32_as_f32(raw)
                print(f"  OT[{i:>3}]{code_str}  const={fval}")
    print()


def print_shape_record_bridge(vdf: VdfFile) -> None:
    """Show the field[6] -> section-3 shape mapping with OT span analysis."""
    print("=== Record Shape Bridge (field[6] -> section-3) ===")
    directory = vdf.parse_section3_directory()
    if directory is None or not directory.entries:
        print("  (no section-3 directory)\n")
        return

    # Build index_word -> entry map
    idx_to_entry: dict[int, Section3Entry] = {}
    for entry in directory.entries:
        idx_to_entry[entry.index_word()] = entry

    # Group records by field[6]
    shape_groups: dict[int, list[VdfRecord]] = {}
    for rec in vdf.records:
        code = rec.shape_code()
        shape_groups.setdefault(code, []).append(rec)

    for code in sorted(shape_groups.keys()):
        recs = shape_groups[code]
        entry = idx_to_entry.get(code)
        if code == 5:
            label = "scalar"
        elif entry is not None:
            state = "active" if entry.flat_size() > 0 else "placeholder"
            label = f"sec3 idx={code}, flat={entry.flat_size()}, axes={entry.axis_sizes()} ({state})"
        elif code == 32 and directory.entries:
            active = [e for e in directory.entries if e.flat_size() > 0]
            label = ("generic arrayed"
                     if not active else
                     "generic arrayed; active sec3="
                     + ", ".join(f"{e.index_word()}/{e.axis_sizes()}" for e in active))
        elif code >= 7000:
            label = f"high-range ({code})"
        else:
            label = f"unknown (not in sec3)"

        # Count records with valid ot_index
        valid_ot = [r for r in recs if 0 < r.ot_index() < vdf.offset_table_count]
        ot_indices = sorted(r.ot_index() for r in valid_ot)

        # Compute OT span distribution (gap between consecutive ot_index values
        # within this bucket)
        spans: list[int] = []
        if len(ot_indices) >= 2:
            spans = [ot_indices[i + 1] - ot_indices[i] for i in range(len(ot_indices) - 1)]

        print(f"  f[6]={code:>5}  records={len(recs):>3}  valid_ot={len(valid_ot):>3}  {label}")
        if ot_indices:
            print(f"         ot_indices(head): {ot_indices[:8]}")
        if spans:
            span_counts: dict[int, int] = {}
            for s in spans:
                span_counts[s] = span_counts.get(s, 0) + 1
            print(f"         ot_span_dist: {dict(sorted(span_counts.items()))}")
    print()


def print_extracted_results(vdf: VdfFile) -> None:
    """Extract and show named results via the direct record map."""
    print("=== Extracted Named Results ===")
    results = extract_named_results(vdf)
    if results is None:
        print("  extraction FAILED")
        print()
        return

    print(f"  total results: {len(results)}")
    for r in results:
        first = r.values[0] if r.values else float("nan")
        last = r.values[-1] if r.values else float("nan")
        print(f"  {r.name}: OT[{r.ot_index}] first={first} last={last}")
    print()


def _markdown_cell(value: object) -> str:
    text = "" if value is None else str(value)
    return text.replace("|", "\\|")


def _format_reason_list(reasons: list[str]) -> str:
    return ", ".join(reasons) if reasons else "-"


def print_section35_bridge(vdf: VdfFile) -> None:
    """Show the section-3 -> section-5 relationship via raw axis refs."""
    print("=== Section 3 -> Section 5 Bridge ===")
    directory = vdf.parse_section3_directory()
    if directory is None or not directory.entries:
        print("  (no section-3 directory)\n")
        return

    sec5_entries = vdf.parse_section5_sets()
    if not sec5_entries:
        print("  (no section-5 entries)\n")
        return

    slot_to_names = build_direct_slot_to_names(vdf)
    axis_ref_anchors = section3_axis_ref_to_dimension_anchor(vdf)

    def axis_ref_label(ref: int) -> str:
        anchor = axis_ref_anchors.get(ref)
        if anchor is not None:
            return f"{ref}:dim:{anchor.name}"
        return resolve_slot_ref(ref, slot_to_names)

    for i, sec3 in enumerate(directory.entries):
        axis_refs = set(sec3.axis_slot_refs())
        if not axis_refs:
            print(f"  sec3[{i}] idx={sec3.index_word()} flat={sec3.flat_size()} "
                  f"axes={sec3.axis_sizes()} -- no axis_slot_refs")
            continue

        matches = classify_section5_bridge_matches(sec3, sec5_entries)

        axis_ref_strs = [axis_ref_label(r) for r in sec3.axis_slot_refs()]
        state = "placeholder" if sec3.flat_size() == 0 else "active"
        print(f"  sec3[{i}] state={state} idx={sec3.index_word()} flat={sec3.flat_size()} "
              f"axes={sec3.axis_sizes()} axis_refs={axis_ref_strs}")

        if matches.exact:
            for j in matches.exact:
                sec5 = sec5_entries[j]
                trailing = section5_trailing_refs(sec5)
                trailing_strs = [resolve_slot_ref(r, slot_to_names) for r in trailing]
                exact_axes = section5_exact_axis_sizes(sec5, directory.entries)
                print(f"    -> exact sec5[{j}] n={sec5.n} marker={sec5.marker} "
                      f"payload_refs={sec5.payload_ref_count()} trailing={trailing_strs} "
                      f"axes={exact_axes}")
        elif matches.partial:
            for j in matches.partial:
                sec5 = sec5_entries[j]
                trailing = section5_trailing_refs(sec5)
                trailing_strs = [resolve_slot_ref(r, slot_to_names) for r in trailing]
                print(f"    -> partial sec5[{j}] n={sec5.n} marker={sec5.marker} "
                      f"payload_refs={sec5.payload_ref_count()} trailing={trailing_strs}")
        elif matches.null_trailing:
            null_idxs = ", ".join(f"sec5[{j}]" for j in matches.null_trailing)
            verb = "ends" if len(matches.null_trailing) == 1 else "end"
            print(f"    (no non-zero sec5 trailing refs; {null_idxs} {verb} in a 0 sentinel)")
        else:
            print(f"    (no matching sec5 entries)")
    print()


def print_slot_reference_inventory(vdf: VdfFile) -> None:
    print("=== Referenced Slot Refs ===")
    inventory = collect_slot_reference_inventory(vdf)
    if not inventory:
        print("  (none)\n")
        return

    for slot_ref in sorted(inventory):
        info = inventory[slot_ref]
        names = "/".join(info.heuristic_names) if info.heuristic_names else "<none>"
        print(f"  {slot_ref:>4}  names?={names:<32} sig={format_u32_words(info.signature)}")
        print(f"        uses={', '.join(info.uses)}")
    print()


def print_validation(vdf: VdfFile) -> None:
    """Check structural invariants and report any violations."""
    print("=== Validation ===")
    errors: list[str] = []
    warnings: list[str] = []

    # 1. Section framing should be stable and ordered.
    if len(vdf.sections) == 8:
        print("  [PASS] section scan found the expected 8 sections")
    else:
        errors.append(f"expected 8 sections, found {len(vdf.sections)}")

    if any(vdf.sections[i].file_offset >= vdf.sections[i + 1].file_offset
           for i in range(len(vdf.sections) - 1)):
        errors.append("section offsets are not strictly increasing")
    elif vdf.sections:
        print("  [PASS] section offsets are strictly increasing")

    # 2. Section headers expose direct word offsets for the decoded section tails.
    sec6_cc_from_header = vdf.section6_class_code_start()
    sec6_cc_from_field1 = vdf.section6_class_code_start_from_field1()
    if sec6_cc_from_header is not None and sec6_cc_from_field1 is not None:
        if sec6_cc_from_header == sec6_cc_from_field1:
            print("  [PASS] sec6 field1 points to the OT class-code array")
        else:
            errors.append(
                "sec6 field1 pointer does not match header-derived class-code start: "
                f"field1=0x{sec6_cc_from_field1:x}, header=0x{sec6_cc_from_header:x}")

    sec7_ot_from_field1 = vdf.section7_offset_table_start_from_field1()
    if sec7_ot_from_field1 is not None:
        if sec7_ot_from_field1 == vdf.offset_table_start:
            print("  [PASS] sec7 field1 points to the offset table")
        else:
            errors.append(
                "sec7 field1 pointer does not match header offset-table start: "
                f"field1=0x{sec7_ot_from_field1:x}, header=0x{vdf.offset_table_start:x}")

    # The slot table is decoded deterministically from sec1 field1 +
    # block1[7]; a decode failure leaves the table empty, so an empty table
    # on a file with names is itself the violation.
    sec1_slot_area = vdf.section1_slot_area_offset_from_field1()
    if vdf.slot_table:
        if sec1_slot_area == vdf.slot_table_offset:
            print("  [PASS] sec1 field1 points to the slot table "
                  f"(count={len(vdf.slot_table)} from block1[7], terminator verified)")
        else:
            errors.append(
                "sec1 field1 pointer does not match the decoded slot-table start: "
                f"field1=0x{sec1_slot_area:x}, decoded=0x{vdf.slot_table_offset:x}")
    elif vdf.names:
        errors.append(
            "slot table failed the deterministic header decode "
            "(field1/block1[7]/terminator cross-check)")

    sec5_last_word = vdf.section5_region_last_word_from_field1()
    if sec5_last_word is not None and len(vdf.sections) > 5:
        expected = vdf.sections[5].region_end - 4
        if sec5_last_word == expected:
            print("  [PASS] sec5 field1 points to the section's final word")
        else:
            errors.append(
                "sec5 field1 pointer does not match section final word: "
                f"field1=0x{sec5_last_word:x}, expected=0x{expected:x}")

    # 3. Slot tables in small/medium fixtures form a contiguous 16-byte lattice.
    slot_layout = analyze_slot_table_offsets(vdf.slot_table)
    if slot_layout is None:
        warnings.append("slot table is empty")
    elif slot_layout.contiguous_16:
        print(f"  [PASS] slot table forms a contiguous 16-byte lattice "
              f"(base={slot_layout.base}, count={len(vdf.slot_table)})")
    else:
        warnings.append(
            "slot table is structurally valid but not a contiguous 16-byte lattice: "
            f"{format_slot_table_layout(slot_layout)}")

    # 4. Section-3 index_words form arithmetic progression (step=27)
    directory = vdf.parse_section3_directory()
    if directory and directory.entries:
        idx_words = [e.index_word() for e in directory.entries]
        if len(idx_words) >= 2:
            diffs = [idx_words[i + 1] - idx_words[i] for i in range(len(idx_words) - 1)]
            # The last entry may have index_word=0, which breaks the progression;
            # only check the non-zero prefix
            nonzero_words = [w for w in idx_words if w != 0]
            if len(nonzero_words) >= 2:
                nonzero_diffs = [nonzero_words[i + 1] - nonzero_words[i]
                                 for i in range(len(nonzero_words) - 1)]
                if all(d == SECTION3_ENTRY_WORDS for d in nonzero_diffs):
                    print(f"  [PASS] sec3 index_words form step-{SECTION3_ENTRY_WORDS} "
                          f"arithmetic progression: {nonzero_words}")
                else:
                    errors.append(
                        f"sec3 index_words do NOT form step-{SECTION3_ENTRY_WORDS} "
                        f"progression: words={idx_words}, diffs={nonzero_diffs}")
            elif len(nonzero_words) == 1:
                print(f"  [PASS] sec3 single nonzero index_word: {nonzero_words[0]}")
        elif len(idx_words) == 1:
            print(f"  [PASS] sec3 single entry, index_word={idx_words[0]}")

        # 5. Sec3 axis refs point to dimension-anchor record field[9] words.
        axis_anchor_by_ref = section3_axis_ref_to_dimension_anchor(vdf)
        all_axis_refs: list[int] = []
        for entry in directory.entries:
            all_axis_refs.extend(ref for ref in entry.axis_slot_refs() if ref > 0)
        if all_axis_refs:
            missing_anchor = [r for r in all_axis_refs if r not in axis_anchor_by_ref]
            if missing_anchor:
                errors.append(
                    f"sec3 axis refs do NOT point to dimension anchors: {missing_anchor}"
                )
            else:
                print(
                    f"  [PASS] all {len(all_axis_refs)} sec3 axis refs point to "
                    "dimension anchors"
                )
    else:
        print(f"  [SKIP] no section-3 directory entries")

    # 6. Section-5 trailing refs overlap with sec3 axis_slot_refs
    sec5_entries = vdf.parse_section5_sets()
    if directory and directory.entries and sec5_entries:
        sec3_axis_set: set[int] = set()
        for entry in directory.entries:
            sec3_axis_set.update(entry.axis_slot_refs())

        sec5_trailing: set[int] = set()
        for sec5 in sec5_entries:
            trailing_count = 1 + sec5.marker
            if len(sec5.refs) >= trailing_count:
                for r in sec5.refs[-trailing_count:]:
                    if r > 0:  # 0 is a null sentinel, not a real ref
                        sec5_trailing.add(r)

        overlap = sec3_axis_set & sec5_trailing
        sec3_only = sec3_axis_set - sec5_trailing
        sec5_only = sec5_trailing - sec3_axis_set

        if overlap:
            print(f"  [PASS] sec3/sec5 axis ref overlap: {len(overlap)} shared refs")
        elif not sec5_trailing:
            print(f"  [SKIP] sec5 has no non-zero trailing refs (single-dimension model)")
        else:
            if sec3_axis_set and sec5_trailing:
                warnings.append(
                    f"sec3 axis refs and sec5 trailing refs have no overlap: "
                    f"sec3={sec3_axis_set}, sec5={sec5_trailing}")
            else:
                print(f"  [SKIP] empty axis ref sets (sec3={len(sec3_axis_set)}, "
                      f"sec5={len(sec5_trailing)})")

        if sec3_only and sec5_trailing:
            warnings.append(f"sec3 axis refs not in sec5 trailing: {sec3_only}")
        if sec5_only and sec3_axis_set:
            warnings.append(f"sec5 trailing refs not in sec3 axis: {sec5_only}")
    elif not sec5_entries:
        print(f"  [SKIP] no section-5 entries for axis ref overlap check")

    # 7. Record field[6] values are either 0, 5, 32, a sec3 index_word, or in the high range (7000+)
    # 0 appears on padding/system/non-model records
    if directory and directory.entries:
        sec3_idx_words = {e.index_word() for e in directory.entries}
        known_codes = {0, 5, 32} | sec3_idx_words
    else:
        known_codes = {0, 5, 32}

    unexpected_codes: dict[int, int] = {}
    for rec in vdf.records:
        code = rec.shape_code()
        if code not in known_codes and code < 7000:
            unexpected_codes[code] = unexpected_codes.get(code, 0) + 1

    if not unexpected_codes:
        print(f"  [PASS] all record f[6] values are 0, 5, 32, sec3 index_word, or >=7000")
    else:
        errors.append(
            f"unexpected record f[6] values (not 0/5/32/sec3-idx/>=7000): "
            f"{dict(sorted(unexpected_codes.items()))}")

    # Report
    for w in warnings:
        print(f"  [WARN] {w}")
    for e in errors:
        print(f"  [FAIL] {e}")
    if not errors and not warnings:
        print(f"  All checks passed.")
    elif not errors:
        print(f"  All checks passed ({len(warnings)} warnings).")
    else:
        print(f"  {len(errors)} failure(s), {len(warnings)} warning(s).")
    print()


def print_summary(vdf: VdfFile) -> None:
    n_block = sum(1 for i in range(vdf.offset_table_count)
                  if (raw := vdf.offset_table_entry(i)) is not None and vdf.is_data_block_offset(raw))
    n_const = vdf.offset_table_count - n_block
    n_system = sum(1 for n in vdf.names if n in SYSTEM_NAMES)
    n_groups = sum(1 for n in vdf.names if n.startswith("."))
    n_units = sum(1 for n in vdf.names if n.startswith("-"))
    n_builtins = sum(1 for n in vdf.names
                     if n not in SYSTEM_NAMES and not n.startswith(".") and not n.startswith("-")
                     and (n.lower() in VENSIM_BUILTINS or (len(n) == 1 and not n[0].isalnum())))
    n_sigs = sum(1 for n in vdf.names if n.startswith("#"))
    n_model = len(vdf.names) - n_system - n_groups - n_units - n_builtins - n_sigs

    ot_count = vdf.offset_table_count
    n_model_recs = sum(1 for r in vdf.records
                       if r.fields[0] != 0 and r.fields[1] != 23
                       and r.fields[10] > 0 and r.fields[11] > 0 and r.fields[11] < ot_count)
    slot_groups = len(set(r.fields[12] for r in vdf.records))

    codes = vdf.section6_ot_class_codes()
    stock_count = sum(1 for c in codes[1:] if c == OT_CODE_STOCK) if codes else 0

    print("=== Summary ===")
    print(f"  File size:      {len(vdf.data)} bytes")
    print(f"  Sections:       {len(vdf.sections)}")
    print(f"  Names:          {len(vdf.names)} ({n_system} system, {n_groups} groups, "
          f"{n_units} units, {n_builtins} builtins, {n_sigs} signatures, {n_model} model)")
    unslotted = len(vdf.names) - len(vdf.slot_table)
    if unslotted > 0:
        print(f"  Unslotted names: {unslotted}")
    print(f"  Records:        {len(vdf.records)} ({n_model_recs} model var, {slot_groups} f[12] groups)")
    print(f"  OT entries:     {vdf.offset_table_count} ({n_block} blocks, {n_const} constants)")
    print(f"  Stocks:         {stock_count}")
    print(f"  Data blocks:    {n_block}")
    print(f"  Slot lattice:   {format_slot_table_layout(analyze_slot_table_offsets(vdf.slot_table))}")


def print_raw_section(vdf: VdfFile, section_idx: int) -> None:
    """Hexdump the full raw data of a specific section."""
    if section_idx >= len(vdf.sections):
        print(f"Section {section_idx} does not exist (file has {len(vdf.sections)} sections)")
        return
    sec = vdf.sections[section_idx]
    role = SECTION_ROLES[section_idx] if section_idx < len(SECTION_ROLES) else "unknown"
    print(f"=== Section {section_idx} raw dump [{role}] ===")
    print(f"  region: 0x{sec.data_offset():08x}..0x{sec.region_end:08x} ({sec.region_data_size()}B)")
    data_start = sec.data_offset()
    region_end = min(sec.region_end, len(vdf.data))
    if data_start >= region_end:
        print("  (empty)")
    else:
        hexdump(vdf.data[data_start:region_end], data_start, max_bytes=16384)
    print()


def print_json_summary(vdf: VdfFile) -> None:
    """Print a machine-readable JSON summary of key structures."""
    codes = vdf.section6_ot_class_codes()
    stock_count = sum(1 for c in codes[1:] if c == OT_CODE_STOCK) if codes else 0

    summary = {
        "magic": vdf.data[:4].hex(),
        "file_size": len(vdf.data),
        "time_point_count": vdf.time_point_count,
        "sections": len(vdf.sections),
        "names_total": len(vdf.names),
        "slot_table_size": len(vdf.slot_table),
        "records": len(vdf.records),
        "offset_table_count": vdf.offset_table_count,
        "stock_count": stock_count,
        "names": vdf.names,
        "records_detail": [
            {
                "offset": f"0x{r.file_offset:08x}",
                "fields": r.fields,
                "has_sentinel": r.has_sentinel(),
                "shape_code": r.shape_code(),
                "ot_index": r.ot_index(),
                "slot_ref": r.slot_ref(),
            }
            for r in vdf.records
        ],
    }

    slot_layout = analyze_slot_table_offsets(vdf.slot_table)
    if slot_layout is not None:
        summary["slot_table_layout"] = {
            "base": slot_layout.base,
            "max_offset": slot_layout.max_offset,
            "distinct_strides": slot_layout.distinct_strides,
            "irregular_stride_count": slot_layout.irregular_stride_count,
            "missing_16_slots": slot_layout.missing_16_slots,
            "contiguous_16": slot_layout.contiguous_16,
        }

    directory = vdf.parse_section3_directory()
    if directory and directory.entries:
        summary["section3_entries"] = [
            {
                "index_word": e.index_word(),
                "flat_size": e.flat_size(),
                "axis_sizes": e.axis_sizes(),
                "axis_slot_refs": e.axis_slot_refs(),
                "terminal_tag": e.terminal_tag(),
            }
            for e in directory.entries
        ]

    if codes:
        code_counts: dict[str, int] = {}
        for c in codes:
            key = f"0x{c:02x}"
            code_counts[key] = code_counts.get(key, 0) + 1
        summary["ot_class_code_counts"] = code_counts

    post_ref_records = vdf.parse_section6_post_ref_records()
    if post_ref_records is not None:
        summary["section6_post_ref_record_count"] = len(post_ref_records)

    print(json.dumps(summary, indent=2))


def slot_words(vdf: VdfFile, slot_offset: int) -> Optional[list[int]]:
    """Read the 16-byte section-1 payload for a slot-table entry."""
    if len(vdf.sections) <= 1 or slot_offset <= 0:
        return None
    abs_off = vdf.sections[1].data_offset() + slot_offset
    if abs_off + 16 > len(vdf.data):
        return None
    return [u32(vdf.data, abs_off + i * 4) for i in range(4)]


def format_u32_words(words: Optional[list[int]]) -> str:
    if words is None:
        return "(out of bounds)"
    return "[" + " ".join(f"{word:08x}" for word in words) + "]"


def ref_signature_fingerprint(vdf: VdfFile, refs: list[int]) -> list[Optional[list[int]]]:
    return [slot_words(vdf, slot_ref) for slot_ref in refs]


def format_ref_signature_fingerprint(vdf: VdfFile, refs: list[int]) -> str:
    if not refs:
        return "[]"
    return "[" + ", ".join(format_u32_words(words)
                           for words in ref_signature_fingerprint(vdf, refs)) + "]"


def describe_slot_ref(vdf: VdfFile, slot_ref: int, *, include_signature: bool = False) -> str:
    names = build_direct_slot_to_names(vdf).get(slot_ref, [])
    label = resolve_slot_ref(slot_ref, {slot_ref: names} if names else {})
    if not include_signature:
        return label
    return f"{label} sig={format_u32_words(slot_words(vdf, slot_ref))}"


def collect_slot_reference_inventory(vdf: VdfFile) -> dict[int, SlotReferenceInfo]:
    slot_to_names = build_direct_slot_to_names(vdf)
    inventory: dict[int, SlotReferenceInfo] = {}

    def add(slot_ref: int, use: str) -> None:
        if slot_ref <= 0:
            return
        info = inventory.setdefault(
            slot_ref,
            SlotReferenceInfo(
                slot_ref=slot_ref,
                heuristic_names=slot_to_names.get(slot_ref, []).copy(),
                signature=slot_words(vdf, slot_ref),
            ),
        )
        info.uses.append(use)

    directory = vdf.parse_section3_directory()
    if directory:
        for entry_idx, entry in enumerate(directory.entries):
            for axis_idx, slot_ref in enumerate(entry.axis_slot_refs()):
                add(slot_ref, f"sec3[{entry_idx}].axis[{axis_idx}]")

    sec4_entries = vdf.parse_section4_entries()
    if sec4_entries:
        for entry_idx, entry in enumerate(sec4_entries):
            for ref_idx, slot_ref in enumerate(entry.refs):
                add(slot_ref, f"sec4[{entry_idx}].ref[{ref_idx}]")

    sec5_entries = vdf.parse_section5_sets()
    if sec5_entries:
        for entry_idx, entry in enumerate(sec5_entries):
            for ref_idx, slot_ref in enumerate(entry.refs):
                add(slot_ref, f"sec5[{entry_idx}].ref[{ref_idx}]")

    sec6_result = vdf.parse_section6_ref_stream()
    if sec6_result:
        for entry_idx, entry in enumerate(sec6_result[1]):
            for ref_idx, slot_ref in enumerate(entry.refs):
                add(slot_ref, f"sec6[{entry_idx}].ref[{ref_idx}]")

    for info in inventory.values():
        info.uses.sort()
    return inventory


def print_compare(left: VdfFile, left_path: str, right: VdfFile, right_path: str) -> None:
    """Compare two parsed simulation-result VDFs at the decoded-structure level."""
    print("=== Compare ===")
    print(f"Left:  {left_path}")
    print(f"Right: {right_path}")
    print()

    print("=== Header / Layout Diffs ===")
    header_fields = [
        ("file_size", len(left.data), len(right.data)),
        ("time_point_count", left.time_point_count, right.time_point_count),
        ("header_final_values_offset", left.header_final_values_offset, right.header_final_values_offset),
        ("header_lookup_mapping_offset", left.header_lookup_mapping_offset, right.header_lookup_mapping_offset),
        ("offset_table_start", left.offset_table_start, right.offset_table_start),
        ("first_data_block", left.first_data_block, right.first_data_block),
    ]
    for label, lhs, rhs in header_fields:
        if lhs != rhs:
            print(f"  {label}: left={lhs} right={rhs}")
    print()

    print("=== Shared Name / Slot Diffs ===")
    # Direct slot_table[i] <-> names[i] pairing on both sides.
    left_pairs = [
        (left.names[i], slot) for i, slot in enumerate(left.slot_table)
        if i < len(left.names)
    ]
    right_pairs = [
        (right.names[i], slot) for i, slot in enumerate(right.slot_table)
        if i < len(right.names)
    ]
    by_name_left: dict[str, tuple[int, int]] = {}
    by_name_right: dict[str, tuple[int, int]] = {}
    for idx, (name, slot) in enumerate(left_pairs):
        by_name_left.setdefault(name, (idx, slot))
    for idx, (name, slot) in enumerate(right_pairs):
        by_name_right.setdefault(name, (idx, slot))
    shared_names = sorted(set(by_name_left) & set(by_name_right))
    any_slot_diff = False
    for name in shared_names:
        _, lslot = by_name_left[name]
        _, rslot = by_name_right[name]
        lwords = slot_words(left, lslot)
        rwords = slot_words(right, rslot)
        if lslot != rslot or lwords != rwords:
            any_slot_diff = True
            print(f"  {name}")
            print(f"    left:  slot={lslot} words={format_u32_words(lwords)}")
            print(f"    right: slot={rslot} words={format_u32_words(rwords)}")
    if not any_slot_diff:
        print("  (no shared-name slot payload differences)")
    print()

    print("=== Referenced Slot Inventory Diffs ===")
    left_inventory = collect_slot_reference_inventory(left)
    right_inventory = collect_slot_reference_inventory(right)
    any_inventory_diff = False
    for slot_ref in sorted(set(left_inventory) | set(right_inventory)):
        linfo = left_inventory.get(slot_ref)
        rinfo = right_inventory.get(slot_ref)
        if linfo is None or rinfo is None:
            any_inventory_diff = True
            print(f"  slot {slot_ref}")
            if linfo is None:
                print("    left:  missing")
            else:
                print(
                    f"    left:  names?={linfo.heuristic_names} sig={format_u32_words(linfo.signature)} "
                    f"uses={linfo.uses}"
                )
            if rinfo is None:
                print("    right: missing")
            else:
                print(
                    f"    right: names?={rinfo.heuristic_names} sig={format_u32_words(rinfo.signature)} "
                    f"uses={rinfo.uses}"
                )
            continue
        if (
            linfo.signature != rinfo.signature
            or linfo.heuristic_names != rinfo.heuristic_names
            or linfo.uses != rinfo.uses
        ):
            any_inventory_diff = True
            print(f"  slot {slot_ref}")
            print(
                f"    left:  names?={linfo.heuristic_names} sig={format_u32_words(linfo.signature)} "
                f"uses={linfo.uses}"
            )
            print(
                f"    right: names?={rinfo.heuristic_names} sig={format_u32_words(rinfo.signature)} "
                f"uses={rinfo.uses}"
            )
    if not any_inventory_diff:
        print("  (no referenced-slot differences)")
    print()

    print("=== Section 3 Diffs ===")
    left_sec3 = left.parse_section3_directory()
    right_sec3 = right.parse_section3_directory()
    left_entries = left_sec3.entries if left_sec3 else []
    right_entries = right_sec3.entries if right_sec3 else []
    any_sec3_diff = False
    max_sec3 = max(len(left_entries), len(right_entries))
    for i in range(max_sec3):
        lentry = left_entries[i] if i < len(left_entries) else None
        rentry = right_entries[i] if i < len(right_entries) else None
        lwords = lentry.words if lentry else None
        rwords = rentry.words if rentry else None
        if lwords != rwords:
            any_sec3_diff = True
            print(f"  sec3[{i}]")
            print(f"    left:  {lwords}")
            print(f"    right: {rwords}")
    if not any_sec3_diff:
        print("  (no section-3 differences)")
    print()

    print("=== Section 5 Diffs ===")
    left_sec5 = left.parse_section5_sets() or []
    right_sec5 = right.parse_section5_sets() or []
    any_sec5_diff = False
    max_sec5 = max(len(left_sec5), len(right_sec5))
    for i in range(max_sec5):
        lentry = left_sec5[i] if i < len(left_sec5) else None
        rentry = right_sec5[i] if i < len(right_sec5) else None
        ltuple = (lentry.n, lentry.marker, lentry.refs) if lentry else None
        rtuple = (rentry.n, rentry.marker, rentry.refs) if rentry else None
        if ltuple != rtuple:
            any_sec5_diff = True
            print(f"  sec5[{i}]")
            if lentry is None:
                print("    left:  missing")
            else:
                print(f"    left:  {ltuple}")
                print(f"           payload={section5_payload_refs(lentry)} "
                      f"trailing={section5_trailing_refs(lentry)} "
                      f"sigseq={format_ref_signature_fingerprint(left, lentry.refs)}")
            if rentry is None:
                print("    right: missing")
            else:
                print(f"    right: {rtuple}")
                print(f"           payload={section5_payload_refs(rentry)} "
                      f"trailing={section5_trailing_refs(rentry)} "
                      f"sigseq={format_ref_signature_fingerprint(right, rentry.refs)}")
    if not any_sec5_diff:
        print("  (no section-5 differences)")
    print()

    print("=== Record Diffs By Index ===")
    any_record_diff = False
    record_count = min(len(left.records), len(right.records))
    for i in range(record_count):
        lrec = left.records[i]
        rrec = right.records[i]
        field_diffs = []
        for field_idx, (lhs, rhs) in enumerate(zip(lrec.fields, rrec.fields)):
            if lhs != rhs:
                field_diffs.append(f"f{field_idx}={lhs}->{rhs}")
        if field_diffs:
            any_record_diff = True
            print(f"  rec[{i}] left@0x{lrec.file_offset:08x} right@0x{rrec.file_offset:08x}")
            print(f"    {'; '.join(field_diffs)}")
    if len(left.records) != len(right.records):
        any_record_diff = True
        print(f"  record_count: left={len(left.records)} right={len(right.records)}")
    if not any_record_diff:
        print("  (no record differences)")
    print()

    print("=== Decoded Record Span Diffs ===")
    left_spans = decoded_record_spans(left)
    right_spans = decoded_record_spans(right)
    left_desc = identify_descriptor_records(left, left_spans).descriptor_indices
    right_desc = identify_descriptor_records(right, right_spans).descriptor_indices

    def span_summary(span: DecodedRecordSpan, descriptors: set[int]) -> tuple:
        kind = "descriptor" if span.rec_idx in descriptors else "owner"
        return (span.name, kind, span.shape_code, span.sort_key, span.slot_ref,
                tuple(span.ot_codes))

    def format_span(vdf: VdfFile, span: Optional[DecodedRecordSpan],
                    descriptors: set[int]) -> str:
        if span is None:
            return "missing"
        kind = "descriptor" if span.rec_idx in descriptors else "owner"
        code_str = "[" + ", ".join(f"0x{code:02x}" for code in span.ot_codes) + "]"
        return (f"{span.name!r} {kind} OT[{span.start}..{span.end}) len={span.length()} "
                f"shape={span.shape_code} sort={span.sort_key} slot={span.slot_ref} "
                f"codes={code_str}")

    any_span_diff = False
    left_by_key = {(s.start, s.end): s for s in left_spans}
    right_by_key = {(s.start, s.end): s for s in right_spans}
    keys = sorted(set(left_by_key) | set(right_by_key))
    for key in keys:
        lspan = left_by_key.get(key)
        rspan = right_by_key.get(key)
        if (
            lspan is None
            or rspan is None
            or span_summary(lspan, left_desc) != span_summary(rspan, right_desc)
        ):
            any_span_diff = True
            print(f"  OT[{key[0]}..{key[1]})")
            print(f"    left:  {format_span(left, lspan, left_desc)}")
            print(f"    right: {format_span(right, rspan, right_desc)}")
    if not any_span_diff:
        print("  (no decoded-record-span differences)")
    print()

    print("=== Section 6 Ref Stream Diffs ===")
    left_ref_stream = left.parse_section6_ref_stream()
    right_ref_stream = right.parse_section6_ref_stream()
    if left_ref_stream is None or right_ref_stream is None:
        print("  (section 6 ref stream unavailable)")
    else:
        _, left_entries, left_stop = left_ref_stream
        _, right_entries, right_stop = right_ref_stream
        print(f"  stop_offset: left=0x{left_stop:08x} right=0x{right_stop:08x}")
        max_entries = max(len(left_entries), len(right_entries))
        left_slots = build_direct_slot_to_names(left)
        right_slots = build_direct_slot_to_names(right)
        for i in range(max_entries):
            lentry = left_entries[i] if i < len(left_entries) else None
            rentry = right_entries[i] if i < len(right_entries) else None
            lrefs_raw = lentry.refs if lentry else []
            rrefs_raw = rentry.refs if rentry else []
            if lrefs_raw != rrefs_raw:
                print(f"  entry[{i}]")
                print(f"    left:  raw={lrefs_raw} refs={[resolve_slot_ref(r, left_slots) for r in lrefs_raw]}")
                print(f"           sigseq={format_ref_signature_fingerprint(left, lrefs_raw)}")
                print(f"    right: raw={rrefs_raw} refs={[resolve_slot_ref(r, right_slots) for r in rrefs_raw]}")
                print(f"           sigseq={format_ref_signature_fingerprint(right, rrefs_raw)}")
    print()

    print("=== OT Class Code Diffs ===")
    left_codes = left.section6_ot_class_codes() or []
    right_codes = right.section6_ot_class_codes() or []
    max_codes = max(len(left_codes), len(right_codes))
    for i in range(max_codes):
        lcode = left_codes[i] if i < len(left_codes) else None
        rcode = right_codes[i] if i < len(right_codes) else None
        if lcode != rcode:
            llabel = ot_code_label(lcode) if lcode is not None else "missing"
            rlabel = ot_code_label(rcode) if rcode is not None else "missing"
            lhex = f"0x{lcode:02x}" if lcode is not None else "--"
            rhex = f"0x{rcode:02x}" if rcode is not None else "--"
            print(f"  OT[{i}] left={lhex} ({llabel}) right={rhex} ({rlabel})")
    print()

    print("=== Final Value / Offset Table Diffs ===")
    left_finals = left.section6_final_values() or []
    right_finals = right.section6_final_values() or []
    max_ots = max(left.offset_table_count, right.offset_table_count)
    for i in range(max_ots):
        lraw = left.offset_table_entry(i)
        rraw = right.offset_table_entry(i)
        lfin = left_finals[i] if i < len(left_finals) else None
        rfin = right_finals[i] if i < len(right_finals) else None
        if lraw != rraw or lfin != rfin:
            print(f"  OT[{i}]")
            if lraw is None:
                print("    left:  missing")
            elif left.is_data_block_offset(lraw):
                print(f"    left:  raw=0x{lraw:08x} block final={lfin}")
            else:
                print(f"    left:  raw=0x{lraw:08x} const={u32_as_f32(lraw)} final={lfin}")
            if rraw is None:
                print("    right: missing")
            elif right.is_data_block_offset(rraw):
                print(f"    right: raw=0x{rraw:08x} block final={rfin}")
            else:
                print(f"    right: raw=0x{rraw:08x} const={u32_as_f32(rraw)} final={rfin}")
    print()


# ---- Main ----

def main() -> None:
    parser = argparse.ArgumentParser(
        description="VDF X-Ray: inspect Vensim VDF binary files",
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    # `--extract-json` accepts multiple files so one interpreter launch can
    # cover a whole corpus; every human-oriented mode still takes exactly one.
    parser.add_argument("paths", nargs="+", metavar="path",
                        help="Path to VDF file (multiple allowed with --extract-json)")
    parser.add_argument("--compare", metavar="OTHER_VDF",
                        help="Compare this VDF against another simulation-result VDF")
    parser.add_argument("--all", action="store_true", help="Show everything")
    parser.add_argument("--names", action="store_true", help="Show name table")
    parser.add_argument("--slots", action="store_true", help="Show slot table")
    parser.add_argument("--records", action="store_true", help="Show variable metadata records")
    parser.add_argument("--sec3", action="store_true", help="Show section 3 array directory")
    parser.add_argument("--sec4", action="store_true", help="Show section 4 entries")
    parser.add_argument("--sec5", action="store_true", help="Show section 5 sets")
    parser.add_argument("--sec6", action="store_true", help="Show section 6 ref stream and tail")
    parser.add_argument("--sec6-post", action="store_true",
                        help="Show section 6 post-ref 16-byte records")
    parser.add_argument("--slot-xref", action="store_true",
                        help="Show section 3/4/5/6 referenced slot refs with signatures")
    parser.add_argument("--ot", action="store_true", help="Show offset table")
    parser.add_argument("--blocks", action="store_true", help="Show data blocks")
    parser.add_argument("--data", action="store_true", help="Extract and show all time series")
    parser.add_argument("--bridge", action="store_true", help="Show record shape -> sec3 bridge")
    parser.add_argument("--sec35-bridge", action="store_true", help="Show section-3 -> section-5 bridge")
    parser.add_argument("--ranges", action="store_true", help="Show record-derived OT ranges")
    parser.add_argument("--validate", action="store_true", help="Check structural invariants")
    parser.add_argument("--extract", action="store_true",
                        help="Extract named results via the record-derived mapping")
    parser.add_argument("--extract-json", action="store_true",
                        help="Extract named results for ALL given paths as one JSON "
                             "object on stdout: path -> [{name, ot_index, values}] "
                             "(NaN encoded as null, infinities as strings)")
    parser.add_argument("--raw-section", type=int, metavar="N", help="Full hexdump of section N")
    parser.add_argument("--json", action="store_true", help="Machine-readable JSON summary")

    args = parser.parse_args()

    if args.extract_json:
        payload = extract_results_json_payload([Path(p) for p in args.paths])
        # allow_nan=False turns any non-finite value that leaked past
        # _encode_series_value into a hard error instead of emitting the
        # nonstandard NaN/Infinity tokens strict parsers reject.
        print(json.dumps(payload, allow_nan=False))
        return

    if len(args.paths) != 1:
        parser.error("multiple paths are only supported with --extract-json")

    path = Path(args.paths[0])
    data = path.read_bytes()

    if data[:4] == VDF_DATASET_MAGIC:
        print(f"Dataset VDF detected ({path}). Dataset parsing not yet implemented in this tool.")
        sys.exit(1)

    vdf = parse_vdf(data)

    if args.compare:
        other_path = Path(args.compare)
        other_data = other_path.read_bytes()
        if other_data[:4] == VDF_DATASET_MAGIC:
            print(f"Dataset VDF detected ({other_path}). Compare mode only supports simulation-result VDFs.")
            sys.exit(1)
        other_vdf = parse_vdf(other_data)
        print_compare(vdf, str(path), other_vdf, str(other_path))
        return

    if args.json:
        print_json_summary(vdf)
        return

    # If no specific flags, show the default overview
    show_all = args.all
    show_specific = any([
        args.names, args.slots, args.records, args.sec3, args.sec4,
        args.sec5, args.sec6, args.sec6_post, args.slot_xref, args.ot, args.blocks, args.data,
        args.bridge, args.sec35_bridge, args.ranges, args.validate, args.extract,
        args.raw_section is not None,
    ])

    # Always show header + layout + summary
    print_header(vdf, str(path))
    print_layout(vdf)

    if show_all or not show_specific:
        print_sections(vdf)
    if show_all or args.names or not show_specific:
        print_names(vdf)
    if show_all or args.slots:
        print_slots(vdf)
    if show_all or args.records or not show_specific:
        print_records(vdf)
    if show_all or args.sec3:
        print_section3(vdf)
    if show_all or args.sec4:
        print_section4(vdf)
    if show_all or args.sec5:
        print_section5(vdf)
    if show_all or args.sec6:
        print_section6_ref_stream(vdf)
    if show_all or args.sec6 or args.sec6_post:
        print_section6_post_ref_records(vdf)
    if show_all or args.sec6:
        print_ot_codes(vdf)
        print_section6_tail(vdf)
    if show_all or args.slot_xref:
        print_slot_reference_inventory(vdf)
    if show_all or args.ranges:
        print_ot_ranges(vdf)
    if show_all or args.bridge:
        print_shape_record_bridge(vdf)
    if show_all or args.sec35_bridge:
        print_section35_bridge(vdf)
    if show_all or args.ot:
        print_offset_table(vdf)
    if show_all or args.blocks:
        print_data_blocks(vdf)
    if args.data:
        print_data_series(vdf)

    if args.raw_section is not None:
        print_raw_section(vdf, args.raw_section)

    if args.validate:
        print_validation(vdf)
    if args.extract:
        print_extracted_results(vdf)

    if not show_specific or show_all:
        print_summary(vdf)


if __name__ == "__main__":
    main()
