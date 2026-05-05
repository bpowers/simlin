#!/usr/bin/env python3
"""
VDF X-Ray: inspect and debug Vensim VDF (binary data file) format.

Distilled from the Rust parser (src/simlin-engine/src/vdf.rs) and the
CLI dump tool (src/simlin-cli/src/vdf_dump.rs). See docs/design/vdf.md
for confirmed structure and reverse-engineering notes.

Usage:
    python tools/vdf_xray.py <path.vdf> [--section N] [--names] [--records]
                                         [--ot] [--blocks] [--data] [--all]
                                         [--raw-section N] [--json]
    python tools/vdf_xray.py --corpus-precision [repo-root]
"""

from __future__ import annotations

import argparse
from bisect import bisect_right
from itertools import product
import json
import math
import re
import subprocess
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

VDF_FILE_MAGIC = bytes([0x7F, 0xF7, 0x17, 0x52])
# Observed on local zambaqui sensitivity/optimization runs. The ordinary
# eight-section result structures parse like 0x52 files, but bytes after the
# normal sparse-block run contain additional payload we have not decoded.
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
class SlotNameAlignment:
    leading_extra_slots: int
    score: int
    hidden_slots: list[int]
    mapped_visible_names: int
    slot_to_names: dict[int, list[str]]


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
class RecordShapeBlock:
    start: int
    end: int
    ot_codes: list[int]
    record_indices: list[int] = field(default_factory=list)
    shape_record_indices: list[int] = field(default_factory=list)
    shape_codes: list[int] = field(default_factory=list)
    sort_keys: list[int] = field(default_factory=list)
    slot_refs: list[int] = field(default_factory=list)

    def length(self) -> int:
        return self.end - self.start

    def homogeneous_ot_codes(self) -> bool:
        return len(set(self.ot_codes)) <= 1


@dataclass
class OwnerRecordBlock:
    start: int
    end: int
    ot_codes: list[int]
    sentinel_record_indices: list[int] = field(default_factory=list)
    shape_codes: list[int] = field(default_factory=list)
    slot_refs: list[int] = field(default_factory=list)
    hidden_slot_refs: list[int] = field(default_factory=list)
    direct_sort_keys: list[int] = field(default_factory=list)
    attached_sort_keys: list[int] = field(default_factory=list)
    sort_anchor_record_indices: list[int] = field(default_factory=list)
    hidden: bool = False

    def length(self) -> int:
        return self.end - self.start

    def homogeneous_ot_codes(self) -> bool:
        return len(set(self.ot_codes)) <= 1


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


@dataclass
class RecordSpanOverlapComponent:
    component_id: int
    start: int
    end: int
    spans: list[DecodedRecordSpan]


@dataclass
class Field11UnionFact:
    """
    Direct record `field[11]` interpretation candidates.

    This deliberately does not decide which interpretation is correct. It only
    reports whether the same raw word is structurally valid as an owner OT
    start, as a section-6 lookup-record index, or both.
    """
    rec_idx: int
    name_idx: int
    name: str
    raw_field11: int
    shape_code: int
    shape_length: Optional[int]
    sort_key: int
    slot_ref: int
    has_sentinel: bool
    owner_start: Optional[int]
    owner_end: Optional[int]
    owner_ot_codes: list[int]
    lookup_index: Optional[int]
    lookup_ot_index: Optional[int]
    lookup_width: Optional[int]
    lookup_dependency_ref_word: Optional[int]

    @property
    def lookup_width_matches_shape(self) -> bool:
        return (
            self.shape_length is not None
            and self.lookup_width is not None
            and self.lookup_width == self.shape_length
        )


@dataclass
class Field11UnionCorrelation:
    """
    Diagnostic relation between an ambiguous `field[11]` record and its lookup
    record's evaluated output OT.

    This is not an owner/descriptor decision. It keeps the relation explicit so
    fixture runs can show where output-sort proximity is strong evidence and
    where it fails as a general discriminator.
    """
    fact: Field11UnionFact
    output_spans: list[DecodedRecordSpan]
    closest_output_span: Optional[DecodedRecordSpan]
    output_sort_delta: Optional[int]
    overlap_component_id: Optional[int]
    overlap_component_start: Optional[int]
    overlap_component_end: Optional[int]
    overlap_component_spans: list[DecodedRecordSpan]


@dataclass
class MdlDimension:
    name: str
    elements: list[str]
    line_no: int


@dataclass
class MdlDefinition:
    name: str
    kind: str
    dimensions: list[str]
    header: str
    source_index: int
    line_no: int
    expression: str = ""

    def is_stock(self) -> bool:
        return self.kind == "stock"

    def is_arrayed(self) -> bool:
        return len(self.dimensions) > 0


@dataclass
class MdlModel:
    dimensions: dict[str, MdlDimension]
    definitions: list[MdlDefinition]
    sketch_names: list[str] = field(default_factory=list)


@dataclass
class MdlBlockMatch:
    definition: MdlDefinition
    candidate_block_indices: list[int]


# ---- VDF File ----

@dataclass
class VdfFile:
    data: bytes
    time_point_count: int
    bitmap_size: int
    block_time_point_count: int
    block_bitmap_size: int
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
        Observed section-1 slot/reference area pointer.

        This lands at or inside the u32 slot/ref area before the name table.
        It is not yet the solved visible slot-table start on all edited files:
        some fixtures have leading helper/stale entries before the visible
        suffix selected by `find_slot_table`.
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
        data_start = self.first_data_block + 2 + self.bitmap_size
        if data_start + count * 4 > len(self.data):
            return None
        return [f32(self.data, data_start + i * 4) for i in range(count)]

    def _block_positions_for_time_values(self, time_values: list[float]) -> list[int]:
        if not time_values:
            return []
        if self.block_time_point_count == len(time_values):
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
        origin = time_values[-1] - (self.block_time_point_count - 1) * step
        positions: list[int] = []
        for value in time_values:
            pos = int(round((value - origin) / step))
            positions.append(pos)
        return positions

    def _block_bitmap_layout(self, block_offset: int, count: int) -> tuple[int, int]:
        """
        Decode the bitmap width for a data block from its declared value count.

        Edited econ runs can mix saved-suffix blocks and full-grid blocks in
        the same file. The C-readable invariant is that the block's u16 count
        equals the popcount of the bitmap bytes that precede its f32 payload.
        Prefer the compact saved-time bitmap when both widths happen to match.
        """
        candidates: list[tuple[int, int]] = [
            (self.bitmap_size, self.time_point_count),
            (self.block_bitmap_size, self.block_time_point_count),
        ]
        seen_sizes: set[int] = set()
        for bitmap_size, grid_count in candidates:
            if bitmap_size in seen_sizes:
                continue
            seen_sizes.add(bitmap_size)
            bm_start = block_offset + 2
            bm_end = bm_start + bitmap_size
            if bm_end > len(self.data):
                continue
            bit_count = sum(byte.bit_count() for byte in self.data[bm_start:bm_end])
            if bit_count == count:
                return bitmap_size, grid_count
        return self.block_bitmap_size, self.block_time_point_count

    def extract_block_series(self, block_offset: int, time_values: list[float]) -> list[float]:
        if block_offset == self.first_data_block:
            extracted = self.extract_time_values()
            if extracted is None:
                return [float("nan")] * len(time_values)
            return extracted

        count = u16(self.data, block_offset)
        bitmap_size, step_count = self._block_bitmap_layout(block_offset, count)
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
        else:
            positions = self._block_positions_for_time_values(time_values)
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

def build_slot_to_names(vdf: VdfFile) -> dict[int, list[str]]:
    return build_slot_to_names_with_offset(vdf, 0)


def build_direct_slot_to_names(vdf: VdfFile) -> dict[int, list[str]]:
    """
    Direct slot-table pairing: slot_table[i] belongs to names[i].

    This is the structural mapping used for format claims. The preferred
    display alignment below is an exploratory xray heuristic for edited files
    with leading helper slots; do not use it as evidence for on-disk refs.
    """
    return build_slot_to_names(vdf)


def build_slot_to_names_with_offset(vdf: VdfFile, leading_extra_slots: int) -> dict[int, list[str]]:
    out: dict[int, list[str]] = {}
    if leading_extra_slots < 0:
        leading_extra_slots = 0
    for i, slot in enumerate(vdf.slot_table[leading_extra_slots:]):
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


def _slot_name_alignment_class_score(name: Optional[str], cls: str,
                                     *, section_kind: str) -> int:
    """RECONSTRUCTION HEURISTIC: score a (name, classification) pair against
    the section that references it.

    Not a decoded format fact: the scoring weights below are tuned to keep
    obvious leading helper slots from shadowing visible owner names in the
    xray display; a 90s C reader would not have needed such a score.
    """
    if name is None:
        return 0
    if section_kind == "sec4":
        if name == "Time":
            return 10
        if cls == "system":
            return 4
        if cls in {"group", "unit", "meta", "builtin?", "signature", "quoted"}:
            return -2
        return 1
    if section_kind == "sec5_payload":
        if cls in {"group", "unit", "meta", "builtin?", "signature", "quoted"}:
            return -2
        if cls == "system":
            return 3
        return 2
    if section_kind == "sec5_trailing":
        if cls in {"group", "unit", "meta", "builtin?", "signature", "quoted"}:
            return -1
        return 1
    if section_kind == "sec6":
        if cls in {"group", "unit", "meta", "builtin?", "signature", "quoted"}:
            return -1
        if cls == "system":
            return 1
        return 1
    return 0


def score_slot_name_alignment(vdf: VdfFile, leading_extra_slots: int) -> SlotNameAlignment:
    """RECONSTRUCTION HEURISTIC: score a visible-name alignment against
    section-4/5/6 reference usage.

    This is an exploratory alignment heuristic, not a decoded file-format
    structure. It is used by xray display helpers and by owner-block discovery
    only to avoid treating obvious leading helper slots as visible owners.
    A 90s C reader would have indexed names directly from the slot table.
    """
    slot_to_names = build_slot_to_names_with_offset(vdf, leading_extra_slots)

    def lookup(slot_ref: int) -> tuple[Optional[str], str]:
        names = slot_to_names.get(slot_ref)
        if not names:
            return None, ""
        name = names[0]
        return name, classify_name(name)

    score = 0

    sec4_entries = vdf.parse_section4_entries() or []
    for entry in sec4_entries:
        for slot_ref in entry.refs:
            name, cls = lookup(slot_ref)
            score += _slot_name_alignment_class_score(name, cls, section_kind="sec4")

    sec5_entries = vdf.parse_section5_sets() or []
    for entry in sec5_entries:
        for slot_ref in section5_payload_refs(entry):
            if slot_ref <= 0:
                continue
            name, cls = lookup(slot_ref)
            score += _slot_name_alignment_class_score(name, cls, section_kind="sec5_payload")
        for slot_ref in section5_trailing_refs(entry):
            if slot_ref <= 0:
                continue
            name, cls = lookup(slot_ref)
            score += _slot_name_alignment_class_score(name, cls, section_kind="sec5_trailing")

    sec6_result = vdf.parse_section6_ref_stream()
    if sec6_result:
        for entry in sec6_result[1]:
            for slot_ref in entry.refs:
                name, cls = lookup(slot_ref)
                score += _slot_name_alignment_class_score(name, cls, section_kind="sec6")

    mapped_visible_names = min(len(vdf.names), max(0, len(vdf.slot_table) - leading_extra_slots))
    return SlotNameAlignment(
        leading_extra_slots=leading_extra_slots,
        score=score,
        hidden_slots=vdf.slot_table[:leading_extra_slots],
        mapped_visible_names=mapped_visible_names,
        slot_to_names=slot_to_names,
    )


def best_slot_name_alignment(vdf: VdfFile, max_leading_extra_slots: int = 8) -> SlotNameAlignment:
    """RECONSTRUCTION HEURISTIC: pick the highest-scoring alignment across
    leading_extra_slots in `0..=max_leading_extra_slots`.

    Supports `preferred_slot_name_alignment` only. Not a decoded format fact.
    """
    if not vdf.slot_table:
        return SlotNameAlignment(0, 0, [], 0, {})

    limit = min(max_leading_extra_slots, len(vdf.slot_table) - 1)
    best = score_slot_name_alignment(vdf, 0)
    for leading in range(1, limit + 1):
        candidate = score_slot_name_alignment(vdf, leading)
        if candidate.score > best.score:
            best = candidate
    return best


def preferred_slot_name_alignment(vdf: VdfFile) -> SlotNameAlignment:
    """RECONSTRUCTION HEURISTIC: use a shifted visible-name mapping only when
    it beats the default clearly.

    The +4 threshold below is an empirical tie-break margin; not a decoded
    format fact. The authoritative slot→name pairing is
    `build_direct_slot_to_names` (slot_table[i] ↔ names[i]); this helper only
    covers edited fixtures where leading helper slots break that direct
    pairing visually.
    """
    default = score_slot_name_alignment(vdf, 0)
    best = best_slot_name_alignment(vdf)
    if best.leading_extra_slots > 0 and best.score >= default.score + 4:
        return best
    return default


def build_display_slot_to_names(vdf: VdfFile) -> dict[int, list[str]]:
    """RECONSTRUCTION HEURISTIC wrapper: display-only slot→name map using the
    preferred (possibly shifted) alignment. For the pinned structural
    pairing use `build_direct_slot_to_names`.
    """
    return preferred_slot_name_alignment(vdf).slot_to_names


def visible_slot_name_pairs(vdf: VdfFile, *,
                            alignment: Optional[SlotNameAlignment] = None) -> list[tuple[str, int]]:
    alignment = alignment or preferred_slot_name_alignment(vdf)
    pairs: list[tuple[str, int]] = []
    for i in range(alignment.mapped_visible_names):
        pairs.append((vdf.names[i], vdf.slot_table[alignment.leading_extra_slots + i]))
    return pairs


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


def parse_mdl_lhs(lhs: str) -> tuple[str, list[str]]:
    match = MDL_LHS_RE.match(lhs.strip())
    if match is None:
        return lhs.strip(), []
    name = match.group("name").strip()
    dims_text = match.group("dims")
    if not dims_text:
        return name, []
    dims = [part.strip() for part in dims_text.split(",") if part.strip()]
    return name, dims


def parse_mdl_expression(lines: list[str], start_idx: int, rhs: str) -> tuple[str, int]:
    parts: list[str] = []
    if rhs.strip():
        parts.append(rhs.strip())

    j = start_idx + 1
    while j < len(lines):
        probe = lines[j].strip()
        if (probe.startswith("~")
                or probe == "|"
                or probe.startswith("********************************************************")):
            break
        if probe:
            parts.append(probe)
        j += 1
    return " ".join(parts).strip(), j


def parse_mdl_sketch_names(lines: list[str]) -> list[str]:
    names: list[str] = []
    seen: set[str] = set()
    in_sketch = False

    for raw in lines:
        line = raw.strip()
        if line.startswith("*View"):
            in_sketch = True
            continue
        if not in_sketch:
            continue
        if line.startswith("///---\\\\\\"):
            break
        match = MDL_SKETCH_VAR_RE.match(line)
        if match is None:
            continue
        name = match.group(1).strip()
        if name == "Time" or name in seen:
            continue
        seen.add(name)
        names.append(name)
    return names


def mdl_definition_runtime_class(definition: MdlDefinition) -> str:
    if definition.is_stock():
        return "stock"
    if MDL_NUMERIC_LITERAL_RE.fullmatch(definition.expression):
        return "const"
    return "dynamic"


