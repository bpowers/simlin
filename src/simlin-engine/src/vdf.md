# VDF Binary Format (Vensim Data File)

VDF is Vensim's proprietary binary format for simulation output. The format is
completely undocumented and no open-source parser previously existed. This
document describes the format as reverse-engineered from multiple VDF files of
varying complexity:

- **Small models** (3-8 variables, 3-7KB): `bact`, `water`, `pop` from
  `third_party/uib_sd/fall_2008/sd202/assignments/`
- **Medium model** (~420 variables, 333KB): `WRLD3-03/SCEN01.VDF`
  (World3-03 from the Limits to Growth model)
- **Large model** (1.8MB): `xmutil_test_models/Ref.vdf` (C-LEARN model)

All values are little-endian. All offsets in this document are byte offsets.

The parser is implemented in `vdf.rs`.


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
  | Sections 3..N             |  (additional metadata sections)
  +---------------------------+
  | Offset table              |  (N u32 entries, immediately before data)
  +---------------------------+
  | Data blocks               |  (packed sparse time series)
  +---------------------------+
```

The sections *after* the name table section (sections 3-7) appear to contain
display/graph settings and other Vensim UI metadata. Their content is not
needed for data extraction.


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
Every observed VDF file has exactly **8 sections**.

### Section header (24 bytes)

```
  Offset  Size  Description
  ------  ----  -----------
  +0      4     Section magic: A1 37 4C BF (= f32 -0.797724 = u32 0xBF4C37A1)
  +4      4     u32 size (byte count of section data following this header)
  +8      4     u32 size2 (always equals size)
  +12     4     u32 field3
  +16     4     u32 field4 (varies per section; not a reliable type identifier)
  +20     4     u32 field5 (for name table: high 16 bits = first name length)
