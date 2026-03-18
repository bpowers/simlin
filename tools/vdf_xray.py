#!/usr/bin/env python3
"""
VDF X-Ray: inspect and debug Vensim VDF (binary data file) format.

Distilled from the Rust parser (src/simlin-engine/src/vdf.rs) and the
CLI dump tool (src/simlin-cli/src/vdf_dump.rs). See docs/design/vdf.md
for the full format specification.

Usage:
    python tools/vdf_xray.py <path.vdf> [--section N] [--names] [--records]
                                         [--ot] [--blocks] [--data] [--all]
                                         [--raw-section N] [--json]
"""

from __future__ import annotations

import argparse
import json
import math
import re
import struct
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Optional

# ---- Constants ----

FILE_HEADER_SIZE = 0x80
SECTION_HEADER_SIZE = 24
RECORD_SIZE = 64
SECTION3_ENTRY_WORDS = 27

VDF_FILE_MAGIC = bytes([0x7F, 0xF7, 0x17, 0x52])
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
        return self.fields[11]

    def is_arrayed(self) -> bool:
        return self.fields[6] != 5

    def has_sentinel(self) -> bool:
        return self.fields[8] == VDF_SENTINEL and self.fields[9] == VDF_SENTINEL

    def shape_code(self) -> int:
        """field[6]: 5=scalar, anything else=arrayed (section-3 index_word or high-range)."""
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
        best = (0, [], 0)

        for skip in range(9):
            entries, stop = self._section5_with_skip(skip, sec1_data_size, slot_set)
            if len(entries) > len(best[1]) or (len(entries) == len(best[1]) and stop > best[2]):
                best = (skip, entries, stop)
        return best[1]

    def _section5_with_skip(self, skip: int, sec1_data_size: int,
                            slot_set: set[int]) -> tuple[list[Section5SetEntry], int]:
        sec = self.sections[5]
        start = sec.data_offset() + skip * 4
        end = min(sec.region_end, len(self.data))
        if start >= end:
            return [], start

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
        return entries, pos

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
        best = (0, [], 0)
        for skip in range(9):
            entries, stop = self._section6_ref_stream_with_skip(skip)
            if len(entries) > len(best[1]) or (len(entries) == len(best[1]) and stop > best[2]):
                best = (skip, entries, stop)
        return best

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

    def extract_block_series(self, block_offset: int, time_values: list[float]) -> list[float]:
        step_count = len(time_values)
        count = u16(self.data, block_offset)
        bm_start = block_offset + 2
        data_start = bm_start + self.bitmap_size

        series = [float("nan")] * step_count
        data_idx = 0
        last_val = float("nan")
        for time_idx in range(step_count):
            byte_idx = time_idx // 8
            bit_idx = time_idx % 8
            bit_set = (self.data[bm_start + byte_idx] >> bit_idx) & 1 == 1
            if bit_set and data_idx < count:
                last_val = f32(self.data, data_start + data_idx * 4)
                data_idx += 1
            series[time_idx] = last_val
        return series


# ---- Slot-to-name helpers ----

