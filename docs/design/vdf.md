# VDF Binary Format (Vensim Data File)

## Overview

VDF is Vensim's proprietary binary format for simulation output. The format is
completely undocumented and no open-source parser previously existed.

Vensim can open a `.vdf` file and show its contents without a corresponding
`.mdl` model file, and open "old" VDF files and show time-series for some
variables even after substantive model changes. This is strong evidence that
the VDF contains enough information to map variable names to their time-series
data, but the complete general rule is not yet decoded. Our goal is to find a
single, deterministic method to convert any VDF file into a `Results` struct
(a mapping of variable names to time series data) without any external model
file. An approach that works on small models but fails on large ones is not a
solution -- it is a partial observation that has not yet uncovered the actual
format mechanism.

### Working assumptions for reverse engineering

Vensim dates to the early 1990s, originally written in C for Windows. This
context is useful for interpreting the file format, but it is not itself
evidence for any specific field:

- **CPUs were slow; RAM and disk were precious.** Every structure in the file
  was designed to be read with simple pointer arithmetic -- seek to an offset,
  read a fixed-width struct, index into an array. O(n^2) algorithms,
  probabilistic matching, and hash tables were not how formats were designed.
  If our reverse-engineering approach involves combinatorial search or
  heuristic scoring, that is a strong signal we have not decoded the actual
  mechanism.

- **The file appears to preserve runtime-oriented structures.** The offset
  table (OT) behaves like the saved runtime variable array; section-1 records
  behave like variable descriptors; section 2 is the string pool; section 3 is
  an array-shape table. This explains why internal SMOOTH/DELAY macro
  variables appear in the file despite not being something users typically
  plot: they exist in the simulation state and are saved as part of it.

- **Every mapping should be O(1) or O(n).** When Vensim opens a VDF, it reads
  the structures back into memory and resolves names to data through direct
  indexing. If a decoded field gives us a mapping that requires scanning a
  range of offsets or scoring candidate solutions, we likely have the right
  field but the wrong formula.

The format has been reverse-engineered from multiple VDF files of varying
complexity:

| Category | Variables | Size | Files |
|----------|-----------|------|-------|
| Small | 3-8 | 3-7KB | `bact`, `water`, `pop`, `consts`, `lookups` |
| Medium | ~420 | 333KB | WRLD3-03 (World3-03 from the Limits to Growth model) |
| Large | ~1000+ | 1.8MB | C-LEARN model |

**Conventions**: all values are little-endian. Data is 32-bit floats unless
noted. All offsets in this document are byte offsets. The parser is implemented
in `src/simlin-engine/src/vdf.rs`.

### Current decoding boundary

This document intentionally distinguishes pinned file-format facts from current
decoder reconstruction.

Pinned facts:

- Header offsets locate section boundaries, section-6 tails, the offset table,
  and sparse data blocks.
- Section 2 contains the printable name table. Record `field[2]` directly keys
  this table as `(name_string_start - section2_data_start) / 4 + 7`; it is not
  a rank or heuristic match.
- The slot table's structural pairing is direct: `slot_table[i]` belongs to
  `names[i]`. `tools/vdf_xray.py` still has a shifted display alignment for
  edited fixtures with leading helper slots, but that alignment is exploratory
  presentation only and must not be used as evidence for on-disk refs.
- Record `field[11]` is an OT start when it is in range. `field[6] == 5` is a
  scalar span; nonzero section-3 shape keys and the single-shape `32` marker
  can provide array spans. `field[6] == 0` remains ambiguous and is excluded
  from fact-only record-span reports.
- Section 3 is a reusable array-shape directory: flat size, axis sizes, and
  axis slot refs are decoded for the observed array fixtures.
- Section 6 contains OT class codes, final-value floats, and fixed-width
  lookup metadata records at header-derived offsets.
- Many dimension element catalogs are recoverable from record `field[8]`
  groups: a dimension anchor and zero-based element records share a compact
  group id. This is currently implemented in `tools/vdf_xray.py`.
- Attached dimension-anchor records can bind a recovered element catalog to a
  reusable section-3 shape template. When that binding is unique, sibling
  owners using the same template inherit the same element labels.

Current reconstruction, not format fact:

- Choosing which overlapping record span owns a saved series.
- Labeling axes when two or more recovered dimensions have the same
  cardinality and no decoded shape-template or axis binding distinguishes
  them.
- Non-overlap interval selection, file-order/shift-by-one owner mapping, and
  stock-first alphabetical assignment.
- Section-4 view/sketch semantics beyond the fixed-width reference stream.
- Exact identification of old-style aliases and descriptor records from VDF
  alone.

For strict debugging, use `tools/vdf_xray.py --record-facts`. It prints only
direct record-name and record-OT span facts before descriptor pruning,
non-overlap selection, hidden-slot display alignment, or array-label guessing.

### Dataset/reference-mode sibling format

Files such as `test/bobby/vdf/econ/data.vdf` are not malformed simulation
results. They are a sibling VDF container used for loaded datasets/reference
modes. The differences observed so far are:

- file magic is `7f f7 17 41` instead of `7f f7 17 52`
- the container has 5 sections instead of 8
- the familiar string/record area is shifted into section 0 and the printable
  name table is shifted into section 1
- section 4 starts with a block-offset list terminated by zero, then reuses
  the same sparse block encoding as result VDFs

The overlap is still substantial: the section-header magic matches, the slot
table and 64-byte record heuristics still work, and the sparse data blocks use
the same forward-fill decoding. For `data.vdf`, the visible dataset series are
recovered by pairing section-1 names with section-0 records sorted by
`(field[2], file_offset)`, then mapping each record's `field[11]` to the
section-4 block list.


## File layout

```
  +--------------------------------------+
  | File header (128 bytes)              |  0x00..0x7F
  +--------------------------------------+
  | Section 0 (simulation command)       |
  +--------------------------------------+
  | Section 1 (string table + metadata)  |
  |   - string table entries             |
  |   - variable metadata records        |
  |   - slot table                       |
  +--------------------------------------+
  | Section 2 (name table)               |
  +--------------------------------------+
  | Section 3 (array directory / zeros)  |
  +--------------------------------------+
  | Section 4 (view/group metadata)      |
  +--------------------------------------+
  | Section 5 (dimension sets)           |
  +--------------------------------------+
  | Section 6 (OT metadata)              |
  +--------------------------------------+
  | Section 7                            |
  |   - lookup table data                |
  |   - offset table                     |
  |   - data blocks                      |
  +--------------------------------------+
```


## File header

128 bytes (0x00..0x7F):

```
  Offset  Size  Description
  ------  ----  -----------
  0x00    4     Magic bytes: 7F F7 17 52
  0x04    116   ASCII timestamp string, null-terminated, zero-padded
                Example: "(Sun Nov 30 23:28:16 2008) From bact.mdl"
                Bytes 0x48..0x53 are zero-padded
  0x50    4     Format constant: 0x012C0065 (observed simulation files)
  0x54    4     Zero
  0x58    4     u32 final_values_offset: absolute file offset to the
                section-6 OT final-values array
  0x5C    4     u32 lookup_mapping_offset: absolute file offset to the
                section-6 lookup mapping records (end of final values)
  0x60    4     u32 offset_table_offset: absolute file offset to the
                section-7 offset table
  0x64    4     u32 offset_table_offset (duplicate, always same as 0x60)
  0x68    4     Zero in observed simulation fixtures; meaning unknown
  0x6C    4     Save/version marker: nonzero only in re-saved *Current.vdf
                files; zero for fresh simulation output
  0x70    4     Total lookup coordinate-pair count across all graphical
                functions. Zero when the model has no lookup data; correlates
                with section-7 lookup-data size (observed: 0 -> 12 bytes,
                5 -> 52, 8 -> 76, 228 -> 3796)
  0x74    4     Zero
  0x78    4     u32 time_point_count
  0x7C    4     u32 time_point_count (duplicate, always same value)
```

### Derived quantities from header fields

The header offsets at 0x58, 0x5C, 0x60 provide direct access to key data
structures, eliminating the need for heuristic scanning:

- **OT count** = `(header[0x5C] - header[0x58]) / 4`
- **Class codes** start at `header[0x58] - OT_count` (one byte per OT entry,
  immediately before the final values array)
- **Final values** start at `header[0x58]` (one f32 per OT entry)
- **Offset table** starts at `header[0x60]`
- **First data block** = `u32` value at `header[0x60]` (OT entry 0 = time block)

These derivations are validated across the observed simulation-result corpus.

The `time_point_count` is the number of output time points stored. Examples:
- bact model (t=0..60, saveper=1): 61
- pop model (t=0..100, saveper=1): 101
- WRLD3-03 (t=1900..2100, dt=0.5): 401

The bitmap size used in data blocks is `ceil(time_point_count / 8)` bytes.


## Section framing

Simulation-result VDF files contain multiple sections, each delimited by a
4-byte magic value. Every observed simulation-result VDF has exactly **8
sections** (indices 0-7). Dataset/reference-mode VDF siblings use the same
section magic but a different 5-section layout.

### Section header (24 bytes)

```
  Offset  Size  Description
  ------  ----  -----------
  +0      4     Section magic: A1 37 4C BF (= f32 -0.797724 = u32 0xBF4C37A1)
  +4      4     u32 field1
  +8      4     u32 field2 (equals field1 in observed simulation files)
  +12     4     u32 field3
  +16     4     u32 field4
  +20     4     u32 field5
```

A section's data region runs from its 24-byte header to the start of the next
section's magic bytes. The last section extends to end-of-file.

**Identifying sections by position, not field4**: field4 values vary across
files (e.g., 2, 42, 473 for section 1). Sections must be identified by index.

### Section roles

| Index | Role | Notes |
|-------|------|-------|
| 0 | Simulation command | ~39-40 bytes; contains simulation command string |
| 1 | String table + metadata | String table entries, variable metadata records, slot table |
| 2 | Name table | Variable names and other strings; field5 high bits encode first name length |
| 3 | Array directory / zeros | Scalar models are zero-filled; array models store fixed-width directory records |
| 4 | View/group metadata | Variable-length structured entries; not fully decoded |
| 5 | Dimension sets | Structured entries in array models; degenerate in scalar models |
| 6 | OT metadata | Ref stream + class codes + final values + lookup mapping records |
| 7 | Lookup data + offset table + data | Packed lookup f32 data, then offset table, then data blocks |


## Section 0: simulation command

A small section (~39-40 bytes) containing the simulation command string
(e.g., `sim bact -I`).


## Section 1: string table and variable metadata

Section 1's region contains three distinct sub-structures packed together:
string table entries, variable metadata records, and the slot table.

### String table entries (runtime descriptor dump)

Section 1 contains 16-byte cells (4 x u32 values) referenced by the slot
table. Many line up with slotted names, but a universal one-cell-per-name
directory is not established. These entries behave like volatile runtime
descriptor structs rather than stable persistent data: observed u32 values
contain absolute 32-bit RAM-address-like values in the `0x0b3xxxxx` range and
change across reruns of the same model. They are NOT a stable "has OT entry"
flag, OT index, or record back-pointer, and they cannot be used as durable
keys for name-to-record linking.

Three side observations from this region ARE stable across the non-dataset
corpus, and are useful as cross-checks during parsing:

- `section[1].data[0..4] == 124` -- a canonical base-slot offset constant
  (39/40 fixtures; WRLD3-03/SCEN01.VDF is the lone exception, carrying 188).
- `section[1].data[4..8] == offset_table_count - 1 - max_stock_ot_index`
  (40/40 fixtures). `max_stock_ot_index` is the largest OT index carrying
  class code `0x08`. On every fixture with a contiguous stock block this
  equals `offset_table_count - 1 - stock_count`; the two formulas diverge
  only on `Ref.vdf` (C-LEARN), where stocks are scattered across 8 OT
  ranges. Ref.vdf is the canary that disambiguates the correct formula:
  `d4 = 3441`, `ot - 1 - max_stock_ot = 3441`,
  `ot - 1 - stock_count = 3704`.