```

Section data immediately follows the 24-byte header and is `size` bytes long.

### Section ordering

All observed VDF files have 8 sections in a consistent structural layout,
though the `field4` values vary across files and do NOT form a reliable type
identifier.

Observed field4 sequences:
- **water** (small): `[18, 2, 55, 0, 8, 0, 1, 0]`
- **WRLD3** (medium): `[19, 42, 3546, 0, 152, 0, 2, 1065353216]`
- **Ref** (large): `[18, 473, 8284, 32, 387, 1, 1, 1156005888]`

The `field3` values are more consistent:
- Sections 0, 1, 2, 4, 5, 7: field3 = 500 (0x1F4)
- Section 3: field3 = 135 (0x87)
- Section 6: field3 = 100 (0x64)

### Section roles by position

| Index | Role | field3 | Notes |
|-------|------|--------|-------|
| 0 | Model info / settings | 500 | ~39-40 bytes; contains simulation command string |
| 1 | Variable slot table | 500 | One 16-byte slot per variable; size grows with model |
| 2 | Name table | 500 | Variable names; identified by field5 high bits |
| 3 | Unknown (all zeros) | 135 | Always 32 bytes of zeros |
| 4 | Unknown metadata | 500 | Variable-length; content unclear |
| 5 | Degenerate/marker | 500 | Always 6 bytes; contains section magic within data (see note) |
| 6 | Unknown metadata | 100 | Variable-length |
| 7 | Display settings | 500 | Graph/display configuration data |

**Note on section 5**: This is a degenerate section whose `size` field
(always 6) extends into the next section's header. Its data begins with the
section magic bytes `A1 37 4C BF` for section 6. This appears to be an
intentional structural quirk rather than a parsing error.

**Identifying sections by position, not field4**: The slot table section is
always at index 1 and the name table section is always at index 2. Their
field4 values vary (e.g., 2, 42, 473 for sec[1]; 55, 3546, 8284 for sec[2])
so identification must be by position.


## 3. Section 0: Model info

A small section (~39-40 bytes) that appears to contain simulation run
parameters. The first few u32 values include what looks like a command
string offset and length. The section data contains an ASCII substring like
`sim bact -I` or `sim 16-2 -I` (the Vensim simulation command).


## 4. Section 1: Variable slot table

This section contains one 16-byte "slot" per variable name. Slots are packed
at a uniform stride of 16 bytes for small models; for larger models, the
stride may vary (variable-length per-slot metadata).

The section has a small pre-slot header (typically 28-44 bytes) before the
first slot. The header size equals the minimum slot offset found in the slot
table.

Each slot consists of 4 x u32 values. The slot contents do NOT contain offset
table indices -- this was extensively tested by checking all 4 u32 words in
each slot against empirically known OT indices (no consistent match found).
The slot data's purpose remains unknown.

### Observed sizes

| Model | sec[1] size | Slot count | Slot stride |
|-------|-------------|------------|-------------|
| bact  | 204 bytes   | 10         | 16          |
| water | 268 bytes   | 14         | 16          |
| pop   | 300 bytes   | 16         | 16          |
| WRLD3 | 6764 bytes  | 138        | variable    |


## 5. Name table (section 2)

Identified by: `field5 >> 16` gives a non-zero length (2-64) for the first
name entry, and the data starts with printable ASCII text.

### Encoding

The first entry has NO u16 length prefix -- its length comes from
`field5 >> 16`. It is always `"Time"` (or another primary output variable).

All subsequent entries are prefixed with a u16 length, followed by that many
bytes of null-terminated, zero-padded string data. A u16 value of 0 is a
group separator and is skipped during parsing.

### Name categories

The name table contains several categories of names, intermixed:

1. **System variable names**: `Time`, `INITIAL TIME`, `FINAL TIME`,
   `TIME STEP`, `SAVEPER`
2. **Model variable names**: The actual simulation variables (e.g.,
   `stock`, `inflow`, `young population`)
3. **Group/view markers**: Prefixed with `.` (e.g., `.pop`, `.Control`)
   -- these correspond to Vensim model views/groups
4. **Unit names**: Prefixed with `-` (e.g., `-Year`, `-Month`)
5. **Vensim builtin function names**: Names of equation functions that the
   model uses, embedded alongside variable names. Examples observed:
   - bact model: `step`, `?`
   - water model: `min`
   - WRLD3 model: `SUM`, `PROD`, `VMIN`, `VMAX`, `ELMCOUNT`, `?`

The builtin function names are a significant pitfall -- they look like
variable names but do not correspond to any simulation variable or OT entry.

### Name completeness

For small models, the name table contains all model variables. For WRLD3,
only 83 of 290 empirically-matched variables appear in the name table;
the remaining 207 names exist in an "extended" region between the section's
declared data end and the offset table. This extended region uses the same
u16-length-prefixed encoding.

### Observed counts

| Model | Names in section | Extended names | Total | System | Groups | Units | Builtins | Model vars |
|-------|-----------------|----------------|-------|--------|--------|-------|----------|------------|
| bact  | 10              | 0              | 10    | 5      | 2      | 0     | 2        | 3          |
| water | 14              | 0              | 14    | 5      | 2      | 2     | 1        | 5          |
| pop   | 16              | 0              | 16    | 5      | 2      | 1     | 0        | 8          |
| WRLD3 | 138             | ~40            | ~178  | 5      | ~20    | ?     | 6+       | ~104       |


## 6. Variable metadata records

Located between section 1's data end and the slot table. Each record is
64 bytes (16 x u32 fields). Records are found by scanning for sentinel pairs
(two consecutive `0xf6800000` values at offsets +32 and +36 within a record),
then extending to fill the entire available space at 64-byte alignment.

Not all records have sentinel pairs -- records for lookup tables, subscript
elements, and structural metadata may have regular values in the sentinel
positions. All records at the same 64-byte alignment are included regardless.

### Record counts

| Model | Records | OT entries | f[12] groups |
|-------|---------|------------|--------------|
| bact  | 7       | 8          | 2            |
| water | 12      | 10         | 3            |
| pop   | 16      | 13         | 2            |
| WRLD3 | 334     | 297        | 45           |

### Field descriptions

All fields are u32 (little-endian, at 4-byte offsets within the record):

```
  Field   Offset  Description
  ------  ------  -----------
  f[0]    +0      Variable type or flags.
                  Values seen: 0 (padding/null), 32, 36, 40, 44 (model vars).
                  Large models also show values like 6412, 12328, 13352, 16416.
                  f[0]=0 records are padding or structural separators.

  f[1]    +4      System variable marker.
                  Value 23 marks system/control variables (INITIAL TIME,
                  FINAL TIME, TIME STEP, SAVEPER). All other records have
                  different f[1] values.

  f[2]    +8      Monotonically increasing across records within a file.
                  Possibly a byte offset into some internal Vensim table.
                  Purpose unknown.

  f[3]    +12     Appears related to name table byte offsets for some records.
                  Values 22, 33, 44, 55 mark control records in some analyses.
                  Not reliably decoded.

  f[4]    +16     Unknown. Various small values.
  f[5]    +20     Unknown. Various small values.
  f[6]    +24     Unknown.
  f[7]    +28     Unknown.

  f[8]    +32     Often the sentinel value 0xf6800000 (f32 -1.2676506e30).
  f[9]    +36     Often the sentinel value 0xf6800000. Records with both
                  f[8] and f[9] as sentinel are typically "real" variable
                  records. Records without sentinels may be lookup tables,
                  subscript elements, or structural metadata.

  f[10]   +40     Alphabetical sort key. See detailed analysis below.

  f[11]   +44     Offset table index for small models. For records where
                  f[0]!=0, f[1]!=23, f[10]>0, and f[11] < OT count, this
                  gives the correct OT index. For large models, some f[11]
                  values exceed the OT count, meaning the field has a
                  different interpretation for those records.

  f[12]   +48     Byte offset into section 1 data. Groups records into
                  clusters: multiple records sharing the same f[12] value
                  belong to the same "group" (which appears to correspond to
                  Vensim views/sectors). NOT a direct name reference.

  f[13]   +52     Always 0 in all observed files.

  f[14]   +56     Sometimes the sentinel value 0xf6800000. Otherwise small
                  values or zero.

  f[15]   +60     Unknown. Various values.