def build_slot_to_names(vdf: VdfFile) -> dict[int, list[str]]:
    return build_slot_to_names_with_offset(vdf, 0)


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
    """
    Score a visible-name alignment against section-4/5/6 reference usage.

    This is intentionally an analysis-layer heuristic. It does not change the
    raw slot table or claim to decode the hidden slot/name relationship; it
    only helps the xray output avoid obviously shifted name labels when helper
    entries appear to occupy leading slot-table positions.
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
    """Use a shifted visible-name mapping only when it beats the default clearly."""
    default = score_slot_name_alignment(vdf, 0)
    best = best_slot_name_alignment(vdf)
    if best.leading_extra_slots > 0 and best.score >= default.score + 4:
        return best
    return default


def build_display_slot_to_names(vdf: VdfFile) -> dict[int, list[str]]:
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


def record_shape_length(vdf: VdfFile, rec: VdfRecord) -> Optional[int]:
    """
    Recover the OT span implied by record field[6], when the binding is decoded.

    This is deterministic structure rather than a display heuristic:
    - `5` always means scalar (len=1)
    - an active sec3 `index_word` gives its flat size, including `index_word=0`
      when that entry is active
    - `32` is the generic array marker and resolves only when section 3 exposes
      a single active flat size
    """
    code = rec.shape_code()
    if code == 5:
        return 1

    idx_to_entry = build_sec3_index_to_entry(vdf)
    entry = idx_to_entry.get(code)
    if entry is not None and entry.flat_size() > 0:
        return entry.flat_size()

    if code == 32:
        active_sizes = sorted({e.flat_size() for e in idx_to_entry.values() if e.flat_size() > 0})
        if len(active_sizes) == 1:
            return active_sizes[0]
    return None


def build_record_shape_blocks(vdf: VdfFile) -> list[RecordShapeBlock]:
    """
    Group records by decoded shape span instead of raw ot_index.

    A visible owner signal can split across records: one record may contribute
    the shape-derived span while another positive-sort record lands inside that
    span. This helper keeps the grouping structural and leaves name ownership
    unresolved when the file does not force it.
    """
    codes = vdf.section6_ot_class_codes() or []
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
    Return indices of records that are structurally model-variable owners.

    The sentinel pair (f[8]=f[9]=0xf6800000) is a strong signal, but not
    exclusive to model variables: system records (FINAL TIME, SAVEPER, etc.)
    can also carry sentinels, especially in small/empty models. After reformat,
    model records can carry f[0]=0 or f[1]=23, so those fields cannot be used
    as filters. System-record discrimination is handled at the mapping layer.
    """
    out: list[int] = []
    for rec_idx, rec in enumerate(vdf.records):
        if not rec.has_sentinel():
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

    In the model-edit fixtures, the structurally "real" visible owners are
    carried by the sentinel model records. Non-sentinel records still matter
    as sort/order anchors, but they over-generate overlapping shape spans.
    This helper keeps the owner candidate set narrow while still attaching
    visible sort anchors back onto those owners.
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

    # Some compiled helpers are structurally real one-element stock owners that
    # sit immediately before the visible stock array block. They can survive
    # cleanup/reformat passes even when slot-based hidden detection collapses.
    for block in blocks:
        if block.hidden:
            continue
        if block.length() != 1 or not block.direct_sort_keys:
            continue
        if not block.ot_codes or any(code != OT_CODE_STOCK for code in block.ot_codes):
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

    # All sentinel-based owner blocks are kept: the sentinel pair (f[8]=f[9]=
    # 0xf6800000) is sufficient evidence that a block is a real variable owner.
    # Variables CAN have sort_key=0 (observed in run_3/run_4 of model_editing
    # fixtures for the first or most recently added variable).
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


def owner_block_runtime_class(block: OwnerRecordBlock) -> str:
    if block.ot_codes and all(code == OT_CODE_STOCK for code in block.ot_codes):
        return "stock"
    if block.ot_codes and all(code == OT_CODE_CONST for code in block.ot_codes):
        return "const"
    return "dynamic"


# ---- VDF-native name mapping ----

# Names that are module IO / stdlib helpers and never own OT entries.
VENSIM_MODULE_NAMES = {"IN", "INI", "OUTPUT"}
VENSIM_STDLIB_HELPERS = {"DEL", "LV1", "LV2", "LV3", "ST", "RT1", "RT2", "DL"}


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
        cls = classify_name(name)
        if cls:
            continue
        if name in VENSIM_MODULE_NAMES or name in VENSIM_STDLIB_HELPERS:
            continue
        candidates.append(name)
    return candidates


def _identify_dimension_element_names(
    candidates: list[str],
    sec5_entries: list[Section5SetEntry],
    vdf: VdfFile,
) -> set[str]:
    """
    Identify dimension definition names and their element names from the
    candidate list, using section 5 cardinalities, name-table adjacency,
    and OT-position validation.

    Uses backtracking: dimension names in the name table are followed by
    their element names (with metadata gaps). A partition is only accepted
    if the remaining variable names produce a valid stocks-first-alphabetical
    OT mapping.
    """
    name_to_idx: dict[str, int] = {}
    for i, name in enumerate(vdf.names):
        if name not in name_to_idx:
            name_to_idx[name] = i

    candidate_set = set(candidates)
    cardinalities = sorted([entry.n for entry in sec5_entries], reverse=True)

    all_blocks = build_owner_record_blocks(vdf)
    visible_blocks = [b for b in all_blocks if not b.hidden]
    target_var_count = len(visible_blocks)

    def find_group(card: int, used: set[str]) -> list[tuple[str, list[str], int]]:
        """
        Find all valid (dim_name, elements, gap_score) groups for a cardinality.

        Returns groups sorted by gap_score (ascending) so the tightest
        name-table clusters are tried first. A lower gap_score means the
        element names are closer to the dimension name in the name table,
        which is the typical Vensim pattern.
        """
        groups: list[tuple[str, list[str], int]] = []
        for cand in candidates:
            if cand in used:
                continue
            cand_idx = name_to_idx.get(cand, -1)
            if cand_idx < 0:
                continue
            elements: list[str] = []
            last_elem_idx = cand_idx
            for j in range(cand_idx + 1, len(vdf.names)):
                if len(elements) >= card:
                    break
                next_name = vdf.names[j]
                if next_name in used or next_name == cand:
                    continue
                cls = classify_name(next_name)
                if cls:
                    continue
                if next_name in VENSIM_MODULE_NAMES or next_name in VENSIM_STDLIB_HELPERS:
                    continue
                if next_name in candidate_set:
                    elements.append(next_name)
                    last_elem_idx = j
            if len(elements) == card and not any(e in used for e in elements):
                gap = last_elem_idx - cand_idx
                groups.append((cand, elements, gap))
        # Prefer tightest clusters. Break ties by preferring dimension names
        # that appear later in the name table (dimensions often appear after
        # variables in the name table).
        groups.sort(key=lambda g: (g[2], -name_to_idx.get(g[0], 0)))
        return groups

    def try_map_remaining(used: set[str]) -> bool:
        """Check if the remaining candidates produce a valid OT mapping."""
        var_names = sorted(
            [c for c in candidates if c not in used], key=_vensim_sort_key)
        if len(var_names) != target_var_count:
            return False
        return _try_name_block_mapping(var_names, visible_blocks, vdf) is not None

    def solve(remaining_cards: list[int], used: set[str]) -> Optional[set[str]]:
        if not remaining_cards:
            if try_map_remaining(used):
                return used.copy()
            return None

        card = remaining_cards[0]
        rest = remaining_cards[1:]
        for dim_name, elements, _ in find_group(card, used):
            new_used = used | {dim_name} | set(elements)
            result = solve(rest, new_used)
            if result is not None:
                return result
        return None

    result = solve(cardinalities, set())
    return result if result is not None else set()


