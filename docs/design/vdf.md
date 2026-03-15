# VDF Binary Format (Vensim Data File)

VDF is Vensim's proprietary binary format for simulation output. The format is
completely undocumented and no open-source parser previously existed.

## Goal

Vensim can open a `.vdf` file and display its contents without a corresponding
`.mdl` model file. This means the VDF format is self-contained: it encodes
enough information to map variable names to their time series data. Our goal is
to replicate this capability -- converting a VDF file into a `Results` struct
(a mapping of variable names to time series data) without any external model
file. Along the way, model-guided mapping serves as a stepping stone and
validation oracle, but the target is fully VDF-native decoding.

The format has been reverse-engineered from multiple VDF files of varying
complexity:

- **Small models** (3-8 variables, 3-7KB): `bact`, `water`, `pop`
- **Medium model** (~420 variables, 333KB): WRLD3-03 (World3-03 from
  the Limits to Growth model)
- **Large model** (1.8MB): C-LEARN model

All values are little-endian. All offsets in this document are byte offsets.
The parser is implemented in `src/simlin-engine/src/vdf.rs`.


## High-level structure

A VDF file contains:

1. A **file header** identifying the file and the simulation's time parameters
2. Multiple **sections** delimited by magic bytes, containing model metadata
3. **Variable metadata records** -- 64-byte blocks encoding variable properties
4. A **slot table** mapping names to section 1 slot entries
5. A **name table** section with variable names (and other names)
6. An **offset table** mapping variables to their time series data
7. **Data blocks** containing the actual time series values

The overall file layout, from lowest to highest offset:

```
  +---------------------------+
  | File header (128 bytes)   |  0x00..0x7F
  +---------------------------+
  | Section 0 (model info)    |
  +---------------------------+
  | Section 1 (slot data)     |
  +---------------------------+
  | Variable metadata records |  (between sec[1] end and slot table)
  +---------------------------+
  | Slot table                |  (N u32 values, one per name)
  +---------------------------+
  | Name table section        |  (section containing variable names)
  +---------------------------+
  | Sections 3..6             |  (additional metadata sections)
  +---------------------------+
  | Section 7                 |  (lookup table data + offset table + data blocks)
  +---------------------------+
```


## 1. File header

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


## 2. Sections

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
  +16     4     u32 field4 (varies per section)
  +20     4     u32 field5 (for name table: high 16 bits = first name length)
```

A section's data runs from its 24-byte header to the start of the next
section's header (magic-to-magic).

### Section roles by position

| Index | Role | Notes |
|-------|------|-------|
| 0 | Model info / settings | ~39-40 bytes; contains simulation command string |
| 1 | Variable slot table | One 16-byte slot per variable name |
| 2 | Name table | Variable names; identified by field5 high bits |
| 3 | Zeros | Always 32 bytes of zeros |
| 4 | View/group metadata | Variable-length structured entries |
| 5 | Dimension sets | Degenerate in scalar models; set-like `n,0,refs...` entries in array models |
| 6 | OT metadata | Ref stream + class codes + final values + display records |
| 7 | Lookup tables + OT + data | Packed f32 lookup data, then offset table, then data blocks |

**Identifying sections by position, not field4**: field4 values vary across
files (e.g., 2, 42, 473 for section 1). Identification must be by index.

**Section 7 field4/field5**: For section 7, these header words double as the
first two f32 values of the lookup table data. See "Section 7" below.


## 3. Section 0: model info

A small section (~39-40 bytes) containing the simulation command string
(e.g., `sim bact -I`).


## 4. Section 1: variable slot table

One 16-byte slot per variable name. Slots are packed at uniform stride 16 for
small models; stride varies for larger models.

Each slot consists of 4 x u32 values. The slot contents do NOT contain offset
table indices. The slot data's per-variable purpose remains partially unknown,
though it serves as a cross-reference key between names and records via the
`f[12]` record field.


## 5. Name table (section 2)

Identified by: `field5 >> 16` gives the first name's byte length.

The first entry has no u16 length prefix -- its length comes from the header.
It is always `"Time"`. Subsequent entries are u16-length-prefixed strings.
A u16 value of 0 is a group separator (skipped).

### Name categories

The name table is a **superset** of stored variables. It contains:

| Category | Prefix/pattern | Has OT entry? |
|----------|---------------|---------------|
| System variables | `Time`, `INITIAL TIME`, etc. | Yes (Time at OT[0]) |
| Model variables | Regular names | Yes |
| Lookup/table definitions | Contains " lookup" or " table" | Yes (as inline constants) |
| `#`-prefixed internal signatures | `#SMOOTH(...)#`, `#LV1<...#` | Yes |
| Participant helpers | `DEL`, `LV1`, `LV2`, `LV3`, `ST`, `RT1`, `RT2`, `DL` | Yes |
| Group/view markers | `.Control`, `.mark2` | No |
| Unit annotations | `-Year`, `-dmnl` | No |
| Builtin function names | `SUM`, `MIN`, `step` | No |
| Module IO names | `IN`, `INI`, `OUTPUT` | No |
| Module function names | `SMOOTH`, `DELAY1`, `TREND` | No |
| Metadata tags | `:SUPPLEMENTARY` | No |
| Single-char placeholders | `?` | No |