- `section[1].data[8..12] == count(section-6 lookup mapping records)` --
  the number of lookup-table-definition records at the tail of section 6.
  Validated on 40/40 non-dataset fixtures. This is a MORE RELIABLE signal
  than the header field at `0x70`, which is the total lookup data-point
  count (total x,y pairs), not the lookup count
  (e.g. Ref.vdf: `data[8..12] = 165` lookup records vs `header[0x70] = 251`
  total data points across them).

Additionally, `#`-prefixed internal signature names (for example
`#SMOOTH(x, 3)#` or `#LV1<SMOOTH3...>#`) participate in OT entries but do
NOT have slot-table entries (validated on `econ/base.vdf`). The hash-
prefixed region sits past the slotted prefix in the name table.

Records and the name table are bound together structurally through
`name_key` (record field[2]) rather than through these 16-byte blobs;
see structural signal #10 and the `to_results_via_records` path.

### Variable metadata records

Each record is 64 bytes (16 x u32 fields). The record array lives at a
fixed offset within section 1's data region:

```
record[k].file_offset = sec1.data_offset() + 12 + k * 64, for k >= 3
```

The 12-byte preamble and the first three 64-byte blocks are reserved as
a **header region**. Block 0 contains runtime pointer state, block 1
holds a small counter word plus a constant `0x64`, and block 2 carries a
float-1.0 marker and other metadata. They never represent variable
records and carry no sentinel pair. Blocks 3 and later are the real
records. This layout is validated across every observed simulation-result VDF
fixture (small models, edited models, WRLD3, C-LEARN). The dataset sibling
format has an analogous structure shifted into a different section.

The sentinel pair (two consecutive `0xf6800000` values at field offsets
8 and 9) is still useful for distinguishing many owner/system records from
padding records (the latter carry `f[6] = 0` and zeroed-out sentinel fields).
The parser's search **start** is no longer derived from the slot-table offsets:
it is the fixed `sec1.data_offset() + 12 + 3*64` offset. After the observed
record anchor, records are read on fixed 64-byte strides until the slot table.
The previous slot-offset-derived search start skipped large portions of the
record array in medium and large models.

Records are sparse in the sense that not every name has a corresponding
record (stdlib helpers and internal `#`-prefixed signature names often
do not), but the record array itself is dense and contiguous within the
declared region.

#### Record fields

| Name            | Index | Purpose |
|-----------------|-------|---------|
| type_flags      | 0     | Variable type/flags; 0 = padding record |
| classification  | 1     | 23 = system variable; 15 = initial-time constant; see below |
| name_key        | 2     | **Direct name-table string key.** `f[2] = (name_string_start - section2_data_start) / 4 + 7`, where `name_string_start` is the offset of the first character of the name table entry, after any u16 length prefix. Numeric values are not canonical across files: small fixtures often put system records at 9/13/17/21, while WRLD3 SCEN01 stores builtin names before `Time` and shifts INITIAL/FINAL/TIME STEP/SAVEPER to 17/21/25/29. Decode the key to a name before interpreting record kind. Stable across simulation reruns of the same model. See structural signal #10. |
| (unknown)       | 3     | Varies per variable; meaning unknown |
| (unknown)       | 4-5   | Usually zero |
| arrayed_flag    | 6     | Shape selector/key. `5` = scalar variable. `32` = arrayed variable (unambiguous when only one sec3 entry exists; in multi-shape files, 32 is a generic "arrayed" marker whose shape must be resolved elsewhere). Other nonzero values observed so far are section-3 shape keys. In `Ref.vdf`, these keys match the **previous** section-3 `index_word`; the following physical section-3 entry carries the actual shape. `0` is ambiguous: it appears on padding, dimension anchors/elements, builtins, descriptors, and some small-file reconstruction candidates, so it is not a fact-only owner shape. |
| (unknown)       | 7     | Usually zero; nonzero in some system records |
| group_or_sentinel | 8   | `0xf6800000` on many real owner/system records; zero on some padding/helper records. Non-sentinel positive values also act as compact record-group IDs in observed dimension metadata: a dimension anchor and its element records share this value (see "Record field[8] dimension groups" below). |
| sentinel_b      | 9     | Often paired with `0xf6800000` on owner/system records; otherwise a secondary small/group value in helper records. |
| sort_key        | 10    | View-local alphabetical ordering key / sort anchor. It is not global on large multi-view files, and some stock/system records carry `0`; see structural signal #8. |
| ot_index        | 11    | **OT block start index.** For arrayed variables, points to the first of N consecutive OT entries (one per subscript element). For scalar variables, points to the single OT entry. Values can exceed the actual OT count; check `ot_index < offset_table_count`. |
| slot_ref        | 12    | Byte offset into section 1 data; groups records by view/sector |
| (unknown)       | 13-15 | Not yet decoded |

Code accessors: `VdfRecord::slot_ref()` (field 12), `VdfRecord::ot_index()`
(field 11), `VdfRecord::is_arrayed()` (field 6 != 5).

#### Classification field (field 1) byte-level structure

The classification field carries a semantic variable type that is related to
but distinct from the section-6 OT class codes. The low byte and high byte
encode different signals:

| Low byte | Meaning |
|----------|---------|
| 0x08     | Associated with stocks (appears on flows with `type_flags=0x28`) |
| 0x0f     | Time-related system variable (INITIAL TIME, SAVEPER) |
| 0x11     | Dynamic non-stock (rate, flow) |
| 0x12     | (appears as high byte) Auxiliary with dependency |
| 0x17     | Constant (system or model) |
| 0x1a     | Lookup-backed variable |
| 0x80+    | Variable with special classification (0x83, 0x87, 0x89, 0x8a, 0x8f) |

The classification does NOT directly encode stock/non-stock status. For
example, `class_lo=0x87` appears for stocks in small models but for non-stock
auxiliaries in the econ model. Similarly, `type_flags=0x28` consistently
pairs with `classification=0x0808` and indicates a flow variable, but 0x0808
also appears on non-flow entries in larger models.

### Slot table

An array of N u32 values (one per name that has a string table entry), located
between the last record and the name table section. Each value is a byte
offset into section 1 data. Found by scanning backward from section 2 for the
largest structurally valid table (offsets must be unique, within section 1's
region, and at minimum 4-byte stride).


## Section 2: name table

Identified by: `field5 >> 16` gives the first name's byte length.

The first entry has no u16 length prefix -- its length comes from the header.
In simulation-result files it is `"Time"`. Subsequent entries are
u16-length-prefixed strings.
A u16 value of 0 is a group separator (skipped).

### Name categories

The name table is a **superset** of stored variables. It contains names from
many categories, only some of which have corresponding offset table (OT)
entries. Treat the table below as a classification aid, not as an owner
decision: validate a saved series through record `field[2]`, `field[11]`,
nonzero shape span, section-6 class/lookup records, and data availability.

| Category | Recognition signal | OT relationship |
|----------|-------------------|-----------------|
| System variables | Exact match: `Time`, `INITIAL TIME`, `FINAL TIME`, `TIME STEP`, `SAVEPER` | Can have direct OT entries; Time is OT[0] |
| Model variables | Slotted names passing metadata filter | Can have direct OT entries |
| Lookup table definitions | Section-6 lookup mapping records and lookupish names | Can be definitions only, emitted series, or descriptors overlapping evaluated outputs |
| Internal signatures | Prefix and suffix `#` (e.g., `#SMOOTH(x,3)#`, `#LV1<model>var#`) | Can have OT entries when runtime helpers are saved |
| Stdlib helper variables | Exact match: `DEL`, `LV1`, `LV2`, `LV3`, `ST`, `RT1`, `RT2`, `DL` | Can have OT entries when runtime helpers are saved |
| Group/view markers | Prefix `.` | No direct OT entry observed |
| Unit annotations | Prefix `-` | No direct OT entry observed |
| Builtin function names | Exact match against known set (`SUM`, `MIN`, `step`, etc.) | Usually no direct saved series; records can still carry descriptor-like claims |
| Module IO names | Exact match: `IN`, `INI`, `OUTPUT` | No direct OT entry observed |
| Module function names | Exact match: `SMOOTH`, `DELAY1`, `TREND`, etc. | No direct OT entry observed |
| Metadata tags | Prefix `:` | No direct OT entry observed |
| Single-char placeholders | `?` | No direct OT entry observed |


## Section 3: array dimension directory

In scalar models, section 3 is 104 bytes (26 u32 words), all zeros, with
`field4=0`. In array models, section 3 extends with structured data encoding
dimension metadata.

### Scalar model layout

104 bytes of zeros. `field4=0`, `field5=1`.

### Array model layout

Observed array-bearing files (`subscripts.vdf`, `Ref.vdf`) use this layout:

```
  u32 zero_prefix[25];
  u32 entry[n][27];
  u32 0;              // trailing zero word
```

The entry width is stable across both validated array fixtures:

| File | Section-3 words | Zero prefix | Entry count | Trailing zero |
|------|-----------------|-------------|-------------|---------------|
| `subscripts.vdf` | 53 | 25 | 1 | yes |
| `Ref.vdf`        | 323 | 25 | 11 | yes |

In `subscripts.vdf`, the sole record is:

```
[0, 3, 3, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 172, 0, 0, 0, 0, 0, 0, 0, 0]
```

In `Ref.vdf`, the first few records begin:

```
[59, 3, 3, ... , 1, ... , 2412, 0, ... , 1]
[86, 3, 3, ... , 1, ... , 540,  0, ... , 1]
[113, 7, 7, ... , 1, ... , 636,  0, ... , 1]
[140, 21, 7, 3, ... , 3, 1, ... , 636, 7036, ... , 2]
```

### Partially decoded entry fields

| Word(s) | Observed meaning |
|---------|------------------|
| 0       | `index_word`: **self-positional**, equals `(entry_file_offset - sec3_file_offset) / 4` (word offset of this entry within section 3). In `Ref.vdf`, the first ten records form the arithmetic progression `59, 86, 113, ... , 302` with step = 27 (the entry width in words), providing a structural checksum. The last entry has `index_word=0`. In the Ref-style multi-shape layout, record field[6] stores the previous entry's index word, and the following physical entry carries the actual shape. For example `GWP of HFC` has `field[6]=275`, and section-3 entry after `index_word=275` is `index_word=302` with flat size 9 (`HFC type`). Any numeric overlap with section-4 `index_word` values is an arithmetic coincidence (section 4 uses the same self-positional convention, see below), not a cross-section binding. |
| 1..3    | Packed shape words. One-dimensional entries duplicate the flattened size (`3, 3` -> one axis of size 3). Composite entries use `flattened_size + axis factors` (`21, 7, 3` -> two axes of sizes 7 and 3). In validated fixtures, `flattened_size = product(axis_sizes)`. |
| 10      | Packing hint. It is `1` for one-dimensional entries; for composite entries it equals the trailing axis size (`3` in `[21, 7, 3]`, `4` in `[12, 3, 4]`). |
| 11      | Small axis counter word. `0` on one-dimensional entries, `1` on the validated two-axis entries. |
| 18..19  | One section-1 slot ref per encoded axis in the validated fixtures. They resolve through the slot table (for example `172 -> "net flow"` in `subscripts`, `2412 -> "FF stop growth year"` in `Ref.vdf`). The refs are useful as axis/dimension anchors, but not yet as direct base-variable owners. |
| 26      | Encoded axis count. Validated values are `1` (one-dimensional entry) and `2` (two-axis composite entry). It matches both the number of axis-size factors and the number of slot refs. |

The `field4=32` in the section header matches the record `arrayed_flag`
value (field[6]=32) in observed single-shape array fixtures, suggesting a
shared "arrayed" signal.

### Emerging interpretation

Section 3 now looks more like a **reusable array-shape directory** than a
per-variable save list. The validated entries normalize cleanly into:

- `flat_size`: total OT span for one array instance
- `axis_sizes`: one size per encoded axis
- `axis_slot_refs`: one section-1 anchor per axis

Examples from `Ref.vdf`:

