# VDF Binary Format (Vensim Data File)

## Overview

VDF is Vensim's proprietary binary format for simulation output. The format is
completely undocumented and no open-source parser previously existed.

Vensim can open a `.vdf` file and show its contents without a corresponding
`.mdl` model file, and open "old" VDF files and show time-series for some
variables even after substantive model changes. This means the VDF format is
self-contained: it encodes enough information to map variable names to their
time series data. Our goal is to replicate this capability -- converting a VDF
file into a `Results` struct (a mapping of variable names to time series data)
without any external model file.

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
  0x50    4     Format constant: 0x012C0065 (all observed files)
  0x54    4     Zero
  0x58    4     u32 final_values_offset: absolute file offset to the
                section-6 OT final-values array
  0x5C    4     u32 lookup_mapping_offset: absolute file offset to the
                section-6 lookup mapping records (end of final values)
  0x60    4     u32 offset_table_offset: absolute file offset to the
                section-7 offset table
  0x64    4     u32 offset_table_offset (duplicate, always same as 0x60)
  0x68    4     Zero in most files; meaning unknown
  0x6C    4     Nonzero in some files; meaning unknown
  0x70    4     Varies; possibly lookup-related count
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

These derivations have been validated across the full test corpus (11+ VDF
files of varying size and complexity).

The `time_point_count` is the number of output time points stored. Examples:
- bact model (t=0..60, saveper=1): 61
- pop model (t=0..100, saveper=1): 101
- WRLD3-03 (t=1900..2100, dt=0.5): 401

The bitmap size used in data blocks is `ceil(time_point_count / 8)` bytes.


## Section framing

The file contains multiple sections, each delimited by a 4-byte magic value.
Every observed VDF file has exactly **8 sections** (indices 0-7).

### Section header (24 bytes)

