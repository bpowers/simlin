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

    def dimension_size(self) -> int:
        # marker=0 entries have n+1 refs (1 trailing axis ref)
        # marker=1 entries have n+2 refs (2 trailing axis refs)
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
class OtRange:
    start: int
    end: int
    record_count: int

    def length(self) -> int:
        return self.end - self.start


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
    out: dict[int, list[str]] = {}
    for i, slot in enumerate(vdf.slot_table):
        if i < len(vdf.names):
            out.setdefault(slot, []).append(vdf.names[i])
    return out


def resolve_slot_ref(slot_ref: int, slot_to_names: dict[int, list[str]]) -> str:
    names = slot_to_names.get(slot_ref)
    if names:
        return f"{slot_ref}:{'/'.join(names)}"
    return f"{slot_ref}:<sec1>"


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
            if not all(v % 4 == 0 and v > 0 and v < sec1_data_size for v in sorted_vals):
                continue
            strides = [sorted_vals[i + 1] - sorted_vals[i] for i in range(len(sorted_vals) - 1)]
            if strides and min(strides) >= 4:
                return table_start, values
    return 0, []


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
    print(f"=== Slot Table ({len(vdf.slot_table)} entries @ 0x{vdf.slot_table_offset:08x}) ===")
    print(f"  {'Idx':>3}  {'Sec1Off':>7}  {'Name':<36}  {'w[0]':>8} {'w[1]':>8} {'w[2]':>8} {'w[3]':>8}")
    for i, offset in enumerate(vdf.slot_table):
        name = vdf.names[i] if i < len(vdf.names) else "???"
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
    for i, slot in enumerate(vdf.slot_table):
        if i < len(vdf.names):
            slot_to_name[slot] = vdf.names[i]

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

    slot_to_names = build_slot_to_names(vdf)
    sec4_entries = vdf.parse_section4_entries()
    sec4_idx_words = set()
    if sec4_entries:
        sec4_idx_words = {e.index_word for e in sec4_entries}

    for i, entry in enumerate(directory.entries):
        slot_refs = [resolve_slot_ref(sr, slot_to_names) for sr in entry.axis_slot_refs()]
        sec4_hit = entry.index_word() in sec4_idx_words
        print(f"  {i:>3} @0x{entry.file_offset:08x} idx={entry.index_word()} sec4_hit={sec4_hit} "
              f"flat={entry.flat_size()} axes={entry.axis_sizes()} "
              f"w10={entry.words[10]} w11={entry.words[11]} "
              f"slot_refs={slot_refs} tail={entry.terminal_tag()}")
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

    slot_to_names = build_slot_to_names(vdf)
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

    slot_to_names = build_slot_to_names(vdf)
    for i, e in enumerate(entries[:16]):
        refs = [resolve_slot_ref(r, slot_to_names) for r in e.refs[:8]]
        print(f"  {i:>3} @0x{e.file_offset:08x} n={e.n} marker={e.marker} "
              f"size={len(e.refs)} dim={e.dimension_size()} "
              f"slotted={e.slotted_ref_count} refs(head)={refs}")
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
    slot_to_names = build_slot_to_names(vdf)
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
              f"slot_hits={e.slotted_ref_count} refs={refs}")
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
        if code == 5:
            label = "scalar"
        elif code in idx_to_entry:
            entry = idx_to_entry[code]
            label = f"sec3 idx={code}, flat={entry.flat_size()}, axes={entry.axis_sizes()}"
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

    slot_to_names = build_slot_to_names(vdf)

    for i, sec3 in enumerate(directory.entries):
        axis_refs = set(sec3.axis_slot_refs())
        if not axis_refs:
            print(f"  sec3[{i}] idx={sec3.index_word()} flat={sec3.flat_size()} "
                  f"axes={sec3.axis_sizes()} -- no axis_slot_refs")
            continue

        # Find sec5 entries whose trailing ref(s) overlap with this sec3's axis_slot_refs
        matched_sec5 = []
        for j, sec5 in enumerate(sec5_entries):
            trailing_count = 1 + sec5.marker
            trailing_refs = set(sec5.refs[-trailing_count:]) if len(sec5.refs) >= trailing_count else set()
            if trailing_refs & axis_refs:
                matched_sec5.append((j, sec5))

        axis_ref_strs = [resolve_slot_ref(r, slot_to_names) for r in sec3.axis_slot_refs()]
        print(f"  sec3[{i}] idx={sec3.index_word()} flat={sec3.flat_size()} "
              f"axes={sec3.axis_sizes()} axis_refs={axis_ref_strs}")

        if matched_sec5:
            for j, sec5 in matched_sec5:
                trailing_count = 1 + sec5.marker
                trailing = sec5.refs[-trailing_count:]
                trailing_strs = [resolve_slot_ref(r, slot_to_names) for r in trailing]
                print(f"    -> sec5[{j}] n={sec5.n} marker={sec5.marker} "
                      f"dim_size={sec5.dimension_size()} trailing={trailing_strs}")
        else:
            print(f"    (no matching sec5 entries)")
    print()