- `[3, 3]` + one slot ref => one axis of size 3
- `[12, 3, 4]` + two slot refs => two axes of sizes 3 and 4
- `[63, 7, 9]` + two slot refs => two axes of sizes 7 and 9

The resulting flat sizes (`3, 4, 6, 7, 9, 12, 21, 42, 63`) all reappear as
record-derived OT range lengths in the validated array fixtures, which ties
section 3 directly to the contiguous OT block structure used for array data.
What section 3 still does **not** give by itself is the owner binding:
multiple OT blocks can share the same flat size, and repeated sizes like 3 and
21 remain ambiguous without another signal.


## Section 4: view/sketch metadata (not a shape-owner directory)

Variable-length structured entries that reference section-1 slot table
values and encode view/sketch information. Section 4 grows proportionally
with model complexity (20 bytes in water, 88 bytes in econ, 600 bytes
in WRLD3, 1540 bytes in `Ref.vdf`).

**Section 4 is empty or terminator-only in every small fixture whose array
shape binding is independently solved**, so it cannot be the only directory
that binds base variables to shape templates. Larger models that populate it
(econ, WRLD3, `Ref.vdf`) emit view/sketch connector metadata here; we have not
found a direct variable-owner record structure in section 4.

### Structure

Begins with a 2-word zero header, followed by variable-length entries.
Each entry contains:

1. A packed word `p`
2. `refs[p_hi + p_lo]`, where `p_hi = p >> 16` and `p_lo = p & 0xffff`
3. A trailing `index_word`, **self-positional** and equal to
   `(entry_file_offset - sec4_file_offset) / 4`. The last entry in every
   observed file has `index_word = 0`, acting as a terminator.

This framing parses the validated corpus end-to-end:

| File | Entries | Distinct `p_lo` values | Distinct `p_hi` values |
|------|---------|-------------------------|------------------------|
| `water` | 1 | `{1}` | `{0}` |
| `pop` | 2 | `{1}` | `{0}` |
| `econ` | 6 | `{1}` | `{0,1}` |
| `WRLD3` | 37 | `{1,2}` | `{0,1,2,3}` |
| `Ref.vdf` | 94 | `{0,1,2,3}` | `{0,1,2,3,4}` |

The exact semantics of `p_lo`/`p_hi` are still unknown, but their **sum**
is validated as the ref count in all of those fixtures. All parsed refs
resolve to in-range section-1 offsets, and every ref is also present in
the slot table.

Apparent numeric overlap between section-4 `index_word` values and
section-3 directory indices (for example `59, 194, 248, 275, 302, 0` in
`Ref.vdf`) is an **arithmetic coincidence**: both sections encode
`index_word` as self-positional (`(entry_file_offset - section_base) / 4`).
The match does not represent a cross-section binding from sec3 shape
templates to sec4 entries.

The slot refs in section 4 consistently resolve to names in the slot
table (view markers like `.Control`, `.mark2`, unit annotations like
`-Month`, and model variable names). The structure therefore encodes
view/sketch groupings; it is NOT the shape-owner directory we were
previously hoping for.


## Section 5: ref sets / dimension hints