def _try_name_block_mapping(
    sorted_names: list[str],
    visible_blocks: list[OwnerRecordBlock],
    vdf: VdfFile,
) -> Optional[dict[str, OwnerRecordBlock]]:
    """
    Try to find a valid name-to-block mapping for the given sorted names
    and visible blocks. Returns the mapping if valid, None otherwise.
    """
    import itertools

    V = len(sorted_names)
    if V != len(visible_blocks):
        return None

    def valid_assignment(mapping: dict[str, OwnerRecordBlock]) -> bool:
        return _validate_name_block_assignment(mapping, vdf)

    keyed_blocks = [(b.attached_sort_keys[0], b)
                    for b in visible_blocks if b.attached_sort_keys]
    keyed_blocks.sort()
    unkeyed_blocks = [b for b in visible_blocks if not b.attached_sort_keys]

    if not unkeyed_blocks:
        ordered_blocks = [b for _, b in keyed_blocks]
        mapping = dict(zip(sorted_names, ordered_blocks))
        if valid_assignment(mapping):
            return mapping
        return None

    for unkeyed_name_indices in itertools.combinations(range(V), len(unkeyed_blocks)):
        keyed_name_indices = [i for i in range(V) if i not in unkeyed_name_indices]
        if len(keyed_name_indices) != len(keyed_blocks):
            continue

        mapping: dict[str, OwnerRecordBlock] = {}
        for name_idx, (_, block) in zip(keyed_name_indices, keyed_blocks):
            mapping[sorted_names[name_idx]] = block

        for perm in itertools.permutations(unkeyed_blocks):
            trial = dict(mapping)
            for name_idx, block in zip(unkeyed_name_indices, perm):
                trial[sorted_names[name_idx]] = block
            if valid_assignment(trial):
                return trial

    return None


@dataclass
class NameMapping:
    """Result of VDF-native name-to-OT mapping."""
    variable_names: list[str]  # Alphabetically sorted variable names
    owner_blocks: list[OwnerRecordBlock]  # Sort-key ordered visible blocks
    name_to_block: dict[str, OwnerRecordBlock]
    system_ot_indices: set[int]  # OT indices owned by system variables
    unmapped_blocks: list[OwnerRecordBlock]  # Blocks that couldn't be mapped


def _vensim_sort_key(name: str) -> str:
    """Vensim sorts names case-insensitively."""
    return name.lower()


def _validate_name_block_assignment(
    name_to_block: dict[str, OwnerRecordBlock],
    vdf: VdfFile,
) -> bool:
    """
    Check that a name-to-block assignment produces an OT layout consistent
    with the stocks-first-alphabetical ordering rule.

    For each model variable, its expected OT position (derived from alphabetical
    sorting within the stock/non-stock group) must match its assigned block's
    actual OT start position.
    """
    codes = vdf.section6_ot_class_codes() or []
    stock_count = sum(1 for c in codes[1:] if c == OT_CODE_STOCK)

    stock_names: list[str] = []
    nonstock_names: list[str] = []
    for name, block in name_to_block.items():
        if block.ot_codes and all(c == OT_CODE_STOCK for c in block.ot_codes):
            stock_names.append(name)
        else:
            nonstock_names.append(name)

    stock_names.sort(key=_vensim_sort_key)
    system_names_sorted = sorted(
        (n for n in SYSTEM_NAMES if n != "Time"), key=_vensim_sort_key)
    all_nonstock = sorted(nonstock_names + system_names_sorted,
                          key=_vensim_sort_key)

    # Expected stock OT positions. Hidden stock blocks (SMOOTH/DELAY helpers)
    # occupy OT slots before visible stocks, so start after them.
    hidden_stock_ots = 0
    if hasattr(vdf, '_hidden_stock_ot_count'):
        hidden_stock_ots = vdf._hidden_stock_ot_count
    expected_stock_pos = 1 + hidden_stock_ots
    for name in stock_names:
        block = name_to_block[name]
        if block.start != expected_stock_pos:
            return False
        expected_stock_pos += block.length()

    # Expected non-stock OT positions
    expected_nonstock_pos = stock_count + 1
    for name in all_nonstock:
        block = name_to_block.get(name)
        if block is not None:
            if block.start != expected_nonstock_pos:
                return False
            expected_nonstock_pos += block.length()
        else:
            # System variable: occupies 1 OT entry
            expected_nonstock_pos += 1

    return True