```
  Offset  Size  Description
  ------  ----  -----------
  +0      4     Section magic: A1 37 4C BF (= f32 -0.797724 = u32 0xBF4C37A1)
  +4      4     u32 field1
  +8      4     u32 field2 (always equals field1 in observed files)
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

### String table entries

One 16-byte entry (4 x u32 values) per name in the name table. The entries'
per-field purpose is not fully decoded, though they serve as a cross-reference
key between names and variable metadata records via the record's `slot_ref`
field (see below).

### Variable metadata records

Each record is 64 bytes (16 x u32 fields). Records are located within
section 1's region and found by scanning for sentinel pairs: two consecutive
`0xf6800000` values at field offsets 8 and 9. Records are then extended at
64-byte alignment from that anchor point.

Records are sparse -- most names do NOT have a corresponding record.

#### Record fields

| Name            | Index | Purpose |
|-----------------|-------|---------|
| type_flags      | 0     | Variable type/flags; 0 = padding record |
| classification  | 1     | 23 = system variable; 15 = initial-time constant; see below |
| (unknown)       | 2     | Incrementing value; not a name-table byte offset |
| (unknown)       | 3     | Varies per variable; meaning unknown |
| (unknown)       | 4-5   | Usually zero |
| arrayed_flag    | 6     | Shape binding. `5` = scalar variable. `32` = arrayed variable (unambiguous when only one sec3 entry exists; in multi-shape files, 32 is a generic "arrayed" marker whose shape must be resolved elsewhere). Other values = section-3 directory `index_word`, directly binding the record to a specific shape template. Confirmed in `Ref.vdf` where field[6] takes values matching sec3 index_words: 59, 86, 113, 140, 167, 194, 221, 275, 302, plus 0 for the last entry with index_word=0. |
| (unknown)       | 7     | Usually zero; nonzero in some system records |
| sentinel_a      | 8     | Always 0xf6800000 |
| sentinel_b      | 9     | Always 0xf6800000 |
| sort_key        | 10    | Ordering value; does not reliably correspond to alphabetical order |
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
It is always `"Time"`. Subsequent entries are u16-length-prefixed strings.
A u16 value of 0 is a group separator (skipped).

### Name categories

The name table is a **superset** of stored variables. It contains names from
many categories, only some of which have corresponding offset table (OT)
entries:

| Category | Recognition signal | Has OT entry? |
|----------|-------------------|---------------|
| System variables | Exact match: `Time`, `INITIAL TIME`, `FINAL TIME`, `TIME STEP`, `SAVEPER` | Yes (Time at OT[0]) |
| Model variables | Slotted names passing metadata filter | Yes |
| Lookup table definitions | Matched 1:1 by section-6 lookup mapping records | Yes (inline constants, code 0x17) |
| Internal signatures | Prefix and suffix `#` (e.g., `#SMOOTH(x,3)#`, `#LV1<model>var#`) | Yes |
| Stdlib helper variables | Exact match: `DEL`, `LV1`, `LV2`, `LV3`, `ST`, `RT1`, `RT2`, `DL` | Yes |
| Group/view markers | Prefix `.` | No |
| Unit annotations | Prefix `-` | No |
| Builtin function names | Exact match against known set (`SUM`, `MIN`, `step`, etc.) | No |
| Module IO names | Exact match: `IN`, `INI`, `OUTPUT` | No |
| Module function names | Exact match: `SMOOTH`, `DELAY1`, `TREND`, etc. | No |
| Metadata tags | Prefix `:` | No |
| Single-char placeholders | `?` | No |


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
| 0       | `index_word`: encodes the word offset of the entry within the directory. In `Ref.vdf`, the first ten records form the arithmetic progression `59, 86, 113, ... , 302` with step = 27 (the entry width in words), providing a structural checksum. The last entry has `index_word=0`. Some of those values also reappear in section 4, but not consistently enough yet to treat the bridge as decoded. Record field[6] references these `index_word` values to bind records to specific shape templates. |
| 1..3    | Packed shape words. One-dimensional entries duplicate the flattened size (`3, 3` -> one axis of size 3). Composite entries use `flattened_size + axis factors` (`21, 7, 3` -> two axes of sizes 7 and 3). In validated fixtures, `flattened_size = product(axis_sizes)`. |
| 10      | Packing hint. It is `1` for one-dimensional entries; for composite entries it equals the trailing axis size (`3` in `[21, 7, 3]`, `4` in `[12, 3, 4]`). |
| 11      | Small axis counter word. `0` on one-dimensional entries, `1` on the validated two-axis entries. |
| 18..19  | One section-1 slot ref per encoded axis in the validated fixtures. They resolve through the slot table (for example `172 -> "net flow"` in `subscripts`, `2412 -> "FF stop growth year"` in `Ref.vdf`). The refs are useful as axis/dimension anchors, but not yet as direct base-variable owners. |
| 26      | Encoded axis count. Validated values are `1` (one-dimensional entry) and `2` (two-axis composite entry). It matches both the number of axis-size factors and the number of slot refs. |

The `field4=32` in the section header matches the record `arrayed_flag`
value (field[6]=32), suggesting a shared "arrayed" signal.

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
21 remain ambiguous without another signal (likely section 1, 4, or 6).


## Section 4: view/group membership

Variable-length structured entries that reference section-1 slot table
values, encoding view/group membership. Section 4 grows proportionally
with model complexity (20 bytes in water, 88 bytes in econ, 600 bytes
in WRLD3, 1540 bytes in `Ref.vdf`).

### Structure

Begins with a 2-word zero header, followed by variable-length entries.
Each entry contains:

1. A packed word `p`
2. `refs[p_hi + p_lo]`, where `p_hi = p >> 16` and `p_lo = p & 0xffff`
3. A trailing `index_word`

This framing now parses the validated corpus end-to-end:

| File | Entries | Distinct `p_lo` values | Distinct `p_hi` values |
|------|---------|-------------------------|------------------------|
| `water` | 1 | `{1}` | `{0}` |
| `pop` | 2 | `{1}` | `{0}` |
| `econ` | 6 | `{1}` | `{0,1}` |
| `WRLD3` | 37 | `{1,2}` | `{0,1,2,3}` |
| `Ref.vdf` | 94 | `{0,1,2,3}` | `{0,1,2,3,4}` |