## 6. Variable metadata records

Located within section 1's region. Each record is 64 bytes (16 x u32 fields).
Found by scanning for sentinel pairs (two consecutive `0xf6800000` at offsets
+32 and +36), then extending at 64-byte alignment.

### Key fields

| Field | Purpose |
|-------|---------|
| f[0] | Variable type/flags. 0 = padding. |
| f[1] | 23 = system variable. 15 = initial-time constant. |
| f[10] | Alphabetical sort key (reliable on small models only) |
| f[11] | OT index for small models; unreliable on large models |
| f[12] | Section-1 byte offset. Groups records by view/sector. |

Records are sparse -- most names do NOT have a corresponding record.


## 7. Slot table

An array of N u32 values (one per slotted name), located between the last
record and the name table section. Each value is a byte offset into section 1
data. Found by scanning backward from section 2 for the largest structurally
valid table.


## 8. Offset table

Located within section 7, between the lookup table data and the first data
block. An array of N u32 entries (one per OT entry, including OT[0] = Time).

Each entry is either:
- **A file offset** to a data block (value >= first_data_block_offset)
- **An inline f32 constant** (all other values, reinterpreted as f32)


## 9. Data blocks

Packed contiguously after the offset table. Each block stores a sparse time
series:

```
  +0      2                        u16 count (stored values)
  +2      ceil(time_point_count/8) Bitmap: bit per time point
  +2+bm   count * 4                f32 values in time order
```

Block 0 is always the time series itself (fully dense bitmap).


## 10. Section 6: OT metadata

Section 6 is the richest source of VDF-native mapping information. Its layout:

1. Optional one-word prefix
2. Leading ref stream: `u32 n_refs; u32 refs[n_refs]` entries
3. **OT class-code array**: `offset_table_count` bytes, one per OT entry
4. **OT final-value array**: `offset_table_count` little-endian f32 values
5. **Display records**: `13 * u32` fixed-width records, terminated by zero word

### OT class codes

The class-code array is the primary VDF-native stock/non-stock signal.
Codes are **contiguous**: all stock entries form a single block at OT[1..S],
followed by all non-stock entries at OT[S+1..N-1].

| Code | Meaning | OT range |
|------|---------|----------|
| 0x0f | Time | OT[0] only |
| 0x08 | Stock-backed variable | OT[1..S] |
| 0x11 | Dynamic non-stock (data block) | OT[S+1..N-1], interleaved |
| 0x17 | Constant non-stock (inline f32) | OT[S+1..N-1], interleaved |

Validated counts:

| Model | Stocks (0x08) | Dynamic (0x11) | Constant (0x17) | Total |
|-------|---------------|----------------|-----------------|-------|
| water |  1            |  3             |  5              |  10   |
| pop   |  2            |  3             |  7              |  13   |
| econ  | 11            | 37             | 29              |  78   |
| WRLD3 | 41            | 174            | 81              | 297   |

### Display records

The display records at the end of section 6 correspond **1:1 with lookup
table definitions** in the VDF name table. Each display record's word[10]
contains the OT index for that lookup table.

This is a direct VDF-native name-to-OT mapping for all graphical function
definitions. All lookupish names (containing " lookup" or " table") have
OT entries as inline constants (code 0x17).

| Model | Display records | Lookupish names | Match |
|-------|----------------|-----------------|-------|
| econ  | 4              | 4               | 1:1   |
| WRLD3 | 55             | 55              | 1:1   |