def _recover_dimension_element_names(vdf: VdfFile) -> set[str]:
    """
    Recover dimension element names from section 5 cardinalities and
    name-table adjacency.

    Vensim writes dimension definitions as contiguous runs in the name
    table: dim_name, [metadata...], elem1, elem2, ..., elemN. The elements
    form a contiguous block with no intervening candidate names. This
    strict contiguity requirement avoids false matches where variable names
    happen to be adjacent.

    Returns the set of identified element names. These structurally cannot
    own arrayed OT blocks.
    """
    sec5_entries = vdf.parse_section5_sets() or []
    if not sec5_entries:
        return set()

    name_to_idx: dict[str, int] = {}
    for i, name in enumerate(vdf.names):
        if name not in name_to_idx:
            name_to_idx[name] = i

    candidate_set = set(visible_variable_candidates(vdf))
    cardinalities = sorted([e.n for e in sec5_entries], reverse=True)
    elements: set[str] = set()
    used_dims: set[str] = set()

    for card in cardinalities:
        best_dim: Optional[str] = None
        best_elems: Optional[list[str]] = None

        for cand in sorted(candidate_set, key=lambda n: name_to_idx.get(n, 0)):
            if cand in used_dims or cand in elements:
                continue
            cand_idx = name_to_idx.get(cand, -1)
            if cand_idx < 0:
                continue

            # Collect the FIRST n candidates after this dim name, requiring
            # they form a contiguous block (only metadata names between them)
            elems: list[str] = []
            contiguous = True
            for j in range(cand_idx + 1, len(vdf.names)):
                if len(elems) >= card:
                    break
                next_name = vdf.names[j]
                if next_name in used_dims or next_name in elements:
                    contiguous = False
                    break
                cls = classify_name(next_name)
                if cls:
                    continue  # skip metadata
                if next_name in VENSIM_MODULE_NAMES or next_name in VENSIM_STDLIB_HELPERS:
                    continue
                if next_name not in candidate_set:
                    contiguous = False
                    break
                elems.append(next_name)

            if contiguous and len(elems) == card:
                # Additional check: elements should be simple names (no spaces)
                # to avoid matching compound variable names as elements
                all_simple = all(" " not in e for e in elems)
                if all_simple:
                    best_dim = cand
                    best_elems = elems
                    break

        if best_dim is not None and best_elems is not None:
            used_dims.add(best_dim)
            elements.update(best_elems)

    return elements


def _score_variable_name_set(var_names: list[str], all_candidates: list[str],
                             vdf: Optional[VdfFile] = None) -> float:
    """
    Score how likely a set of names are actual model variables vs
    dimension/element names. Higher score = more likely variables.

    Primary signals:
    1. Excluded names should form proper dimension groups (dim + N elements)
    2. Variable names tend to be longer/compound; elements tend to be short
    3. Excluding a compound name (contains space) strongly penalized
    """
    score = 0.0
    var_set = set(var_names)
    excluded = [n for n in all_candidates if n not in var_set]

    # Primary: bonus if excluded names form dimension groups matching sec5
    if vdf is not None:
        sec5_entries = vdf.parse_section5_sets() or []
        if sec5_entries:
            cardinalities = sorted([e.n for e in sec5_entries], reverse=True)
            dim_score = _score_excluded_as_dimensions(
                excluded, cardinalities, vdf)
            score += dim_score * 100

    # Strong signal: compound names (with spaces) are almost always variables,
    # never dimension elements. Penalize hard if a compound name is excluded.
    for name in excluded:
        if " " in name:
            score -= 50
    for name in var_names:
        if " " in name:
            score += 10

    # Variable names tend to be longer than element names
    for name in var_names:
        score += min(len(name), 10)
    for name in excluded:
        # Excluding short names is fine; excluding long names is bad
        if len(name) > 6:
            score -= 5

    return score