def mdl_sketch_definitions(model: MdlModel, *,
                           include_kinds: Optional[set[str]] = None) -> list[MdlDefinition]:
    if include_kinds is None:
        include_kinds = {"stock", "var"}
    by_name = {
        definition.name: definition
        for definition in model.definitions
        if definition.kind in include_kinds
    }
    return [by_name[name] for name in model.sketch_names if name in by_name]


def parse_mdl_model(text: str) -> MdlModel:
    """
    Parse the definition and dimension headers from a Vensim .mdl source file.

    This is intentionally shallow: it does not parse expressions, only the
    model declarations needed to align visible names with VDF structure.
    """
    dimensions: dict[str, MdlDimension] = {}
    definitions: list[MdlDefinition] = []
    source_index = 0
    lines = text.splitlines()
    i = 0
    while i < len(lines):
        raw = lines[i].rstrip()
        line = raw.strip()

        if line.startswith("********************************************************"):
            break
        if not line or line == "{UTF-8}" or line.startswith("~") or line == "|":
            i += 1
            continue

        if line.endswith(":") and "=" not in line:
            name = line[:-1].strip()
            elements: list[str] = []
            j = i + 1
            while j < len(lines):
                probe = lines[j].strip()
                if not probe:
                    j += 1
                    continue
                if (probe.startswith("~")
                        or probe.startswith("|")
                        or probe.startswith("********************************************************")):
                    break
                elements.extend(part.strip() for part in probe.split(",") if part.strip())
                j += 1
            dimensions[name] = MdlDimension(name=name, elements=elements, line_no=i + 1)
            i = j
            continue

        if "=" in raw:
            lhs, rhs = raw.split("=", 1)
            name, dims = parse_mdl_lhs(lhs)
            expression, next_idx = parse_mdl_expression(lines, i, rhs)
            source_index += 1
            kind = "stock" if "INTEG" in expression.upper() else "var"
            definitions.append(MdlDefinition(
                name=name,
                kind=kind,
                dimensions=dims,
                header=line,
                source_index=source_index,
                line_no=i + 1,
                expression=expression,
            ))
            i = next_idx
            continue

        if line.endswith("("):
            source_index += 1
            definitions.append(MdlDefinition(
                name=line[:-1].strip(),
                kind="lookup",
                dimensions=[],
                header=line,
                source_index=source_index,
                line_no=i + 1,
                expression=line,
            ))
        i += 1

    return MdlModel(
        dimensions=dimensions,
        definitions=definitions,
        sketch_names=parse_mdl_sketch_names(lines),
    )


def mdl_definition_flat_size(model: MdlModel, definition: MdlDefinition) -> Optional[int]:
    if not definition.dimensions:
        return 1
    size = 1
    for dim_name in definition.dimensions:
        dim = model.dimensions.get(dim_name)
        if dim is None or not dim.elements:
            return None
        size *= len(dim.elements)
    return size


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


def record_shape_length(vdf: VdfFile, rec: VdfRecord) -> Optional[int]:
    """
    Recover the OT span implied by record field[6] for current reconstruction.

    This helper intentionally preserves older exploratory behavior around
    active `index_word=0` shapes because some small edit-chain analyses still
    compare those candidates. Use `decoded_record_shape_length` for fact-only
    record reports that exclude the ambiguous `f[6]=0` case.

    Current decoded/reconstruction rules:
    - `5` always means scalar (len=1)
    - an active sec3 `index_word` gives its flat size
    - Ref-style multi-shape directories bind explicit field[6] codes to the
      following physical sec3 entry
    - `32` is the generic array marker and resolves only when section 3 exposes
      a single active flat size
    """
    code = rec.shape_code()
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


def decoded_record_shape_length(vdf: VdfFile, rec: VdfRecord) -> Optional[int]:
    """
    Fact-only shape span for direct record reports.

    Records with `f[6]=0` are excluded here. They can coincide with an active
    section-3 `index_word=0` in some files, but Ref.vdf shows many such records
    are dimension anchors, dimension elements, builtins, or descriptors rather
    than emitted series owners. Until the direct discriminator is decoded, that
    case remains reconstruction-only.
    """
    code = rec.shape_code()
    if code == 0:
        return None
    return record_shape_length(vdf, rec)


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


def build_record_shape_blocks(vdf: VdfFile) -> list[RecordShapeBlock]:
    """
    Group records by decoded shape span instead of raw ot_index.

    A visible owner signal can split across records: one record may contribute
    the shape-derived span while another positive-sort record lands inside that
    span. This helper keeps the grouping structural and leaves name ownership
    unresolved when the file does not force it.
    """
    codes = vdf.section6_ot_class_codes() or []
    system_keys = system_record_name_keys(vdf)
    by_range: dict[tuple[int, int], RecordShapeBlock] = {}

    def add_record(block: RecordShapeBlock, rec_idx: int, rec: VdfRecord, *,
                   is_shape_record: bool) -> None:
        if rec_idx not in block.record_indices:
            block.record_indices.append(rec_idx)
        if is_shape_record and rec_idx not in block.shape_record_indices:
            block.shape_record_indices.append(rec_idx)
        shape_code = rec.shape_code()
        if is_shape_record and shape_code not in block.shape_codes:
            block.shape_codes.append(shape_code)
        sort_key = rec.fields[10]
        if sort_key > 0 and sort_key not in block.sort_keys:
            block.sort_keys.append(sort_key)
        slot_ref = rec.slot_ref()
        if slot_ref > 0 and slot_ref not in block.slot_refs:
            block.slot_refs.append(slot_ref)

    for rec_idx, rec in enumerate(vdf.records):
        # System records point at system OT slots, not model-variable spans.
        # Filter them here so that record-shape blocks reflect only model
        # owners.
        if rec.fields[2] in system_keys:
            continue
        start = rec.ot_index()
        if start <= 0 or start >= vdf.offset_table_count:
            continue
        length = record_shape_length(vdf, rec)
        if length is None or length <= 0:
            continue
        end = min(vdf.offset_table_count, start + length)
        key = (start, end)
        block = by_range.setdefault(
            key,
            RecordShapeBlock(
                start=start,
                end=end,
                ot_codes=codes[start:end],
            ),
        )
        add_record(block, rec_idx, rec, is_shape_record=True)

    for rec_idx, rec in enumerate(vdf.records):
        if rec.fields[2] in system_keys:
            continue
        start = rec.ot_index()
        if start <= 0 or start >= vdf.offset_table_count or rec.fields[10] <= 0:
            continue
        if record_shape_length(vdf, rec) is not None:
            continue
        for block in by_range.values():
            if block.start <= start < block.end:
                add_record(block, rec_idx, rec, is_shape_record=False)

    blocks = sorted(by_range.values(), key=lambda block: (block.start, block.end))
    for block in blocks:
        block.record_indices.sort()
        block.shape_record_indices.sort()
        block.shape_codes.sort()
        block.sort_keys.sort()
        block.slot_refs.sort()
    return blocks


def sentinel_model_record_indices(vdf: VdfFile) -> list[int]:
    """
    Return indices of sentinel records used as owner-candidate anchors.

    The sentinel pair (f[8]=f[9]=0xf6800000) is a strong signal, but not
    exclusive to model variables: INITIAL TIME and FINAL TIME system records
    carry sentinels too and pair (via the f[2] string-key formula) with the
    matching system name-table entries. Their numeric f[2] keys are not
    globally fixed, so system filtering decodes f[2] to names before dropping
    them from model-owner block construction.

    After reformat, owner records can carry f[0]=0 or f[1]=23, so those
    fields cannot be used as filters. Further system-record discrimination
    (lookup definitions, stdlib-helper records) is handled at the mapping
    layer.
    """
    out: list[int] = []
    system_keys = system_record_name_keys(vdf)
    for rec_idx, rec in enumerate(vdf.records):
        if not rec.has_sentinel():
            continue
        if rec.fields[2] in system_keys:
            continue
        start = rec.ot_index()
        if start <= 0 or start >= vdf.offset_table_count:
            continue
        if record_shape_length(vdf, rec) is None:
            continue
        out.append(rec_idx)
    return out


def build_owner_record_blocks(vdf: VdfFile) -> list[OwnerRecordBlock]:
    """
    Build the narrower owner-oriented block set from sentinel model records.

    In the model-edit fixtures, visible owners are carried by sentinel
    records. Larger fixtures show that sentinel-ness is not the final
    discriminator, so this helper is an owner-candidate reconstruction step,
    not a format proof. Non-sentinel records still matter as sort/order
    anchors, but they over-generate overlapping shape spans in the current
    decoder.
    """
    codes = vdf.section6_ot_class_codes() or []
    alignment = preferred_slot_name_alignment(vdf)
    hidden_slots = set(alignment.hidden_slots)
    by_range: dict[tuple[int, int], OwnerRecordBlock] = {}

    def add_unique(values: list[int], value: int) -> None:
        if value > 0 and value not in values:
            values.append(value)

    for rec_idx in sentinel_model_record_indices(vdf):
        rec = vdf.records[rec_idx]
        start = rec.ot_index()
        length = record_shape_length(vdf, rec)
        if length is None or length <= 0:
            continue
        end = min(vdf.offset_table_count, start + length)
        key = (start, end)
        block = by_range.setdefault(
            key,
            OwnerRecordBlock(
                start=start,
                end=end,
                ot_codes=codes[start:end],
            ),
        )
        block.sentinel_record_indices.append(rec_idx)
        add_unique(block.shape_codes, rec.shape_code())
        slot_ref = rec.slot_ref()
        add_unique(block.slot_refs, slot_ref)
        if slot_ref in hidden_slots:
            add_unique(block.hidden_slot_refs, slot_ref)
        if rec.fields[10] > 0:
            add_unique(block.direct_sort_keys, rec.fields[10])
            add_unique(block.attached_sort_keys, rec.fields[10])

    blocks = sorted(by_range.values(), key=lambda block: (block.start, block.end))
    slot_ref_counts: dict[int, int] = {}
    for block in blocks:
        for slot_ref in block.slot_refs:
            slot_ref_counts[slot_ref] = slot_ref_counts.get(slot_ref, 0) + 1

    for block in blocks:
        block.sentinel_record_indices.sort()
        block.shape_codes.sort()
        block.slot_refs.sort()
        block.hidden_slot_refs.sort()
        block.direct_sort_keys.sort()
        block.attached_sort_keys.sort()
        block.hidden = (
            bool(block.slot_refs)
            and len(block.hidden_slot_refs) == len(block.slot_refs)
            and all(slot_ref_counts.get(slot_ref, 0) == 1 for slot_ref in block.slot_refs)
        )

    visible_blocks = [block for block in blocks if not block.hidden]
    hidden_blocks = [block for block in blocks if block.hidden]

    def attach_sort(block: OwnerRecordBlock, rec_idx: int, sort_key: int) -> None:
        add_unique(block.attached_sort_keys, sort_key)
        add_unique(block.sort_anchor_record_indices, rec_idx)

    def adjacent_visible_from_hidden(hidden_block: OwnerRecordBlock) -> list[OwnerRecordBlock]:
        out: list[OwnerRecordBlock] = []
        for block in visible_blocks:
            if hidden_block.end == block.start:
                if hidden_block.ot_codes and block.ot_codes and hidden_block.ot_codes[-1] == block.ot_codes[0]:
                    out.append(block)
            elif block.end == hidden_block.start:
                if hidden_block.ot_codes and block.ot_codes and hidden_block.ot_codes[0] == block.ot_codes[-1]:
                    out.append(block)
        return out

    for rec_idx, rec in enumerate(vdf.records):
        sort_key = rec.fields[10]
        ot_index = rec.ot_index()
        if rec.has_sentinel() or sort_key <= 0 or ot_index <= 0 or ot_index >= vdf.offset_table_count:
            continue

        direct_hits = [block for block in visible_blocks if block.start <= ot_index < block.end]
        if len(direct_hits) == 1:
            attach_sort(direct_hits[0], rec_idx, sort_key)
            continue

        if direct_hits:
            continue

        hidden_hits = [block for block in hidden_blocks if block.start <= ot_index < block.end]
        if len(hidden_hits) != 1:
            continue

        neighbors = adjacent_visible_from_hidden(hidden_hits[0])
        if len(neighbors) == 1:
            attach_sort(neighbors[0], rec_idx, sort_key)

    for block in blocks:
        block.attached_sort_keys.sort()
        block.sort_anchor_record_indices.sort()

    key_to_name_idx = build_record_name_key_to_name_index(vdf)

    def record_name(rec_idx: int) -> Optional[str]:
        if rec_idx >= len(vdf.records):
            return None
        name_idx = key_to_name_idx.get(vdf.records[rec_idx].fields[2])
        if name_idx is None:
            return None
        return vdf.names[name_idx]

    def new_style_signature_alias(name: str) -> Optional[str]:
        if not name.startswith("#") or not name.endswith("#"):
            return None
        inner = name[1:-1]
        alias, sep, _rest = inner.partition(">")
        if not sep or not alias:
            return None
        return alias

    def block_has_visible_alias_owner(block: OwnerRecordBlock) -> bool:
        aliases = [
            alias.lower()
            for rec_idx in block.sentinel_record_indices
            if (name := record_name(rec_idx)) is not None
            if (alias := new_style_signature_alias(name)) is not None
        ]
        if not aliases:
            return False

        for candidate in visible_blocks:
            if candidate is block:
                continue
            candidate_names = [
                name.lower()
                for rec_idx in candidate.sentinel_record_indices
                if (name := record_name(rec_idx)) is not None
            ]
            if any(alias in candidate_names for alias in aliases):
                return True
        return False

    # Edited SMOOTH fixtures can save a `#alias>FUNC#` helper stock immediately
    # before the visible stock array. The decoded signature name is the guard:
    # without it, adjacency alone would be too broad an ownership rule.
    for block in blocks:
        if block.hidden:
            continue
        if block.length() != 1 or not block.direct_sort_keys:
            continue
        if not block.ot_codes or any(code != OT_CODE_STOCK for code in block.ot_codes):
            continue
        if not block_has_visible_alias_owner(block):
            continue
        neighbor = next(
            (
                candidate for candidate in blocks
                if not candidate.hidden
                and candidate.start == block.end
                and candidate.length() > block.length()
                and candidate.ot_codes
                and all(code == OT_CODE_STOCK for code in candidate.ot_codes)
                and not candidate.direct_sort_keys
            ),
            None,
        )
        if neighbor is not None:
            block.hidden = True
            for sort_key in block.attached_sort_keys:
                if sort_key not in block.direct_sort_keys:
                    add_unique(neighbor.attached_sort_keys, sort_key)
            for rec_idx in block.sort_anchor_record_indices:
                add_unique(neighbor.sort_anchor_record_indices, rec_idx)
            neighbor.attached_sort_keys.sort()
            neighbor.sort_anchor_record_indices.sort()

    # Filter out unanchored system variable sentinel blocks. These have a
    # slot_ref not shared by any other block (system variables live in their own
    # slot group, separate from model views), no sort keys, and only constant or
    # time OT codes. Model variables can have sort_key=0 but they share slot_refs
    # with other model variables in the same view.
    for block in blocks:
        if block.hidden:
            continue
        if block.direct_sort_keys or block.attached_sort_keys:
            continue
        if not block.slot_refs:
            continue
        if not all(
            code in (OT_CODE_CONST, OT_CODE_TIME) for code in block.ot_codes
        ):
            continue
        if all(
            slot_ref_counts.get(sr, 0) == 1 for sr in block.slot_refs
        ):
            block.hidden = True

    return blocks


