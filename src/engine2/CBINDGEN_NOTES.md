# cbindgen Header Generation Notes

## Overview
This document describes the cbindgen configuration used to generate the C header file (simlin2.h) for the engine2 FFI interface.

## Final Configuration
The cbindgen.toml configuration achieves most of the desired header structure automatically:

### Key Settings
- `prefix_with_name = true` - Prefixes enum variants with enum name
- `rename_variants = "ScreamingSnakeCase"` - Converts variants to SCREAMING_SNAKE_CASE
- Explicit type includes to control what gets exported
- Opaque struct definitions using zero-sized arrays

### Comparison with Manual Header

| Feature | Manual Header | cbindgen Header | Notes |
|---------|--------------|-----------------|-------|
| Error codes | `SIMLIN_ERR_NO_ERROR` | `SIMLIN_ERROR_CODE_NO_ERROR` | cbindgen uses full enum name |
| Loop polarity | `SIMLIN_LOOP_REINFORCING` | `SIMLIN_LOOP_POLARITY_REINFORCING` | cbindgen uses full enum name |
| Opaque structs | ✅ Clean typedefs | ✅ Clean typedefs | Identical |
| Function signatures | ✅ `simlin_*` prefix | ✅ `simlin_*` prefix | Identical |
| Documentation | ✅ Comments preserved | ✅ Comments preserved | Identical |

## Trade-offs

### cbindgen Advantages
- **Automatic generation** - No manual maintenance required
- **Consistency** - Naming patterns are predictable and uniform
- **Type safety** - Automatically stays in sync with Rust definitions
- **Documentation** - Rust doc comments automatically appear in header

### Manual Header Advantages
- **Shorter names** - More concise enum variant names
- **Full control** - Can customize naming exactly as desired

## Recommendation
Use the cbindgen-generated header (simlin2.h) for the following reasons:
1. Maintainability - Automatically stays in sync with Rust code
2. The longer enum variant names are more explicit and self-documenting
3. Eliminates risk of manual header getting out of sync
4. Standard tooling that other Rust FFI projects use

## Usage
To regenerate the header:
```bash
cbindgen -o simlin2.h
```

## Configuration Experiments Tried
1. **Per-enum rename patterns** - Not supported by cbindgen
2. **Manual variant renaming in export.rename** - Didn't work for enum variants
3. **Custom prefix patterns** - Limited to global settings
4. **Attribute-based renaming** - Would require modifying all Rust source

The final configuration represents the best balance between automation and desired output format.