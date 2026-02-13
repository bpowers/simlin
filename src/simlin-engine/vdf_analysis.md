# VDF Section Model

## Region-based section model (implemented)

Each section in a VDF file is delimited by 4-byte magic `0xbf4c37a1`. The
section header's `declared_size` field describes only a "core" or "initial"
portion of the section's data. The real extent of a section -- its **region**
-- runs from its header to the start of the next section's header (magic-to-magic).

The `Section` struct captures both views:

- `declared_size` / `declared_data_end()` -- the header's size field
- `region_end` / `region_data_size()` -- the full magic-to-magic extent

`find_sections()` computes `region_end` for each section after the magic scan:
sections 0..n-1 have `region_end = sections[i+1].file_offset`, and the last
section has `region_end = data.len()`.

### Section header format

```
+0   magic (4 bytes, 0xbf4c37a1)
+4   declared_size (u32) -- "core" data bytes after header
+8   size2 (u32) -- always equals declared_size
+12  field3 (u32)
+16  field4 (u32) -- section type (for some sections)
+20  field5 (u32)
+24  ... data starts here, extends to region_end ...
```

### Section 5: degenerate/marker case

In small and econ models, section 5 has `declared_size=6` and its header
plus declared data overlaps with section 6's header. This means
`region_end <= data_offset()`, so `region_data_size()` returns 0.
Section 5 is effectively a zero-content marker in these files. In the
zambaqui model, section 5 has real content (`declared_size=477`).

### How regions simplify parsing

With the region model, structures that previously appeared to be in "gaps"
between sections are now understood to be within their section's region:

- **Section 1** (slot table): `declared_size` covers initial slot entries.
  Variable metadata records and the slot lookup table are in the rest of
  section 1's region (past `declared_data_end()`, before `region_end`).
- **Section 2** (name table): `declared_size` covers a subset of names.
  The remaining names are in the region past the declared boundary.
  `section_name_count` tracks how many names fit within `declared_size`
  (this count determines slot table sizing).
- **Section 7** (display/graph settings): full graph data arrays extend
  past the declared size within the region.

## Section header field patterns across files

```
small (Current.vdf, 3814 bytes):
  [0] 0x0000a8  declared_size=    40  f3= 500  f4=          19       f5=0x001a0000
  [1] 0x000148  declared_size=   236  f3= 500  f4=           2       f5=0x00000000
  [2] 0x000528  declared_size=   505  f3= 500  f4=          31       f5=0x00060000
  [3] 0x000d0c  declared_size=    32  f3= 135  f4=           0       f5=0x00000001
  [4] 0x000d8c  declared_size=    11  f3= 500  f4=           8       f5=0x00000000
  [5] 0x000db8  declared_size=     6  f3= 500  f4=           0       f5=0x00000000
  [6] 0x000dd0  declared_size=    15  f3= 100  f4=           1       f5=0x0000002c
  [7] 0x000e34  declared_size=    10  f3= 500  f4=           0       f5=0x00000000

econ (base.vdf, 71960 bytes):
  [0] 0x0000a8  declared_size=    41  f3= 500  f4=          20       f5=0x001a0000
  [1] 0x00014c  declared_size=  1532  f3= 500  f4=          12       f5=0x00000000
  [2] 0x001ab0  declared_size=  1016  f3= 500  f4=         682       f5=0x00060000
  [3] 0x002a90  declared_size=    32  f3= 135  f4=           0       f5=0x00000001
  [4] 0x002b10  declared_size=    28  f3= 500  f4=          25       f5=0x00000000
  [5] 0x002b80  declared_size=     6  f3= 500  f4=           0       f5=0x00000000
  [6] 0x002b98  declared_size=   187  f3= 100  f4=           2       f5=0x000005ac
  [7] 0x0030da  declared_size=   956  f3= 500  f4=           0       f5=0x3f800000 (1.0)

zambaqui (baserun.vdf, 369470 bytes):
  [0] 0x0000a8  declared_size=    43  f3= 500  f4=          22       f5=0x001a0000
  [1] 0x000154  declared_size=  7612  f3= 500  f4=         192       f5=0x00000006
  [2] 0x007fa8  declared_size=  3554  f3= 500  f4=        3191       f5=0x00060000
  [3] 0x00b730  declared_size=   113  f3= 135  f4=          32       f5=0x00000001
  [4] 0x00b8f4  declared_size=   189  f3= 500  f4=         185       f5=0x00000000
  [5] 0x00bbe8  declared_size=   477  f3= 500  f4=           1       f5=0x0000065c
  [6] 0x00c35c  declared_size=   928  f3= 100  f4=           1       f5=0x00001d7c
  [7] 0x011300  declared_size=  9356  f3= 500  f4=  1097859072       f5=0x41a00000
                                                    (f32=15.0)        (f32=20.0)
```

Consistent patterns:
- **Section 0** always at 0xa8, f3=500, f4 is a small integer (19-22), f5 high 16 bits = 0x001a
- **Section 1** f3=500, f4 is a small integer (2-192)
- **Section 2** (name table) f3=500, f5=0x00060000 (high 16 bits = 6 = length of "Time\0\0")
- **Section 3** always f3=135 (not 500), f5=0x00000001
- **Section 5** always tiny declared_size (6 in small/econ), degenerate in 2 of 3 files
- **Section 6** always f3=100 (not 500)
- **Section 7** field4/field5 are f32 values in zambaqui, zero/1.0 in smaller files

## Section 7's field4 is a float, not a type ID

In the zambaqui file, section 7 has `field4=0x41700000` which is `15.0` as f32,
and `field5=0x41a00000` = `20.0`. In econ, `field5=0x3f800000` = `1.0`. The data
immediately after is a sequence of round floats (25, 30, 35, 40... in zambaqui;
2, 3, 4, 5... in econ). For section 7, field4 and field5 are f32 graph-range
values, not integer metadata. This means the header format isn't fully consistent
across all sections -- field4/field5 meaning changes based on section position.