## 11. Section 7: lookup table storage

Section 7 stores all graphical function / lookup table definitions as packed
f32 arrays, followed by the offset table and data blocks.

### Layout

```
  [section header: 16 bytes (magic + field1 + field2 + field3)]
  [field4 = first lookup x-value (f32)]         <- data starts here
  [field5 = second lookup x-value (f32)]
  [...packed lookup f32 data...]
  [4-5 zero u32 padding]
  [OFFSET TABLE]
  [DATA BLOCKS]
```

**field1** counts total f32 values from field4 through part of the offset
table (the boundary extends ~5 entries into the OT).

### Lookup table packing

Each table: `[x_0, x_1, ..., x_n, y_0, y_1, ..., y_n]` -- x-values as a
contiguous f32 array followed immediately by y-values. Tables are packed with
**no per-table headers, counts, or separators**. Boundaries are inferred from
x-value monotonicity (x-values increase within a table; y-to-next-x breaks
monotonicity).

Tables appear in the same order as their lookupish names in the VDF name
table (typically alphabetical).


## 12. Section 5: dimension sets (array models)

In array-heavy files, section 5 contains structured entries:

```
  u32 n;
  u32 0;
  u32 refs[n+1];
```

The `n+1` sizes correspond to model dimension cardinalities + 1. This is
useful for array element OT decoding.


## 13. Name-to-OT mapping: current approach

### What is solved

1. **Section-6 class codes** give VDF-native stock/non-stock classification
   with contiguous stock block at OT[1..S].

2. **Section-6 display records** give direct OT assignments for all lookup
   table definitions (1:1 correspondence with lookupish names).

3. **Stocks-first-alphabetical ordering**: among non-lookup participants,
   stocks sort alphabetically into OT[1..S] and non-stocks into OT[S+1..N-1].
   Validated on small models; consistent with large model diagnostics.

4. **Internal variable classification**: `#`-prefixed signature names encode
   stock/non-stock in their prefix pattern (e.g., `#LV1<` = stock,
   `#DELAY3(` = non-stock). Participant helper names (DEL, LV1, LV2, LV3,
   ST, RT1, RT2, DL) have deterministic classification.

### Current production path

`VdfFile::to_results_with_model(project, model_name)` uses:
1. Model-based stock classification
2. VDF name table filtering (builtins, metadata, module names removed)
3. Stocks-first-alphabetical ordering with section-6 stock boundary
4. SMOOTH/DELAY alias resolution via compiled module structure

This works correctly on small models (water, pop) and partially on econ.

### What remains for VDF-only mapping

Two problems prevent fully VDF-native (no model) mapping on larger files:

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


## Hypotheses tested and ruled out

These approaches were investigated and found unreliable for the name-to-OT
mapping problem:

- **f[10] as global alphabetical key**: works on tiny models (bact, water,
  pop) but Kendall's tau = 0.46 on WRLD3. Not general.

- **Record f[12] groups as name anchors**: records are sparse (23 of 296 OTs
  have records in WRLD3). Not enough coverage.

- **Slot 16-byte payloads as durable keys**: change across Vensim
  versions/reruns. Not stable.

- **Section-6 leading refs as a save list**: overlap between saved and unsaved
  names. Not discriminating.

- **Name-based lookup heuristics**: names containing " table" were initially
  assumed to lack OT entries, but display records proved ALL lookupish names
  have OTs.

- **First-fit OT allocation using model offsets**: produced unreliable
  mappings on econ/WRLD3. Deleted in favor of stocks-first-alphabetical.


## Known pitfalls

- **Name table builtins**: Vensim embeds function names (SUM, MIN, step)
  alongside model variables. These must be filtered.

- **Offset table constant ambiguity**: f32 constants like `4.8e9` produce u32
  values within file-offset range. Distinguish by comparing against
  `first_data_block_offset`.

- **Mixed control/model record groups**: Some VDFs (notably pop) mix system
  records (f[1]=23) and model records in the same f[12] group. Filter
  per-record, not per-group.

- **Section 5 degenerate**: In small models, section 5's next header starts
  before its data offset, yielding zero region data. This is structural, not
  a parsing error.

- **f[11] overflow in large models**: Some records have f[11] exceeding the OT
  count. Exclude via `f[11] < offset_table_count`.
