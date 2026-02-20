"""CFFI build configuration for simlin.

Reads the cbindgen-generated simlin.h directly rather than maintaining
a hand-written copy of the C declarations.  The header is lightly
transformed to strip preprocessor directives that CFFI's cdef parser
cannot handle (#include, include guards, extern "C").
"""

import os
import platform
import re
from pathlib import Path

from cffi import FFI

ffibuilder = FFI()

# Paths
repo_root = Path(__file__).resolve().parents[3]
libsimlin_dir = repo_root / "src" / "libsimlin"
header_path = libsimlin_dir / "simlin.h"


def _header_to_cdef(header_text: str) -> str:
    """Transform a cbindgen-generated header into CFFI-compatible cdef text.

    Strips preprocessor directives that CFFI cannot handle while
    preserving type definitions, struct layouts, and function
    declarations.  ``#define NAME (expr)`` constants are converted to
    the CFFI ``#define NAME ...`` placeholder form so their values are
    resolved at compile time from the real header.
    """
    lines = header_text.split("\n")
    result_lines: list[str] = []
    skip_depth = 0

    for line in lines:
        stripped = line.strip()

        # Skip include guard (#ifndef / #define GUARD / #endif at end)
        if stripped.startswith("#ifndef"):
            continue
        if re.match(r"#define\s+SIMLIN_ENGINE", stripped):
            continue

        # Skip #include directives
        if stripped.startswith("#include"):
            continue

        # Skip #ifdef __cplusplus / extern "C" blocks and
        # #if defined(...) feature-gated blocks (e.g. SIMLIN_PNG_RENDER)
        if stripped == "#ifdef __cplusplus" or re.match(
            r"#if\s+defined\(", stripped
        ):
            skip_depth += 1
            continue
        if skip_depth > 0:
            if stripped.startswith("#endif"):
                skip_depth -= 1
            continue

        # Skip bare extern "C" and its closing brace
        if 'extern "C"' in stripped:
            continue
        if stripped.startswith("}") and "extern" in stripped:
            continue

        # Skip trailing #endif (close of include guard)
        if stripped.startswith("#endif"):
            continue

        # Convert #define constants to CFFI placeholder syntax
        m = re.match(r"#define\s+(SIMLIN_\w+)\s+.+", stripped)
        if m:
            result_lines.append(f"#define {m.group(1)} ...")
            continue

        result_lines.append(line)

    text = "\n".join(result_lines)

    # Convert opaque cbindgen structs (uint8_t _private[0]) to simple
    # forward declarations that CFFI treats as opaque pointer targets.
    text = re.sub(
        r"typedef struct \{\s*uint8_t _private\[0\];\s*\} (\w+);",
        r"typedef struct \1 \1;",
        text,
    )

    # CFFI needs uintptr_t mapped to size_t since it doesn't process
    # <stdint.h>.
    return "typedef size_t uintptr_t;\n\n" + text


header_text = header_path.read_text()
cdef_content = _header_to_cdef(header_text)
ffibuilder.cdef(cdef_content)


def get_library_path() -> str:
    lib_name = "libsimlin.a"
    # Common locations: native release, cross-target release, then debug
    workspace_target = repo_root / "target"
    candidates = [
        # Workspace targets (default for cargo in a workspace)
        workspace_target / "release" / lib_name,
        *list(workspace_target.glob("*/release/" + lib_name)),
        # Crate-local targets (if building in crate dir)
        libsimlin_dir / "target" / "release" / lib_name,
        *list((libsimlin_dir / "target").glob("*/release/" + lib_name)),
        libsimlin_dir / "target" / "debug" / lib_name,
    ]
    for path in candidates:
        if path.exists():
            return str(path)
    raise RuntimeError(
        f"libsimlin library not found. Build with: cd {libsimlin_dir} && cargo build --release"
    )


asan_enabled = str(os.environ.get("ASAN", "")).lower() in ("1", "true", "yes", "on")

extra_compile_args = []
extra_link_args = []

if platform.system() == "Linux":
    extra_link_args.append("-lm")
    extra_link_args.append("-lstdc++")

if asan_enabled:
    # Ensure the CFFI module is compiled and linked with ASan when consuming a
    # sanitized libsimlin.a. This avoids unresolved sanitizer symbols.
    extra_compile_args.append("-fsanitize=address")
    extra_link_args.append("-fsanitize=address")

ffibuilder.set_source(
    "simlin._clib",
    """
    #include <stdint.h>
    #include <stdbool.h>
    #include "simlin.h"
    """,
    include_dirs=[str(libsimlin_dir)],
    libraries=[],
    extra_objects=[get_library_path()],
    extra_link_args=extra_link_args,
    extra_compile_args=extra_compile_args,
)


if __name__ == "__main__":
    ffibuilder.compile(verbose=True)