def owner_blocks_in_sentinel_order(vdf: VdfFile, *,
                                   include_hidden: bool = False) -> list[OwnerRecordBlock]:
    """
    Return owner blocks in their sentinel-record/file order.

    This is a VDF-local ordering signal, not a guaranteed sketch-order signal.
    Older fixtures often preserve sketch order here, but cleanup/reformat saves
    can reshuffle the sentinel records without changing owner identity.
    """
    blocks = build_owner_record_blocks(vdf)
    if not include_hidden:
        blocks = [block for block in blocks if not block.hidden]
    return sorted(
        blocks,
        key=lambda block: (
            block.sentinel_record_indices[0] if block.sentinel_record_indices else len(vdf.records),
            block.start,
            block.end,
        ),
    )


def _block_sort_rank(block: OwnerRecordBlock) -> int:
    keys = block.direct_sort_keys or block.attached_sort_keys
    if not keys:
        return 1_000_000_000
    return min(keys)


def _select_non_overlapping_owner_blocks(
    blocks: list[OwnerRecordBlock],
) -> list[OwnerRecordBlock]:
    """RECONSTRUCTION HEURISTIC: resolve conflicting record-derived owner
    spans into an OT partition.

    `field[11]` is an owner OT start for ordinary model-variable records, but
    `Ref.vdf` shows lookup/graphical-function descriptor records whose
    address-like fields point into the same runtime OT region as real saved
    variables. A Vensim-era C writer would not need to normalize that before
    dumping structs (because the writer knows which record owns which OT),
    so this layer resolves it by keeping the largest non-overlapping set of
    candidate spans via DP. When two choices cover the same number of OT
    slots, the lower sort/order keys win; in observed conflicts those are
    the real variable records, while late lookup descriptors sort far away
    from the local owner run. The fact that we need this at all means we
    have not yet decoded the format's owner/descriptor discriminator.
    """
    if len(blocks) <= 1:
        return blocks[:]

    ordered = sorted(blocks, key=lambda block: (block.end, block.start))
    ends = [block.end for block in ordered]

    # dp[i] covers ordered[:i] and stores (score, selected ordered indices).
    dp: list[tuple[tuple[int, int, int], list[int]]] = [((0, 0, 0), [])]
    for idx, block in enumerate(ordered):
        prev_idx = bisect_right(ends, block.start) - 1
        prev_score, prev_selection = dp[prev_idx + 1] if prev_idx >= 0 else ((0, 0, 0), [])

        rank = _block_sort_rank(block)
        weight = (block.length(), -rank, 1)
        include_score = tuple(prev_score[i] + weight[i] for i in range(3))
        include_selection = prev_selection + [idx]

        exclude_score, exclude_selection = dp[-1]
        if include_score > exclude_score:
            dp.append((include_score, include_selection))
        else:
            dp.append((exclude_score, exclude_selection))

    selected_ids = {id(ordered[idx]) for idx in dp[-1][1]}
    return [block for block in blocks if id(block) in selected_ids]


def owner_block_runtime_class(block: OwnerRecordBlock) -> str:
    if block.ot_codes and all(code == OT_CODE_STOCK for code in block.ot_codes):
        return "stock"
    if block.ot_codes and all(code == OT_CODE_CONST for code in block.ot_codes):
        return "const"
    return "dynamic"


# ---- VDF-native name mapping ----

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
        if rec.fields[6] != 0:
            continue
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
            element_groups.setdefault(group_id, []).append((rec.fields[11], rec_idx, name))
            continue

        if rec.fields[14] == VDF_SENTINEL:
            candidate_dims.setdefault(group_id, []).append((rec_idx, name, rec.fields[11]))

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
    # records. Partial-single-element roots keep the one recorded element
    # (e.g. Ref.vdf's `scenario`); callers already treat those as partial
    # via precision diagnostics.
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


def visible_variable_candidates(vdf: VdfFile) -> list[str]:
    """
    Filter the name table to candidate variable names (names that could own OT
    entries). Removes system variables, metadata prefixed names, builtins,
    module IO names, stdlib helpers, and unslotted names.

    Dimension/element filtering is NOT done here because it requires OT
    validation to distinguish ambiguous cases. That's handled in the mapping
    stage.
    """
    slotted_count = len(vdf.slot_table)
    candidates: list[str] = []
    for i, name in enumerate(vdf.names):
        if i >= slotted_count:
            break
        if not _is_visible_model_name(name):
            continue
        candidates.append(name)
    return candidates


@dataclass
class NameMapping:
    """Result of the current record-key owner-to-OT reconstruction."""
    variable_names: list[str]  # Alphabetically sorted structural result names
    owner_blocks: list[OwnerRecordBlock]  # Sort-key ordered owner blocks
    name_to_block: dict[str, OwnerRecordBlock]
    system_ot_indices: set[int]  # OT indices owned by system variables
    unmapped_blocks: list[OwnerRecordBlock]  # Blocks that couldn't be mapped


def _vensim_sort_key(name: str) -> str:
    """Vensim sorts names case-insensitively."""
    return name.lower()


def _heuristic_name_looks_lookupish(name: str) -> bool:
    """
    RECONSTRUCTION HEURISTIC: lexical test for lookup/table/graphical-function
    names.

    Matches any name containing the substrings "lookup", "table", or
    "graphical function" (case-insensitive). This is a pure name-string
    heuristic, not a decoded file-format rule. Callers use it to route
    lookup-shaped names away from stock ownership decisions; see
    `_heuristic_name_allowed_for_block` and `_lookup_record_names`.
    """
    lower = name.lower()
    return "lookup" in lower or "table" in lower or "graphical function" in lower


def _heuristic_name_allowed_for_block(name: str, block: OwnerRecordBlock, *,
                                      excluded_names: Optional[set[str]] = None,
                                      allow_stock_lookupish: bool = False) -> bool:
    """
    RECONSTRUCTION HEURISTIC: decide whether a name may own `block`.

    Filters out display/navigation-metadata prefixes (".", "-", ":") and
    module names, then, when `allow_stock_lookupish` is False, rejects
    lookup/table names from stock-coded owner blocks. This second step is a
    lexical rule (via `_heuristic_name_looks_lookupish`), not a decoded
    Vensim field. Callers run the mapping pass twice, first with
    `allow_stock_lookupish=False` and then with `True`, so the heuristic
    narrows ambiguous stock blocks without hiding lookup-shaped real stocks.
    """
    if not name:
        return False
    if name in VENSIM_MODULE_NAMES:
        return False
    if excluded_names is not None and name.lower() in excluded_names:
        return False
    # Only display/navigation metadata is excluded structurally. Quoted names,
    # builtin-looking names, stdlib helpers, and #...# runtime signatures can
    # carry direct record-key OT bindings and must remain extractable.
    if name.startswith((".", "-", ":")):
        return False
    # Lookup/table names that land on stock-coded owner blocks behave like
    # internal aliases, not visible stock variables. Keep them out of the
    # stock ordering pass; standalone lookup extraction is handled separately.
    if (
        not allow_stock_lookupish and
        block.ot_codes
        and all(code == OT_CODE_STOCK for code in block.ot_codes)
        and _heuristic_name_looks_lookupish(name)
    ):
        return False
    return True


def _group_ot_positions(codes: list[int], *, want_stock: bool) -> list[int]:
    return [
        ot_idx for ot_idx, code in enumerate(codes)
        if ot_idx > 0 and ((code == OT_CODE_STOCK) == want_stock)
    ]


def _assign_group_positions(
    items: list[tuple[str, int, Optional[int]]],
    positions: list[int],
) -> Optional[dict[str, int]]:
    """RECONSTRUCTION HEURISTIC: assign ordered named items into available OT
    positions.

    Anchored items carry a concrete OT start and must land there. Unanchored
    items are placed greedily at the earliest position that still leaves room
    for the remaining anchored items. This is a linear feasibility check over
    the real OT layout, allowing anonymous helper gaps anywhere in the group.
    Used only as a fallback when deterministic record→OT mapping leaves gaps.
    """
    total_len = sum(length for _, length, _ in items)
    if total_len > len(positions):
        return None

    pos_index = {pos: i for i, pos in enumerate(positions)}
    anchored_start_indices: list[Optional[int]] = [None] * len(items)
    anchored_after_lengths = [0] * len(items)

    next_anchor_idx: Optional[int] = None
    lengths_before_next_anchor = 0
    for item_idx in range(len(items) - 1, -1, -1):
        _, length, start = items[item_idx]
        anchored_after_lengths[item_idx] = lengths_before_next_anchor
        if start is not None:
            start_idx = pos_index.get(start)
            if start_idx is None or start_idx + length > len(positions):
                return None
            if positions[start_idx:start_idx + length] != list(range(start, start + length)):
                return None
            anchored_start_indices[item_idx] = start_idx
            next_anchor_idx = item_idx
            lengths_before_next_anchor = 0
        else:
            lengths_before_next_anchor += length

    assigned: dict[str, int] = {}
    cursor = 0
    for item_idx, (name, length, start) in enumerate(items):
        start_idx = anchored_start_indices[item_idx]
        if start_idx is not None and start is not None:
            if cursor > start_idx:
                return None
            assigned[name] = start
            cursor = start_idx + length
            continue

        if cursor + length > len(positions):
            return None
        remaining_before_anchor = anchored_after_lengths[item_idx]
        next_anchor_pos = None
        for future_idx in range(item_idx + 1, len(items)):
            future_start_idx = anchored_start_indices[future_idx]
            if future_start_idx is not None:
                next_anchor_pos = future_start_idx
                break
        if next_anchor_pos is not None and cursor + length + remaining_before_anchor > next_anchor_pos:
            return None
        assigned[name] = positions[cursor]
        cursor += length

    return assigned


def _nonstock_assignment_items(
    name_to_block: dict[str, OwnerRecordBlock],
) -> list[tuple[str, int, Optional[int]]]:
    """RECONSTRUCTION HEURISTIC: merge non-stock model names with system
    names into a single Vensim-sorted item stream for gap-filled OT
    assignment.

    The merge interleaves two alphabetically-sorted sequences; a 90s C
    reader would read system variables directly from their dedicated OT
    slots. Kept as fallback when a deterministic mapping does not reach
    system variables.
    """
    nonstock_names = sorted(
        (
            name for name, block in name_to_block.items()
            if not (block.ot_codes and all(code == OT_CODE_STOCK for code in block.ot_codes))
        ),
        key=_vensim_sort_key,
    )
    system_names_sorted = sorted(
        (name for name in SYSTEM_NAMES if name != "Time"),
        key=_vensim_sort_key,
    )

    items: list[tuple[str, int, Optional[int]]] = []
    i = 0
    j = 0
    while i < len(nonstock_names) or j < len(system_names_sorted):
        if j >= len(system_names_sorted):
            name = nonstock_names[i]
            block = name_to_block[name]
            items.append((name, block.length(), block.start))
            i += 1
            continue
        if i >= len(nonstock_names):
            items.append((system_names_sorted[j], 1, None))
            j += 1
            continue
        if _vensim_sort_key(nonstock_names[i]) <= _vensim_sort_key(system_names_sorted[j]):
            name = nonstock_names[i]
            block = name_to_block[name]
            items.append((name, block.length(), block.start))
            i += 1
        else:
            items.append((system_names_sorted[j], 1, None))
            j += 1
    return items


def _lookup_record_names(vdf: VdfFile) -> list[str]:
    """
    Return lookup/table names in slotted name-table order.

    Some files write section-6 lookup records 1:1 with lookupish name-table
    entries, so the caller may pair them by position when the counts match.
    Ref.vdf proves this is not universal; count mismatch means the lookup
    payload structure still needs a more direct decoder.
    """
    out: list[str] = []
    seen: set[str] = set()
    for name in vdf.names[:len(vdf.slot_table)]:
        if classify_name(name):
            continue
        if not _heuristic_name_looks_lookupish(name):
            continue
        key = name.lower()
        if key in seen:
            continue
        seen.add(key)
        out.append(name)
    return out


def _array_element_labels_for_block(
    vdf: VdfFile,
    block: OwnerRecordBlock,
    dimension_sets: list[RecoveredDimensionSet],
    shape_label_bindings: Optional[dict[int, list[str]]] = None,
) -> Optional[list[str]]:
    if block.length() <= 1:
        return None

    anchor_labels = _array_element_labels_from_sort_anchor(vdf, block, dimension_sets)
    if anchor_labels is not None:
        return anchor_labels

    shape_key = _shape_template_key_for_block(vdf, block)
    if shape_label_bindings is not None and shape_key is not None:
        labels = shape_label_bindings.get(shape_key)
        if labels is not None and len(labels) == block.length():
            return labels

    matches = [dim.elements for dim in dimension_sets if len(dim.elements) == block.length()]
    if len(matches) == 1:
        return matches[0]

    shape_entries = [
        entry
        for code in block.shape_codes
        if (entry := section3_entry_for_record_shape_code(vdf, code)) is not None
        and entry.flat_size() == block.length()
    ]
    if len(shape_entries) != 1:
        return None

    axis_sizes = shape_entries[0].axis_sizes()
    if len(axis_sizes) <= 1 or math.prod(axis_sizes) != block.length():
        return None

    axes: list[list[str]] = []
    for axis_size in axis_sizes:
        axis_matches = [dim.elements for dim in dimension_sets if len(dim.elements) == axis_size]
        if len(axis_matches) != 1:
            return None
        axes.append(axis_matches[0])

    return [",".join(coords) for coords in product(*axes)]


def _shape_template_entry_for_block(
    vdf: VdfFile,
    block: OwnerRecordBlock,
) -> Optional[Section3Entry]:
    """
    Resolve the section-3 shape template used by an owner block.

    Explicit shape codes can point directly through the decoded section-3
    bridge. The generic `32` array marker needs the same conservative handling
    as span decoding: it resolves only when exactly one active section-3 entry
    has the block's flat size.
    """
    by_offset: dict[int, Section3Entry] = {}
    for code in block.shape_codes:
        entry = section3_entry_for_record_shape_code(vdf, code)
        if entry is not None and entry.flat_size() == block.length():
            by_offset[entry.file_offset] = entry

    if not by_offset and 32 in block.shape_codes:
        directory = vdf.parse_section3_directory()
        if directory is not None:
            active = [
                entry
                for entry in directory.entries
                if entry.flat_size() == block.length() and entry.flat_size() > 0
            ]
            if len(active) == 1:
                by_offset[active[0].file_offset] = active[0]

    if len(by_offset) != 1:
        return None
    return next(iter(by_offset.values()))


def _shape_template_key_for_block(vdf: VdfFile, block: OwnerRecordBlock) -> Optional[int]:
    entry = _shape_template_entry_for_block(vdf, block)
    return entry.file_offset if entry is not None else None


def _shape_template_label_bindings(
    vdf: VdfFile,
    blocks: list[OwnerRecordBlock],
    dimension_sets: list[RecoveredDimensionSet],
) -> dict[int, list[str]]:
    """
    Bind reusable section-3 shape templates to dimension labels.

    Edited fixtures show one owner with an attached dimension-anchor record
    and sibling owners with the same generic section-3 shape. Treat the
    template as the binding target; conflicting anchors leave it unbound.
    """
    bindings: dict[int, list[str]] = {}
    conflicts: set[int] = set()
    for block in blocks:
        shape_key = _shape_template_key_for_block(vdf, block)
        if shape_key is None:
            continue
        labels = _array_element_labels_from_sort_anchor(vdf, block, dimension_sets)
        if labels is None or len(labels) != block.length():
            continue
        previous = bindings.get(shape_key)
        if previous is not None and previous != labels:
            conflicts.add(shape_key)
            continue
        bindings[shape_key] = labels

    for shape_key in conflicts:
        bindings.pop(shape_key, None)
    return bindings


