# VDF Binary Format (Vensim Data File)

## Overview

VDF is Vensim's proprietary, undocumented binary format for simulation output.
The format preserves runtime-oriented structures: the offset table behaves like
the saved runtime variable array, section-1 records behave like variable
descriptors, section 2 is the string pool, section 3 is an array-shape table.
This is why internal SMOOTH/DELAY helper variables appear in a VDF -- they exist
in the simulation state and are saved as part of it.

Vensim can open a `.vdf` file and show its contents without the `.mdl` model,
because the file carries enough metadata to map variable names to time series.
This document describes that metadata and the deterministic procedure that
reconstructs a `Results` struct (variable names -> time series) from a VDF
alone. The implementation is in `src/simlin-engine/src/vdf.rs` and its
submodules; `tools/vdf_xray.py` is a structural inspector for the same format.

**Conventions.** All values are little-endian. Numeric data is 32-bit floats
unless noted. All offsets are byte offsets.

**Container kinds.** The first four bytes identify the container:

| Magic | Kind | Layout |
|---|---|---|
| `7f f7 17 52` | simulation run | 8 sections (described here) |
| `7f f7 17 41` | dataset / reference mode | 5 sections (see "Dataset sibling format") |
| `7f f7 17 53` | sensitivity / optimization run | 8 sections plus a `header[0x68]`-anchored payload |

There is **no Vensim version field** anywhere in the file: a corpus survey
across 2005-2026 found no header word or section-header field that partitions
files by era. The format evolved compatibly -- 2008-era Vensim writes small
integers into the section-1 pre-record header region where 2019+ writes
arena-pointer-shaped values, and a reader simply tolerates whatever is there
because those words are not read for decoding. The magic byte is the only
structural fork.


## File layout

```
  +--------------------------------------+
  | File header (168 bytes)              |  0x00..0xA7
  +--------------------------------------+
  | Section 0: simulation command        |  starts at 0xA8
  +--------------------------------------+
  | Section 1: string table + records    |
  |   - 12-byte preamble + 3 header blocks
  |   - 64-byte variable metadata records
  |   - slot table                       |
  +--------------------------------------+
  | Section 2: name table                |
  +--------------------------------------+
  | Section 3: array shape directory     |  (zero-filled in scalar models)
  +--------------------------------------+
  | Section 4: view/sketch metadata      |
  +--------------------------------------+
  | Section 5: dimension sets            |  (degenerate in scalar models)
  +--------------------------------------+
  | Section 6: OT metadata               |
  |   - section-6 ref/record streams     |
  |   - OT class-code array              |
  |   - OT final-value array             |
  |   - lookup mapping records           |
  +--------------------------------------+
  | Section 7: lookup data + OT + blocks |
  |   - packed lookup f32 arrays         |
  |   - offset table                     |
  |   - sparse data blocks               |
  +--------------------------------------+
```


## File header

168 bytes, `0x00..0xA7`. Section 0's magic bytes begin at `0xA8`.

| Offset | Size | Description |
|---|---|---|
| `0x00` | 4 | Magic bytes (`7F F7 17 52` for run files) |
| `0x04` | 116 | ASCII timestamp + origin, null-terminated, e.g. `"(Sun Nov 30 23:28:16 2008) From bact.mdl"` |
| `0x58` | 4 | `u32` absolute file offset of the section-6 OT final-values array |
| `0x5C` | 4 | `u32` absolute file offset of the section-6 lookup mapping records (one past the final-values array) |
| `0x60` | 4 | `u32` absolute file offset of the section-7 offset table |
| `0x64` | 4 | duplicate of `0x60` |
| `0x70` | 4 | total lookup coordinate-pair count across all graphical functions (zero when the model has none) |
| `0x78` | 4 | `u32` saved time-point count -- the number of values stored in the Time block and returned to callers |
| `0x7C` | 4 | `u32` block time-point grid count; usually equals `0x78`, but saved-suffix runs (`risk.vdf`) have `0x78=213`, `0x7C=225` |

Derived quantities:

- **OT count** = `(header[0x5C] - header[0x58]) / 4`
- **OT class-code array** starts at `header[0x58] - OT_count` (one byte per OT entry)
- **OT final-value array** starts at `header[0x58]` (one f32 per OT entry)
- **Offset table** starts at `header[0x60]`
- **First data block** = the `u32` at `header[0x60]` (OT entry 0, the Time block)

