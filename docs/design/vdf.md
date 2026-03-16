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
  | Section 3 (zeros)                    |
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
  0x78    4     u32 time_point_count
  0x7C    4     u32 time_point_count (duplicate, always same value)
```

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
| 3 | Zeros | Always 32 bytes of zeros |
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
| classification  | 1     | 23 = system variable; 15 = initial-time constant |
| (unknown)       | 2-7   | Not yet decoded |
| sentinel_a      | 8     | Always 0xf6800000 |
| sentinel_b      | 9     | Always 0xf6800000 |
| sort_key        | 10    | Ordering value; does not reliably correspond to alphabetical order |
| ot_index        | 11    | Appears OT-related; values can exceed the actual OT count |
| slot_ref        | 12    | Byte offset into section 1 data; groups records by view/sector |
| (unknown)       | 13-15 | Not yet decoded |

Code accessors: `VdfRecord::slot_ref()` (field 12), `VdfRecord::ot_index()`
(field 11).

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


## Section 3: zeros

Always 32 bytes of zeros in all observed files.


## Section 4: view/group metadata

Variable-length structured entries. Not fully decoded.


## Section 5: dimension sets

In scalar models, section 5 is degenerate (the next section header starts
before section 5's data offset, yielding zero region data).

In array models, section 5 contains structured entries:

```
  u32 n;
  u32 0;
  u32 refs[n+1];
```

The `n+1` sizes correspond to model dimension cardinalities + 1. This is
relevant for array element OT decoding.


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

4. **Internal variable classification**: `#`-prefixed signature names encode
   stock/non-stock in their prefix pattern (e.g., `#LV1<` = stock,
   `#DELAY3(` = non-stock). Stdlib helper names (DEL, LV1, LV2, LV3, ST,
   RT1, RT2, DL) have deterministic classification.

### Current production path (model-guided)

`VdfFile::to_results_with_model(project, model_name)` uses:
1. Model-based stock classification
2. VDF name table filtering (builtins, metadata, module names removed)
3. Stocks-first-alphabetical ordering with section-6 stock boundary
4. SMOOTH/DELAY alias resolution via compiled module structure

### Open problems (VDF-only mapping)

Three problems prevent fully VDF-native (no model) mapping:

1. **Stock classification of regular model variable names.** Section-6 gives
   the stock count S, and `#`-prefixed and helper names have deterministic
   classification, but regular model variable names (e.g., "Population 0 To
   14") require the model to identify as stocks. Without this, the
   stocks-first sort cannot be applied.

   *Possible approach*: use section-6 final values and inline constant values
   from the offset table to anchor known constants (e.g., FINAL TIME = 2100.0,
   THOUSAND = 1000.0), then use the contiguous stock block to partition
   remaining names by elimination.

2. **Participant filtering.** The VDF name table contains more names than OT
   entries (~340 candidates for 296 OTs in WRLD3). The excess names are model
   variables that Vensim chose not to save in this particular run. No VDF-local
   signal has been found to distinguish saved from unsaved model variables (slot
   data, records, and section-6 refs were all tested without success). Vensim's
   save configuration is not encoded in the MDL file either.

   *Possible approach*: the exact OT count is known from section-6. If the
   candidate count can be narrowed to match (e.g., by using constant-value
   matching to anchor some entries and exclude others by elimination), the
   mapping becomes determined.

3. **Array element mapping.** For array models, section-5 dimension sets
   provide cardinalities, and record OT ranges show dimension-scale blocks.
   Combining these with VDF-visible element naming would enable outputs like
   `population[young]`.


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

- **Section-6 leading refs as a save list**: overlap between saved and unsaved
  names. Not discriminating.

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