def _score_excluded_as_dimensions(
    excluded: list[str],
    cardinalities: list[int],
    vdf: VdfFile,
) -> float:
    """
    Score how well excluded names form dimension groups matching the given
    cardinalities. Returns 0-1 representing fraction of cardinalities matched.
    """
    if not cardinalities:
        return 1.0 if not excluded else 0.0

    expected_excluded = sum(1 + c for c in cardinalities)
    if len(excluded) != expected_excluded:
        return 0.0

    name_to_idx: dict[str, int] = {}
    for i, name in enumerate(vdf.names):
        if name not in name_to_idx:
            name_to_idx[name] = i

    excluded_set = set(excluded)
    matched = 0
    score = 0.0

    # Try to match each cardinality to a group: dim_name followed by N
    # elements, all from the excluded set, tight in the name table
    used: set[str] = set()
    for card in cardinalities:
        best_gap = float("inf")
        best_group: Optional[tuple[str, list[str]]] = None
        for name in excluded:
            if name in used:
                continue
            idx = name_to_idx.get(name, -1)
            if idx < 0:
                continue
            elements: list[str] = []
            for j in range(idx + 1, len(vdf.names)):
                if len(elements) >= card:
                    break
                next_name = vdf.names[j]
                if next_name in used or next_name == name:
                    continue
                if next_name not in excluded_set:
                    continue
                elements.append(next_name)
            if len(elements) == card:
                last_idx = name_to_idx.get(elements[-1], idx)
                gap = last_idx - idx
                if gap < best_gap:
                    best_gap = gap
                    best_group = (name, elements)
        if best_group is not None:
            used.add(best_group[0])
            used.update(best_group[1])
            matched += 1
            # Tighter groups (smaller gap) are more likely correct
            score += 1.0 / (1.0 + best_gap)

    # Combine match fraction with tightness bonus
    return (matched / len(cardinalities)) + score * 0.1


def map_names_to_owner_blocks(vdf: VdfFile) -> Optional[NameMapping]:
    """
    Map visible variable names to owner blocks using the stocks-first-
    alphabetical ordering rule, validated against actual OT positions.

    Sort keys on owner blocks give the global alphabetical ordering.
    When all blocks have sort keys, we zip directly. When some blocks
    lack sort keys, we try all valid insertions and validate against OT
    structure.
    """
    all_blocks = build_owner_record_blocks(vdf)
    visible_blocks = [b for b in all_blocks if not b.hidden]
    hidden_blocks = [b for b in all_blocks if b.hidden]
    candidates = visible_variable_candidates(vdf)

    # Compute hidden stock OT count for validation
    hidden_stock_ots = sum(
        b.length() for b in hidden_blocks
        if b.ot_codes and all(c == OT_CODE_STOCK for c in b.ot_codes))
    vdf._hidden_stock_ot_count = hidden_stock_ots  # type: ignore[attr-defined]

    if not visible_blocks or not candidates:
        return NameMapping(
            variable_names=sorted(candidates),
            owner_blocks=[],
            name_to_block={},
            system_ot_indices=set(),
            unmapped_blocks=visible_blocks,
        )

    B = len(visible_blocks)

    # When there are excess blocks (system records that leaked through),
    # try to select the right B blocks. First attempt: use all visible blocks.
    # If that fails, try subsets.
    system_blocks: list[OwnerRecordBlock] = []

    # Fast path: if candidate count matches block count, try direct mapping
    if len(candidates) == B:
        sorted_names = sorted(candidates, key=_vensim_sort_key)
        trial = _try_name_block_mapping(sorted_names, visible_blocks, vdf)
        if trial is not None:
            return _build_result(sorted_names, trial, system_blocks, vdf)

    # General case: try all subsets of B candidates from the full candidate
    # list. This handles dimension/element filtering and excess system blocks
    # simultaneously. For C(14,4) = 1001 this is fast.
    # Collect ALL valid solutions and pick the best one by scoring.
    import itertools
    solutions: list[tuple[float, list[str], dict[str, OwnerRecordBlock]]] = []
    for subset in itertools.combinations(range(len(candidates)), B):
        trial_names = sorted([candidates[i] for i in subset],
                             key=_vensim_sort_key)
        trial = _try_name_block_mapping(trial_names, visible_blocks, vdf)
        if trial is not None:
            score = _score_variable_name_set(trial_names, candidates, vdf)
            solutions.append((score, trial_names, trial))
    if solutions:
        solutions.sort(key=lambda s: -s[0])
        _, best_names, best_mapping = solutions[0]
        return _build_result(best_names, best_mapping, system_blocks, vdf)

    # If no subset of candidates matches all visible blocks, some visible
    # blocks may be system-record artifacts. Try with fewer blocks.
    # Collect all solutions and pick the best by scoring.
    for fewer in range(min(B, len(candidates)), 0, -1):
        fewer_solutions: list[tuple[float, list[str], dict[str, OwnerRecordBlock], list[OwnerRecordBlock]]] = []
        for block_subset in itertools.combinations(visible_blocks, fewer):
            block_list = list(block_subset)
            for name_subset in itertools.combinations(range(len(candidates)), fewer):
                trial_names = sorted([candidates[i] for i in name_subset],
                                     key=_vensim_sort_key)
                trial = _try_name_block_mapping(trial_names, block_list, vdf)
                if trial is not None:
                    sys_blocks = [b for b in visible_blocks if b not in block_list]
                    score = _score_variable_name_set(trial_names, candidates, vdf)
                    fewer_solutions.append((score, trial_names, trial, sys_blocks))
        if fewer_solutions:
            fewer_solutions.sort(key=lambda s: -s[0])
            _, best_names, best_mapping, best_sys = fewer_solutions[0]
            return _build_result(best_names, best_mapping, best_sys, vdf)

    return None