```

### Identifying model variable records

To select records representing model variables (not system variables, padding,
or structural metadata), filter on:

```
  f[0]  != 0               (not padding)
  f[1]  != 23              (not a system/control variable)
  f[10] > 0                (has a non-zero alphabetical sort key)
  f[11] > 0                (OT index > 0; index 0 is always the time series)
  f[11] < offset_table_count  (valid OT index)
```

**Important**: This filtering must be done per-record, not per-f[12]-group.
Some VDFs (notably `pop`) mix control records (f[1]=23) and model records
in the same f[12] group.


## 7. Slot table

An array of N u32 values (one per name in the name table), located between
the last variable metadata record and the name table section. There are
typically 0-4 bytes of padding between the slot table and the section magic.

Each value is a byte offset relative to section 1's data start. For small
models, the offsets have uniform stride 16. For larger models, the stride
varies (variable-length slot metadata per entry).

### Validation

The slot table is identified by checking that the N u32 values immediately
before the name table section:
- Are all unique (no duplicates)
- Are all 4-byte aligned (`value % 4 == 0`)
- Are all within section 1's data size
- Are all non-zero (minimum value > 0)
- Have minimum stride >= 4 between sorted values

### Example (bact model, 10 names)

```
  Raw:    [156, 124, 140, 172, 76, 60, 188, 108, 44, 92]
  Sorted: [44, 60, 76, 92, 108, 124, 140, 156, 172, 188]
  Stride: uniform 16 bytes between consecutive entries
```

### What slots are NOT

Extensive testing confirmed that slot data does NOT contain offset table
indices. All 4 u32 words within each 16-byte slot were checked against
empirically known OT indices -- no consistent mapping was found.


## 8. Offset table

An array of N u32 entries located immediately before the first data block.
Found by scanning backwards from the first data block for a u32 entry whose
value equals the first data block's file offset (OT entry 0 always points
to the time series data block).

### Entry interpretation

Each OT entry is either:

- **A file offset** to a data block: value >= first_data_block_offset
  AND value < file_size. These point to sparse time series blocks.
- **An inline f32 constant**: all other values. The 4 bytes are reinterpreted
  as a little-endian f32. These represent variables that are constant
  throughout the simulation (e.g., parameter values).

**Pitfall**: Some f32 constants (like 4.8e9 = 0x4F8F0D18) produce u32
values that could look like file offsets. The distinction relies on
comparing against the known first_data_block_offset.

### Observed counts

| Model | OT entries | Data blocks | Constants |
|-------|------------|-------------|-----------|
| bact  | 8          | ~5          | ~3        |
| water | 10         | ~7          | ~3        |
| pop   | 13         | ~9          | ~4        |
| WRLD3 | 297        | ~200+       | ~90+      |


## 9. Data blocks

Data blocks store sparse time series and are packed contiguously after the
offset table (starting at the first data block offset).

### Block format

```
  Offset  Size                     Description
  ------  ----                     -----------
  +0      2                        u16 count (number of stored values)
  +2      ceil(time_point_count/8) Bitmap: one bit per time point
  +2+bm   count * 4                f32 values, in time order