Bytes in `0x80..0xA7` are mostly zero padding plus runtime-state residue (one
word at `0x94` carries either a small integer or a RAM-pointer-shaped value and
is volatile across reruns of the same model). The parser locates section 0 by
scanning for the section magic starting at `0x80`, so this region does not
affect decoded output.


## Section framing

Every section is delimited by a 4-byte magic value and has a 24-byte header:

| Offset | Size | Description |
|---|---|---|
| `+0` | 4 | Section magic: `A1 37 4C BF` |
| `+4` | 4 | `u32 field1` |
| `+8` | 4 | `u32 field2` (equals `field1` in every observed file) |
| `+12` | 4 | `u32 field3` -- a per-section "kind" code: `135` on the name-table section and the array-directory section, `100` on the OT-metadata section, `500` everywhere else |
| `+16` | 4 | `u32 field4` |
| `+20` | 4 | `u32 field5` |

A section's data region runs from the end of its 24-byte header to the start of
the next section's magic; the last section runs to end-of-file. **Identify
sections by index, not by `field4`** -- `field4` values vary across files.

`field1` is a 1-based word pointer from the section magic:
`section.file_offset + 4 * (field1 - 1)`. For **section 1** it points at the
first entry of the slot table (see "Slot table"); for section 6 it points at
the OT class-code array start (`header[0x58] - OT_count`); for section 7 it
points at `header[0x60]`; for section 5 it points at the section's final word
(`region_end - 4`). (In dataset files the record/header area and slot table
shift into section 0, so section 0's `field1` plays section 1's role.)

| Index | Role |
|---|---|
| 0 | Simulation command string (~39-40 bytes, e.g. `sim bact -I`) |
| 1 | String table, variable metadata records, slot table |
| 2 | Name table |
| 3 | Array shape directory (zero-filled in scalar models) |
| 4 | View/sketch metadata |
| 5 | Dimension sets (degenerate in scalar models) |
| 6 | OT class codes, final values, lookup records, ref/record streams |
| 7 | Packed lookup data, offset table, sparse data blocks |


## Section 1: string table, variable metadata records, slot table

Section 1's data region holds three packed sub-structures.

### Header region

The first 204 bytes are reserved: a 12-byte preamble followed by three 64-byte
"header blocks". They never represent variable records. Most of the 51 `u32`
words here are byte-identical across reruns of the same model; only
`block0[14]`, `block0[15]`, and `block1[1]` vary (as a deterministic
`(N-1, N, N+1)` triple). Block 2 carries a float-`1.0` marker at `block2[9]`.

Three words in the preamble are stable cross-corpus invariants:

- `data[0..4] == 124` (a base-slot offset constant; `188` on WRLD3-03 SCEN01.VDF)
- `data[4..8] == OT_count - 1 - max_stock_ot_index`, where `max_stock_ot_index`
  is the largest OT index whose class code is `0x08`
- `data[8..12] == count of section-6 lookup mapping records`

Block 1 also satisfies `block1[10] >> 16 == block1[11]` on every observed file.
**`block1[7]` is the slot count** -- the number of slot-table entries -- on
every run-file and dataset VDF in the corpus (see "Slot table").

The first variable record starts at `sec1.data_offset() + 204` and records
follow on 64-byte strides until just before the slot table. A few files leave a
sub-64-byte trailer; it is not a record.

### Variable metadata records (64 bytes, 16 `u32` fields)