In scalar models, section 5 is degenerate (the next section header starts
before section 5's data offset, yielding zero region data).

In array models, section 5 contains section-1 ref-set entries in two forms
distinguished by a marker word:

**Marker=0 (single trailing axis ref):**
```
  u32 n;
  u32 0;
  u32 refs[n+1];
```

**Marker=1 (two trailing axis refs):**
```
  u32 n;
  u32 1;
  u32 refs[n+2];
```

Marker=0 entries have `n+1` refs (one trailing ref). Marker=1 entries have
`n+2` refs (two trailing refs). The current parser treats `n` as the payload
ref count before the trailing axis refs, not as a decoded dimension
cardinality. In simple array fixtures, `n` happens to match the model
dimension cardinality; in edited and large fixtures, the payload refs look
more like use-list/view/compiler refs than element descriptors.

The trailing refs often match section-3 `axis_slot_ref` values. In `Ref.vdf`,
6 of 7 unique section-3 axis refs are shared with section-5 trailing refs,
which is evidence for a section-5/section-3 bridge, but the missing 1-of-7 and
the ambiguous payload refs mean this bridge is not fully decoded.

The non-trailing refs do **not** directly name every element: in the
`subscripts` fixture they resolve to `TIME STEP`, `sub1`, `.Control`, and `0`.
The useful simple-fixture signal is that the sole non-metadata ref identifies
the dimension name (`sub1`). The element names are then recovered from the name
table by taking the next `n` non-metadata names after that dimension name
(`a`, `b`, `c` here).

This is enough to infer dimension names and element names in the simple
single-entry fixtures, and to exclude them from generic OT-participant
filtering there, but not enough to say which base variable uses which
dimension in edited or larger files.

Current extraction keeps this as a fallback path only. The stronger element
list signal found so far is in the section-1 records themselves, via record
field[8].

### Record field[8] dimension groups

Observed array fixtures use record field[8] as a group ID that links a
dimension anchor record to its element records. This is a compact, C-like
structure rather than a section-5 ref walk:

- the dimension anchor has the shared field[8] group value and usually carries
  the field[14] sentinel;
- each element record has the same field[8], `field[6]=0`, `field[10]=0`,
  `field[12]=124`, a zero-based element index in `field[11]`, and no
  field[14] sentinel;
- element records may appear out of file order, so `field[11]` is the ordering
  key.

Pinned examples:

| File | Group ID | Dimension | Element indices and names |
|------|----------|-----------|---------------------------|
| `subscripts.vdf` | 6 | `sub1` | `0:a`, `1:b`, `2:c` |
| `run_8.vdf` | 12 | `sub2` | `0:i`, `1:j` |
| `run_8.vdf` | 17 | `sub3` | `0:x`, `1:y` |
| `Ref.vdf` | 17 | `COP` | `0:OECD US` ... `6:COP Developing B` |
| `Ref.vdf` | 53 | `HFC type` | `0:HFC134a` ... `8:HFC4310mee` |
| `Ref.vdf` | 65 | `layers` | `0:layer1`, `1:layer2`, `2:layer3`, `3:layer4` |
| `Ref.vdf` | 85 | `Semi Agg` | `0:US` ... `5:Other Developing` |
| `Ref.vdf` | 100 | `Target` | `0:t1`, `1:t2`, `2:t3` |

The current `tools/vdf_xray.py` extractor uses this path to recover dimension
element lists. It labels one-dimensional blocks when the block length matches
exactly one recovered dimension, or when an attached dimension-anchor record
binds a same-cardinality catalog to the block's reusable section-3 shape
template. The latter is what distinguishes `sub2=[i,j]` from `sub3=[x,y]` in
`run_8.vdf`: the stock owner carries the anchor, and the same-template `flow`
owner inherits `flow[i]` / `flow[j]`.

For multi-axis shapes, xray uses section-3 axis sizes and record-field[8]
element lists only when each axis cardinality is unique in the recovered
dimension set. Same-size dimensions still stay numeric when no owner in that
shape template carries a unique attached anchor. Recovered dimension anchors
and element names are also excluded from the visible series-owner candidate
set; they describe array structure, not independent time-series owners.


## Section 6: OT metadata

Section 6 is the richest source of VDF-native mapping information. Its layout:

1. Optional one-word prefix
2. Leading ref stream: variable-length `u32 n_refs; u32 refs[n_refs]` entries
3. **Post-ref-stream padding region** (see below). Zero bytes on 39/40
   fixtures; populated with a fixed-width 16-byte record stream on
   `Ref.vdf` only.
4. **OT class-code array**: `offset_table_count` bytes, one per OT entry
5. **OT final-value array**: `offset_table_count` little-endian f32 values
6. **Lookup mapping records**: `13 * u32` fixed-width records, terminated by a
   single zero word

### Post-ref-stream record region (Ref.vdf only)

Between the end of the leading ref stream and the start of the OT
class-code array, `Ref.vdf` carries a 3616-byte region parsing as 226
fixed-width 16-byte records. Every other non-dataset VDF in the
corpus has exactly zero bytes here. On `Ref.vdf`:

- `226 == stock_count (209) + view_header_count (17)` exactly.
- Each record decodes as `(u32 heap_ptr, u32 w1, u32 w2, u32 w3)` where
  `w2 ∈ {1, 3, 7}` (182x/8x/36x) and `w3` is either `0` or a monotone
  u32 in `3657..4465`.
- `heap_ptr` (w0) values are `0x05eaXXXX`-range RAM addresses that
  increase monotonically in file order with a 24-byte stride between
  conceptual groups -- textbook runtime-struct-dump pattern. They are
  volatile across reruns of the same model and cannot be used as
  durable keys.
- `w1` repeats in group patterns: the first 14 records form 7 pairs
  with repeated `(w1=1817, w1=3060)`; the next 22 records form 1
  header + 7 triplets with `(w1=2809, w1=3022, w1=3081)`. The 7x
  repetition matches the 7-element `COP` dimension, strongly
  suggesting per-element records rather than per-variable.
- `w1` values partially map to section-2 (name table) byte offsets
  but with inconsistent alignment (some land on u16 length prefixes,
  others land two bytes past the prefix, inside the string payload).
  They do not pass a uniform name-table-offset interpretation.

The region's purpose is not yet decoded. Leading hypotheses: (a) a
per-stock-OT runtime-state dump (one record per stock class-coded OT
plus one per view header) that supports Vensim's UI when opening a
VDF with scattered stocks, or (b) a per-element dimension-binding
table that exists only when the model has enough array complexity
to need the indirection -- small scalar models omit it entirely.
Either way it is structurally absent on 39/40 fixtures and cannot be
a universal decoding signal.

### OT class codes

The class-code array is the primary VDF-native stock/non-stock signal. In
small and medium fixtures, stock entries form a contiguous block at OT[1..S],
followed by non-stock entries at OT[S+1..N-1]. `Ref.vdf` is the known
counterexample: stock-coded entries are split across multiple ranges.

| Code | Meaning | OT range |
|------|---------|----------|
| 0x0f | Time | OT[0] only |
| 0x08 | Stock-backed variable | Contiguous in small/medium fixtures; scattered in `Ref.vdf` |
| 0x11 | Dynamic non-stock / sometimes inline in array-heavy files | Usually non-stock range |
| 0x17 | Constant non-stock / lookup-definition value | Usually inline f32 |

Validated counts across test corpus:

| Model | Stocks (0x08) | Dynamic (0x11) | Constant (0x17) | Total |
|-------|---------------|----------------|-----------------|-------|
| water |  1            |  3             |  5              |  10   |
| pop   |  2            |  3             |  7              |  13   |
| econ  | 11            | 37             | 29              |  78   |
| WRLD3 | 41            | 174            | 81              | 297   |

`Ref.vdf` extends this code space: it contains `0x16` and `0x18` entries in
addition to `0x08/0x0f/0x11/0x17`. In the current corpus, those `0x16` and
`0x18` entries are all inline OT values rather than data-block offsets.
`Ref.vdf` also shows that `0x11` is **not** a pure "data block" code in
array-heavy files: some `0x11` entries point at blocks, others are inline.
So the small-model interpretation above is still useful, but incomplete.

### Lookup mapping records

The records at the end of section 6 identify lookup-related descriptors and
carry candidate OT indices. Each record's word[10] contains an OT index
associated with the descriptor. This is the strongest VDF-native mechanism we
have for identifying lookup-definition names, but it is not, by itself, an
emission rule: lookup-definition OTs can overlap evaluated variable outputs or
owner-record spans, and `Ref.vdf` has far more lookup records than simple
lookupish name-table entries.

**Conservative extraction rule**: on larger models (`econ`, `WRLD3`) these
lookup-record OT indices land on otherwise-unclaimed OT slots, so the
name-table order of lookupish names can be paired directly with the section-6
record order to recover missing lookup outputs. On small fixtures like
`lookup_ex`, the parsed lookup-record OTs overlap already-owned variable slots,
so generic extraction should only auto-add lookup names when their OT index is
otherwise unused. The record payload clearly carries more structure, but that
name/payload binding is not decoded yet.

`Ref.vdf` adds a second overlap form: ordinary-looking section-1 records for
graphical-function descriptors can claim OT ranges that cross real saved
variable ranges (`RS N2O` over `C AF Sequestered` / `C in Atmosphere`, and
`UN population * LOOKUP` over cumulative CO2 variables). Current extraction
uses a largest non-overlapping span selection as a diagnostic reconstruction
step, breaking equal-coverage ties by lower record sort keys. That behavior is
not yet decoded as an original Vensim format rule; it is a way to keep the
emitted result table from duplicating OT slots while we look for the direct
descriptor/owner discriminator.

Validated counts:

| Model | Lookup mapping records | Lookup definitions in name table | Match |
|-------|----------------------|----------------------------------|-------|
| econ  | 4                    | 4                                | 1:1   |
| WRLD3 | 55                   | 55                               | 1:1   |


## Section 7: lookup data, offset table, and data blocks

Section 7 contains three tightly packed sub-structures.

**Section 7 field4/field5**: unlike other sections, these header words double
as the first two f32 values of the lookup table data.

### Lookup table packing

Lookup tables are packed as contiguous f32 arrays with **no per-table headers,
counts, or separators**:

```
  [section header: 16 bytes (magic + field1 + field2 + field3)]
  [field4 = first lookup x-value (f32)]         <- data starts here
  [field5 = second lookup x-value (f32)]
  [...packed lookup f32 data...]
  [4-5 zero u32 padding]
  [OFFSET TABLE]
  [DATA BLOCKS]
```

Each table: `[x_0, x_1, ..., x_n, y_0, y_1, ..., y_n]` -- x-values as a
contiguous f32 array followed immediately by y-values. Table boundaries are
inferred from x-value monotonicity (x-values increase within a table; the
transition from y-values to the next table's x-values breaks monotonicity).

Tables appear in the same order as their lookup definitions in the name table.

### Offset table

Located between the lookup data and the first data block. An array of N u32
entries (one per OT entry, including OT[0] = Time).

Each entry is either:
- **A file offset** to a data block (value >= first_data_block_offset)
- **An inline f32 constant** (all other values, reinterpreted as f32)

### Data blocks

OT entries point to sparse time-series blocks after the offset table. The
reader must follow OT offsets rather than assuming the referenced blocks are a
gapless stream: observed files can contain padding or unreferenced bytes
between referenced blocks. Each referenced block stores:

```
  +0      2                        u16 count (stored values)
  +2      ceil(time_point_count/8) Bitmap: bit per time point
  +2+bm   count * 4                f32 values in time order
```

Block 0 is always the time series itself (fully dense bitmap).

Most files use the same bitmap width for the Time block and all other sparse
blocks: `ceil(header[0x78] / 8)`. `risk.vdf` and `risk2.vdf` prove a wider
case: `header[0x78] = 213`, but `header[0x7c] = 225`, and non-Time sparse
blocks require `ceil(225 / 8) = 29` bitmap bytes while the Time block uses
`ceil(213 / 8) = 27`. In those files, the Time block stores the saved suffix
`13..225`, while variable blocks are bitmapped against the full `1..225` grid.
Extraction must therefore decode the full block grid and sample positions
corresponding to the saved Time values. Using the shorter Time bitmap width
for every block shifts the value payload by two bytes and produces nonsense
series.

Offset-table raw zero is also class-sensitive. In `Ref.vdf`, 455 OT entries
have raw value `0`, class code `0x11`, and final value
`-1.298074214633707e+33`; these are missing/no-saved-data entries, not numeric
zero constants. Raw-zero entries with constant-like class codes still decode as
ordinary `0.0`.


## Name-to-OT mapping

The central challenge in VDF parsing is mapping names from the name table to
offset table (OT) entries. The record-to-name bridge is now decoded for the
observed simulation-result corpus: record field[2] is a section-2 string-pool
word offset plus seven, pointing at the first printable byte of the name. The
remaining hard parts are deciding which owner-like records should emit saved
series when descriptors overlap, and binding same-cardinality array axes to
their element-name catalogs.

### What is structurally determined

1. **Section-6 class codes** give VDF-native stock/non-stock classification
   for individual OT entries. In small and medium fixtures the stock-coded OTs
   are contiguous; `Ref.vdf` proves this is not universal.

2. **Section-6 lookup mapping records** give candidate OT assignments for
   lookup-related descriptors. They are direct records, but they are not a
   complete emission rule because descriptor OTs can overlap evaluated
   variables and the lookup-record count can exceed simple lookupish names.

3. **Record name key (field 2)** directly maps records to names:
   `(name_string_start - section2_data_start) / 4 + 7`, where
   `name_string_start` is the first printable byte after any u16 length
   prefix. The first name uses key `7`; it is usually `Time`, but files such
   as WRLD3 SCEN01 store builtin names before `Time`, so system-variable
   record keys must be decoded through this table rather than compared with
   hard-coded numeric constants.

4. **Stocks-first-alphabetical ordering is a reconstruction rule, not a
   decoded file-format rule.** Among non-lookup OT participants in several
   small fixtures, stocks sort alphabetically into the stock-coded OT positions
   and non-stocks into the remaining positions.
   **Caveat**: this contiguous-block property holds for 7 of 8 test files but
   breaks down in `Ref.vdf` (C-LEARN), where 209 stock entries are split
   across 8 non-contiguous ranges spanning OT[1..473], interleaved with
   dynamic entries. The scattering is driven by array variables whose element
   ranges straddle stock/non-stock code boundaries. Only 4 of 684
   record-derived OT ranges actually contain mixed stock/non-stock class
   codes.

5. **Internal variable classification**: `#`-prefixed signature names encode
   stock/non-stock in their prefix pattern (e.g., `#LV1<` = stock,
   `#DELAY3(` = non-stock). Stdlib helper names (DEL, LV1, LV2, LV3, ST,
   RT1, RT2, DL) have deterministic classification.

6. **Record arrayed_flag (field 6)** gives the decoded shape only when it is
   nonzero. `5` = scalar; `32` = arrayed (generic marker); other nonzero values
   are section-3 shape keys. In multi-shape files like `Ref.vdf`, field[6]
   takes specific section-3 self-positional values rather than the generic
   `32`; the actual shape is the following physical section-3 entry in the
   Ref-style progression. `field[6] == 0` is not an owner shape fact.

7. **Record ot_index (field 11)** gives the OT block start for each
   variable's contiguous OT entries. For arrayed variables, consecutive
   OT slots hold one element each in subscript order. In the `subscripts`
   fixture, ot_index values 1, 6, 9 correspond to the starts of the 3
   arrayed variables' 3-element blocks.

8. **Overlapping record-derived spans are descriptor/owner conflicts, not
   duplicate saved series.** `Ref.vdf` proves that owner-looking records can
   overlap. The current xray extraction keeps a non-overlapping OT partition
   as a conservative reconstruction step, but the direct C-style discriminator
   between descriptor and emitted owner records is still unknown.

9. **Attached dimension-anchor records can break some same-cardinality ties.**
   In `run_7.vdf` and `run_8.vdf`, the stock owner record has `sort_key=0`
   and gets its visible sort/order anchor from the dimension record that also
   defines the stock's element list (`sub3` in run 7, `sub2` in run 8). When
   exactly one attached anchor is a decoded dimension with the same flat size
   as the owner block, xray binds that dimension to the owner block's
   section-3 shape template. Sibling variables with the same resolved shape
   template inherit the labels, which recovers `flow[i]` / `flow[j]` in
   `run_8.vdf` without consulting the MDL.

10. **Section-3 extension in array models** provides dimension cardinality
   through each directory entry's shape words (for example `[3, 3]` for one
   3-element axis, `[21, 7, 3]` for a 7-by-3 shape).

11. **Record sort_key (field 10) as alphabetical ordering signal**. Within a
   decoded view block, sorting records by f[10] often produces case-insensitive
   alphabetical name order among user, stdlib-helper, and system names. The key
   is not global across all records in large multi-view files, and `#`-prefixed
   signature names sit outside the slotted record array in older fixtures.

   Validated on water, pop, bact, lookup_ex, model_editing runs, and the
   first alphabetical prefix of econ/base (16 names before the first
   alias breaks alignment). `f[10] = 0` is a sentinel on specific system
   records (observed on FINAL TIME and SAVEPER records for water; varies
   across fixtures). System records must be identified by decoding `f[2]`
   through the name-table key formula, because the numeric keys shift across
   files.

   **Scope limitation**: the alphabetical ordering is global across a
   single Vensim "view block" but restarts at view boundaries on large
   multi-view files (WRLD3 SCEN01 exhibits ~54 alphabetical runs after
   f[10] sort, consistent with one run per sketch view). For single-view
   or small multi-view files, the global alphabetical claim holds.

   **Alias limitation**: Vensim stores both a user-alias variable
   record and its `#FUNC(args)#` signature in the VDF. User aliases DO
   carry dedicated records (classified with `f[1] == 2065` on simple
   stdlib-call forms, see "Confirmed structural signals" below); the
   `#FUNC(args)#` signature names have no slot-table entries and do
   not participate in the f[10]-sorted record array. Pairing
   f[10]-sorted records alphabetically with visible names breaks
   because alias records frequently carry `sort_key = 0` (sentinel)
   rather than the alphabetical position of the alias name.

   Once the alias *set* is known (typically from a parsed model),
   aliases and output signatures pair up deterministically **by file
   order in the name table** -- see "Confirmed structural signals"
   below. Identifying aliases *from VDF alone* on old-style
   `#FUNC(args)#` fixtures is partially solved by the `f[1] == 2065`
   classification signal (`VdfFile::identify_potential_aliases`), which
   recovers 4/5 aliases on `econ/base.vdf` and 6/7 on `econ/risk.vdf`;
   the one-per-fixture gap corresponds to aliases with expression
   arguments (`SMTH1(a - b, t)`). `build_section6_guided_ot_map`
   resolves the full alias set via the parsed model's variable
   equations, and the new-style `#alias>FUNC#` encoding on re-saved
   files closes the gap without any model help.

   Stock sentinel records typically have sort_key=0; their sort keys
   come from attached sort anchor records (non-sentinel records whose
   ot_index falls within the stock's OT range).

   **Structural invariant**: `#` signature names lack slot-table entries
   (they appear at the tail of the name table, beyond `slot_count`).
   In econ/base, the 6 names at positions 94..99 are exactly the 6 `#`
   signatures; positions 0..93 cover `Time`, system variables, metadata,
   and slotted user/stdlib names.

9. **OT-position validation for stock classification**. Given a proposed
   name-to-block assignment, the stocks-first-alphabetical ordering
   produces expected OT positions for each name. Checking expected vs
   actual block start position uniquely determines which names are stocks.
   This eliminates the need for external stock classification when sort
   keys are available for all blocks. Validated on `model_editing/run_5`
   (scalar stock with sort_key=0) through `run_9` (arrayed stock with
   hidden SMOOTH helper).

10. **Record field[2] as deterministic record-to-name link**. Record f[2]
    is a direct word-offset key into the section-2 name table:

    ```
    f[2] = (name_string_start - section2_data_start) / 4 + 7
    ```

    `name_string_start` is the first byte of the printable name, after the
    u16 length prefix for all names after `Time`. All observed simulation
    VDF name starts are 4-byte aligned, so this is integer pointer
    arithmetic rather than a rank or heuristic score. This explains the
    previously "irregular" f[2] strides: they are just variable string
    lengths in the packed name table.

    System-variable records use the same f[2] formula, but their numeric keys
    are not globally canonical. Small fixtures often have `INITIAL TIME=9`,
    `FINAL TIME=13`, `TIME STEP=17`, `SAVEPER=21`; WRLD3 SCEN01 has builtin
    names before `Time`, so those records decode as 17, 21, 25, and 29.
    `Time` is bound to OT[0] and can also appear as an ordinary keyed record
    with `field[11]=0`.

    This direct key is validated across the small and edited fixtures,
    including `level_vs_aux/x_is_*` (where sort-key-based mapping had no
    name to attach), `model_editing/run_9` and `run_10` (where hidden
    helper blocks and reordering broke the rank-offset approximation), and
    the large WRLD3/Ref fixtures for the records whose names are present in
    the visible name table.

11. **Record `field[1] == 138` marks view headers**. Every simulation VDF file
    contains a run of records with `field[1] == 138` (also `field[0] ==
    0`, making them look like "padding" records). On small single-view
    fixtures and on WRLD3 SCEN01 / experiment the count matches the
    dot-prefix name count exactly:

    | Fixture              | `f[1]==138` count | dot-prefix count |
    |----------------------|-------------------|------------------|
    | water                | 2                 | 2                |
    | pop                  | 2                 | 2                |
    | bact, lookup_ex, ... | 2                 | 2                |
    | econ/base            | 2                 | 2                |
    | econ/risk            | 2                 | 2                |
    | WRLD3 SCEN01         | 20                | 20               |
    | WRLD3 experiment     | 20                | 20               |

    The 1:1 alignment does **not** hold universally. Two divergent cases
    have been observed and are pinned by tests in
    `src/simlin-engine/src/vdf/view_blocks.rs`:

    | Fixture        | `f[1]==138` count | dot-prefix count | Divergence cause |
    |----------------|-------------------|------------------|------------------|
    | `econ/risk2`   | 2                 | 1                | Edited file dropped the `.Control` dot-name but the header record survived |
    | `Ref.vdf`      | 17                | 69               | C-LEARN nests modules, so many dot names describe sub-groups (`.Agriculture.Loop1`) that share a parent view's header record |

    Between two consecutive view-header records lies one view's worth of
    variable records. On 1:1-aligned fixtures, the group sizes match
    `names[dot[i] + 1 .. dot[i + 1]]` on every non-terminal view block.
    (The final `.Supplementary`-style view is the one exception because
    `#` signature names and stdlib tail records sit past the slot
    boundary.) The public API exposes two helpers:
    `VdfFile::record_view_groups()` returns the groups, and
    `VdfFile::record_view_groups_with_diagnostics()` returns the groups
    plus a `ViewBlockDiagnostics` struct listing unmatched headers and
    unmatched dot names so callers can detect divergent fixtures
    without silently dropping them.

12. **Partial shift-by-one record-to-name OT link**. For file-order record-to-name
    pairs `(rec[i], name[i])` (where variable records pair with non-dot
    names and view headers pair with dot-prefix names), the OT index of
    `name[i+1]` equals `rec[i].field[11]`. Each record's `field[11]`
    identifies its file-order successor's OT slot, not its own.

    The only special case is `Time`, which always lives at OT[0]; because
    the first variable record already carries the OT for `name[1]`, the
    Time binding is implicit in the shift-by-one rule. Records with
    `field[11] == 0` for a non-Time successor are treated as sentinels
    meaning "no OT entry for that name". **Known imprecision**: the
    sentinel over-filters on WRLD3 SCEN01, where 59 successors have
    f[11]==0. Most are metadata/unit/stdlib-helper names without OT
    entries, but a handful are real variables (`unit agricultural input`,
    several `#SMOOTH3(...)#` signatures) that are silently lost through
    this path. Closing this gap requires an additional structural signal
    that separates aliases/metadata from real variables; the
    `FileOrderPairDiagnostics` returned by `build_file_order_pairs` lets
    callers detect the lost names.

    For arrayed records (paired name's record has `field[6] != 5`),
    the OT slot from the shift-by-one link is expanded to `N`
    consecutive slots where `N` is the `flat_size` of the matching
    section-3 shape entry, matching the pattern in
    `to_results_via_records`. Element labels use the `name[i]`
    convention (0-indexed).

    Validated on water, pop, consts, and small single-view fixtures. On WRLD3
    SCEN01 / experiment, time-series equality against
    `build_section6_guided_ot_map` agrees on only ~40-50 of ~260 overlapping
    names, so the link is a useful partial signal there, not a solved mapping.

    On compilation-order files with interleaved dimension-element
    records (subscripts.vdf) and on edited/re-saved files (risk2.vdf,
    `Ref.vdf`), the file-order pairing drifts on unmatched dot names and
    orphan headers; the path surfaces a partial mapping in those cases.
    The direct f[2] string-key record path remains more robust on those
    fixtures.

    Exposed as `VdfFile::to_results_via_file_order_records()`.

### VDF-structural path (stock classifier required)

`VdfFile::to_results_with_stock_classifier(is_stock)` uses only VDF structural
data plus a stock-classification function:

1. Header offsets (0x58, 0x5C, 0x60) for direct access to class codes, final
   values, and offset table -- no scanning required
2. Name table filtering (`filter_ot_candidate_names`): excludes metadata
   (`.`, `-`, `:`, `#`, `"` prefixes), builtins, numeric strings, stdlib
   helpers, single non-alphanumeric chars, and inferred section-5 dimension
   bookkeeping names
3. Excess candidate trimming: when candidates exceed OT capacity, lookupish
   names (containing "lookup", "table", "graphical function") are removed
4. Section-6 stock boundary reconciliation: promotes/demotes names between
   stock and non-stock groups to match the authoritative section-6 count
5. Stocks-first-alphabetical ordering into OT[1..S] and OT[S+1..N-1]

Validated on consts, lookups, water, pop, bact models -- produces identical
time series to the model-guided path.

### Conservative fully VDF-only path

`VdfFile::to_results()` now exposes the subset of fully VDF-native mapping that
is actually forced by the file structure:

1. Parse the visible scalar candidate names from the VDF name table
2. Use section-6 class codes to recover the authoritative stock count `S`
3. Trim standalone lookup-definition names when they are the only excess
4. Treat system variables as deterministically non-stock
5. Succeed only when the remaining unresolved visible names must be **all
   stock** (`remaining == S`) or **all non-stock** (`S == 0`)

This is intentionally conservative. It does **not** guess when multiple visible
names could legally occupy the stock-coded OT slots.

Validated behavior:

- `level_vs_aux/x_is_stock.vdf` and `level_vs_aux/x_is_aux.vdf` now round-trip
  to `Results` with no model input
- `water`, `pop`, `bact`, `consts`, `lookups`, and `sd202_a2` still fail with
  an explicit ambiguity error because the VDF tells us **how many** stock OTs
  exist but not **which visible names** own them
- `econ/*.vdf` still fails earlier with candidate-count mismatches because
  hidden stdlib participants / aliases consume OT slots that are not yet fully
  reconstructible without model help

### Model-guided path (full project required)

`VdfFile::build_section6_guided_ot_map(project, model_name)` uses:
1. Model-based stock classification
2. Section-6 stock counts and OT class codes
3. VDF name table filtering (builtins, metadata, module names removed)
4. Sorted candidate groups for the unresolved names
5. SMOOTH/DELAY alias resolution via compiled module structure

### Validated partial approaches (not yet general)

These approaches produce useful results on selected fixtures. They are useful
for validating structural hypotheses, but they are NOT the general solution.
Several fallback paths still rely on offset-range scanning, non-overlap
selection, and OT-position scoring; those are reconstruction techniques -- a
90s C program would not have used them. The direct record key below is a
confirmed format mechanism, but record ownership semantics are still only
partially decoded.

1. **f[2] string-key record-to-name mapping.** Record field[2] (the
   `name_key`) encodes the section-2 string-table word offset plus seven,
   giving a direct record-to-name link with no sort-rank approximation. The
   Rust `to_results_via_records` and Python `_try_f2_name_key_mapping` paths now
   use this decoded key directly. The primary gating in this path is
   structural:
   a record is dropped from fact-only extraction when its `field[6] == 0`
   (ambiguous no-shape/descriptor metadata) or its `field[11]` falls outside
   the offset-table range. Name category is mostly
   not filtered -- stdlib helpers (`DEL`, `LV1`, `LV2`, `LV3`, `ST`, `RT1`,
   `RT2`, `DL`), internal signatures (`#SMOOTH(x, y)#`), metadata markers
   (`.mark2`, `-months`), and Vensim builtin tokens (`MIN`, `SMOOTH`,
   `DELAY1`) all retain their OT claims when a record legitimately points to
   them. The current exception is a lookup-like graphical-function definition
   pointing at a stock-coded OT that is also claimed by an evaluated variable;
   that definition is skipped so the evaluated variable can own the series.
   Vensim writes helper records deliberately, so broad filtering drops real OT
   data. Callers that want a cleaner user-facing symbol table can strip these
   columns from the resulting `Results`.
   Remaining limitations are no longer rank/offset alignment; they are
   owner interpretation problems when multiple records share an OT start
   (for example graphical-function definitions vs evaluated variables), and
   element-label recovery for arrays whose dimension metadata remains
   ambiguous.

2. **OT-position validation for stock classification.** Given a proposed
   name-to-block assignment, the stocks-first-alphabetical ordering produces
   expected OT positions. Comparing expected vs actual uniquely determines
   stock classification. This is useful as a validation signal, but it
   depends on already having a correct name-to-record assignment. It also
   has not been tested on large models where the stocks-first property
   breaks down (e.g., `Ref.vdf`'s non-contiguous stock ranges).

3. **Dimension element-list recovery.** Array elements occupy contiguous OT
   blocks in subscript order. Section 5 gives partial cardinality/axis hints,
   but the stronger decoded element-list path is record field[8]: dimension
   anchors and zero-based element records share a group ID. This recovers
   `sub1=[a,b,c]` in `subscripts.vdf`, `sub1`/`sub2`/`sub3` in the edited
   fixtures, and six non-singleton dimensions in `Ref.vdf` (`Aggregated
   Regions`, `COP`, `HFC type`, `layers`, `Semi Agg`, `Target`). Element
   labels are applied only when the block/axis cardinality uniquely identifies
   a recovered dimension.

### The core unsolved problem

The large-model name-to-OT link has two partial decoders with complementary
failure modes. `VdfFile::to_results_via_file_order_records()` uses the
`field[1] == 138` view-header marker (signal #11) and the shift-by-one
`field[11]` link (signal #12) to recover some WRLD3 mappings, but agreement
with the model-guided path is partial rather than decisive. The record-key path
uses the direct `field[2]` string-table key (signal #10) and is the more robust
path on small fixtures, `subscripts.vdf`, edited/re-saved fixtures, and files
where dim-element names interleave with variable names. Neither is universally
correct.

Remaining gaps:

1. **Trailing `.Supplementary` / `#`-signature region.** The last
   view block (`.Supplementary` on WRLD3) has extra record/name entries
   for internal stdlib helpers and `#` signature names past the slot
   boundary. Record-count and name-count diverge here (e.g. SCEN01: 68
   records vs 53 names; experiment: 43 records vs 66 names). The
   shift-by-one link still applies for the first ~8 entries of the
   block but breaks once the `#`-signature region starts. The
   remaining variables in this block may need a separate handling.

2. **SCEN01-style "zeroed" placeholder records.** SCEN01 carries 21
   records with `f[2]==0 AND f[6]==0 AND f[11]==0` ("zeroed"), all
   inside `.Supplementary` (the final view block), with `f[0]` values
   in the ghost range {12324, 12328, 13352} paired with `f[1]` in
   {8, 17, 255}, or `f[0]==32` paired with `f[1]==255`. These are
   slot-less placeholders, not variable records. Dropping all 21
   zeroed records realigns the SCEN01 Supplementary pair walk: record
   count 419 - 21 = 398, vs 404 slots, and the post-filter pairing
   recovers `unit population`, `unit agricultural input`, and 4 other
   previously-sentinel-lost real variables (290/305 -> 296/305 against
   the guided map reference).
   Naively filtering all `f[2]==f[6]==f[11]==0` records across the
   corpus is **too aggressive**: `bact/euler.vdf` carries 2 similar
   zeroed records (`f[0]=32, f[1] in {143, 255}`) in the middle of
   its main view block, and dropping them mis-shifts the subsequent
   pair walk and loses the `outflow -> OT 1` binding that the
   baseline accidentally recovers via chain succession. Filtering
   only ghost-range (`f[0] in [12000, 17000]`) zeroed records drops
   15/21 on SCEN01 without touching euler. Full generalization
   remains open; the workable rule for now is: "drop zeroed records
   that sit inside a view block whose record-count exceeds its name-
   count after also dropping them" (i.e. still a per-fixture signal
   rather than a global rule). SCEN01 is the only large fixture
   currently exhibiting this pattern.
   The 15 residual SCEN01 losses (after ghost-R1) include trailing
   `#`-signature names that have no record at all and a pre-existing
   `assimilation half life mult table` lookup-name bug that also
   affects experiment.vdf.

3. **Ambiguous same-cardinality axis labels.** Section-3
   shape templates plus record field[8] dimension groups can label axes when
   cardinalities are unique, or when an attached dimension anchor uniquely
   binds a same-cardinality catalog to the reusable shape template. Same-size
   dimensions still need another binding when no owner using that shape carries
   such an anchor (`Ref.vdf` has multiple cardinality-3 dimensions). Base-name
   ownership for the previously broken `Ref.vdf` examples is now handled by
   field[2] name keys, Ref-style predecessor shape codes, and non-overlapping
   owner-span selection; the remaining gap is choosing the right axis labels
   when dimensions share cardinality and no template-local anchor is present.

4. **Lookup-record payload structure.** Section-6 lookup records
   identify lookup definitions and their OT indices, but the internal
   payload is not fully decoded. When parsed lookup-record OT indices overlap
   already-owned variable slots, extraction emits the already-owned series and
   does not add a duplicate lookup-definition column.

5. **C-LEARN (`Ref.vdf`) view-grouping.** `Ref.vdf` has 69 dot-prefix
   names but only 17 `field[1] == 138` records. The view-header-per-
   dot-prefix rule that holds on small fixtures and on WRLD3 breaks
   here, probably because C-LEARN's module nesting surfaces sub-group
   dot-prefix entries (e.g. `.Agriculture.Loop1`) that share their
   parent view's header record rather than owning their own. The
   `ViewBlockDiagnostics` returned by
   `record_view_groups_with_diagnostics` surfaces these unmatched dot
   names so callers can avoid silent misalignment.

6. **Re-saved/edited files with orphan view-header records.** Files
   edited to delete a trailing view (e.g. `econ/risk2.vdf` dropping
   `.Control` from the name table) keep the view-header record and
   emit a header count greater than the dot-prefix count. Pinned by
   `test_record_view_groups_divergent_fixtures`.


## Appendix: reverse-engineering notes

### Hypotheses tested and ruled out

These approaches were investigated and found unreliable for the name-to-OT
mapping problem. The list is organized by target region; within each region
the most recent findings come first.

#### Claims about records

- **Record field[2] as a sort rank or simple name index**: refuted. The old
  `f[2]`-sort + `(slot_count - record_count)` offset approximation pairs
  records with names correctly on many name-ordered fixtures, but fails on
  edited/re-saved files and tiny fixtures with no sort anchor. The confirmed
  formula is not a rank: it is
  `(name_string_start - section2_data_start) / 4 + 7`.

- **Record sort_key (field 10) as a GLOBAL alphabetical key**: sorting ALL
  non-padding records by `f[10]` on compilation-order fixtures produces
  names in multiple short alphabetical runs (~54 runs in WRLD3 SCEN01, each
  8-36 members). f[10] is alphabetical WITHIN view blocks (signal #8) but
  restarts at view boundaries; it is NOT a global key. The earlier
  Kendall's-tau result (0.46) reflects the same view-local structure.

- **Record slot_ref (field 12) as a per-name pointer**: previously
  misdescribed as "sparse record coverage"; post record-finder fix every
  slotted name has a record (see `to_results_via_records`). The actual
  reason f[12] cannot drive name mapping is that it is a view/sector
  anchor with 2-30 unique values per file (2 on water/pop, 30-31 on
  WRLD3), and records with the same f[12] are NOT contiguous in file
  order. Within a f[12]-anchored group, f[10] is not monotonic on large
  fixtures (only 7 of 31 groups strictly increasing on wrld3_experiment).

- **Record slot_ref (field 12) inverts to slot_table[rank]**: refuted.
  Of WRLD3's 404 slot_table entries, 10 point to the pre-record header
  region and 394 point INTO record memory at 16-byte-aligned offsets
  {0, 16, 32, 48} within 64-byte records. Because all four positions
  within a single record share one `f[11]`, at most 1 of the up-to-4
  slot_table entries landing in one record could carry the correct OT
  for that record's variable. Empirical hit rate 0-0.7% on
  compilation-order fixtures. The slot_table entries are 16-byte runtime
  string-descriptor cells whose POSITION happens to fall in record
  memory when slot count exceeds the 12-entry pre-record pool, but the
  landing position has no semantic meaning.

- **Record undecoded fields (f[3], f[7], f[13..15]) as name-rank holders**:
  f[13] and f[15] are 0 on >=99% of records; f[14] is usually the sentinel
  `0xF6800000`. f[3] and f[7] vary but zero correlation with rank on
  econ/WRLD3 (exhaustive per-field test across 2119 records on 9
  fixtures yielded 2 coincidental matches).

#### Claims about sections 1 / 2

- **Section-1 16-byte per-name entries as durable keys**: the entries are a
  volatile runtime-descriptor-like region containing absolute 32-bit
  RAM-address-like values (`0x0b3xxxxx`) and sequence numbers. The raw bytes
  change across reruns of the same model, so they cannot serve as a stable
  "has OT entry" flag, OT index, or record back-pointer. Three side
  observations from the same region ARE stable (documented under "Section 1
  string table entries"):
  `data[0..4] == 124`, `data[4..8] == OT_count - 1 - max_stock_ot_index`,
  and `data[8..12] == count(section-6 lookup mapping records)`.

- **Section-1 bytes 8..44 (36-byte undecoded header slice) as a structural
  counts/offsets table**: contains RAM pointer residue on fresh-simulation
  files and tight-packed small ints on re-saved files (`econ/base.vdf`,
  `WRLD3-03/SCEN01.VDF`). The small ints (e.g. `[77,78,79,38,39,40,41,42]`
  on WRLD3 SCEN01) do not equal OT count, record count, section offsets,
  or any permutation of OT indices; they are reallocated save-allocator
  RAM descriptor indices, not file-structural metadata.

- **Gap between the last record and `slot_table_offset` as a
  compilation-order-to-name-order translation table**: on `WRLD3
  experiment.vdf` the gap is zero bytes, so no such region exists there.
  On `WRLD3 SCEN01.VDF` the 60-byte gap decodes to 15 u32s of
  section-1-internal byte offsets that do not correspond to slot_table
  entries, f[12] anchors, or record file offsets.

#### Claims about section 4

- **Section 4 as the sole shape-owner directory**: section 4 is empty or
  terminator-only in small fixtures whose array shape binding is solved
  elsewhere, so it cannot be the only structure that binds base variables to
  section-3 shape templates. Apparent numeric overlap between sec3 and sec4
  `index_word` values is an arithmetic coincidence: both encode `index_word`
  as self-positional (`(entry_file_offset - section_base) / 4`). Section 4
  carries view/sketch connector metadata; no direct variable-owner record has
  been decoded there.

- **Section 4 as an `(axis_slot_ref, dim_name_slot_ref)` binding**: refuted.
  Of the 18 declared dimensions in C-LEARN (`Ref.vdf`), only `COP` is ever
  referenced from section 4 under direct slot->name mapping, and only in
  8 of 94 entries. Each of those entries pairs `COP` with an unrelated
  variable slot (`FF change target year`, `UN population HIGH`, etc.),
  consistent with sketch-connector metadata, not a clean dim-axis binding.
  The other 17 dimensions (`Target`, `HFC type`, `Semi Agg`, `layers`,
  `lower`, `upper`, `bottom`, `scenario`, `Aggregated Regions`, `Developing A`,
  `Developing B`, `COP Developed`, `COP Developing A`,
  `COP Remaining Developing`, `set targets`, `tNext`, `tPrev`) do not
  appear as refs in any section-4 entry.

#### Claims about multi-dim element naming (sec3 <-> sec5 binding)

These investigations target: for an arrayed variable with 2-D shape like
`[COP, Target]`, Vensim's VDF shows element names (`OECD US, t1`, `OECD EU,
t1`, ...) even without the MDL. The element names must be in the file.
None of the following signals, however, encodes a deterministic
`axis_slot_ref -> dim_name -> element_list` binding.

- **Candidate A: sec4 as `(axis_slot_ref, dim_name_slot_ref)` binding**:
  refuted (see above).

- **Candidate B: sec5 `n` as dim cardinality, 1:1 pairing to dims**: in
  `subscripts.vdf` (1D), the single sec5 entry's payload contains `sub1`
  (the dim name) and `n=3` matches the cardinality. In `Ref.vdf`
  (C-LEARN, 18 dims), sec5 has exactly 18 entries with `n` values that
  sort-match the 18 declared cardinalities, which is suggestive -- but
  0 of 59 sec5 payload refs resolve to any dim name under direct slot
  mapping. Every sec5 payload ref resolves to a VARIABLE name instead.
  The entry ordering within sec5 does not correspond to MDL declaration
  order, alphabetical dim-name order, or name-table appearance order of
  dim names, so there is no deterministic way to pair a given sec5 entry
  with a specific dim. This is consistent with sec5 being a variable-group
  (by view/axis) catalog rather than a dim descriptor table.

- **Candidate C: element names follow the dim name contiguously in the
  name table**: refuted on `Ref.vdf`. Of 8 tested dims, only `COP`
  (elements at name-table indices 71..77, immediately after the dim name
  at 62) has contiguous elements. `HFC type`'s 9 elements are scattered
  across name-table indices 163, 859, 861, 863, 865, 867, 868, 870, 872
  (span 709). `scenario`'s 3 elements span indices 90, 1114, 1115. Most
  dims have their elements NOT adjacent to the dim name in the name table.

- **Candidate D: sec3 word 10 / word 11 / unused words as secondary
  dim-identity key**: refuted. Word 10 is a packing hint equal to the
  trailing axis size (or 1 for 1-D). Word 11 is a small axis counter (0
  or 1). Unused words 4..9, 12..17, 20..25 are zero on every validated
  sec3 entry across all fixtures.

- **Candidate E: section 0 as a dim directory tail**: refuted. Section 0
  is 132-140 bytes across all observed fixtures (small, medium, and
  array-heavy), containing only the simulation command string and zero
  padding. No trailing array-dim directory.

- **sec3 axis_slot_refs as direct pointers to dim names**: refuted. In
  `Ref.vdf`, the size-7 axis slot `636` resolves under direct slot_table
  mapping to the scalar `watt per J s`. The size-9 slot `1852` resolves
  to `Sea Level Rise` (a 3-element scenario-dim variable, not a 9-dim
  variable). No axis slot's direct-mapped name corresponds to the dim
  itself or to any variable of matching cardinality.

- **sec3 axis-anchor record's f[2] as the first-element name-table index**:
  refuted. For the size-7 axis in `Ref.vdf`, the anchor record (rec[9]
  at byte offset 636) has `f[2]=72` and `names[72..78] = ['OECD EU',
  'G77 China', ..., 'UN population LOW LOOKUP']`. The actual 7 COP
  elements start at `names[71]='OECD US'`, so f[2] is off-by-one, and
  the resulting slice still includes a non-element `UN population LOW
  LOOKUP` at the tail. Other anchor records' f[2] values (`237=layers`,
  `189=CH4 per C`, `323=Developing B stop growth year`) produce
  mismatched or unrelated name slices. The f[2]-pointer hypothesis does
  not generalize.

- **sec3 anchor record's 16-byte substructure as a dim descriptor**:
  refuted. Dumping the 16 bytes at each `axis_slot_ref` in `Ref.vdf`
  yields diverse u32 tuples: `(0, 0, 0, 1)` for slot 636, `(8460, 0,
  1008981770, 0)` for slot 1852, `(17, 0, 0, 5)` for slot 3436, etc.
  Some values look like float constants (`0.01`, `0.1`), others are
  small ints, none encode a cardinality or a name-table pointer. Two
  "dim-flavored" anchor records (rec[9] with `f[1]=143, f[5]=8028,
  f[6]=0, f[11]=0` and rec[28] with `f[1]=135, f[5]=508, f[6]=0,
  f[11]=0`) have a distinctive shape, but the pattern does not hold for
  other axis anchors (rec[8], rec[37], rec[53], rec[70], rec[109]
  are ordinary variable records).

- **sec5 entry index -> dim mapping by tested ordering rules**: refuted for
  the rules tested so far.
  `Ref.vdf` has exactly 18 sec5 entries and 18 MDL-declared dims, with
  IDENTICAL cardinality multisets `[1,1,2,2,2,2,2,3,3,3,3,3,3,3,4,6,7,9]`.
  But none of the tested ordering rules (file order, MDL declaration order,
  name-table position order, alphabetical, reverse alphabetical, or the
  structural fields we have decoded so far) pairs the 18 sec5 entries to the
  18 dims. Current evidence fits sec5 entries being per-(view, shape) variable
  catalogs rather than simple dim descriptors, but the exact role is still
  unresolved.

- **sec5 payload refs as dim-owner or element-owner slot values**:
  refuted. Zero of the 46 distinct sec5 payload refs on `Ref.vdf` equals
  any of the 18 dim-name slot values (`{316, 524, 1052, 1724, 1980,
  2028, 5164, 9036, 10220, 10508, 10796, 11484, 12332, 13068, 13084,
  13868, 16784, 18464}`), and zero equals any of the 35 element-name
  slot values. Sec5 payload refs are slot-table values resolving to
  VARIABLE names (mostly scalars or unrelated-dim arrayed variables);
  they do not reference dim metadata at all.

- **sec3 axis_slot_ref variable's MDL subscripts reveal the dim**:
  refuted. Axis anchor variables have subscript shapes that do NOT
  match the axis cardinality. In `Ref.vdf`: size-7 axis at slot 636 =
  scalar `watt per J s` (no subscripts); size-9 axis at slot 1852 =
  3-dim variable `Sea Level Rise[scenario]`; size-6 axis at slot 4508 =
  3-dim variable `FF change target year Aggregated[Developed Countries]`.
  Vensim's compiler chose these as memory-layout anchors for reasons
  unrelated to their dim content.

- **Pre-OT sec7 tail past lookup X/Y arrays as dim descriptor table**:
  refuted on `Ref.vdf`. The sec7 pre-OT region (297508 bytes = 74377
  words) is fully accounted for by lookup f32 X-axis arrays and the
  matching Y-value arrays (sec6 lookup records' `word[5..6]` index
  pairs). 165 lookup records cover roughly half the region as X-axes;
  the remainder is Y-values paired with each X-axis. No hidden dim
  descriptor space in pre-OT.

- **R5b 226-record region as dim descriptor**: refuted. The R5b
  identity `226 = 209 stocks + 17 view headers` implicates stock/view
  structure, not dim structure. Partition tests (group records by
  classifier-pattern and check whether group sizes sum to the 18 dim
  cardinalities) FAIL: R5b's natural groups are 14-record (7 pairs) and
  22-record (1 header + 7 triplets) blocks, not per-dim blocks. R5b
  is also empty on 39/40 fixtures, so even if it encoded dim data on
  Ref.vdf it cannot be a universal decoding signal.

Practical consequence after the field[8] discovery: section 5 should no
longer be treated as the only dimension-element source. `tools/vdf_xray.py`
now recovers element lists from record field[8] groups and uses section-5 only
as a fallback for the old single-entry layout. Multi-dim labels are now
possible when section-3 axis cardinalities uniquely identify recovered
field[8] dimension groups. The remaining hard problems are (a) same-size
dimension disambiguation and (b) correct base-name-to-OT ownership on large
multi-view files like `Ref.vdf`.

**Current unresolved direction:** We have ruled out several simple sec3/sec4/
sec5/sec6/sec7/R5b/name-table interpretations on `Ref.vdf`, but this is not
proof that the mapping is absent from the VDF. The most likely possibilities
are that (1) one of the known regions is framed incorrectly, (2) an entry
contains a compressed or indirect key rather than a direct name/slot ref, or
(3) dimension ownership must be reconstructed from formula/reference streams.
Concrete remaining experiments include a whole-file scan for dim-name and
element-name slot values in fixed-width tuples, checking for 16-bit or
delta-coded variants of those refs, and mining section-6 ref-stream groups for
dim-name occurrences attached to arrayed variable records.

New pinned evidence:

- `Ref.vdf` record field[8] groups recover the non-singleton dimensions
  `Aggregated Regions`, `COP`, `HFC type`, `layers`, `Semi Agg`, and `Target`;
  see `test_record_field8_recovers_dimension_element_groups` in
  `tools/test_vdf_xray.py`.
- In the edited `run_7`..`run_10` fixtures, an attached dimension-anchor
  record binds a same-cardinality element catalog to the reusable section-3
  shape template. This labels sibling owners with the same template
  (`flow[x/y]` in `run_7`, `flow[i/j]` in `run_8`..`run_10`) without using
  the MDL.
- `Ref.vdf` section-6 ref-stream entries directly reference dimension names in
  short entries (for example `lower`, `upper`, `Target`, `layers`, `set
  targets`, `Semi Agg`, `COP Developed`, and `COP Developing A`). These refs
  do not align naively by entry index with records or name-table order, but
  they are too structured to ignore. See
  `ref_vdf_section6_ref_stream_contains_direct_dimension_refs` in
  `src/simlin-engine/tests/vdf_multidim.rs`.

#### Claims about section 6

- **Section-6 leading refs as a save list**: Resolved refs include model
  variables, unit annotations (e.g., `-Month`), view markers (`.Control`),
  builtin function names (`SMOOTH`, `DELAY1`, `if then else`), system
  variables, and stdlib helpers. The mix means the ref stream is NOT a
  clean variable save list. WRLD3's 342 entries correlate with the
  candidate population but cannot be used directly for participant
  filtering without further classification of entry types.

- **Section-6 bytes after the lookup-mapping terminator as a per-OT name
  array**: measured remainder is zero bytes on every tested fixture. The
  lookup_mapping terminator zero-word ends exactly at
  `sec6.region_end`; there is no trailing data.

#### Claims about view-block structure

- **Hypothesis F: f[12] as view anchor + alphabetical within group**:
  refuted. The algorithm (group records by f[12], sort each group by
  f[10], pair with names[anchor_rank+1 .. next_anchor_rank)) reaches
  0.7-4.8% agreement on compilation-order fixtures -- strictly worse
  than `to_results_via_records`. The structural premises (records with
  same f[12] are contiguous; all f[12] values are valid slot_table
  offsets; f[10] monotonic within a group) all fail on large files.

- **Hypothesis G: f[10]-sorted records pair 1:1 with
  alphabetically-sorted visible names**: refuted at global scope. f[10]
  IS alphabetical within view blocks but restarts at view boundaries:
  WRLD3 SCEN01 exhibits 54 alphabetical runs (sizes 8..36) after
  f[10]-sort, consistent with one run per sketch view. Global
  alphabetical sort is not the right shape for the mapping on
  compilation-order fixtures. **Superseded**: the actual view-block
  partitioning comes from `field[1] == 138` marker records (signal
  #11), and the within-view record-to-name pairing comes from the
  shift-by-one `field[11]` link (signal #12). The `f[10]` alphabetical
  runs are a secondary ordering within some but not all views, and
  are not needed for the name-to-OT mapping.

- **Assumption: the name-to-OT mapping on compilation-order files
  universally requires a model**: partially refuted. The `field[1]
  == 138` view-header markers plus the shift-by-one `field[11]`
  link give a VDF-native mapping path
  (`to_results_via_file_order_records`) that improves some WRLD3
  mapping cases, but agreement with `build_section6_guided_ot_map`
  is still partial and fixture-pinned. It is useful evidence for a
  direct on-disk owner order, not a completed general decoder.
  `build_section6_guided_ot_map` remains the reference mapping on
  small fixtures (where its alphabetical-within-class assumption
  matches Vensim's output) and on arrayed fixtures (`subscripts.vdf`):
  the file-order path is imperfect on those because the dimension-
  element names and compilation-order artefacts shift the 1:1
  pairing. The two paths are complementary; neither is uniformly
  more correct than the other across the full fixture corpus.

#### Alias-identification hypotheses (ruled out April 2026)

The task "decode the alias-to-signature link" swept five candidate
mechanisms for a VDF-internal signal that marks a slotted user name as an
alias (as opposed to a regular variable). None of them produced a
deterministic alias-bit; documented here so a future investigator does
not re-do the same searches.

- **Candidate A: per-slot 16-byte pointee as alias marker**. Each slot
  in `slot_table` points to a 16-byte cell in section-1 data; many cells
  are overlaid on record memory, so the cell content is just a slice of a
  nearby record's fields. Across `econ/base.vdf`, `econ/policy.vdf`,
  `econ/risk.vdf`, and WRLD3 SCEN01/experiment, the five alias cells have
  no common structural content: they land at distinct record offsets
  (0, 16, 32, 48) and hold whatever record bytes happen to be there. In
  particular, none of the four words in an alias cell equals that alias's
  target OT. Hit rate 0/5 on econ/base. Also ruled out: one of the four
  words being an OT index, a `#` name index, or a section-6 ref-stream
  index.

- **Candidate B: section-6 leading ref stream as alias directory**.
  Parsed the 79 entries on `econ/base.vdf` and the 342 on WRLD3 SCEN01
  with entry widths 1-4 refs. No entry pairs an alias slot ref with a
  target signature slot ref (the `#` signatures past `slot_count` have no
  slot-table entries to begin with). The entries resolve to a mix of
  variable names, view markers, unit annotations, and builtin names, as
  the pre-existing ref-stream analysis reported.

- **Candidate C: pre-record 16-byte cells as alias table**. Section-1
  data bytes 12..204 (preamble 12 + three 64-byte "header blocks") hold
  12 cells of 16 bytes. On `econ/base.vdf` ten of those cells are
  claimed by slot entries (offsets 44, 60, 76, 92, 108, 124, 140, 156,
  172, 188); the slotted names at those offsets are a mix of stdlib
  helpers (`DEL`, `DELAY1`, `SMOOTH`), unit annotations (`-dmnl`,
  `-months`), and regular user variables (`base housing supply`). No
  subset of those ten cells coincides with the 5-alias set, and the 16
  bytes at each cell vary unstructured across re-saved fixtures
  (RAM descriptor residue on `policy.vdf`/`mark2.vdf`).

- **Candidate D: hidden alias table as a fixed-width (u32, u32) pair
  array**. Checked every byte range marked "undecoded" or "varies" in
  the docs -- section-0 tail, section-1 bytes 8..44, section-5 scalar
  degenerate region, section-6 prefix-word region -- for a sequence of
  `(u32, u32)` pairs whose first word could be an alias slot ref and
  whose second word could be a target OT or signature name index. No
  such pair stream exists on any tested fixture.

- **Candidate E: section-6 lookup mapping records generalized to stdlib
  aliases**. The 13-u32 fixed-width records at the end of section 6
  carry lookup-table definitions (1:1 with lookup names; word[10] holds
  the OT). On `econ/base.vdf` the 4 lookup records account for exactly
  the 4 lookup-table definitions in the model and do not extend to
  aliases; no similar record block exists for stdlib outputs.

The **file-order pairing** of aliases with output signatures (see
"Confirmed structural signals") still works once the alias set is known;
what remains unsolved is identifying aliases from VDF alone on old-style
fixtures. The new-style `#alias>FUNC#` encoding closes the gap on
re-saved files from current Vensim builds.

#### Name-based claims

- **Name-based lookup heuristics**: names containing " table" were initially
  assumed to lack OT entries, but section-6 lookup mapping records proved all
  lookup definitions have OTs.

- **First-fit OT allocation using model offsets**: produced unreliable
  mappings on econ/WRLD3. Deleted in favor of stocks-first-alphabetical.

### Known pitfalls

- **Name table builtins**: Vensim embeds function names (SUM, MIN, step)
  alongside model variables. These must be filtered.

- **Offset table constant ambiguity**: f32 constants like `4.8e9` produce u32
  values within file-offset range. Distinguish by comparing against
  `first_data_block_offset`.

- **Mixed control/model record groups**: Some VDFs mix system records
  (classification=23) and model records in the same slot_ref group. Filter
  per-record, not per-group.

- **Section 5 degenerate**: In scalar models, section 5's next header starts
  before its data offset, yielding zero region data. This is structural, not
  a parsing error.

- **Record ot_index (field 11) overflow**: Some records have ot_index values
  exceeding the OT count. Exclude via `ot_index < offset_table_count`.

### Signals backed by current tests

These patterns are covered by current Rust/Python tests. They are still scoped
to the fixtures named below, not proof that every edge case is solved:

- **Record field[6] as shape binding**: 5 = scalar, 32 = arrayed (generic
  marker in single-shape files), other values = section-3 shape keys. Scalar
  owner records use field[6]=5. In `Ref.vdf`, explicit field[6] values match
  the previous section-3 index words, while the following physical entry gives
  the actual flat size. This is pinned by `GWP of HFC` (`275 -> 302`, len 9),
  `Layer Depth` (`248 -> 275`, len 4), and `Semi Agg Definition`
  (`302 -> 0`, len 42).

- **Record ot_index as array block start**: In the `subscripts` fixture,
  ot_index values {1, 6, 9, 13} correspond exactly to the OT block starts
  for {a stock[3 elem], net flow[3 elem], other const[3 elem], some rate}.
  Each arrayed variable's 3 consecutive OT entries share the same class code.

- **Section 3 fixed-width directory**: In array models, section 3 is not
  just a cardinality tail. It has a 25-word zero prefix, a run of 27-word
  records, and a trailing zero word. `subscripts.vdf` has one record;
  `Ref.vdf` has eleven. Record word 0 is an index-like value, words 1..3
  encode shape-like cardinalities, and words 18..19 resolve through the
  section-1 slot table. Scalar models remain 26 zero words with no records.

- **Record field[8] dimension-element groups**: Dimension anchors and their
  element records share a non-sentinel field[8] group ID. Element records use
  zero-based field[11] indices, so elements can be ordered even when file
  order is scrambled (`layers` in `Ref.vdf` appears as 0,2,1,3 in record
  order). Covered by `test_record_field8_recovers_dimension_element_groups`
  and the edited-fixture dimension recovery test in `tools/test_vdf_xray.py`.

- **Section 4 slot refs**: All non-trivial u32 values in section 4 that
  are 4-byte-aligned and within section-1 range appear in the slot table.
  The section grows proportionally with model complexity (20 bytes for
  water, 600 bytes for WRLD3).

- **Record field[1] == 138 as view-header marker**: Each VDF contains
  a run of `field[1] == 138` records that act as view-group boundaries.
  On small single-view fixtures and on WRLD3 SCEN01 / experiment the
  header count matches the dot-prefix name count exactly (1:1). On
  edited files (`econ/risk2.vdf` drops `.Control` but keeps its record)
  and on multi-level module fixtures (`Ref.vdf`, 17 headers vs 69
  dot-names with sub-groups) the 1:1 alignment breaks. Between two
  consecutive view-header records lie that view's variable records;
  on 1:1 fixtures the count matches the names between the two
  corresponding dot-prefix entries. Exposed as
  `VdfFile::record_view_groups()` and
  `VdfFile::record_view_groups_with_diagnostics()` (returns unmatched
  headers / dot-names alongside the groups).

- **Shift-by-one record-to-name OT link via `field[11]`**: For file-
  order record-to-name pairs `(rec[i], name[i])`, the OT index of
  `name[i+1]` equals `rec[i].field[11]`. `Time` is the sole `OT[0]`
  owner (implicit); `field[11] == 0` for a non-Time successor is a
  "no OT entry" sentinel. Arrayed records (`f[6] != 5`) expand to
  `flat_size` consecutive OT slots via the section-3 shape
  directory, matching the pattern in `to_results_via_records`.
  On WRLD3 SCEN01 / experiment the two paths produce the same
  time series for roughly 40-50 of ~260-270 shared names (~16-18%
  per-series agreement via exact time-series equality against
  `build_section6_guided_ot_map`); they diverge on the rest because
  the guided path and the file-order path have different failure
  modes. Every smaller single-view scalar fixture agrees in full.
  The sentinel over-filters a handful of real variables on WRLD3
  SCEN01
  (quantified by `test_field11_zero_sentinel_loss_on_wrld3_is_pinned`
  and the `FileOrderPairDiagnostics` return from
  `build_file_order_pairs`). On arrayed fixtures (subscripts.vdf)
  and edited/multi-level-module fixtures (risk2, Ref), the pairing
  drifts and the path surfaces a partial mapping; use
  `to_results_via_records` when the fixture has dim-element names
  interleaved with variables. Exposed as
  `VdfFile::to_results_via_file_order_records()`.

- **Two stdlib signature encodings** (`#` name table entries): Vensim emits
  stdlib-call signatures in two forms that coexist across our test corpus:

  | Style | Output sig form | Internal stocks |
  |-------|-----------------|-----------------|
  | Old   | `#FUNCNAME(args)#`     | `#LV1<FUNCNAME(args)#`, `#DL<...`, `#RT1<...`, ... |
  | New   | `#alias>FUNC#`         | `#alias>FUNC>LV1#`, `#alias>FUNC>DL#`, ... |

  The new-style form **encodes the user alias name directly** in the
  signature prefix (e.g. `#defaults>DELAY1#`), so user-alias -> output-OT
  resolution requires no external model. The old-style form leaves the
  alias name implicit.

  An "output" signature is one that a user alias binds to. The classifier
  requires a **positive** structural signal to avoid false positives on
  non-stdlib `#`-bracketed names (Ref.vdf has many `#BAU atm conc CO2#`-
  style display names, and `model_editing/run_1.vdf` has a bare `#` from
  an empty aux):
  - Old-style output: the name contains `(` somewhere AND does NOT start
    with one of the seven internal prefixes `#LV1<`, `#LV2<`, `#LV3<`,
    `#ST<`, `#DL<`, `#RT1<`, `#RT2<`.
  - New-style output: the name contains exactly ONE `>` at the top
    level AND does NOT end with one of the matching internal suffixes
    `>LV1#`, `>LV2#`, `>LV3#`, `>ST#`, `>DL#`, `>RT1#`, `>RT2#`. Names
    with two or more `>` are sub-parts of multi-output macros
    (`#alias>RAMP FROM TO>linear#`, `>slope#`, `>rate#`, `>interval#`,
    `>input#` for SSHAPE, ...) and are NOT treated as user-alias
    outputs; they are internal helpers per top-level alias.

  Validated on all econ VDFs (base/rk/policy/mark2/risk), WRLD3-03
  SCEN01 / experiment, `Ref.vdf` (C-LEARN with RAMP FROM TO / SSHAPE
  macros), and `model_editing/run_1.vdf` (bare `#` fixture).

  Rust helpers: `VdfFile::output_signatures()`, `new_style_alias_signatures()`.

- **User aliases and output signatures appear in the name table in
  matching file order**: on old-style fixtures (`econ/base`, `econ/rk`,
  `econ/risk`) the user-facing alias names (slotted user names that
  declare a stdlib call in the MDL) occur in the name table in the
  same order as their target `#FUNC(args)#` output signatures occur.
  The pairing is 1:1 once the internal `#LV1<...>` / `#LV2<...>` / ...
  signatures are removed.

  **WRLD3 SCEN01 breaks this 1:1 upper bound**: the MDL declares 12+
  stdlib-call aliases but the VDF emits only 8 old-style output sigs.
  The gap may reflect Vensim re-using a single module for multiple
  aliases (for example, two auxiliaries that both call the same SMOOTH
  with identical arguments can share one compiled module and thus one
  output sig). Pinned by `wrld3_scen01_alias_to_sig_pairing_is_not_1to1`:
  the pairing is an upper bound, not a guarantee.

  Example (`econ/base.vdf`, 5 aliases vs 5 old-style output sigs):

  | file-order idx | alias name                     | output sig (later in name table) |
  |---------------:|---------------------------------|-----------------------------------|
  | 30             | `defaults`                      | `#DELAY1(insolvencyrisk,...)` @94 |
  | 46             | `perceived inflation rate`      | `#SMOOTH(realinflationrate,3)` @96 |
  | 62             | `perceived HPI`                 | `#SMOOTH(indexedHPI,...)` @97     |
  | 72             | `perceived risk of insolvency`  | `#SMOOTH(insolvencyrisk,6)` @98   |
  | 84             | `perceived mortgage balance`    | `#SMOOTH(interest...)` @99        |

  Verified against the MDL parser on `econ/base.vdf` and `econ/risk.vdf`
  (7 aliases, 7 output sigs), and confirmed deterministic by the
  `test_old_style_alias_to_output_sig_pair_by_file_order` unit test.

  **What this closes**: once a caller knows the alias list (currently
  from the parsed model), the alias -> output-sig mapping is a pure
  pairing of the two ordered lists -- no per-name lookup required.

- **Alias records carry classification `f[1] == 2065` (`0x811`)** on
  old-style fixtures. The classification byte structure is high-byte
  `0x08` "associated with stocks" + low-byte `0x11` "dynamic non-stock",
  as documented under "Classification field (field 1) byte-level
  structure". Every alias-backed user variable record observed on
  `econ/base`, `econ/risk`, and the WRLD3 SCEN01 family carries this
  classification when the stdlib-call argument list consists of simple
  name references (`SMTH1(x, t)` or `DELAY1(x, t)`). This is NOT a
  complete signal: aliases whose argument is an expression (e.g.
  `SMTH1(a - b, t)` on `perceived mortgage balance`) are classified as
  regular variables with `f[1] == 17`. Exposed as
  `VdfFile::identify_potential_aliases()`, which combines the
  classification signal with name-category filtering to drop
  time/metadata/unit/stdlib-helper names; on `econ/base.vdf` it
  recovers 4 of 5 MDL-declared aliases and on `econ/risk.vdf` 6 of 7,
  with no false positives. On `WRLD3-03/SCEN01.VDF` it recovers 6 of
  12 declared aliases; on `WRLD3-03/experiment.vdf` and new-style
  fixtures (re-saved `econ/policy.vdf`, `econ/mark2.vdf`) the
  classifier returns an empty or near-empty set because the alias
  encoding shifts to `#alias>FUNC#` and uses a different
  classification byte. Treat the result as "necessary but not
  sufficient".

  The cross-agent `field[11] == 0` sentinel from structural signal #12
  does NOT independently identify aliases on old-style fixtures. In
  `econ/base.vdf` exactly 4 records carry `f[1] == 2065` (not 5 as an
  earlier draft stated); their predecessor records' `f[11]` values are
  `{23, 67, 68, 70}` -- none are zero -- so the sentinel rule alone
  cannot separate aliases from regular variables. The combined
  classifier is the best-available old-style alias detector at this
  time.

  **What this does NOT yet close**: exact-match alias identification
  from VDF alone on old-style fixtures with expression-argument
  stdlib calls. Callers that need the precise alias set should either
  (a) parse the MDL and compare aliases against
  `identify_potential_aliases()` for cross-validation, or (b) rely on
  the new-style `#alias>FUNC#` encoding (signal below) which re-saved
  files from newer Vensim builds produce deterministically. A sweep
  over candidates A-E in the 2026-04 reverse-engineering task did not
  reveal a *deterministic* single-signal alias-bit anywhere in the
  record array, slot pointees, pre-record header cells, section-4
  entries, or section-6 ref stream -- see "Hypotheses tested and
  ruled out" below for the numeric evidence.