def _build_result(
    sorted_names: list[str],
    name_to_block: dict[str, OwnerRecordBlock],
    system_blocks: list[OwnerRecordBlock],
    vdf: VdfFile,
) -> NameMapping:
    ordered_blocks = [name_to_block[n] for n in sorted_names]
    system_ots: set[int] = set()
    for b in system_blocks:
        system_ots.update(range(b.start, b.end))
    return NameMapping(
        variable_names=sorted_names,
        owner_blocks=ordered_blocks,
        name_to_block=name_to_block,
        system_ot_indices=system_ots,
        unmapped_blocks=system_blocks,
    )


@dataclass
class NamedResult:
    """A single named time series from a VDF file."""
    name: str
    ot_index: int
    values: list[float]


def extract_named_results(vdf: VdfFile) -> Optional[list[NamedResult]]:
    """
    Extract named time series from a VDF file using VDF-native structure only.

    Returns a list of NamedResult for each mapped variable (scalar variables
    get one entry, arrayed variables get one entry per element). System
    variables (FINAL TIME, INITIAL TIME, SAVEPER, TIME STEP) are also included.
    """
    mapping = map_names_to_owner_blocks(vdf)
    if mapping is None:
        return None

    # Extract time values
    if vdf.first_data_block + 2 + vdf.bitmap_size > len(vdf.data):
        return None
    time_count = u16(vdf.data, vdf.first_data_block)
    if time_count != vdf.time_point_count:
        return None
    data_start = vdf.first_data_block + 2 + vdf.bitmap_size
    time_values = [f32(vdf.data, data_start + i * 4) for i in range(time_count)]

    results: list[NamedResult] = []

    # Time itself
    results.append(NamedResult(name="Time", ot_index=0, values=time_values))

    # Mapped variable results
    codes = vdf.section6_ot_class_codes() or []
    for name in mapping.variable_names:
        block = mapping.name_to_block.get(name)
        if block is None:
            continue

        if block.length() == 1:
            # Scalar variable
            ot_idx = block.start
            raw = vdf.offset_table_entry(ot_idx)
            if raw is None:
                continue
            if vdf.is_data_block_offset(raw):
                series = vdf.extract_block_series(raw, time_values)
            else:
                const_val = u32_as_f32(raw)
                series = [const_val] * len(time_values)
            results.append(NamedResult(name=name, ot_index=ot_idx, values=series))
        else:
            # Arrayed variable: one result per OT element
            for elem_offset in range(block.length()):
                ot_idx = block.start + elem_offset
                raw = vdf.offset_table_entry(ot_idx)
                if raw is None:
                    continue
                if vdf.is_data_block_offset(raw):
                    series = vdf.extract_block_series(raw, time_values)
                else:
                    const_val = u32_as_f32(raw)
                    series = [const_val] * len(time_values)
                elem_name = f"{name}[{elem_offset}]"
                results.append(NamedResult(
                    name=elem_name, ot_index=ot_idx, values=series))

    # System variables (the unmapped OT entries that are inline constants)
    system_names_sorted = sorted(
        n for n in SYSTEM_NAMES if n != "Time")
    system_ot_indices: list[int] = []
    for ot_idx in range(1, vdf.offset_table_count):
        if ot_idx in mapping.system_ot_indices:
            system_ot_indices.append(ot_idx)
            continue
        # Check if this OT is covered by any mapped block
        covered = any(
            block.start <= ot_idx < block.end
            for block in mapping.owner_blocks
        )
        if not covered:
            # Also check hidden blocks
            hidden_covered = any(
                block.start <= ot_idx < block.end
                for block in build_owner_record_blocks(vdf)
                if block.hidden
            )
            if not hidden_covered:
                system_ot_indices.append(ot_idx)

    # Map system names alphabetically to system OT indices
    for i, name in enumerate(system_names_sorted):
        if i >= len(system_ot_indices):
            break
        ot_idx = system_ot_indices[i]
        raw = vdf.offset_table_entry(ot_idx)
        if raw is None:
            continue
        if vdf.is_data_block_offset(raw):
            series = vdf.extract_block_series(raw, time_values)
        else:
            const_val = u32_as_f32(raw)
            series = [const_val] * len(time_values)
        results.append(NamedResult(name=name, ot_index=ot_idx, values=series))

    return results


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