| Field | Name | Meaning |
|---|---|---|
| 0 | `type_flags` | variable type/flags; `0` on padding records |
| 1 | `classification` | semantic variable kind. `138` marks a **view header** (also `f[0]==0`); `23` a system variable. The byte structure relates to but does not directly encode stock/non-stock (use the section-6 class code at `f[11]` for that) |
| 2 | `name_key` | **direct section-2 name-table key**: `f[2] == (name_string_start - sec2_data_start) / 4 + 7`, where `name_string_start` is the first character of the name (after any `u16` length prefix). The first name uses key `7`. This is integer pointer arithmetic, not a sort rank, so it is stable on edited and compilation-order files |
| 6 | `shape_code` | `5` = scalar (one OT slot); `32` = generic arrayed marker (binds when exactly one section-3 entry has a non-zero flat size); other non-zero values are section-3 shape keys; `0` = non-shape (padding, dimension anchors/elements, builtins, descriptors) |
| 8 | `group_or_sentinel` | `0xf6800000` on many owner/system/descriptor records; otherwise a compact record-group id (a dimension anchor and its element records share this value -- see "Section 5") |
| 9 | `sentinel_b` | paired `0xf6800000` with field 8 on owner-like records |
| 10 | `sort_key` | view-local case-insensitive alphabetical order key; `0` on some stock/system records; used only as a descriptor tie-break |
| 11 | `ot_or_lookup_index` | **union field.** Owner records: the OT block start index (arrayed variables point at the first of N consecutive OT entries). Graphical-function descriptor records: a zero-based index into the section-6 lookup-record array. `0` is never an owner OT start (OT[0] is Time). Validate `1 <= f[11] < OT_count` and that the OT class code at `f[11]` is a real-data code before treating it as an owner |
| 12 | `slot_ref` | byte offset into section-1 data; groups records by view/sector |
| 14 | `has_lookup_marker` | `0xf6800000` when the variable is associated with a lookup table (a standalone definition or a `WITH LOOKUP` expression), `0` otherwise. This is *not* the owner/descriptor discriminator |

Fields 3, 4, 5, 7, 13, 15 are zero or per-variable values of unknown meaning.

The format does not store a tag that distinguishes a graphical-function
**descriptor** record (whose `f[11]` is a lookup-record index) from an **owner**
record (whose `f[11]` is an OT start): every field's value set on descriptor
records is a subset of the owner records' value set, and the section-6 lookup
record carries no back-pointer. A reader that has the model knows the descriptor
set; a model-free reader recognises it from the lookup-def names (see
"Name-to-OT mapping"). Once the descriptor records are set aside, the remaining
owner spans form a clean, non-overlapping OT partition.

### Slot table

An array of `N` `u32` values, each a byte offset into section-1 data. The
pairing is direct: `slot_table[i]` belongs to `names[i]`. (`#`-signature
internal-helper names sit past the slotted prefix in the name table and have no
slot-table entry, so `N <= name_count`.)

The table's location is **fully determined by the header** -- no scan or stride
heuristic is needed:

- **start** = section 1's `field1` 1-based word pointer
  (`sec1.file_offset + 4 * (field1 - 1)`);
- **count** `N` = `block1[7]` (the actively-written slot count);
- the table is followed by a single `0x00430000` terminator word, then the
  name-table section magic.

These three facts over-determine each other -- `start + (N + 1) * 4` equals the
name-table section's file offset on every run-file and dataset VDF in the corpus
(138 run files + 6 datasets, zero exceptions). A reader can take any two and
cross-check the third; `vdf::slot_table_from_header` reads `field1` + `block1[7]`
and verifies the terminator and boundary. (An earlier reader scanned backward
for "the largest run of unique, in-range, 4-byte-aligned offsets"; that heuristic
under-counted on edited files whose name table contained stale entries and
over-counted by one elsewhere. It has been replaced by the structural decode.)


## Section 2: name table

The name table is a superset of the stored variables. The section header's
`field5 >> 16` gives the byte length of the first name; that first name (`Time`
in run files) has no length prefix. Every subsequent entry is a `u16`
length-prefixed string. A `u16` value of `0` is a group separator. Some edited
files contain length-prefixed entries whose payload is non-printable binary --
treat these as stale/deleted entries and skip exactly the declared byte count
rather than stopping the table.