The exact semantics of `p_lo`/`p_hi` are still unknown, but their **sum**
is now validated as the ref count in all of those fixtures. All parsed refs
resolve to in-range section-1 offsets, and in the validated fixtures every
ref is also present in the slot table.

The `index_word` rises through most of the stream and ends with a final
`0` entry in all observed fixtures. In `Ref.vdf`, a subset of section-3
directory indices (`59, 194, 248, 275, 302, 0`) reappear here, which makes
section 4 the strongest decoded bridge so far from section-3 shape indices
toward view/slot clusters.

The slot refs in section 4 consistently resolve to names in the slot table
(view markers like `.Control`, `.mark2`, unit annotations like `-Month`,
and model variable names). The structure encodes which variables belong
to which views/groups in the model.

What remains open is the **meaning** of the packed halves and the precise
semantics of `index_word`. We can parse the entry stream structurally now,
but do not yet know which entry type binds a section-3 shape to a specific
base variable / OT block.


## Section 5: dimension sets

In scalar models, section 5 is degenerate (the next section header starts
before section 5's data offset, yielding zero region data).

In array models, section 5 contains structured entries in two forms
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
`n+2` refs (two trailing refs). The trailing refs are section-3
`axis_slot_ref` values -- they bridge section-5 dimension sets to section-3
shape templates. For marker=0, the last ref is a sec3 axis ref. For marker=1,
the last two refs are sec3 axis refs, corresponding to a two-axis composite
shape. In `Ref.vdf`, 6 of 7 unique axis refs are shared between section-5
trailing refs and section-3 `axis_slot_refs`, confirming this as the
structural bridge from dimension sets to shape templates. For 2D shapes, the
two trailing refs in a marker=1 entry correspond to the two `axis_slot_refs`
in the matching sec3 entry.

The `n` sizes correspond to model dimension cardinalities. The non-trailing
refs do **not** directly name every element: in the `subscripts` fixture they
resolve to `TIME STEP`, `sub1`, `.Control`, and `0`. The useful signal is
that the sole non-metadata ref identifies the dimension name (`sub1`). The
element names are then recovered from the name table by taking the next `n`
non-metadata names after that dimension name (`a`, `b`, `c` here).

This is enough to infer dimension names and element names conservatively, and
to exclude them from generic OT-participant filtering, but not enough to say
which base variable uses which dimension.


## Section 6: OT metadata

Section 6 is the richest source of VDF-native mapping information. Its layout:

1. Optional one-word prefix
2. Leading ref stream: variable-length `u32 n_refs; u32 refs[n_refs]` entries
3. **OT class-code array**: `offset_table_count` bytes, one per OT entry
4. **OT final-value array**: `offset_table_count` little-endian f32 values
5. **Lookup mapping records**: `13 * u32` fixed-width records, terminated by a
   single zero word

### OT class codes

The class-code array is the primary VDF-native stock/non-stock signal.
Stock entries form a contiguous block at OT[1..S], followed by all non-stock
entries at OT[S+1..N-1].

| Code | Meaning | OT range |
|------|---------|----------|
| 0x0f | Time | OT[0] only |
| 0x08 | Stock-backed variable | OT[1..S] |
| 0x11 | Dynamic non-stock (data block) | OT[S+1..N-1], interleaved |
| 0x17 | Constant non-stock (inline f32) | OT[S+1..N-1], interleaved |

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

The records at the end of section 6 correspond **1:1 with lookup table
definitions** in the name table. Each record's word[10] contains the OT index
for that lookup table. This is the authoritative VDF-native mechanism for
identifying which names are lookup definitions and mapping them to OT entries.
All lookup definitions have OT entries as inline constants (code 0x17).

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

Packed contiguously after the offset table. Each block stores a sparse time
series:

```
  +0      2                        u16 count (stored values)
  +2      ceil(time_point_count/8) Bitmap: bit per time point
  +2+bm   count * 4                f32 values in time order
```

Block 0 is always the time series itself (fully dense bitmap).


## Name-to-OT mapping

The central challenge in VDF parsing is mapping names from the name table to
offset table (OT) entries. Several structural signals provide partial mapping;
the remaining gap requires the model file.

### What is structurally determined

1. **Section-6 class codes** give VDF-native stock/non-stock classification
   with a contiguous stock block at OT[1..S].

2. **Section-6 lookup mapping records** give direct OT assignments for all
   lookup table definitions (1:1 correspondence).

3. **Stocks-first-alphabetical ordering**: among non-lookup OT participants,
   stocks sort alphabetically into OT[1..S] and non-stocks into OT[S+1..N-1].
   **Caveat**: this contiguous-block property holds for 7 of 8 test files but
   breaks down in `Ref.vdf` (C-LEARN), where 209 stock entries are split
   across 8 non-contiguous ranges spanning OT[1..473], interleaved with
   dynamic entries. The scattering is driven by array variables whose element
   ranges straddle stock/non-stock code boundaries. Only 4 of 684
   record-derived OT ranges actually contain mixed stock/non-stock class
   codes.

4. **Internal variable classification**: `#`-prefixed signature names encode
   stock/non-stock in their prefix pattern (e.g., `#LV1<` = stock,
   `#DELAY3(` = non-stock). Stdlib helper names (DEL, LV1, LV2, LV3, ST,
   RT1, RT2, DL) have deterministic classification.

5. **Record arrayed_flag (field 6)** distinguishes arrayed from scalar
   variables. `5` = scalar; `32` = arrayed (generic marker); other values =
   section-3 `index_word` binding directly to a shape template. In multi-shape
   files like `Ref.vdf`, field[6] takes specific `index_word` values rather
   than the generic `32`.

6. **Record ot_index (field 11)** gives the OT block start for each
   variable's contiguous OT entries. For arrayed variables, consecutive
   OT slots hold one element each in subscript order. In the `subscripts`
   fixture, ot_index values 1, 6, 9 correspond to the starts of the 3
   arrayed variables' 3-element blocks.

7. **Section-3 extension in array models** provides dimension cardinality
   at words 26-27 of the section data (e.g., 3 for a 3-element dimension).

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

`VdfFile::to_results_with_model(project, model_name)` uses:
1. Model-based stock classification
2. VDF name table filtering (builtins, metadata, module names removed)
3. Stocks-first-alphabetical ordering with section-6 stock boundary
4. SMOOTH/DELAY alias resolution via compiled module structure
5. Empirical time-series correlation as a refinement layer

### Open problems (fully VDF-only mapping)

Two problems still prevent generic fully VDF-native (no external input)
mapping:

1. **Stock classification of regular model variable names.** Section-6 gives
   the stock count S, and `#`-prefixed and helper names have deterministic
   classification, but regular model variable names (e.g., "Population 0 To
   14") require external knowledge to identify as stocks. Without this, the
   stocks-first sort cannot be applied.

   Investigated and ruled out: block density (stocks are always data blocks,
   but many non-stocks are too), time-series variance patterns, record field
   combinations, section-6 ref stream ordering, and final-value anchoring.
   None reliably distinguish stocks from non-stocks for regular variable
   names. The viable elimination rules are: constants (0x17) are always
   non-stocks, lookups are always non-stocks, stock OTs are always data
   blocks, and internal `#`-prefixed signatures have deterministic
   classification.

   *Possible approach*: use section-6 final values and inline constant values
   from the offset table to anchor known constants (e.g., FINAL TIME = 2100.0,
   THOUSAND = 1000.0), then use the contiguous stock block to partition
   remaining names by elimination. The new `consts` fixtures show the limit of
   this idea: repeated scalar values (`b = 3`, `c = 3`) mean final-value
   anchors alone do not uniquely identify every regular variable.

2. **Participant filtering for large models.** For small/medium models, the
   filtered name count matches or slightly exceeds OT count (excess = lookup
   definitions). For large models like WRLD3, the name table contains more
   names than OT entries (~340 candidates for 296 OTs). The excess includes
   model variables that Vensim chose not to save.

   The new section-5-based filtering removes array bookkeeping names
   (dimensions/elements) reliably, but that only helps array models; WRLD3 has
   no section-5 content, so the large-model filtering problem remains open.

   *Possible approach*: the exact OT count is known from the header. If the
   candidate count can be narrowed to match (e.g., by using constant-value
   matching to anchor some entries and exclude others by elimination), the
   mapping becomes determined.

3. **Array element mapping (partially solved).** Array elements occupy
   contiguous OT blocks in subscript order. The name table stores base
   variable names (e.g., "a stock") alongside dimension names ("sub1") and
   element names ("a", "b", "c"). Section 5 encodes dimension cardinalities,
   and now also gives enough information to recover named dimension sets by
   combining its refs with local name-table order. `to_results_with_array_info()`
   handles array expansion when provided with stock classification and array
   dimension info; the dimension/element names themselves can now be inferred
   and filtered automatically.

   **New signal: record field[6] as shape binding.** Field[6] != 5 identifies
   which base names are arrayed. In single-shape files like `subscripts`,
   field[6]=32 is the generic arrayed marker (`net flow` and `other const`
   have f6=32 while `some rate` has f6=5). In multi-shape files like `Ref.vdf`,
   field[6] takes specific section-3 `index_word` values, directly binding each
   record to a shape template. Combined with ot_index giving block starts
   and section-5 giving dimension cardinalities, the array mapping is now
   fully determined for single-dimension models without the model file.

   The remaining gap is multi-dimension models: section 3 now clearly encodes
   reusable shape templates (`flat_size + axis_sizes + axis slot refs`), and
   the field[6] -> section-3 `index_word` -> section-5 trailing refs chain is
   now confirmed as the structural path from records through shape templates
   to dimension sets. However, record -> base variable name binding remains
   unsolved. Records don't directly indicate which name they describe (the
   slot_ref groups records by view, not by individual variable), so linking a
   record's ot_index to a specific base variable name still requires another
   discriminator from section 1, 4, or 6.


## Appendix: reverse-engineering notes

### Hypotheses tested and ruled out

These approaches were investigated and found unreliable for the name-to-OT
mapping problem:

- **Record sort_key (field 10) as global alphabetical key**: Kendall's tau =
  0.46 against alphabetical order on WRLD3. Not general.

- **Record slot_ref (field 12) groups as name anchors**: records are sparse
  (23 of 296 OTs have records in WRLD3). Not enough coverage.

- **String table 16-byte payloads as durable keys**: change across Vensim
  versions/reruns. Not stable.

- **Section-6 leading refs as a save list**: Resolved refs include model
  variables, unit annotations (e.g., `-Month`), view markers (`.Control`),
  builtin function names (`SMOOTH`, `DELAY1`, `if then else`), system
  variables, and stdlib helpers. The mix means the ref stream is NOT a
  clean variable save list. WRLD3's 342 entries correlate with the
  candidate population but cannot be used directly for participant
  filtering without further classification of entry types.

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

### Confirmed structural signals

These patterns were validated across the full test corpus:

- **Record field[6] as shape binding**: 5 = scalar, 32 = arrayed (generic
  marker in single-shape files), other values = section-3 `index_word`
  binding to a specific shape template. All records in scalar-only models
  have field[6]=5. In `Ref.vdf`, field[6] values match all 11 sec3
  index_words.

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

- **Section 4 slot refs**: All non-trivial u32 values in section 4 that
  are 4-byte-aligned and within section-1 range appear in the slot table.
  The section grows proportionally with model complexity (20 bytes for
  water, 600 bytes for WRLD3).