def _array_element_labels_from_sort_anchor(
    vdf: VdfFile,
    block: OwnerRecordBlock,
    dimension_sets: list[RecoveredDimensionSet],
) -> Optional[list[str]]:
    """
    Use an attached dimension-anchor record as a same-cardinality tie-breaker.

    In the model-edit fixtures, stock records can have sort_key=0 and borrow
    their visible sort anchor from the dimension record whose elements define
    the stock array. That is a structural relation: the anchor record lands
    inside the stock's OT block, has field[6]=0, has the dimension-anchor
    sentinel in field[14], and its name is one of the decoded dimension sets.
    """
    if not block.sort_anchor_record_indices:
        return None

    key_to_name_idx = build_record_name_key_to_name_index(vdf)
    dims_by_name = {
        dim.name.lower(): dim
        for dim in dimension_sets
        if len(dim.elements) == block.length()
    }
    if not dims_by_name:
        return None

    matches: list[RecoveredDimensionSet] = []
    for rec_idx in block.sort_anchor_record_indices:
        if rec_idx >= len(vdf.records):
            continue
        rec = vdf.records[rec_idx]
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


def decoded_record_spans(vdf: VdfFile) -> list[DecodedRecordSpan]:
    """
    Return direct record -> name -> OT span facts, without owner selection.

    This deliberately avoids hidden-slot alignment, descriptor pruning,
    non-overlap selection, name-category filtering, and array label guessing.
    A span here means only that a record carries:
    - f[2] resolving through the decoded section-2 name key formula;
    - an in-range f[11] under the owner OT-start interpretation;
    - a non-zero f[6] shape code whose span is structurally decoded.

    Whether that record is an emitted user-facing series owner remains a
    separate question when spans overlap or when f[11] is a lookup-record
    index rather than an owner OT start.
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


def record_span_overlaps(spans: list[DecodedRecordSpan]) -> dict[int, list[DecodedRecordSpan]]:
    by_ot: dict[int, list[DecodedRecordSpan]] = {}
    for span in spans:
        for ot_idx in range(span.start, span.end):
            by_ot.setdefault(ot_idx, []).append(span)
    return {ot_idx: hits for ot_idx, hits in by_ot.items() if len(hits) > 1}


def record_span_overlap_components(spans: list[DecodedRecordSpan]) -> list[RecordSpanOverlapComponent]:
    """
    Return connected components of direct spans that overlap in OT space.

    Components are built only from actual overlapping OT slots, not from
    adjacent ranges. This keeps the diagnostic tied to the unresolved
    owner/descriptor conflict rather than to normal neighboring variables.
    """
    overlaps = record_span_overlaps(spans)
    if not overlaps:
        return []

    span_idx_by_rec = {span.rec_idx: idx for idx, span in enumerate(spans)}
    adjacency: dict[int, set[int]] = {idx: set() for idx in range(len(spans))}
    for hits in overlaps.values():
        indices = [
            span_idx_by_rec[span.rec_idx]
            for span in hits
            if span.rec_idx in span_idx_by_rec
        ]
        for idx in indices:
            adjacency[idx].update(other for other in indices if other != idx)

    raw_components: list[list[DecodedRecordSpan]] = []
    seen: set[int] = set()
    for idx in range(len(spans)):
        if idx in seen or not adjacency[idx]:
            continue
        stack = [idx]
        component_indices: set[int] = set()
        seen.add(idx)
        while stack:
            current = stack.pop()
            component_indices.add(current)
            for other in adjacency[current]:
                if other in seen:
                    continue
                seen.add(other)
                stack.append(other)

        raw_components.append(sorted(
            (spans[i] for i in component_indices),
            key=lambda span: (span.start, span.end, span.rec_idx),
        ))

    raw_components.sort(key=lambda component: (
        min(span.start for span in component),
        max(span.end for span in component),
        min(span.rec_idx for span in component),
    ))

    components: list[RecordSpanOverlapComponent] = []
    for component_id, component_spans in enumerate(raw_components):
        components.append(RecordSpanOverlapComponent(
            component_id=component_id,
            start=min(span.start for span in component_spans),
            end=max(span.end for span in component_spans),
            spans=component_spans,
        ))
    return components


def decoded_field11_union_facts(vdf: VdfFile) -> list[Field11UnionFact]:
    """
    Return direct `field[11]` owner-vs-lookup interpretation candidates.

    A `field[11]` word can be a valid OT start, a valid section-6 lookup
    record index, or both. Lookup-record indices are zero-based, while OT[0]
    is Time and is not a record owner start. This function keeps those checks
    independent so callers can inspect the unresolved union without descriptor
    pruning, non-overlap owner selection, or name filtering.
    """
    key_to_name_idx = build_record_name_key_to_name_index(vdf)
    lookup_records = vdf.section6_lookup_records() or []
    codes = vdf.section6_ot_class_codes() or []
    facts: list[Field11UnionFact] = []

    for rec_idx, rec in enumerate(vdf.records):
        name_idx = key_to_name_idx.get(rec.fields[2])
        if name_idx is None:
            continue

        raw = rec.fields[11]
        shape_length = decoded_record_shape_length(vdf, rec)

        owner_start: Optional[int] = None
        owner_end: Optional[int] = None
        owner_ot_codes: list[int] = []
        if (
            shape_length is not None
            and shape_length > 0
            and 0 < raw < vdf.offset_table_count
            and raw + shape_length <= vdf.offset_table_count
        ):
            owner_start = raw
            owner_end = raw + shape_length
            owner_ot_codes = codes[owner_start:owner_end]

        lookup_index: Optional[int] = None
        lookup_ot_index: Optional[int] = None
        lookup_width: Optional[int] = None
        lookup_dependency_ref_word: Optional[int] = None
        if raw < len(lookup_records):
            lookup = lookup_records[raw]
            lookup_index = raw
            lookup_ot_index = lookup.ot_index()
            lookup_width = lookup.output_width()
            lookup_dependency_ref_word = lookup.dependency_ref_word()

        if owner_start is None and lookup_index is None:
            continue

        facts.append(Field11UnionFact(
            rec_idx=rec_idx,
            name_idx=name_idx,
            name=vdf.names[name_idx],
            raw_field11=raw,
            shape_code=rec.shape_code(),
            shape_length=shape_length,
            sort_key=rec.fields[10],
            slot_ref=rec.slot_ref(),
            has_sentinel=rec.has_sentinel(),
            owner_start=owner_start,
            owner_end=owner_end,
            owner_ot_codes=owner_ot_codes,
            lookup_index=lookup_index,
            lookup_ot_index=lookup_ot_index,
            lookup_width=lookup_width,
            lookup_dependency_ref_word=lookup_dependency_ref_word,
        ))

    return facts


def decoded_field11_union_correlations(vdf: VdfFile) -> list[Field11UnionCorrelation]:
    """
    Correlate both-valid `field[11]` facts with lookup-record output OTs.

    For each record whose `field[11]` is structurally valid both as an owner OT
    start and as a zero-based lookup-record index, this reports the spans that
    cover `lookup[field[11]].word[10]`. Output-sort proximity is useful
    evidence on several fixtures, but Ref.vdf has counterexamples; callers
    should treat this as an xray relation, not as the final discriminator.
    """
    spans = decoded_record_spans(vdf)
    components = record_span_overlap_components(spans)
    component_by_rec: dict[int, RecordSpanOverlapComponent] = {}
    for component in components:
        for span in component.spans:
            component_by_rec[span.rec_idx] = component

    rows: list[Field11UnionCorrelation] = []
    for fact in decoded_field11_union_facts(vdf):
        if (
            fact.owner_start is None
            or fact.lookup_index is None
            or fact.lookup_ot_index is None
        ):
            continue

        output_spans = [
            span
            for span in spans
            if span.start <= fact.lookup_ot_index < span.end
        ]
        closest_output_span: Optional[DecodedRecordSpan] = None
        output_sort_delta: Optional[int] = None
        if output_spans:
            closest_output_span = min(
                output_spans,
                key=lambda span: (
                    0 if span.start == fact.lookup_ot_index else 1,
                    0 if fact.lookup_width is not None and span.length() == fact.lookup_width else 1,
                    abs(span.sort_key - fact.sort_key),
                    span.rec_idx,
                ),
            )
            output_sort_delta = abs(closest_output_span.sort_key - fact.sort_key)

        component = component_by_rec.get(fact.rec_idx)
        rows.append(Field11UnionCorrelation(
            fact=fact,
            output_spans=output_spans,
            closest_output_span=closest_output_span,
            output_sort_delta=output_sort_delta,
            overlap_component_id=component.component_id if component is not None else None,
            overlap_component_start=component.start if component is not None else None,
            overlap_component_end=component.end if component is not None else None,
            overlap_component_spans=component.spans if component is not None else [],
        ))

    return rows


def _mapping_from_record_name_keys(
    vdf: VdfFile,
    visible_blocks: list[OwnerRecordBlock],
) -> dict[str, OwnerRecordBlock]:
    key_to_name_idx = build_record_name_key_to_name_index(vdf)
    excluded_names: set[str] = set()
    for dim in _recover_dimension_sets(vdf):
        excluded_names.add(dim.name.lower())
        excluded_names.update(element.lower() for element in dim.elements)

    rec_to_name: dict[int, str] = {}
    rec_sort_names: dict[int, list[tuple[int, VdfRecord, str]]] = {}
    for ri, rec in enumerate(vdf.records):
        name_idx = key_to_name_idx.get(rec.fields[2])
        if name_idx is None:
            continue
        name = vdf.names[name_idx]
        rec_to_name[ri] = name
        rec_sort_names.setdefault(rec.fields[10], []).append((ri, rec, name))

    ordered_blocks = sorted(
        visible_blocks,
        key=lambda block: (
            block.attached_sort_keys[0] if block.attached_sort_keys else math.inf,
            block.start,
            block.end,
        ),
    )

    used_names: set[str] = set()
    name_to_block: dict[str, OwnerRecordBlock] = {}

    for block in ordered_blocks:
        chosen: Optional[str] = None
        for allow_stock_lookupish in (False, True):
            for rec_idx in block.sentinel_record_indices:
                name = rec_to_name.get(rec_idx)
                if name is None or name in used_names:
                    continue
                if _heuristic_name_allowed_for_block(
                    name,
                    block,
                    excluded_names=excluded_names,
                    allow_stock_lookupish=allow_stock_lookupish,
                ):
                    chosen = name
                    break
            if chosen is not None:
                break

        for allow_stock_lookupish in (False, True):
            for sort_key in block.attached_sort_keys:
                if chosen is not None:
                    break
                candidates = [
                    name
                    for _, _, name in rec_sort_names.get(sort_key, [])
                    if (
                        name not in used_names
                        and _heuristic_name_allowed_for_block(
                            name,
                            block,
                            excluded_names=excluded_names,
                            allow_stock_lookupish=allow_stock_lookupish,
                        )
                    )
                ]
                if candidates:
                    chosen = candidates[0]
                    break
            if chosen is not None:
                break

        if chosen is None:
            continue
        used_names.add(chosen)
        name_to_block[chosen] = block

    return name_to_block


def _try_f2_name_key_mapping(vdf: VdfFile) -> Optional[dict[str, OwnerRecordBlock]]:
    """
    Record-to-name mapping via the decoded f[2] name key, composed with
    non-overlap owner-block selection.

    The f[2] key is a fact: it decodes to the section-2 name string's 4-byte
    word offset plus seven, giving a direct record -> name-table entry link
    and avoiding fixture-specific shifts. But this function wraps the fact
    with `_select_non_overlapping_owner_blocks` (a DP-over-intervals
    reconstruction step; see audit B.2.1). The output is therefore a
    reconstruction composed from one decoded key and one reconstruction
    filter, not a fully decoded mapping. Use the underlying
    `_mapping_from_record_name_keys` when the caller already has an
    owner-block set and does not want the non-overlap filter rerun.
    """
    n_recs = len(vdf.records)
    if n_recs == 0 or not vdf.names:
        return None

    all_blocks = build_owner_record_blocks(vdf)
    visible_blocks = _select_non_overlapping_owner_blocks(
        [b for b in all_blocks if not b.hidden]
    )

    return _mapping_from_record_name_keys(vdf, visible_blocks)

def map_names_to_owner_blocks(vdf: VdfFile) -> Optional[NameMapping]:
    """
    Map visible variable names to owner blocks through decoded record keys.

    Uses the direct f[2] string-table key plus sort-key anchored block
    ownership. Descriptor/owner overlap handling is still a reconstruction
    step, not a fully decoded Vensim emission rule.
    """
    all_blocks = build_owner_record_blocks(vdf)
    visible_blocks = _select_non_overlapping_owner_blocks(
        [b for b in all_blocks if not b.hidden]
    )

    if not visible_blocks:
        hidden_ots: set[int] = set()
        hidden_unmapped: list[OwnerRecordBlock] = []
        for block in all_blocks:
            if block.hidden:
                hidden_ots.update(range(block.start, block.end))
                hidden_unmapped.append(block)
        return NameMapping(
            variable_names=[],
            owner_blocks=[],
            name_to_block={},
            system_ot_indices=hidden_ots,
            unmapped_blocks=hidden_unmapped,
        )

    f2_result = _try_f2_name_key_mapping(vdf)
    if f2_result is None:
        return None

    mapped_names = sorted(f2_result.keys(), key=_vensim_sort_key)
    mapped_blocks = [f2_result[name] for name in mapped_names]
    mapped_ranges = {(block.start, block.end) for block in mapped_blocks}
    # Unmapped visible blocks plus hidden system blocks both contribute to
    # system_ot_indices -- those OT slots are system-owned regardless of
    # whether the block was hidden at the owner-building stage.
    sys_blocks = [block for block in visible_blocks if (block.start, block.end) not in mapped_ranges]
    sys_ots: set[int] = set()
    for block in sys_blocks:
        sys_ots.update(range(block.start, block.end))
    for block in all_blocks:
        if block.hidden:
            sys_ots.update(range(block.start, block.end))
    return NameMapping(
        variable_names=mapped_names,
        owner_blocks=mapped_blocks,
        name_to_block=f2_result,
        system_ot_indices=sys_ots,
        unmapped_blocks=sys_blocks,
    )


@dataclass
class NamedResult:
    """A single named time series from a VDF file."""
    name: str
    ot_index: int
    values: list[float]


@dataclass
class PrecisionReport:
    """
    Conservative extraction-precision report for the current xray decoder.

    `exact-by-xray` means the Python decoder found no known blockers in the
    VDF structures it currently understands. It is not a proof of the whole
    file format; it is a precise statement about the current extraction path.
    Any non-empty `reasons` list means the file remains `not-proven`.
    """
    status: str
    reasons: list[str]
    magic: str
    header_text: str
    header_year: Optional[int]
    header_0x50: int
    header_0x68: int
    header_0x6c: int
    header_0x70: int
    header_0x74: int
    time_point_count: int
    block_time_point_count: int
    names_total: int
    slots_total: int
    records_total: int
    offset_table_count: int
    result_count: int
    duplicate_result_name_count: int
    duplicate_result_ot_count: int
    mapped_variable_count: int
    array_result_count: int
    numeric_array_label_count: int
    record_span_count: int
    record_span_overlap_slots: int
    unmapped_block_count: int
    dimension_anchor_count: int
    incomplete_dimension_anchor_count: int
    data_block_count: int
    data_block_decode_failures: int
    data_block_tail_mismatches: int
    bitmap_widths: list[int]
    # Flags mirroring the two silent-reconstruction reasons. Equivalent to
    # scanning `reasons` for the matching string, but faster to inspect in
    # aggregate corpus reports.
    used_system_variable_fallback: bool = False
    used_lookup_name_order_pairing: bool = False

    def is_exact_by_xray(self) -> bool:
        return self.status == "exact-by-xray"


@dataclass
class CorpusPrecisionRow:
    path: str
    status: str
    reasons: list[str]
    magic: str
    year: Optional[int]
    header_0x6c: Optional[int]
    header_0x74: Optional[int]
    names_total: Optional[int]
    records_total: Optional[int]
    offset_table_count: Optional[int]
    result_count: Optional[int]
    array_result_count: Optional[int]


@dataclass
class NamedResultsDiagnostics:
    """
    Side-channel diagnostics from `extract_named_results_with_diagnostics`.

    The `used_*` flags record when extraction fell back onto a known
    reconstruction step. `precision_report` forwards each flag into the
    blocker list so `exact-by-xray` status never hides reconstruction.
    """
    used_system_variable_fallback: bool = False
    used_lookup_name_order_pairing: bool = False


def extract_named_results_with_diagnostics(
    vdf: VdfFile,
) -> tuple[Optional[list[NamedResult]], NamedResultsDiagnostics]:
    """
    Like `extract_named_results`, but also returns a diagnostics record
    flagging any reconstruction paths taken during extraction.

    `used_system_variable_fallback` is set when system variables (INITIAL
    TIME, FINAL TIME, SAVEPER, TIME STEP) are placed via
    `_assign_group_positions(_nonstock_assignment_items(...))` or the bare
    alphabetical zip-onto-remaining fallback. Both are reconstruction rules
    (Vensim-sort-order placement), not decoded fields.

    `used_lookup_name_order_pairing` is set when the zip-by-order pairing
    between lookupish name-table entries and section-6 lookup records is
    taken. That pairing is explicitly reconstruction and breaks on Ref.vdf,
    so callers using it deserve to see the blocker string.
    """
    diagnostics = NamedResultsDiagnostics()

    mapping = map_names_to_owner_blocks(vdf)
    if mapping is None:
        return None, diagnostics

    # Extract time values
    time_values = vdf.extract_time_values()
    if time_values is None:
        return None, diagnostics
    dimension_sets = _recover_dimension_sets(vdf)
    shape_label_bindings = _shape_template_label_bindings(
        vdf,
        list(mapping.name_to_block.values()),
        dimension_sets,
    )
    codes = vdf.section6_ot_class_codes()
    final_values = vdf.section6_final_values()

    results: list[NamedResult] = []
    emitted_names: set[str] = set()
    emitted_ot_indices: set[int] = set()

    # Time itself
    results.append(NamedResult(name="Time", ot_index=0, values=time_values))
    emitted_names.add("Time")
    emitted_ot_indices.add(0)

    # Mapped variable results
    for name in mapping.variable_names:
        block = mapping.name_to_block.get(name)
        if block is None:
            continue

        if block.length() == 1:
            # Scalar variable
            ot_idx = block.start
            series = vdf.extract_ot_series(ot_idx, time_values, codes, final_values)
            if series is None:
                continue
            results.append(NamedResult(name=name, ot_index=ot_idx, values=series))
            emitted_names.add(name)
            emitted_ot_indices.add(ot_idx)
        else:
            # Arrayed variable: one result per OT element
            element_labels = _array_element_labels_for_block(
                vdf,
                block,
                dimension_sets,
                shape_label_bindings,
            )
            for elem_offset in range(block.length()):
                ot_idx = block.start + elem_offset
                series = vdf.extract_ot_series(ot_idx, time_values, codes, final_values)
                if series is None:
                    continue
                if element_labels is not None and elem_offset < len(element_labels):
                    elem_name = f"{name}[{element_labels[elem_offset]}]"
                else:
                    elem_name = f"{name}[{elem_offset}]"
                results.append(NamedResult(
                    name=elem_name, ot_index=ot_idx, values=series))
                emitted_names.add(elem_name)
                emitted_ot_indices.add(ot_idx)

    # Standalone lookup/table outputs have direct OT bindings in section 6.
    # The "zip by order" pairing below is reconstruction: it assumes that
    # lookupish name-table entries align 1:1 with section-6 lookup records.
    # Flag it on the diagnostics so precision_report can surface it.
    lookup_names = _lookup_record_names(vdf)
    lookup_records = vdf.section6_lookup_records() or []
    if lookup_names and lookup_records and len(lookup_names) == len(lookup_records):
        paired_any = False
        for name, record in zip(lookup_names, lookup_records):
            ot_idx = record.ot_index()
            if name in emitted_names or ot_idx in emitted_ot_indices:
                continue
            raw = vdf.offset_table_entry(ot_idx)
            if raw is None:
                continue
            series = vdf.extract_ot_series(ot_idx, time_values, codes, final_values)
            if series is None:
                continue
            results.append(NamedResult(name=name, ot_index=ot_idx, values=series))
            emitted_names.add(name)
            emitted_ot_indices.add(ot_idx)
            paired_any = True
        if paired_any:
            diagnostics.used_lookup_name_order_pairing = True

    # System variables have direct scalar records of their own (except Time,
    # which is OT[0]). Prefer those decoded record bindings; the gap-aware
    # nonstock layout remains only as a fallback for malformed/partial files.
    codes = vdf.section6_ot_class_codes() or []
    system_positions = system_ot_indices_from_records(vdf)
    missing_system_names = [
        name
        for name in sorted((n for n in SYSTEM_NAMES if n != "Time"), key=_vensim_sort_key)
        if name not in system_positions
    ]
    if missing_system_names:
        nonstock_positions = _group_ot_positions(codes, want_stock=False)
        fallback_positions = _assign_group_positions(
            _nonstock_assignment_items(mapping.name_to_block),
            nonstock_positions,
        )
        if fallback_positions is None:
            claimed = {block.start + i for block in mapping.name_to_block.values()
                       for i in range(block.length())}
            remaining = [pos for pos in nonstock_positions if pos not in claimed]
            fallback_positions = {
                name: pos
                for name, pos in zip(
                    sorted((n for n in SYSTEM_NAMES if n != "Time"), key=_vensim_sort_key),
                    remaining,
                )
            }
        # Only mark the fallback as "used" if at least one of the missing
        # system variables actually picked up a position through it. A file
        # whose system records supplied everything directly does not trip.
        for name in missing_system_names:
            if name in fallback_positions:
                system_positions[name] = fallback_positions[name]
                diagnostics.used_system_variable_fallback = True

    if not system_positions:
        claimed = {block.start + i for block in mapping.name_to_block.values()
                   for i in range(block.length())}
        nonstock_positions = _group_ot_positions(codes, want_stock=False)
        remaining = [pos for pos in nonstock_positions if pos not in claimed]
        fallback_count = 0
        for name, pos in zip(
            sorted((n for n in SYSTEM_NAMES if n != "Time"), key=_vensim_sort_key),
            remaining,
        ):
            system_positions[name] = pos
            fallback_count += 1
        if fallback_count:
            diagnostics.used_system_variable_fallback = True

    for name in sorted((n for n in SYSTEM_NAMES if n != "Time"), key=_vensim_sort_key):
        ot_idx = system_positions.get(name)
        if ot_idx is None:
            continue
        raw = vdf.offset_table_entry(ot_idx)
        if raw is None:
            continue
        series = vdf.extract_ot_series(ot_idx, time_values, codes, final_values)
        if series is None:
            continue
        results.append(NamedResult(name=name, ot_index=ot_idx, values=series))
        emitted_names.add(name)
        emitted_ot_indices.add(ot_idx)

    return results, diagnostics


def extract_named_results(vdf: VdfFile) -> Optional[list[NamedResult]]:
    """
    Extract named time series using the current record-derived reconstruction.

    Returns a list of NamedResult for each mapped variable (scalar variables
    get one entry, arrayed variables get one entry per element). System
    variables (FINAL TIME, INITIAL TIME, SAVEPER, TIME STEP) are also included.

    This is a thin wrapper over `extract_named_results_with_diagnostics`;
    callers that need to see reconstruction flags should use that function
    directly.
    """
    results, _ = extract_named_results_with_diagnostics(vdf)
    return results


def _header_text(data: bytes) -> str:
    raw = data[4:0x78].split(b"\0", 1)[0]
    return raw.decode("ascii", errors="replace")


def _header_year(text: str) -> Optional[int]:
    match = re.search(r"\b(19|20)\d{2}\b", text)
    return int(match.group(0)) if match else None


def _float_almost_equal(lhs: float, rhs: float) -> bool:
    if math.isnan(lhs) or math.isnan(rhs):
        return math.isnan(lhs) and math.isnan(rhs)
    if math.isinf(lhs) or math.isinf(rhs):
        return lhs == rhs
    return abs(lhs - rhs) <= max(1e-5, 1e-6 * max(abs(lhs), abs(rhs), 1.0))


def _data_block_precision_stats(
    vdf: VdfFile,
    time_values: Optional[list[float]],
    codes: Optional[list[int]],
    final_values: Optional[list[float]],
) -> tuple[int, int, int, list[int]]:
    data_blocks = 0
    decode_failures = 0
    tail_mismatches = 0
    bitmap_widths: set[int] = set()

    for ot_idx in range(vdf.offset_table_count):
        raw = vdf.offset_table_entry(ot_idx)
        if raw is None or not vdf.is_data_block_offset(raw):
            continue

        data_blocks += 1
        if raw == vdf.first_data_block:
            bitmap_widths.add(vdf.bitmap_size)
        elif raw + 2 <= len(vdf.data):
            count = u16(vdf.data, raw)
            bitmap_width, _ = vdf._block_bitmap_layout(raw, count)
            bitmap_widths.add(bitmap_width)

        if time_values is None or final_values is None or ot_idx >= len(final_values):
            decode_failures += 1
            continue

        series = vdf.extract_ot_series(ot_idx, time_values, codes, final_values)
        if series is None or not series:
            decode_failures += 1
            continue
        if not _float_almost_equal(series[-1], final_values[ot_idx]):
            tail_mismatches += 1

    return data_blocks, decode_failures, tail_mismatches, sorted(bitmap_widths)


def precision_report(vdf: VdfFile) -> PrecisionReport:
    """
    Report whether current Python extraction has any known precision blockers.

    The report is deliberately conservative. It marks files as `not-proven`
    when owner spans overlap, owner blocks remain unmapped, array labels fall
    back to numeric positions, incomplete dimension anchors are present, or
    decoded data blocks fail the final-value tail check.
    """
    reasons: list[str] = []

    if vdf.data[:4] == VDF_ALT_RESULT_MAGIC:
        reasons.append("alt-result-extra-payload")
    if len(vdf.sections) != 8:
        reasons.append("section-count")

    mapping = map_names_to_owner_blocks(vdf)
    if mapping is None:
        reasons.append("name-mapping-unavailable")

    time_values = vdf.extract_time_values()
    if time_values is None:
        reasons.append("time-series-unavailable")

    codes = vdf.section6_ot_class_codes()
    if codes is None or len(codes) != vdf.offset_table_count:
        reasons.append("ot-class-codes-unavailable")

    final_values = vdf.section6_final_values()
    if final_values is None or len(final_values) != vdf.offset_table_count:
        reasons.append("final-values-unavailable")

    results, diagnostics = extract_named_results_with_diagnostics(vdf)
    if results is None:
        reasons.append("named-results-unavailable")
        results = []

    result_name_counts: dict[str, int] = {}
    result_ot_counts: dict[int, int] = {}
    for result in results:
        result_name_counts[result.name] = result_name_counts.get(result.name, 0) + 1
        result_ot_counts[result.ot_index] = result_ot_counts.get(result.ot_index, 0) + 1
    duplicate_result_name_count = sum(count - 1 for count in result_name_counts.values() if count > 1)
    duplicate_result_ot_count = sum(count - 1 for count in result_ot_counts.values() if count > 1)
    if duplicate_result_name_count:
        reasons.append("duplicate-result-names")
    if duplicate_result_ot_count:
        reasons.append("duplicate-result-ots")

    spans = decoded_record_spans(vdf)
    overlaps = record_span_overlaps(spans)
    if overlaps:
        reasons.append("record-span-overlap")

    unmapped_block_count = len(mapping.unmapped_blocks) if mapping is not None else 0
    if unmapped_block_count:
        reasons.append("unmapped-owner-blocks")

    numeric_array_label_count = sum(
        1 for result in results
        if NUMERIC_ARRAY_LABEL_RE.search(result.name) is not None
    )
    if numeric_array_label_count:
        reasons.append("numeric-array-labels")

    # Dimension anchors are considered "incomplete" only when the subseq
    # recovery also fails to provide an element list. Ref.vdf has 11
    # subrange anchors with no element records; those are fully decodable
    # via the sec5 payload subsequence rule and should not count as
    # blockers once recovery has run.
    anchors = decoded_record_dimension_anchors(vdf)
    recovered_dim_names = {dim.name for dim in _recover_dimension_sets(vdf)}
    incomplete_anchor_count = sum(
        1 for anchor in anchors
        if anchor.status != "complete" and anchor.name not in recovered_dim_names
    )
    if incomplete_anchor_count:
        reasons.append("incomplete-dimension-anchors")

    # Flag silent reconstruction paths that would otherwise let a file pass
    # as "exact-by-xray" while the underlying mapping relies on the
    # lookup-name zip-by-order pairing or the system-variable alphabetical
    # placement fallback. See /tmp/vdf_audit_phase1.md Section B.3.1.
    if diagnostics.used_system_variable_fallback:
        reasons.append("used-system-variable-fallback")
    if diagnostics.used_lookup_name_order_pairing:
        reasons.append("used-lookup-name-order-pairing")

    data_blocks, decode_failures, tail_mismatches, bitmap_widths = _data_block_precision_stats(
        vdf,
        time_values,
        codes,
        final_values,
    )
    if decode_failures:
        reasons.append("data-block-decode-failures")
    if tail_mismatches:
        reasons.append("data-block-tail-mismatch")

    # Preserve order while dropping duplicate reason strings from cascading
    # failures in partial parses.
    reasons = list(dict.fromkeys(reasons))
    status = "exact-by-xray" if not reasons else "not-proven"

    array_result_count = sum(
        1 for result in results
        if "[" in result.name and result.name.endswith("]")
    )

    header_text = _header_text(vdf.data)
    return PrecisionReport(
        status=status,
        reasons=reasons,
        magic=vdf.data[:4].hex(),
        header_text=header_text,
        header_year=_header_year(header_text),
        header_0x50=u32(vdf.data, 0x50) if len(vdf.data) >= 0x54 else 0,
        header_0x68=u32(vdf.data, 0x68) if len(vdf.data) >= 0x6C else 0,
        header_0x6c=u32(vdf.data, 0x6C) if len(vdf.data) >= 0x70 else 0,
        header_0x70=u32(vdf.data, 0x70) if len(vdf.data) >= 0x74 else 0,
        header_0x74=u32(vdf.data, 0x74) if len(vdf.data) >= 0x78 else 0,
        time_point_count=vdf.time_point_count,
        block_time_point_count=vdf.block_time_point_count,
        names_total=len(vdf.names),
        slots_total=len(vdf.slot_table),
        records_total=len(vdf.records),
        offset_table_count=vdf.offset_table_count,
        result_count=len(results),
        duplicate_result_name_count=duplicate_result_name_count,
        duplicate_result_ot_count=duplicate_result_ot_count,
        mapped_variable_count=len(mapping.variable_names) if mapping is not None else 0,
        array_result_count=array_result_count,
        numeric_array_label_count=numeric_array_label_count,
        record_span_count=len(spans),
        record_span_overlap_slots=len(overlaps),
        unmapped_block_count=unmapped_block_count,
        dimension_anchor_count=len(anchors),
        incomplete_dimension_anchor_count=incomplete_anchor_count,
        data_block_count=data_blocks,
        data_block_decode_failures=decode_failures,
        data_block_tail_mismatches=tail_mismatches,
        bitmap_widths=bitmap_widths,
        used_system_variable_fallback=diagnostics.used_system_variable_fallback,
        used_lookup_name_order_pairing=diagnostics.used_lookup_name_order_pairing,
    )


def _tracked_vdf_paths(root: Path) -> list[Path]:
    root = root.resolve()
    try:
        result = subprocess.run(
            ["git", "-C", str(root), "ls-files"],
            check=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        return [
            root / line
            for line in result.stdout.splitlines()
            if Path(line).suffix.lower() == ".vdf"
        ]
    except (OSError, subprocess.CalledProcessError):
        return sorted(path for path in root.rglob("*") if path.suffix.lower() == ".vdf")


def _corpus_row_for_path(path: Path, root: Path) -> CorpusPrecisionRow:
    rel = str(path.relative_to(root))
    data = path.read_bytes()
    magic = data[:4].hex()
    text = _header_text(data) if len(data) >= 0x78 else ""
    year = _header_year(text)
    h6c = u32(data, 0x6C) if len(data) >= 0x70 else None
    h74 = u32(data, 0x74) if len(data) >= 0x78 else None

    if data[:4] == VDF_DATASET_MAGIC:
        return CorpusPrecisionRow(
            path=rel,
            status="dataset/not-implemented",
            reasons=["dataset-vdf"],
            magic=magic,
            year=year,
            header_0x6c=h6c,
            header_0x74=h74,
            names_total=None,
            records_total=None,
            offset_table_count=None,
            result_count=None,
            array_result_count=None,
        )

    try:
        report = precision_report(parse_vdf(data))
    except Exception as exc:
        return CorpusPrecisionRow(
            path=rel,
            status="parse-error",
            reasons=[type(exc).__name__],
            magic=magic,
            year=year,
            header_0x6c=h6c,
            header_0x74=h74,
            names_total=None,
            records_total=None,
            offset_table_count=None,
            result_count=None,
            array_result_count=None,
        )

    return CorpusPrecisionRow(
        path=rel,
        status=report.status,
        reasons=report.reasons,
        magic=report.magic,
        year=report.header_year,
        header_0x6c=report.header_0x6c,
        header_0x74=report.header_0x74,
        names_total=report.names_total,
        records_total=report.records_total,
        offset_table_count=report.offset_table_count,
        result_count=report.result_count,
        array_result_count=report.array_result_count,
    )


def corpus_precision_rows(root: Path) -> list[CorpusPrecisionRow]:
    root = root.resolve()
    return [_corpus_row_for_path(path, root) for path in _tracked_vdf_paths(root)]


def mdl_definition_matches_block(model: MdlModel, definition: MdlDefinition,
                                 block: RecordShapeBlock) -> bool:
    expected_size = mdl_definition_flat_size(model, definition)
    if expected_size is not None and block.length() != expected_size:
        return False
    if definition.is_stock():
        return bool(block.ot_codes) and all(code == OT_CODE_STOCK for code in block.ot_codes)
    return all(code != OT_CODE_STOCK for code in block.ot_codes)


def mdl_definition_matches_owner_block(model: MdlModel, definition: MdlDefinition,
                                       block: OwnerRecordBlock) -> bool:
    expected_size = mdl_definition_flat_size(model, definition)
    if expected_size is not None and block.length() != expected_size:
        return False
    if definition.is_stock():
        return bool(block.ot_codes) and all(code == OT_CODE_STOCK for code in block.ot_codes)
    return all(code != OT_CODE_STOCK for code in block.ot_codes)


def match_mdl_definitions_to_blocks(vdf: VdfFile, model: MdlModel) -> list[MdlBlockMatch]:
    blocks = build_record_shape_blocks(vdf)
    matches: list[MdlBlockMatch] = []
    for definition in model.definitions:
        if definition.kind not in {"stock", "var", "lookup"}:
            continue
        candidate_block_indices = [
            idx for idx, block in enumerate(blocks)
            if mdl_definition_matches_block(model, definition, block)
        ]
        matches.append(MdlBlockMatch(
            definition=definition,
            candidate_block_indices=candidate_block_indices,
        ))
    return matches


def match_mdl_definitions_to_owner_blocks(vdf: VdfFile, model: MdlModel) -> list[MdlBlockMatch]:
    blocks = build_owner_record_blocks(vdf)
    matches: list[MdlBlockMatch] = []
    for definition in model.definitions:
        if definition.kind not in {"stock", "var", "lookup"}:
            continue
        candidate_block_indices = [
            idx for idx, block in enumerate(blocks)
            if not block.hidden and mdl_definition_matches_owner_block(model, definition, block)
        ]
        matches.append(MdlBlockMatch(
            definition=definition,
            candidate_block_indices=candidate_block_indices,
        ))
    return matches


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


def find_slot_table(data: bytes, name_sec: Section, max_name_count: int,
                    sec1_data_size: int) -> tuple[int, list[int]]:
    """RECONSTRUCTION HEURISTIC: locate the slot table by scanning backward
    from the name-table section magic, trying every `(gap, name_count)`
    pair and picking the largest structurally valid candidate.

    A 90s C reader would have computed the slot-table offset directly from
    a header or section field; we have not yet identified that field, so
    this scan is the xray's current workaround.
    """
    if max_name_count == 0:
        return 0, []
    end = name_sec.file_offset

    best_key: Optional[tuple[int, int, int, int, int, int]] = None
    best: tuple[int, list[int]] = (0, [])

    for gap in range(20):
        for name_count in range(max_name_count, 0, -1):
            table_size = name_count * 4
            if end < gap + table_size:
                continue
            table_start = end - gap - table_size

            values = [u32(data, table_start + i * 4) for i in range(name_count)]
            sorted_vals = sorted(set(values))
            if len(sorted_vals) != name_count:
                continue
            if not all(v % 4 == 0 and v > 0 and v + 16 <= sec1_data_size for v in sorted_vals):
                continue
            strides = [sorted_vals[i + 1] - sorted_vals[i] for i in range(len(sorted_vals) - 1)]
            if strides and min(strides) < 4:
                continue

            layout = analyze_slot_table_offsets(values)
            if layout is None:
                continue

            # Prefer the largest structurally valid table first. Within that
            # count, prefer the candidates that preserve the observed 16-byte
            # slot lattice rather than whichever suffix happens to appear with
            # the smallest gap before section 2.
            key = (
                name_count,
                1 if layout.contiguous_16 else 0,
                -layout.irregular_stride_count,
                -layout.missing_16_slots,
                -table_start,
                -gap,
            )
            if best_key is None or key > best_key:
                best_key = key
                best = (table_start, values)

    return best


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

    header_fv_off = u32(data, 0x58)
    header_lm_off = u32(data, 0x5C)
    header_ot_off = u32(data, 0x60)

    sections = find_sections(data)
    name_section_idx = find_name_table_section_idx(data, sections)

    names = []
    if name_section_idx is not None:
        ns = sections[name_section_idx]
        names = parse_name_table(data, ns, ns.region_end)

    sec1_data_size = sections[1].region_data_size() if len(sections) > 1 else 0

    slot_table_offset, slot_table = (0, [])
    if name_section_idx is not None:
        slot_table_offset, slot_table = find_slot_table(
            data, sections[name_section_idx], len(names), sec1_data_size)

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
    print(f"  0x74 block grid mirror?:     {u32(vdf.data, 0x74)}")
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
    alignment = preferred_slot_name_alignment(vdf)
    default_alignment = score_slot_name_alignment(vdf, 0)
    print(f"=== Slot Table ({len(vdf.slot_table)} entries @ 0x{vdf.slot_table_offset:08x}) ===")
    print(f"  layout: {format_slot_table_layout(analyze_slot_table_offsets(vdf.slot_table))}")
    if alignment.leading_extra_slots > 0:
        hidden = ", ".join(str(slot) for slot in alignment.hidden_slots)
        print(f"  visible-name alignment: skip {alignment.leading_extra_slots} leading slot entries "
              f"(score {alignment.score} vs default {default_alignment.score})")
        print("  note: this is an exploratory display alignment; structural refs use direct slot_table[i] -> names[i]")
        print(f"  hidden slot refs: [{hidden}]")
    print(f"  {'Idx':>3}  {'Sec1Off':>7}  {'Name':<36}  {'w[0]':>8} {'w[1]':>8} {'w[2]':>8} {'w[3]':>8}")
    for i, offset in enumerate(vdf.slot_table):
        name_idx = i - alignment.leading_extra_slots
        name = vdf.names[name_idx] if 0 <= name_idx < len(vdf.names) else "<hidden>"
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


def print_decoded_record_facts(vdf: VdfFile, *, max_spans: int = 80, max_overlaps: int = 16) -> None:
    print("=== Owner-Interpretation Record Spans (No Owner Selection) ===")
    spans = decoded_record_spans(vdf)
    overlaps = record_span_overlaps(spans)
    covered_ot = {
        ot_idx
        for span in spans
        for ot_idx in range(span.start, span.end)
    }
    sentinel_spans = sum(1 for span in spans if span.has_sentinel)
    print("  source: direct record f[2] name key + f[11] interpreted as owner OT start + decoded nonzero f[6] shape span")
    print("  excluded: hidden-slot alignment, descriptor pruning, lookup-index interpretation, non-overlap owner selection, array-label guessing")
    print(f"  spans={len(spans)} sentinel_spans={sentinel_spans} "
          f"covered_ot_slots={len(covered_ot)} overlap_ot_slots={len(overlaps)}")

    dims = _recover_dimension_sets(vdf)
    if dims:
        print("  decoded dimension sets:")
        for dim in dims[:12]:
            elements = ", ".join(dim.elements)
            print(f"    {dim.name} ({dim.source}) = [{elements}]")
        if len(dims) > 12:
            print(f"    ... ({len(dims) - 12} more)")

    anchors = decoded_record_dimension_anchors(vdf)
    incomplete = [anchor for anchor in anchors if anchor.status != "complete"]
    if incomplete:
        print("  incomplete record-field8 dimension anchors:")
        for anchor in incomplete[:16]:
            elements = ", ".join(
                f"{idx}:{name}" for idx, _, name in anchor.elements
            )
            if not elements:
                elements = "<none>"
            print(
                f"    rec[{anchor.record_index}] \"{anchor.name}\" "
                f"group={anchor.group_id} dim_id={anchor.dimension_id} "
                f"status={anchor.status} elements=[{elements}]"
            )
        if len(incomplete) > 16:
            print(f"    ... ({len(incomplete) - 16} more incomplete anchors)")

    if spans:
        print("  spans:")
        for span in spans[:max_spans]:
            sentinel = " yes" if span.has_sentinel else " no"
            print(f"    rec[{span.rec_idx:>3}] name[{span.name_idx:>4}] \"{span.name}\" "
                  f"OT[{span.start}..{span.end}) len={span.length()} "
                  f"shape={span.shape_code} sort={span.sort_key} slot={span.slot_ref} "
                  f"group={span.group_id} sentinel={sentinel} codes={_format_ot_code_span(span.ot_codes)}")
        if len(spans) > max_spans:
            print(f"    ... ({len(spans) - max_spans} more spans)")

    if overlaps:
        print("  overlap examples:")
        for ot_idx in sorted(overlaps)[:max_overlaps]:
            hit_labels = [
                f"rec[{span.rec_idx}] \"{span.name}\" OT[{span.start}..{span.end})"
                for span in overlaps[ot_idx][:4]
            ]
            if len(overlaps[ot_idx]) > 4:
                hit_labels.append(f"... {len(overlaps[ot_idx]) - 4} more")
            print(f"    OT[{ot_idx}] <- {'; '.join(hit_labels)}")
        if len(overlaps) > max_overlaps:
            print(f"    ... ({len(overlaps) - max_overlaps} more overlap slots)")
    print()


def print_field11_union_facts(vdf: VdfFile, *, max_facts: int = 96) -> None:
    print("=== Record field[11] Union Facts (No Discriminator) ===")
    facts = decoded_field11_union_facts(vdf)
    if not facts:
        print("  (none)\n")
        return

    both = [
        fact for fact in facts
        if fact.owner_start is not None and fact.lookup_index is not None
    ]
    owner_only = [
        fact for fact in facts
        if fact.owner_start is not None and fact.lookup_index is None
    ]
    lookup_only = [
        fact for fact in facts
        if fact.owner_start is None and fact.lookup_index is not None
    ]
    print("  source: direct record f[2] name key; f[11] independently checked as OT start and lookup-record index")
    print("  excluded: descriptor pruning, non-overlap owner selection, name filtering, lookup-name pairing")
    print(f"  facts={len(facts)} owner_only={len(owner_only)} lookup_only={len(lookup_only)} both_valid={len(both)}")

    shown = 0
    for label, group in [
        ("both owner+lookup candidates", both),
        ("lookup-index only candidates", lookup_only),
    ]:
        if not group:
            continue
        print(f"  {label}:")
        for fact in group[:max(0, max_facts - shown)]:
            shown += 1
            owner = "owner=-"
            if fact.owner_start is not None and fact.owner_end is not None:
                owner = (
                    f"owner=OT[{fact.owner_start}..{fact.owner_end}) "
                    f"codes={_format_ot_code_span(fact.owner_ot_codes)}"
                )
            lookup = "lookup=-"
            if fact.lookup_index is not None:
                dep = fact.lookup_dependency_ref_word
                dep_label = f" dep_ref={dep}" if dep else ""
                width_match = "yes" if fact.lookup_width_matches_shape else "no"
                lookup = (
                    f"lookup[{fact.lookup_index}]->OT[{fact.lookup_ot_index}] "
                    f"width={fact.lookup_width} width_matches_shape={width_match}{dep_label}"
                )
            sentinel = "yes" if fact.has_sentinel else "no"
            print(
                f"    rec[{fact.rec_idx:>3}] name[{fact.name_idx:>4}] \"{fact.name}\" "
                f"f11={fact.raw_field11} shape={fact.shape_code} "
                f"len={fact.shape_length if fact.shape_length is not None else '?'} "
                f"sort={fact.sort_key} slot={fact.slot_ref} sentinel={sentinel} "
                f"{owner}; {lookup}"
            )
            if shown >= max_facts:
                break
        if shown >= max_facts:
            break

    remaining = len(both) + len(lookup_only) - shown
    if remaining > 0:
        print(f"    ... ({remaining} more lookup-valid facts)")
    print()


def print_field11_union_correlations(vdf: VdfFile, *, max_rows: int = 96) -> None:
    print("=== Record field[11] Lookup-Output Correlation (No Discriminator) ===")
    rows = decoded_field11_union_correlations(vdf)
    if not rows:
        print("  (none)\n")
        return

    with_output = sum(1 for row in rows if row.closest_output_span is not None)
    with_component = sum(1 for row in rows if row.overlap_component_id is not None)
    print("  source: both-valid field[11] facts; lookup[field[11]].word[10] treated as evaluated-output OT")
    print("  excluded: owner/descriptor selection, name filtering, lookupish-name assumptions")
    print(
        f"  rows={len(rows)} with_output_span={with_output} "
        f"with_overlap_component={with_component}"
    )

    groups: dict[tuple[int, int], list[Field11UnionCorrelation]] = {}
    for row in rows:
        if row.overlap_component_id is None or row.fact.lookup_index is None:
            continue
        groups.setdefault((row.overlap_component_id, row.fact.lookup_index), []).append(row)
    comparable = [
        group
        for group in groups.values()
        if len(group) > 1 and all(row.output_sort_delta is not None for row in group)
    ]
    unique_closest = 0
    ties = 0
    for group in comparable:
        best = min(row.output_sort_delta for row in group if row.output_sort_delta is not None)
        if sum(1 for row in group if row.output_sort_delta == best) == 1:
            unique_closest += 1
        else:
            ties += 1
    if comparable:
        print(
            f"  same-component/same-lookup groups={len(comparable)} "
            f"unique_closest_by_sort={unique_closest} ties={ties}"
        )

    sorted_rows = sorted(rows, key=lambda row: (
        row.overlap_component_id if row.overlap_component_id is not None else 1_000_000,
        row.fact.raw_field11,
        row.fact.rec_idx,
    ))
    for row in sorted_rows[:max_rows]:
        fact = row.fact
        component = "-"
        if row.overlap_component_id is not None:
            component = (
                f"{row.overlap_component_id}="
                f"OT[{row.overlap_component_start}..{row.overlap_component_end})"
            )
        output = "-"
        if row.closest_output_span is not None:
            output_span = row.closest_output_span
            output = (
                f"rec[{output_span.rec_idx}] \"{output_span.name}\" "
                f"OT[{output_span.start}..{output_span.end}) "
                f"sort={output_span.sort_key} delta={row.output_sort_delta}"
            )

        closest = "?"
        group = groups.get((row.overlap_component_id, fact.lookup_index))
        if group is not None and len(group) > 1 and row.output_sort_delta is not None:
            deltas = [
                other.output_sort_delta
                for other in group
                if other.output_sort_delta is not None
            ]
            if deltas:
                best = min(deltas)
                closest = "yes" if row.output_sort_delta == best else "no"

        competitors = [
            f"rec[{span.rec_idx}] \"{span.name}\""
            for span in row.overlap_component_spans
            if span.rec_idx != fact.rec_idx
        ]
        competitor_text = ", ".join(competitors[:4]) if competitors else "-"
        if len(competitors) > 4:
            competitor_text += f", ... {len(competitors) - 4} more"

        print(
            f"    rec[{fact.rec_idx:>3}] \"{fact.name}\" "
            f"owner=OT[{fact.owner_start}..{fact.owner_end}) "
            f"lookup[{fact.lookup_index}]->OT[{fact.lookup_ot_index}] "
            f"width={fact.lookup_width} sort={fact.sort_key} "
            f"component={component} output={output} "
            f"closest_in_component_lookup={closest} competitors={competitor_text}"
        )

    if len(rows) > max_rows:
        print(f"    ... ({len(rows) - max_rows} more correlation rows)")
    print()


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
    sec4_entries = vdf.parse_section4_entries()
    sec4_idx_words = set()
    if sec4_entries:
        sec4_idx_words = {e.index_word for e in sec4_entries}

    for i, entry in enumerate(directory.entries):
        slot_refs = [resolve_slot_ref(sr, slot_to_names) for sr in entry.axis_slot_refs()]
        sec4_hit = entry.index_word() in sec4_idx_words
        state = "placeholder" if entry.flat_size() == 0 else "active"
        print(f"  {i:>3} @0x{entry.file_offset:08x} idx={entry.index_word()} sec4_hit={sec4_hit} "
              f"state={state} flat={entry.flat_size()} axes={entry.axis_sizes()} "
              f"w10={entry.words[10]} w11={entry.words[11]} "
              f"shape_words={entry.words[:4]} "
              f"slot_refs={slot_refs} raw_slot_refs={entry.axis_slot_refs()} "
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
    owner_hints: dict[tuple[int, int], list[str]] = {}
    mapping = map_names_to_owner_blocks(vdf)
    if mapping is not None:
        for name, block in mapping.name_to_block.items():
            owner_hints.setdefault((block.start, block.length()), []).append(name)

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
        else:
            bitmap_size, grid_count = vdf._block_bitmap_layout(offset, count)
        if offset + 2 + bitmap_size > len(vdf.data):
            print(f"  {idx:>3}  0x{offset:08x}  (truncated)")
            continue
        block_size = 2 + bitmap_size + count * 4
        density = (count / grid_count * 100) if grid_count > 0 else 0

        data_start = offset + 2 + bitmap_size
        first_val = f32(vdf.data, data_start) if count > 0 and data_start + 4 <= len(vdf.data) else float("nan")
        last_val = f32(vdf.data, data_start + (count - 1) * 4) if count > 1 and data_start + count * 4 <= len(vdf.data) else first_val

        label = "  [TIME]" if offset == vdf.first_data_block else ""
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


def print_record_shape_blocks(vdf: VdfFile) -> None:
    print("=== Record Shape Blocks ===")
    blocks = build_record_shape_blocks(vdf)
    if not blocks:
        print("  (none)\n")
        return

    for i, block in enumerate(blocks):
        code_str = "[" + ", ".join(f"0x{code:02x}" for code in block.ot_codes) + "]"
        slot_labels = [describe_slot_ref(vdf, slot_ref) for slot_ref in block.slot_refs]
        print(f"  {i:>3}  OT[{block.start}..{block.end}) len={block.length()} "
              f"shape_codes={block.shape_codes} recs={block.record_indices} "
              f"shape_recs={block.shape_record_indices} sorts={block.sort_keys} "
              f"codes={code_str} homogeneous={block.homogeneous_ot_codes()} "
              f"slots={slot_labels}")
    print()


def print_owner_record_blocks(vdf: VdfFile) -> None:
    print("=== Owner Record Blocks ===")
    blocks = build_owner_record_blocks(vdf)
    if not blocks:
        print("  (none)\n")
        return

    for i, block in enumerate(blocks):
        code_str = "[" + ", ".join(f"0x{code:02x}" for code in block.ot_codes) + "]"
        slot_labels = [describe_slot_ref(vdf, slot_ref) for slot_ref in block.slot_refs]
        hidden_label = "hidden" if block.hidden else "visible"
        print(f"  {i:>3}  OT[{block.start}..{block.end}) len={block.length()} "
              f"{hidden_label} sentinel_recs={block.sentinel_record_indices} "
              f"shape_codes={block.shape_codes} direct_sorts={block.direct_sort_keys} "
              f"attached_sorts={block.attached_sort_keys} "
              f"sort_anchors={block.sort_anchor_record_indices} "
              f"codes={code_str} homogeneous={block.homogeneous_ot_codes()} "
              f"slots={slot_labels}")
    print()


def format_record_shape_block(vdf: VdfFile, block: Optional[RecordShapeBlock], *,
                              block_idx: Optional[int] = None) -> str:
    if block is None:
        return "missing"
    prefix = f"block[{block_idx}] " if block_idx is not None else ""
    code_str = "[" + ", ".join(f"0x{code:02x}" for code in block.ot_codes) + "]"
    return (f"{prefix}OT[{block.start}..{block.end}) len={block.length()} "
            f"shape_codes={block.shape_codes} sorts={block.sort_keys} "
            f"codes={code_str} slots={block.slot_refs}")


def format_owner_record_block(vdf: VdfFile, block: Optional[OwnerRecordBlock], *,
                              block_idx: Optional[int] = None) -> str:
    if block is None:
        return "missing"
    prefix = f"owner[{block_idx}] " if block_idx is not None else ""
    code_str = "[" + ", ".join(f"0x{code:02x}" for code in block.ot_codes) + "]"
    hidden_label = "hidden" if block.hidden else "visible"
    return (f"{prefix}OT[{block.start}..{block.end}) len={block.length()} {hidden_label} "
            f"class={owner_block_runtime_class(block)} "
            f"sentinel_recs={block.sentinel_record_indices} "
            f"direct_sorts={block.direct_sort_keys} attached_sorts={block.attached_sort_keys} "
            f"anchors={block.sort_anchor_record_indices} codes={code_str} slots={block.slot_refs}")


def print_mdl_alignment(vdf: VdfFile, model: MdlModel, mdl_path: str) -> None:
    print("=== MDL Alignment ===")
    print(f"  mdl: {mdl_path}")
    print(f"  dimensions={len(model.dimensions)} definitions={len(model.definitions)}")
    if model.dimensions:
        for dim in model.dimensions.values():
            print(f"    dim {dim.name}({len(dim.elements)}): {dim.elements}")

    blocks = build_record_shape_blocks(vdf)
    matches = match_mdl_definitions_to_blocks(vdf, model)
    matched_block_indices: set[int] = set()

    for match in matches:
        definition = match.definition
        flat_size = mdl_definition_flat_size(model, definition)
        if len(match.candidate_block_indices) == 1:
            status = "unique"
        elif len(match.candidate_block_indices) == 0:
            status = "missing"
        else:
            status = "ambiguous"
        print(f"  src[{definition.source_index:>2}] {definition.kind:<6} {definition.name}"
              f"{'[' + ','.join(definition.dimensions) + ']' if definition.dimensions else ''} "
              f"flat={flat_size if flat_size is not None else '?'} "
              f"candidates={len(match.candidate_block_indices)} {status}")
        for block_idx in match.candidate_block_indices:
            matched_block_indices.add(block_idx)
            print(f"        {format_record_shape_block(vdf, blocks[block_idx], block_idx=block_idx)}")

    unmatched = [idx for idx in range(len(blocks)) if idx not in matched_block_indices]
    if unmatched:
        print("  unmatched blocks:")
        for idx in unmatched:
            print(f"        {format_record_shape_block(vdf, blocks[idx], block_idx=idx)}")
    print()


def print_owner_mdl_alignment(vdf: VdfFile, model: MdlModel, mdl_path: str) -> None:
    print("=== Owner MDL Alignment ===")
    print(f"  mdl: {mdl_path}")

    blocks = build_owner_record_blocks(vdf)
    matches = match_mdl_definitions_to_owner_blocks(vdf, model)
    matched_block_indices: set[int] = set()

    for match in matches:
        definition = match.definition
        flat_size = mdl_definition_flat_size(model, definition)
        if len(match.candidate_block_indices) == 1:
            status = "unique"
        elif len(match.candidate_block_indices) == 0:
            status = "missing"
        else:
            status = "ambiguous"
        print(f"  src[{definition.source_index:>2}] {definition.kind:<6} {definition.name}"
              f"{'[' + ','.join(definition.dimensions) + ']' if definition.dimensions else ''} "
              f"flat={flat_size if flat_size is not None else '?'} "
              f"candidates={len(match.candidate_block_indices)} {status}")
        for block_idx in match.candidate_block_indices:
            matched_block_indices.add(block_idx)
            print(f"        {format_owner_record_block(vdf, blocks[block_idx], block_idx=block_idx)}")

    unmatched = [idx for idx, block in enumerate(blocks) if idx not in matched_block_indices and not block.hidden]
    if unmatched:
        print("  unmatched visible owners:")
        for idx in unmatched:
            print(f"        {format_owner_record_block(vdf, blocks[idx], block_idx=idx)}")

    hidden = [idx for idx, block in enumerate(blocks) if block.hidden]
    if hidden:
        print("  hidden owner blocks:")
        for idx in hidden:
            print(f"        {format_owner_record_block(vdf, blocks[idx], block_idx=idx)}")
    print()


def print_name_mapping(vdf: VdfFile) -> None:
    """Show the current record-key owner-to-OT reconstruction."""
    print("=== Record-Key Owner Mapping (Current Reconstruction) ===")

    candidates = visible_variable_candidates(vdf)
    all_blocks = build_owner_record_blocks(vdf)
    visible_blocks = [b for b in all_blocks if not b.hidden]
    print(f"  candidates={len(candidates)} visible_owner_blocks={len(visible_blocks)}")
    print(f"  candidate names: {candidates}")

    mapping = map_names_to_owner_blocks(vdf)
    if mapping is None:
        print("  mapping FAILED (more candidates than blocks)")
        print()
        return

    print(f"  mapped={len(mapping.name_to_block)} unmapped_blocks={len(mapping.unmapped_blocks)}")

    for name in mapping.variable_names:
        block = mapping.name_to_block.get(name)
        if block is None:
            print(f"  {name}: unmapped")
            continue
        cls = owner_block_runtime_class(block)
        sort_str = str(block.attached_sort_keys) if block.attached_sort_keys else "[]"
        print(f"  {name}: OT[{block.start}..{block.end}) len={block.length()} "
              f"class={cls} sorts={sort_str}")

    if mapping.unmapped_blocks:
        print("  unmapped (system) blocks:")
        for block in mapping.unmapped_blocks:
            print(f"    OT[{block.start}..{block.end}) class={owner_block_runtime_class(block)}")

    if mapping.system_ot_indices:
        print(f"  system OT indices: {sorted(mapping.system_ot_indices)}")

    # Verify against final values
    finals = vdf.section6_final_values()
    if finals:
        print("  verification (final values):")
        for name in mapping.variable_names:
            block = mapping.name_to_block.get(name)
            if block is None:
                continue
            for offset in range(block.length()):
                ot_idx = block.start + offset
                if ot_idx < len(finals):
                    elem_label = f"{name}[{offset}]" if block.length() > 1 else name
                    print(f"    {elem_label}: OT[{ot_idx}] final={finals[ot_idx]}")
    print()


def print_extracted_results(vdf: VdfFile) -> None:
    """Extract and show named results using the current reconstruction."""
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


def print_precision_report(vdf: VdfFile) -> None:
    """Show the conservative precision status for the current extraction path."""
    report = precision_report(vdf)

    print("=== Precision Report ===")
    print(f"  Status: {report.status}")
    if report.reasons:
        print(f"  Blockers: {', '.join(report.reasons)}")
    else:
        print("  Blockers: none known in current xray decoder")
    print(f"  Header: year={report.header_year or '-'} magic={report.magic} "
          f"0x50=0x{report.header_0x50:08x} 0x6c=0x{report.header_0x6c:08x} "
          f"0x74=0x{report.header_0x74:08x}")
    print(f"  Time grid: saved={report.time_point_count} block_grid={report.block_time_point_count} "
          f"bitmap_widths={report.bitmap_widths}")
    print(f"  Structures: names={report.names_total} slots={report.slots_total} "
          f"records={report.records_total} OT={report.offset_table_count}")
    print(f"  Extraction: results={report.result_count} mapped_vars={report.mapped_variable_count} "
          f"arrays={report.array_result_count} numeric_array_labels={report.numeric_array_label_count} "
          f"duplicate_names={report.duplicate_result_name_count} "
          f"duplicate_ots={report.duplicate_result_ot_count}")
    print(f"  Owner facts: spans={report.record_span_count} "
          f"overlap_slots={report.record_span_overlap_slots} "
          f"unmapped_blocks={report.unmapped_block_count}")
    print(f"  Dimensions: anchors={report.dimension_anchor_count} "
          f"incomplete={report.incomplete_dimension_anchor_count}")
    print(f"  Data blocks: checked={report.data_block_count} "
          f"decode_failures={report.data_block_decode_failures} "
          f"tail_mismatches={report.data_block_tail_mismatches}")
    print()


def _markdown_cell(value: object) -> str:
    text = "" if value is None else str(value)
    return text.replace("|", "\\|")


def _format_reason_list(reasons: list[str]) -> str:
    return ", ".join(reasons) if reasons else "-"


def print_corpus_precision_report(root: Path) -> None:
    """Print a Markdown table for tracked VDF extraction precision."""
    root = root.resolve()
    rows = corpus_precision_rows(root)

    print("=== Corpus Precision Report ===")
    print(f"Root: {root}")
    print(f"Tracked VDF files: {len(rows)}")
    status_counts: dict[str, int] = {}
    for row in rows:
        status_counts[row.status] = status_counts.get(row.status, 0) + 1
    print("Status counts: " + ", ".join(
        f"{status}={count}" for status, count in sorted(status_counts.items())
    ))
    print()
    print("| File | Year | Magic | 0x6c | 0x74 | Names | Records | OT | Results | Arrays | Status | Blockers |")
    print("|------|------|-------|------|------|-------|---------|----|---------|--------|--------|----------|")
    for row in sorted(rows, key=lambda r: r.path.lower()):
        h6c = f"0x{row.header_0x6c:08x}" if row.header_0x6c is not None else "-"
        h74 = f"0x{row.header_0x74:08x}" if row.header_0x74 is not None else "-"
        cells = [
            row.path,
            row.year if row.year is not None else "-",
            row.magic,
            h6c,
            h74,
            row.names_total if row.names_total is not None else "-",
            row.records_total if row.records_total is not None else "-",
            row.offset_table_count if row.offset_table_count is not None else "-",
            row.result_count if row.result_count is not None else "-",
            row.array_result_count if row.array_result_count is not None else "-",
            row.status,
            _format_reason_list(row.reasons),
        ]
        print("| " + " | ".join(_markdown_cell(cell) for cell in cells) + " |")
    print()


def print_owner_sketch_alignment(vdf: VdfFile, model: MdlModel, mdl_path: str) -> None:
    print("=== Owner Sketch Alignment ===")
    print(f"  mdl: {mdl_path}")

    sketch_defs = mdl_sketch_definitions(model)
    blocks = owner_blocks_in_sentinel_order(vdf)

    print(f"  sketch_names={len(model.sketch_names)} visible_defs={len(sketch_defs)} "
          f"visible_owner_blocks={len(blocks)}")
    if model.sketch_names:
        print(f"  sketch_order={model.sketch_names}")
    sketch_classes = [mdl_definition_runtime_class(definition) for definition in sketch_defs]
    owner_classes = [owner_block_runtime_class(block) for block in blocks]
    if sketch_classes != owner_classes:
        print("  note: sentinel/file owner order does not match mdl sketch order in this fixture")

    max_len = max(len(sketch_defs), len(blocks))
    for idx in range(max_len):
        definition = sketch_defs[idx] if idx < len(sketch_defs) else None
        block = blocks[idx] if idx < len(blocks) else None

        lhs = (
            f"sketch[{idx:>2}] {definition.name} "
            f"class={mdl_definition_runtime_class(definition)}"
            if definition is not None else
            f"sketch[{idx:>2}] missing"
        )
        rhs = (
            f"owner[{idx:>2}] OT[{block.start}..{block.end}) "
            f"class={owner_block_runtime_class(block)} "
            f"sentinel_recs={block.sentinel_record_indices} "
            f"attached_sorts={block.attached_sort_keys}"
            if block is not None else
            f"owner[{idx:>2}] missing"
        )
        print(f"  {lhs} -> {rhs}")

    hidden = owner_blocks_in_sentinel_order(vdf, include_hidden=True)
    hidden = [block for block in hidden if block.hidden]
    if hidden:
        print("  hidden owner blocks:")
        for block in hidden:
            print(f"        OT[{block.start}..{block.end}) class={owner_block_runtime_class(block)} "
                  f"sentinel_recs={block.sentinel_record_indices} attached_sorts={block.attached_sort_keys}")
    print()


def print_section35_bridge(vdf: VdfFile) -> None:
    """Show the section-3 -> section-5 relationship via shared axis_slot_refs."""
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

    for i, sec3 in enumerate(directory.entries):
        axis_refs = set(sec3.axis_slot_refs())
        if not axis_refs:
            print(f"  sec3[{i}] idx={sec3.index_word()} flat={sec3.flat_size()} "
                  f"axes={sec3.axis_sizes()} -- no axis_slot_refs")
            continue

        matches = classify_section5_bridge_matches(sec3, sec5_entries)

        axis_ref_strs = [resolve_slot_ref(r, slot_to_names) for r in sec3.axis_slot_refs()]
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

    sec1_slot_area = vdf.section1_slot_area_offset_from_field1()
    if sec1_slot_area is not None and vdf.slot_table_offset > 0:
        if sec1_slot_area == vdf.slot_table_offset:
            print("  [PASS] sec1 field1 points to the visible slot table")
        else:
            warnings.append(
                "sec1 field1 points into the slot/ref area but not exactly at the "
                f"visible slot table: field1=0x{sec1_slot_area:x}, "
                f"visible=0x{vdf.slot_table_offset:x}")

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

        # 5. All sec3 axis_slot_refs are in the slot table
        slot_set = set(vdf.slot_table)
        all_axis_refs: list[int] = []
        for entry in directory.entries:
            all_axis_refs.extend(entry.axis_slot_refs())
        if all_axis_refs:
            in_slot = [r for r in all_axis_refs if r in slot_set]
            not_in_slot = [r for r in all_axis_refs if r not in slot_set]
            if not_in_slot:
                errors.append(
                    f"sec3 axis_slot_refs NOT in slot table: {not_in_slot}")
            else:
                print(f"  [PASS] all {len(in_slot)} sec3 axis_slot_refs found in slot table")
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
                    f"sec3 axis_slot_refs and sec5 trailing refs have no overlap: "
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


def print_compare(left: VdfFile, left_path: str, right: VdfFile, right_path: str, *,
                  left_mdl: Optional[tuple[MdlModel, str]] = None,
                  right_mdl: Optional[tuple[MdlModel, str]] = None) -> None:
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
    left_alignment = preferred_slot_name_alignment(left)
    right_alignment = preferred_slot_name_alignment(right)
    if (left_alignment.leading_extra_slots != right_alignment.leading_extra_slots
            or left_alignment.score != right_alignment.score):
        print(f"  visible_slot_alignment: left=skip{left_alignment.leading_extra_slots}/score{left_alignment.score} "
              f"right=skip{right_alignment.leading_extra_slots}/score{right_alignment.score}")
    print()

    print("=== Shared Name / Slot Diffs ===")
    left_pairs = visible_slot_name_pairs(left, alignment=left_alignment)
    right_pairs = visible_slot_name_pairs(right, alignment=right_alignment)
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

    print("=== Record Shape Block Diffs ===")
    left_blocks = build_record_shape_blocks(left)
    right_blocks = build_record_shape_blocks(right)
    any_block_diff = False
    keys = sorted({(b.start, b.end) for b in left_blocks} | {(b.start, b.end) for b in right_blocks})
    left_by_key = {(b.start, b.end): b for b in left_blocks}
    right_by_key = {(b.start, b.end): b for b in right_blocks}
    for key in keys:
        lblock = left_by_key.get(key)
        rblock = right_by_key.get(key)
        if lblock is None or rblock is None:
            any_block_diff = True
            print(f"  OT[{key[0]}..{key[1]})")
            print(f"    left:  {format_record_shape_block(left, lblock) if lblock else 'missing'}")
            print(f"    right: {format_record_shape_block(right, rblock) if rblock else 'missing'}")
            continue
        ltuple = (lblock.shape_codes, lblock.sort_keys, lblock.slot_refs, lblock.ot_codes)
        rtuple = (rblock.shape_codes, rblock.sort_keys, rblock.slot_refs, rblock.ot_codes)
        if ltuple != rtuple:
            any_block_diff = True
            print(f"  OT[{key[0]}..{key[1]})")
            print(f"    left:  {format_record_shape_block(left, lblock)}")
            print(f"    right: {format_record_shape_block(right, rblock)}")
    if not any_block_diff:
        print("  (no record-shape-block differences)")
    print()

    print("=== Owner Block Diffs ===")
    left_owner_blocks = build_owner_record_blocks(left)
    right_owner_blocks = build_owner_record_blocks(right)
    any_owner_diff = False
    keys = sorted({(b.start, b.end) for b in left_owner_blocks} | {(b.start, b.end) for b in right_owner_blocks})
    left_by_key = {(b.start, b.end): b for b in left_owner_blocks}
    right_by_key = {(b.start, b.end): b for b in right_owner_blocks}
    for key in keys:
        lblock = left_by_key.get(key)
        rblock = right_by_key.get(key)
        if lblock is None or rblock is None:
            any_owner_diff = True
            print(f"  OT[{key[0]}..{key[1]})")
            print(f"    left:  {format_owner_record_block(left, lblock) if lblock else 'missing'}")
            print(f"    right: {format_owner_record_block(right, rblock) if rblock else 'missing'}")
            continue
        ltuple = (
            lblock.hidden,
            lblock.sentinel_record_indices,
            lblock.direct_sort_keys,
            lblock.attached_sort_keys,
            lblock.slot_refs,
        )
        rtuple = (
            rblock.hidden,
            rblock.sentinel_record_indices,
            rblock.direct_sort_keys,
            rblock.attached_sort_keys,
            rblock.slot_refs,
        )
        if ltuple != rtuple:
            any_owner_diff = True
            print(f"  OT[{key[0]}..{key[1]})")
            print(f"    left:  {format_owner_record_block(left, lblock)}")
            print(f"    right: {format_owner_record_block(right, rblock)}")
    if not any_owner_diff:
        print("  (no owner-block differences)")
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

    if left_mdl is not None:
        print_mdl_alignment(left, left_mdl[0], left_mdl[1])
        print_owner_mdl_alignment(left, left_mdl[0], left_mdl[1])
        print_owner_sketch_alignment(left, left_mdl[0], left_mdl[1])
    if right_mdl is not None:
        print_mdl_alignment(right, right_mdl[0], right_mdl[1])
        print_owner_mdl_alignment(right, right_mdl[0], right_mdl[1])
        print_owner_sketch_alignment(right, right_mdl[0], right_mdl[1])


# ---- Main ----

def main() -> None:
    parser = argparse.ArgumentParser(
        description="VDF X-Ray: inspect Vensim VDF binary files",
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument("path", nargs="?", help="Path to VDF file")
    parser.add_argument("--corpus-precision", nargs="?", const=".", metavar="ROOT",
                        help="Scan tracked .vdf files under ROOT and print a precision table")
    parser.add_argument("--compare", metavar="OTHER_VDF",
                        help="Compare this VDF against another simulation-result VDF")
    parser.add_argument("--mdl", metavar="MODEL.mdl",
                        help="Optional Vensim model source for mdl-aware alignment output")
    parser.add_argument("--compare-mdl", metavar="OTHER_MODEL.mdl",
                        help="Optional model source for the VDF passed to --compare")
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
    parser.add_argument("--record-blocks", action="store_true",
                        help="Show record groups merged by decoded shape span")
    parser.add_argument("--record-facts", action="store_true",
                        help="Show direct record->name and record->OT spans without reconstruction")
    parser.add_argument("--field11-union", action="store_true",
                        help="Show direct record field[11] owner-vs-lookup candidates")
    parser.add_argument("--field11-union-correlation", action="store_true",
                        help="Show ambiguous field[11] records correlated to lookup output OTs")
    parser.add_argument("--owner-blocks", action="store_true",
                        help="Show owner-oriented blocks built from sentinel model records")
    parser.add_argument("--sec35-bridge", action="store_true", help="Show section-3 -> section-5 bridge")
    parser.add_argument("--ranges", action="store_true", help="Show record-derived OT ranges")
    parser.add_argument("--validate", action="store_true", help="Check structural invariants")
    parser.add_argument("--map-names", action="store_true",
                        help="Show current record-key owner-to-OT reconstruction")
    parser.add_argument("--extract", action="store_true",
                        help="Extract named results using current reconstruction")
    parser.add_argument("--precision", action="store_true",
                        help="Show whether current extraction has known precision blockers")
    parser.add_argument("--raw-section", type=int, metavar="N", help="Full hexdump of section N")
    parser.add_argument("--json", action="store_true", help="Machine-readable JSON summary")

    args = parser.parse_args()

    if args.corpus_precision is not None:
        print_corpus_precision_report(Path(args.corpus_precision))
        return

    if args.path is None:
        parser.error("path is required unless --corpus-precision is used")

    path = Path(args.path)

    data = path.read_bytes()

    if data[:4] == VDF_DATASET_MAGIC:
        print(f"Dataset VDF detected ({path}). Dataset parsing not yet implemented in this tool.")
        sys.exit(1)

    vdf = parse_vdf(data)
    mdl_model: Optional[tuple[MdlModel, str]] = None
    if args.mdl:
        mdl_path = Path(args.mdl)
        mdl_model = (parse_mdl_model(mdl_path.read_text(errors="replace")), str(mdl_path))

    if args.compare:
        other_path = Path(args.compare)
        other_data = other_path.read_bytes()
        if other_data[:4] == VDF_DATASET_MAGIC:
            print(f"Dataset VDF detected ({other_path}). Compare mode only supports simulation-result VDFs.")
            sys.exit(1)
        other_vdf = parse_vdf(other_data)
        other_mdl_model: Optional[tuple[MdlModel, str]] = None
        if args.compare_mdl:
            other_mdl_path = Path(args.compare_mdl)
            other_mdl_model = (
                parse_mdl_model(other_mdl_path.read_text(errors="replace")),
                str(other_mdl_path),
            )
        print_compare(
            vdf,
            str(path),
            other_vdf,
            str(other_path),
            left_mdl=mdl_model,
            right_mdl=other_mdl_model,
        )
        return

    if args.json:
        print_json_summary(vdf)
        return

    # If no specific flags, show the default overview
    show_all = args.all
    show_specific = any([
        args.names, args.slots, args.records, args.sec3, args.sec4,
        args.sec5, args.sec6, args.sec6_post, args.slot_xref, args.ot, args.blocks, args.data,
        args.bridge, args.record_blocks, args.sec35_bridge, args.ranges, args.validate,
        args.record_facts, args.field11_union, args.field11_union_correlation,
        args.owner_blocks, args.map_names, args.extract, args.precision,
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
    if show_all or args.record_blocks:
        print_record_shape_blocks(vdf)
    if show_all or args.record_facts:
        print_decoded_record_facts(vdf)
    if show_all or args.field11_union:
        print_field11_union_facts(vdf)
    if show_all or args.field11_union_correlation:
        print_field11_union_correlations(vdf)
    if show_all or args.owner_blocks:
        print_owner_record_blocks(vdf)
    if mdl_model is not None:
        print_mdl_alignment(vdf, mdl_model[0], mdl_model[1])
        print_owner_mdl_alignment(vdf, mdl_model[0], mdl_model[1])
        print_owner_sketch_alignment(vdf, mdl_model[0], mdl_model[1])
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
    if show_all or args.map_names:
        print_name_mapping(vdf)
    if args.extract:
        print_extracted_results(vdf)
    if show_all or args.precision:
        print_precision_report(vdf)

    if not show_specific or show_all:
        print_summary(vdf)


if __name__ == "__main__":
    main()