Name categories (a classification aid, not an ownership rule -- validate a saved
series through a record's `f[2]`/`f[11]`/`f[6]` and the section-6 class code):

| Category | Recognition | OT relationship |
|---|---|---|
| System variables | exact match `Time`, `INITIAL TIME`, `FINAL TIME`, `TIME STEP`, `SAVEPER` | `Time` is OT[0]; the rest have ordinary owner records |
| Model variables | slotted user names | have owner records |
| Lookup definitions | section-6 lookup records and lookupish names | may be definitions only, emitted series, or descriptors overlapping evaluated outputs |
| Internal signatures | prefix and suffix `#` (`#SMOOTH(x,3)#`, `#alias>SMOOTH#`, `#LV1<...#`) | own real OT slots when the runtime helper is saved |
| Stdlib helpers | exact match `DEL`, `LV1`, `LV2`, `LV3`, `ST`, `RT1`, `RT2`, `DL` | own OT slots when saved |
| Group/view markers | prefix `.` | no direct OT entry |
| Unit annotations | prefix `-` | no direct OT entry |
| Builtin function names | exact match `SUM`, `MIN`, `step`, ... | usually no saved series |
| Metadata tags | prefix `:` | no direct OT entry |

### Stdlib-call `#` signatures

Vensim emits stdlib-call output and internal-stock names in two encodings:

| Style | Output signature | Internal stocks |
|---|---|---|
| Old | `#FUNCNAME(args)#` | `#LV1<FUNCNAME(args)#`, `#DL<...#`, ... |
| New | `#alias>FUNC#` | `#alias>FUNC>LV1#`, `#alias>FUNC>DL#`, ... |

The new-style form encodes the user alias directly in the prefix; the old-style
form leaves it implicit. An *output* signature (the name a user alias binds to)
is recognised by a positive structural signal -- a `(` for old-style, exactly
one top-level `>` for new-style -- which rejects non-stdlib `#`-bracketed
display names and the multi-`>` sub-part names that stateful macros like
`RAMP FROM TO` emit. `VdfFile::output_signatures` and
`VdfFile::new_style_alias_signatures` expose these.


## Section 3: array shape directory

Scalar models keep section 3 as 104 zero bytes (`field4 == 0`). Array models
store a 25-word zero prefix, a run of fixed-width 27-word entries, and a single
trailing zero word.

| Word(s) | Meaning |
|---|---|
| 0 | `index_word`: self-positional, `(entry_file_offset - sec3_file_offset) / 4`. The last entry has `index_word == 0`. Multi-shape directories (`Ref.vdf`) chain entries so a record's `f[6]` points at the *previous* entry's `index_word` and the following physical entry carries the shape |
| 1..3 | packed shape: one-dimensional entries duplicate the flattened size (`[3, 3]`); composite entries store `[flat_size, axis_a, axis_b]` (`[21, 7, 3]`), where `flat_size == product(axis_sizes)` |
| 10 | packing hint: `1` for one-dimensional entries, the trailing axis size for composite entries |
| 18..19 | one axis ref per encoded axis -- a section-1 word pointer to `field[9]` of the dimension-anchor record for that axis: `axis_ref == 60 + 16 * k`, where `k` is the anchor's record index. These are **not** slot-table refs |
| 26 | encoded axis count (`1` or `2` in the validated corpus) |

The decoded shape normalizes to `flat_size`, `axis_sizes` (one per axis), and
`axis_refs` (one anchor pointer per axis). The same template can be referenced
by several record `f[6]` values, which is why section 3 is a shape *directory*
rather than a per-variable save list.


## Section 4: view/sketch metadata

Variable-length structured entries that reference section-1 slot values and
encode view/sketch (diagram) information. Each entry is a packed count word
`p`, then `(p >> 16) + (p & 0xffff)` slot refs, then a trailing self-positional
`index_word` (`(entry_file_offset - sec4_file_offset) / 4`; the last entry's is
`0`, acting as a terminator). All parsed refs resolve to in-range section-1
offsets that also appear in the slot table. This section is view-connector
metadata; it is **not** a variable-owner or shape-owner directory. Numeric
overlap between section-3 and section-4 `index_word` values is an arithmetic
coincidence (both encode `index_word` self-positionally).


## Section 5: dimension sets

In scalar models section 5 is degenerate -- the next section header starts
before section 5's data offset, so it has zero region data.

In array models, section 5 holds `u32 n; u32 marker; u32 refs[refs_len]`
entries (`marker == 0` => `refs_len == n + 1`; `marker == 1` => `refs_len ==
n + 2`; the trailing one or two refs are axis anchors, the leading `n` are the
payload). The entries start immediately at the section's data offset.

**Section-5 entries pair 1:1 with record `field[8]` dimension-anchor groups.**
Sorting the anchors by `f[8]` ascending produces a sequence whose cardinalities
match `sec5[i].n` pointwise (validated across the array corpus, including
`Ref.vdf`'s 18 dimensions).

**Root dimensions list their elements directly via record `field[8]` groups:**
the dimension anchor carries the group's `f[8]` value (and usually the `f[14]`
sentinel; on anchors `f[11]` is a compact dimension id, not an OT start); each
element record has the same `f[8]`, `f[6] == 0`, `f[10] == 0`, `f[12] == 124`,
and a zero-based element index in `f[11]`. Element records may be out of file
order, so `f[11]` is the ordering key. A mixed catalog can also use a compact
late-record layout (`f[12]` = group id, `f[15]` = element index, `f[6]` = the
section-2 name key) -- `Ref.vdf`'s `scenario` does this for two of its three
elements.

**Subrange dimensions recover their elements from the parent root.** A
subrange's section-5 payload is a strict in-order subsequence of its parent
root's payload; the positions where the subrange's refs occur in the parent's
payload are the element indices into the parent's element list. The root is the
dimension whose payload is not a subsequence of any other dimension's payload
(when a subrange matches multiple candidates, prefer the actual root). The
payload refs themselves resolve to unrelated variable slots -- the VDF uses
their physical slot identity as opaque "axis-participation tokens"; only the
subsequence relationship is load-bearing.


## Section 6: OT metadata

Layout, in order:

1. Skip `max(0, sec6.field4 - 1)` 4-byte words. Almost always 0 (when
   `field4 == 1`); when `field4 == 2`, one section-1-descriptor-offset-shaped
   prefix word of unknown binding.
2. **Leading ref stream**: variable-length `u32 n_refs; u32 refs[n_refs]`
   entries. The refs resolve to a mix of model variables, unit annotations,
   view markers, builtin names, system variables, and stdlib helpers -- not a
   clean variable save list.
3. **Post-ref record region** (empty on small/medium fixtures): a stream of
   fixed-width 16-byte records. On `Ref.vdf` (226 records) these form a
   linked-list node pool: `word[0]` is runtime residue, `word[1]` is an OT
   start, `word[2]` an OT width, `word[3]` the next node's 1-based section-6
   word pointer (or 0). A reader walks each lookup's input-dependency chain in
   O(n) from the lookup record's `word[12]`.
4. **OT class-code array**: `OT_count` bytes, one per OT entry. Boundary fact:
   this array starts at both `header[0x58] - OT_count` and
   `sec6.file_offset + 4 * (sec6.field1 - 1)`.
5. **OT final-value array**: `OT_count` little-endian f32 values (the last saved
   value for dynamic entries, or the constant itself for inline-constant
   entries).
6. **Lookup mapping records**: a stream of 13-`u32` records terminated by a
   single zero word.

### OT class codes

| Code | Meaning |
|---|---|
| `0x0f` | Time (OT[0] only) |
| `0x05` | input / data-like block (uses the `ceil(0x7C/8)` bitmap width on saved-suffix files) |
| `0x08` | stock-backed variable. Contiguous at `OT[1..S]` in small/medium fixtures; scattered across several ranges in `Ref.vdf` |
| `0x11` | dynamic non-stock; usually a per-step data block, but inline in some array-heavy files |
| `0x16`, `0x18` | observed only in `Ref.vdf`; inline OT values |
| `0x17` | constant non-stock; inline f32 in the final-value array |

Stock counts validated across the small/medium corpus (water: 1 stock / 3
dynamic / 5 const / 10 total; pop: 2/3/7/13; econ: 11/37/29/78; WRLD3:
41/174/81/297).

### Lookup mapping records

These describe graphical-function definitions. Each record is 13 `u32` words:

| Word | Role |
|---|---|
| 0..4 | IEEE floats: graph/rendering metadata (y-min, y-max, x-min, x-max, slope hints) |
| 5 | section-7 word offset to the start of the x-array |
| 6 | section-7 word offset to the start of the y-array |
| 7 | xy-pair-count derivative (observed `{0, w8-2, w8-1, 0xffffffff}`) |
| 8 | xy-pair count (`word[8] == word[6] - word[5]`) |
| 9 | runtime arena pointer; not a file offset |
| 10 | evaluated-output OT (the OT of a *consumer* of the lookup, not the lookup-def name's own record; can be shared by several lookup records) |
| 11 | output width |
| 12 | optional 1-based section-6 word pointer to the root of a post-ref dependency chain (zero when the lookup has no dependencies) |

The lookup-record array is in **case-insensitive alphabetical order of the
lookup-definition names**, so a descriptor record's `f[11]` (a zero-based index
into this array) is a direct, O(1) link to the lookup's x/y arrays and output
OT. There is no reverse link from a lookup record to its descriptor record.


## Section 7: lookup data, offset table, data blocks

Section 7 packs three sub-structures, with no separators between lookup tables:

```
  [section header: 16 bytes]
  [field4 = first lookup x-value (f32)]   <- lookup data starts here
  [field5 = second lookup x-value (f32)]
  [...packed lookup f32 data...]
  [4-5 zero u32 padding]
  [offset table]
  [data blocks]
```

### Lookup table packing

Each lookup table is `[x_0..x_n, y_0..y_n]` -- a contiguous f32 x-array
followed immediately by the y-array. Tables appear in lookup-definition order
(matching the section-6 lookup-record array). Table boundaries are inferred
from x-value monotonicity, but the section-6 lookup records' `word[5..6]` give
the exact x/y offsets directly. The section header's `field4`/`field5` double
as the first two f32 values, so `sec7.data_offset()` is already two words into
the lookup-data stream.

### Offset table

`OT_count` `u32` entries (one per OT entry, including OT[0] = Time), starting at
`header[0x60]`. Each entry is either a **file offset to a data block** (value
`>= first_data_block_offset`) or an **inline f32 constant** (any smaller value,
reinterpreted as f32). A raw `0` decodes as the constant `0.0` for
constant-like class codes, but on `Ref.vdf` raw-`0` entries with class `0x11`
and final value `-1.3e33` are missing/no-saved-data slots, not numeric zero.

### Data blocks

```
  +0       2          u16 count (stored values)
  +2       bm         bitmap: one bit per time point
  +2+bm    count * 4  f32 values, in time order
```

Block 0 is the Time series, with a fully dense bitmap. The reader follows OT
offsets rather than assuming the referenced blocks form a gapless stream --
files can contain padding or unreferenced bytes between blocks. A non-time
block's value at a time point with a clear bit holds (zero-order hold).

The bitmap width is decoded **per block**: most blocks use `ceil(header[0x78]
/ 8)` bytes, but saved-suffix files (`risk.vdf`) mix that with
`ceil(header[0x7C] / 8)`. The deterministic discriminator is local to the
block: the `u16 count` equals the bitmap popcount for the correct width. When a
block uses the larger grid, decode the full grid and sample the positions
corresponding to the saved Time values.


## Name-to-OT mapping

Reconstructing the result set is a single pass over the section-1 records:

1. **Decoded record spans.** For each record whose `f[2]` resolves through the
   section-2 name-key formula, whose `f[11]` is in `[1, OT_count)`, whose `f[6]`
   yields a structural flat span (`5` -> 1; `32` -> the sole section-3 flat
   size; otherwise the section-3 entry keyed by `f[6]`), and whose covered OT
   slots all carry a real-data class code (`0x05`/`0x08`/`0x11`/`0x16`/`0x17`/
   `0x18`), emit the span `[f[11], f[11] + flat_span)`. (A record whose `f[2]`
   does not resolve to a parsed name is re-save cruft, not an owner -- this is
   what discards SCEN01's "zeroed" `.Supplementary` records and `bact`'s zeroed
   records.)

2. **Descriptor pruning.** Spans that overlap in OT space form a connected
   component. Within each component, peel off the graphical-function descriptor
   record: if exactly one candidate's name is lexically lookupish (contains
   `lookup`, `table`, or `graphical function`), it is the descriptor; otherwise
   the candidate with the highest `f[10]` is treated as the descriptor (this
   fallback fires on `Ref.vdf`, where descriptor names like `RS N2O` are domain
   abbreviations). A descriptor's `f[11]` is its index into the section-6
   lookup-record array; its data lives there, not at `f[11]` as an OT start.