def parse_name_table(data: bytes, sec: Section, parse_end: int) -> list[str]:
    names = []
    data_start = sec.data_offset()
    parse_end = min(parse_end, len(data))

    first_len = (sec.field5 >> 16) & 0xFFFF
    if first_len == 0 or data_start + first_len > len(data):
        return names

    s = ""
    for b in data[data_start:data_start + first_len]:
        if b == 0:
            break
        s += chr(b)
    names.append(s)

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
        s = ""
        for b in data[pos:pos + length]:
            if b == 0:
                break
            s += chr(b)
        if not s or not all(c in range(0x20, 0x7f) for c in s.encode("ascii", errors="replace")):
            break
        names.append(s)
        pos += length
    return names


def find_slot_table(data: bytes, name_sec: Section, max_name_count: int,
                    sec1_data_size: int) -> tuple[int, list[int]]:
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
    if data[:4] != VDF_FILE_MAGIC:
        raise ValueError(f"invalid VDF magic: {data[:4].hex()}")

    time_point_count = u32(data, 0x78)
    bitmap_size = math.ceil(time_point_count / 8)

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

    # Find records
    if slot_table:
        sec1_data_start = sections[1].data_offset() if len(sections) > 1 else FILE_HEADER_SIZE
        sorted_slots = sorted(slot_table)
        max_offset = sorted_slots[-1]
        last_stride = sorted_slots[-1] - sorted_slots[-2] if len(sorted_slots) >= 2 else max_offset
        search_start = sec1_data_start + max_offset + last_stride
    else:
        search_start = sections[1].data_offset() if len(sections) > 1 else FILE_HEADER_SIZE

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
    print(f"Timestamp:    {timestamp}")
    print(f"Time points:  {vdf.time_point_count}")
    print(f"Bitmap size:  {vdf.bitmap_size} bytes")
    print()

    print("=== Header Offsets ===")
    print(f"  0x58 final_values_offset:    0x{vdf.header_final_values_offset:08x}")
    print(f"  0x5c lookup_mapping_offset:  0x{vdf.header_lookup_mapping_offset:08x}")
    print(f"  0x60 offset_table_offset:    0x{vdf.offset_table_start:08x}")
    print(f"  OT count (derived):          {vdf.offset_table_count}")
    print(f"  First data block:            0x{vdf.first_data_block:08x}")
    print()


def print_layout(vdf: VdfFile) -> None:
    entries = [(0, f"File header ({FILE_HEADER_SIZE} bytes)")]
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
    for slot, names in build_display_slot_to_names(vdf).items():
        if names:
            slot_to_name[slot] = names[0]

    print(f"  SENT = sentinel 0x{VDF_SENTINEL:08x}")
    print(f"  Known: f[0]=type f[1]=class f[6]=shape f[10]=sort f[11]=ot_idx f[12]=slot_ref")
    print()

    # Header
    hdr = f"  {'#':>3} {'offset':>10}"
    for i in range(16):
        hdr += f" {'f'+str(i):>6}"
    hdr += "  class slot"
    print(hdr)

    ot_count = vdf.offset_table_count
    for i, rec in enumerate(vdf.records):
        f = rec.fields
        if f[0] == 0:
            cls = "zero"
        elif f[1] == 23:
            cls = "system"
        elif f[10] > 0 and f[11] > 0 and f[11] < ot_count:
            cls = f"model sort={f[10]} ot={f[11]}"
        elif f[11] > 0 and f[11] >= ot_count:
            cls = f"ot_oob({f[11]})"
        else:
            cls = ""

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

    slot_to_names = build_display_slot_to_names(vdf)
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

    slot_to_names = build_display_slot_to_names(vdf)
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
    print(f"  sets={len(entries)}")
    if not entries:
        print()
        return

    slot_to_names = build_display_slot_to_names(vdf)
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
        print("  (none)\n")
        return

    skip, entries, stop = result
    slot_to_names = build_display_slot_to_names(vdf)
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
        if offset + 2 + vdf.bitmap_size > len(vdf.data):
            print(f"  {idx:>3}  0x{offset:08x}  (truncated)")
            continue
        count = u16(vdf.data, offset)
        block_size = 2 + vdf.bitmap_size + count * 4
        density = (count / vdf.time_point_count * 100) if vdf.time_point_count > 0 else 0

        data_start = offset + 2 + vdf.bitmap_size
        first_val = f32(vdf.data, data_start) if count > 0 and data_start + 4 <= len(vdf.data) else float("nan")
        last_val = f32(vdf.data, data_start + (count - 1) * 4) if count > 1 and data_start + count * 4 <= len(vdf.data) else first_val

        label = "  [TIME]" if offset == vdf.first_data_block else ""
        print(f"  {idx:>3}  0x{offset:08x}  {count}/{vdf.time_point_count} "
              f"({density:.0f}%)  {block_size}B  first={first_val} last={last_val}{label}")
    print()