```

The bitmap encodes which time points have stored values. Bit `i` is at
`bitmap[i / 8] >> (i % 8) & 1`. If a bit is set, the next f32 value from
the data array corresponds to that time point.

For time points without a stored value, a zero-order hold is used (the
previous value is carried forward).

Block 0 is always the time series itself (e.g., `[0.0, 1.0, 2.0, ...]` or
`[1900.0, 1900.5, 1901.0, ...]`). Its bitmap is always fully set (all bits
= 1) and its count equals `time_point_count`.

### First block identification

The first data block is found by scanning the file (starting at offset 0x100)
for a location where:
1. The u16 count equals `time_point_count`
2. The bitmap has exactly `time_point_count` bits set
3. The first f32 value is a plausible simulation start time
   (year in 1800-2200, or 0.0)


## 10. Name-to-data mapping

This is the central unsolved problem for VDF parsing. Given a variable name,
how do you find its offset table entry (and thus its time series data)?

The metadata chain linking names -> slots -> records -> OT entries has NOT
been fully decoded despite extensive reverse engineering.

### What works: deterministic mapping (small models)

For small-to-medium models, `f[10]` serves as an alphabetical sort key that
enables direct name-to-record matching:

1. Filter records to model variables (see criteria above)
2. Sort filtered records by `f[10]`
3. Filter names from the name table: remove system names, group/unit markers
   (`.` and `-` prefixes), and Vensim builtin function names
4. Sort candidate names alphabetically (case-insensitive)
5. Pair sorted records with sorted names 1:1; each record's `f[11]` gives
   the OT index

This is implemented in `VdfFile::build_deterministic_ot_map()` and validated
against simulation output for bact, water, and pop models (all produce
perfect matches).

### What fails: deterministic mapping for large models

For WRLD3 (and presumably other large models), the deterministic approach
completely breaks down:

- **Record/name count mismatch**: WRLD3 has 260 model records but only ~104
  model variable names. Many records correspond to internal module expansion
  variables (e.g., SMOOTH3 expands to multiple stocks and flows).
- **f[10] is NOT globally alphabetical**: Kendall's tau between f[10] order
  and alphabetical order is 0.46 for WRLD3 (where 1.0 = perfect agreement).
  Only 148/219 adjacent alphabetical pairs maintain the same relative order
  in f[10].
- **0 correct matches**: When attempting the alphabetical pairing globally,
  0 out of 151 pairs match the empirically known OT indices.

### What works: empirical matching (all models)

`build_vdf_results()` and `build_empirical_ot_map()` match VDF data entries
against a reference simulation by comparing time series values at sample
points. This reliably matches 290+ variables for WRLD3-03 with < 0.5%
maximum relative error vs Vensim's output.

The downside is that a reference simulation must be run first -- the VDF
cannot be decoded standalone for large models.


## 11. Deep dive: f[10] analysis

f[10] is the most promising field for understanding the name-to-data chain,
but its behavior differs dramatically between small and large models.

### Small models: perfect alphabetical sort key

For bact, water, and pop, filtering to model variable records (f[0]!=0,
f[1]!=23, f[10]>0, f[11]>0, f[11]<OT count) and sorting by f[10] produces
a sequence that exactly matches alphabetical name order.

Example (bact model, 3 model variables):

```
  f[10]=7  f[11]=2  ->  "inflow"     (alphabetically 1st)
  f[10]=9  f[11]=5  ->  "outflow"    (alphabetically 2nd)
  f[10]=13 f[11]=3  ->  "stock"      (alphabetically 3rd)
```

Example (water model, 5 model variables):

```
  f[10]=5  f[11]=2  ->  "adjustment time"      (1st)
  f[10]=7  f[11]=3  ->  "desired water level"   (2nd)
  f[10]=9  f[11]=5  ->  "gap"                   (3rd)
  f[10]=11 f[11]=6  ->  "inflow"                (4th)
  f[10]=17 f[11]=1  ->  "water level"           (5th)