3. **Emit.** The remaining owner spans plus `Time` at OT[0] are the result set.
   System variables (`INITIAL TIME`, `FINAL TIME`, `SAVEPER`, `TIME STEP`) are
   ordinary records here; `#`-signature internal helpers own real OT slots and
   are emitted under their decoded names. Within each span, an OT entry that is
   a file offset reads its sparse data block; an inline f32 constant fills a
   flat series. Multi-slot (arrayed) spans whose section-3 shape resolves
   through axis refs to dimension anchors with matching cardinalities get
   element labels (`name[a]`, `name[b]`, ...); otherwise elements get numeric
   labels (`name[0]`, `name[1]`, ...).

`VdfFile::to_results_via_records` implements this. The "stocks-first
alphabetical" ordering visible in the OT array is a consequence of Vensim's
compiler allocation, not a rule a reader needs.

### Standalone graphical-function ("lookup-only") descriptors

A lookup-only variable is a **graphical function = a table indexed by an
explicit input** (`y = lookup(input)`). A *bare* lookup -- a table with no
call site of its own -- is **not a time series**, so Vensim saves no data block
for it: only a descriptor record exists, with no separate consumer-owner record.
The overlap-pruning step above never sees it (it collides with nothing), so it
would otherwise decode at its `f[11]`-as-OT-start ghost slot (a class-`0x08`
stock slot holding `0`/garbage). The reader recognises it structurally (its
ghost slots all carry the stock class code -- a lookup is never a stock -- its
`f[11]` is a valid lookup-record index, and the forward link
`lookup_record[f[11]].word[10]` is a valid owner OT, with `word[11]` matching the
descriptor's element count for the arrayed case) and **drops it**, exactly like
an overlapping descriptor (`record_results::standalone_lookup_only_descriptors`).
The table's values, where they matter, are carried by the **consumer** variables
that call it with a real input -- those are ordinary owners the reader emits
under their own names.

This is why the reader does not (and should not) reconstruct a series for a
lookup-only variable: the variable's value is `lookup(input)` for whatever input
the model passes, which the VDF does not store. The forward link only points at
*a* consumer, and the model defines how that consumer relates to the lookup --
on `Ref.vdf`: an identity pass-through (`Historical GDP[COP] = IF Time<=cutoff
THEN Historical GDP LOOKUP(Time/One year) ELSE :NA:`), a unit-scaled copy
(`RS GDP = RS GDP in trillions(...) * million per trillion dollars`), a fixed-time
snapshot (`Forestry emissions at start year = Historical forestry LOOKUP(start
year)`), or one row of a wider 2-D consumer (`rs_hfc125` is the `HFC125` column
of `RS HFC[COP, HFC type]`). Recovering the lookup variable's own series from any
of these needs the model, not the VDF. (A `gf(Time)` lowering for such a bare
lookup is an engine bug -- a table is not generally a function of time; see
#597.)

### Worked example: a `SMOOTH1` call

One `SMOOTH(in, t)` call (= `SMOOTH1`) adds: **+1 OT** (the level, class `0x08`,
inserted into the contiguous stock block); **+2 records** (a function-token stub
with `f[6] == 0` and no OT, plus a `#alias>SMOOTH#` helper record with
`f[6] == 5` and `f[11]` = the level's OT); **+5 names** (`FUNC` ×2 -- the
call-site copy and the macro-definition copy -- the two macro parameter names,
and `#alias>FUNC#`); **+3 slots** (the three slotted names). Per-macro internal
helper-slot counts: `SMOOTH1`/`SMOOTHI` 1, `SMOOTH3` 4 (LV3=output, LV2, LV1,
DL), `DELAY1` 2, `DELAY3` 7, `RAMP FROM TO` 7, `SSHAPE` 2, `SAMPLE UNTIL` 1. A
`#`-signature helper record's authoritative stock/non-stock signal is the OT
class code at `f[11]`, not its `f[1]` (those records carry opaque/recycled
`f[0]`/`f[1]` values).


## Dataset sibling format (`7f f7 17 41`)

Dataset / reference-mode files share the section framing, slot-table and
record layout, and sparse-block encoding with run files, but:

- 5 sections instead of 8
- the string/record area is in section 0 and the printable name table is in
  section 1
- section 4 starts with a zero-terminated block-offset list, then reuses the
  sparse-block encoding

For `data.vdf`, the visible dataset series are recovered by pairing section-1
names with section-0 records sorted by `(f[2], file_offset)` and mapping each
record's `f[11]` to the section-4 block list.


## Sensitivity / optimization format (`7f f7 17 53`)

These files have the same eight-section layout as run files; the ordinary
header offsets, section-6 class/final/lookup tail, offset table, and sparse
blocks parse with the same rules. Header word `0x68` is nonzero and points past
the normal sparse-block run into an additional sensitivity payload that is not
decoded -- treat any data past the normal sparse-block run as unknown.