def print_validation(vdf: VdfFile) -> None:
    """Check structural invariants and report any violations."""
    print("=== Validation ===")
    errors: list[str] = []
    warnings: list[str] = []

    # 1. Section-3 index_words form arithmetic progression (step=27)
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

        # 2. All sec3 axis_slot_refs are in the slot table
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

    # 3. Section-5 trailing refs overlap with sec3 axis_slot_refs
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

        if sec3_only:
            warnings.append(f"sec3 axis refs not in sec5 trailing: {sec3_only}")
        if sec5_only:
            warnings.append(f"sec5 trailing refs not in sec3 axis: {sec5_only}")
    elif not sec5_entries:
        print(f"  [SKIP] no section-5 entries for axis ref overlap check")

    # 4. Record field[6] values are either 0, 5, 32, a sec3 index_word, or in the high range (7000+)
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
    if len(vdf.sections) <= 1:
        return None
    abs_off = vdf.sections[1].data_offset() + slot_offset
    if abs_off + 16 > len(vdf.data):
        return None
    return [u32(vdf.data, abs_off + i * 4) for i in range(4)]


def format_u32_words(words: Optional[list[int]]) -> str:
    if words is None:
        return "(out of bounds)"
    return "[" + " ".join(f"{word:08x}" for word in words) + "]"


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
    by_name_left = {name: i for i, name in enumerate(left.names[:len(left.slot_table)])}
    by_name_right = {name: i for i, name in enumerate(right.names[:len(right.slot_table)])}
    shared_names = sorted(set(by_name_left) & set(by_name_right))
    any_slot_diff = False
    for name in shared_names:
        li = by_name_left[name]
        ri = by_name_right[name]
        lslot = left.slot_table[li]
        rslot = right.slot_table[ri]
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
        left_slots = build_slot_to_names(left)
        right_slots = build_slot_to_names(right)
        for i in range(max_entries):
            lentry = left_entries[i] if i < len(left_entries) else None
            rentry = right_entries[i] if i < len(right_entries) else None
            lrefs = [resolve_slot_ref(r, left_slots) for r in lentry.refs] if lentry else []
            rrefs = [resolve_slot_ref(r, right_slots) for r in rentry.refs] if rentry else []
            if lrefs != rrefs:
                print(f"  entry[{i}]")
                print(f"    left:  {lrefs}")
                print(f"    right: {rrefs}")
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
    parser.add_argument("path", help="Path to VDF file")
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
    parser.add_argument("--ot", action="store_true", help="Show offset table")
    parser.add_argument("--blocks", action="store_true", help="Show data blocks")
    parser.add_argument("--data", action="store_true", help="Extract and show all time series")
    parser.add_argument("--bridge", action="store_true", help="Show record shape -> sec3 bridge")
    parser.add_argument("--sec35-bridge", action="store_true", help="Show section-3 -> section-5 bridge")
    parser.add_argument("--ranges", action="store_true", help="Show record-derived OT ranges")
    parser.add_argument("--validate", action="store_true", help="Check structural invariants")
    parser.add_argument("--raw-section", type=int, metavar="N", help="Full hexdump of section N")
    parser.add_argument("--json", action="store_true", help="Machine-readable JSON summary")

    args = parser.parse_args()
    path = Path(args.path)

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
        args.sec5, args.sec6, args.ot, args.blocks, args.data,
        args.bridge, args.sec35_bridge, args.ranges, args.validate,
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

    if not show_specific or show_all:
        print_summary(vdf)


if __name__ == "__main__":
    main()