```

### Large models: correlated but not alphabetical

For WRLD3, f[10] is positively correlated with alphabetical order but far
from a perfect match:

- **Kendall's tau**: 0.4574 (concordant: 17,554; discordant: 6,536)
- **Adjacent pairs preserving order**: 148 out of 219 (67.6%)
- **Global matching accuracy**: 0/151 correct

### What f[10] is NOT

Hypotheses tested and ruled out:

1. **NOT a direct name table byte offset**: f[10] values don't correspond to
   byte positions in the name table for any observed model.

2. **NOT globally alphabetical for large models**: Kendall's tau of 0.46
   shows significant disorder. The correlation is too high to be coincidental
   but too low to be useful for matching.

3. **NOT per-f[12]-group alphabetical for large models**: Even within
   individual f[12] groups, f[10] ordering doesn't consistently match
   alphabetical name ordering.

4. **NOT a simple hash**: Values are too correlated with alphabetical order
   to be a hash function.

### What f[10] might be

The most likely hypothesis is that f[10] encodes an alphabetical index within
some Vensim-internal namespace or compilation unit that differs from the
model's overall variable list. Possible explanations for the WRLD3 breakdown:

- Vensim may compute f[10] across ALL variables including internal module
  expansions (SMOOTH3 stocks, DELAY3 flows, etc.), while the name table only
  contains user-visible variables. The alphabetical ranking of 260 variables
  (including internals) produces different relative orderings than ranking
  just the 104 user-visible ones.

- f[10] might be assigned during an intermediate compilation pass where
  variables are organized differently (e.g., by sector/view, then
  alphabetically within each sector).

- The f[10] assignment algorithm may have changed between Vensim versions.
  The small models were created with Vensim in 2008; WRLD3 may use a
  different version.

### f[10] uniqueness

Within a single VDF file, f[10] values are globally unique across all records
that have f[10] > 0. No two records share the same non-zero f[10] value.
Records with f[10] = 0 are padding or structural entries.


## 12. f[12] grouping

f[12] groups records into clusters that share the same byte offset into
section 1 data. These groups appear to correspond to Vensim views or model
sectors.

### Properties

- Multiple records can share the same f[12] value
- f[12] values are byte offsets into section 1, so they are always
  4-byte-aligned
- Groups may contain a mix of record types (model vars, control vars,
  internal module vars)
- The number of groups grows with model complexity:
  bact: 2, water: 3, pop: 2, WRLD3: 45

### What f[12] groups are NOT

f[12] does NOT provide a name-to-record mapping. The group structure
reflects Vensim's internal model organization (views/sectors), not a
lookup table for resolving variable names.


## 13. Known pitfalls and edge cases

### Name table builtins

Vensim embeds equation function names in the name table alongside model
variable names. These must be filtered out when matching names to records.
Observed builtins:

- bact: `step`, `?`
- water: `min`
- WRLD3: `SUM`, `PROD`, `VMIN`, `VMAX`, `ELMCOUNT`, `?`

The single character `?` appears to be a structural placeholder.

### Mixed control/model f[12] groups

In some VDFs (notably `pop`), system variables (f[1]=23) share the same
f[12] value as model variables. Filtering must be per-record, not per-group.

### Offset table constant ambiguity

Float constants like `4.8e9` produce u32 values (`0x4F8F0D18`) that fall
within a plausible file-offset range. The only reliable way to distinguish
constants from data block offsets is to compare against the known first data
block offset.

### Section 5 overlap

Section 5 is always 6 bytes and its data contains the section magic for
section 6. This is a structural quirk; the section's declared size extends
into section 6's header. Parsers should handle this gracefully.

### Extended name region (WRLD3)

For WRLD3, the name table section's declared data size covers only 138 names,
but ~40 additional names exist between the section's data end and the offset
table. These extended names use the same u16-length-prefixed encoding and
include variable names not found in the section proper. A complete parser
should scan beyond the section boundary.

### f[11] overflow in large models

For WRLD3, some records have f[11] values exceeding the offset table count
(297), sometimes by large amounts (e.g., f[11]=5836). These records likely
serve a different purpose (perhaps subscript dimension metadata or internal
bookkeeping) and must be excluded from name-to-OT matching by checking
`f[11] < offset_table_count`.


## 14. Open questions

1. **What is the complete name-to-data mapping for large models?** The
   metadata chain (records -> slots -> names -> OT entries) is partially
   decoded but no complete algorithm has been found that works without
   a reference simulation.

2. **What is f[10] exactly?** It behaves as an alphabetical sort key for
   small models but breaks down for large ones. Understanding its computation
   would likely unlock the full deterministic mapping.

3. **What do the 16-byte slot entries contain?** The 4 x u32 values per slot
   don't contain OT indices, but their purpose is otherwise unknown.

4. **What is f[2]?** Monotonically increasing across records, possibly a
   byte offset, but into what structure is unclear.

5. **What do sections 3-7 contain?** Sections 3, 4, and 6 have structured
   data that hasn't been decoded. Section 7 appears to contain display/graph
   settings (f32 values matching axis ranges have been observed).

6. **How does the extended name region work?** For WRLD3, ~40 names exist
   beyond the name table section's declared boundary. It's unclear whether
   this is intentional or a Vensim serialization bug, and whether the slot
   table accounts for these extra names.