def print_data_series(vdf: VdfFile) -> None:
    """Extract and print first/last values for every OT entry."""
    print("=== Data Series (first/last values per OT) ===")
    # Get time values first
    if vdf.first_data_block + 2 + vdf.bitmap_size > len(vdf.data):
        print("  (time block truncated)\n")
        return

    count = u16(vdf.data, vdf.first_data_block)
    if count != vdf.time_point_count:
        print(f"  (time block count {count} != expected {vdf.time_point_count})\n")
        return

    data_start = vdf.first_data_block + 2 + vdf.bitmap_size
    time_values = [f32(vdf.data, data_start + i * 4) for i in range(count)]

    codes = vdf.section6_ot_class_codes()
    for i in range(vdf.offset_table_count):
        raw = vdf.offset_table_entry(i)
        if raw is None:
            continue
        code_str = ""
        if codes and i < len(codes):
            code_str = f" ({ot_code_label(codes[i])})"

        if vdf.is_data_block_offset(raw):
            series = vdf.extract_block_series(raw, time_values)
            first = series[0] if series else float("nan")
            last = series[-1] if series else float("nan")
            print(f"  OT[{i:>3}]{code_str}  first={first}  last={last}")
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
    """Show the VDF-native name-to-OT mapping."""
    print("=== VDF-Native Name Mapping ===")

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
    """Extract and show named results using VDF-native mapping."""
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

    slot_to_names = build_display_slot_to_names(vdf)

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

    # 2. Slot tables in small/medium fixtures form a contiguous 16-byte lattice.
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

    # 3. Section-3 index_words form arithmetic progression (step=27)
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

        # 4. All sec3 axis_slot_refs are in the slot table
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

    # 5. Section-5 trailing refs overlap with sec3 axis_slot_refs
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

    # 6. Record field[6] values are either 0, 5, 32, a sec3 index_word, or in the high range (7000+)
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
    names = build_display_slot_to_names(vdf).get(slot_ref, [])
    label = resolve_slot_ref(slot_ref, {slot_ref: names} if names else {})
    if not include_signature:
        return label
    return f"{label} sig={format_u32_words(slot_words(vdf, slot_ref))}"


def collect_slot_reference_inventory(vdf: VdfFile) -> dict[int, SlotReferenceInfo]:
    slot_to_names = build_display_slot_to_names(vdf)
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
        left_slots = build_display_slot_to_names(left)
        right_slots = build_display_slot_to_names(right)
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
    parser.add_argument("path", help="Path to VDF file")
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
    parser.add_argument("--slot-xref", action="store_true",
                        help="Show section 3/4/5/6 referenced slot refs with signatures")
    parser.add_argument("--ot", action="store_true", help="Show offset table")
    parser.add_argument("--blocks", action="store_true", help="Show data blocks")
    parser.add_argument("--data", action="store_true", help="Extract and show all time series")
    parser.add_argument("--bridge", action="store_true", help="Show record shape -> sec3 bridge")
    parser.add_argument("--record-blocks", action="store_true",
                        help="Show record groups merged by decoded shape span")
    parser.add_argument("--owner-blocks", action="store_true",
                        help="Show owner-oriented blocks built from sentinel model records")
    parser.add_argument("--sec35-bridge", action="store_true", help="Show section-3 -> section-5 bridge")
    parser.add_argument("--ranges", action="store_true", help="Show record-derived OT ranges")
    parser.add_argument("--validate", action="store_true", help="Check structural invariants")
    parser.add_argument("--map-names", action="store_true",
                        help="Show VDF-native name-to-OT mapping")
    parser.add_argument("--extract", action="store_true",
                        help="Extract named results using VDF-native mapping")
    parser.add_argument("--raw-section", type=int, metavar="N", help="Full hexdump of section N")
    parser.add_argument("--json", action="store_true", help="Machine-readable JSON summary")

    args = parser.parse_args()
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
        args.sec5, args.sec6, args.slot_xref, args.ot, args.blocks, args.data,
        args.bridge, args.record_blocks, args.sec35_bridge, args.ranges, args.validate,
        args.owner_blocks, args.map_names, args.extract,
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

    if not show_specific or show_all:
        print_summary(vdf)


if __name__ == "__main__":
    main()
