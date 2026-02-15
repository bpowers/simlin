"""Tests for the _ffi_build header-to-cdef transformation."""

from __future__ import annotations

from simlin._ffi_build import _header_to_cdef


SAMPLE_HEADER = """\
#ifndef SIMLIN_ENGINE2_H
#define SIMLIN_ENGINE2_H

#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>

#define SIMLIN_VARTYPE_STOCK (1 << 0)

typedef enum {
  SIMLIN_ERROR_CODE_NO_ERROR = 0,
  SIMLIN_ERROR_CODE_GENERIC = 32,
} SimlinErrorCode;

typedef struct {
  uint8_t _private[0];
} SimlinProject;

typedef struct {
  char *from;
  char *to;
} SimlinLink;

#ifdef __cplusplus
extern "C" {
#endif // __cplusplus

void simlin_project_ref(SimlinProject *project);

#ifdef __cplusplus
}  // extern "C"
#endif  // __cplusplus

#endif  /* SIMLIN_ENGINE2_H */
"""


class TestHeaderToCdef:
    """Verify _header_to_cdef strips preprocessor and rewrites structs."""

    def test_strips_include_guard(self) -> None:
        cdef = _header_to_cdef(SAMPLE_HEADER)
        assert "#ifndef" not in cdef
        assert "SIMLIN_ENGINE2_H" not in cdef

    def test_strips_includes(self) -> None:
        cdef = _header_to_cdef(SAMPLE_HEADER)
        assert "#include" not in cdef

    def test_converts_defines_to_placeholder(self) -> None:
        cdef = _header_to_cdef(SAMPLE_HEADER)
        assert "#define SIMLIN_VARTYPE_STOCK ..." in cdef
        assert "(1 << 0)" not in cdef

    def test_converts_opaque_structs(self) -> None:
        cdef = _header_to_cdef(SAMPLE_HEADER)
        assert "typedef struct SimlinProject SimlinProject;" in cdef
        assert "_private" not in cdef

    def test_preserves_real_structs(self) -> None:
        cdef = _header_to_cdef(SAMPLE_HEADER)
        assert "char *from;" in cdef
        assert "SimlinLink" in cdef

    def test_strips_extern_c(self) -> None:
        cdef = _header_to_cdef(SAMPLE_HEADER)
        assert 'extern "C"' not in cdef
        assert "__cplusplus" not in cdef

    def test_preserves_function_declarations(self) -> None:
        cdef = _header_to_cdef(SAMPLE_HEADER)
        assert "void simlin_project_ref(SimlinProject *project);" in cdef

    def test_preserves_enums(self) -> None:
        cdef = _header_to_cdef(SAMPLE_HEADER)
        assert "SIMLIN_ERROR_CODE_NO_ERROR = 0" in cdef
        assert "SIMLIN_ERROR_CODE_GENERIC = 32" in cdef

    def test_adds_uintptr_typedef(self) -> None:
        cdef = _header_to_cdef(SAMPLE_HEADER)
        assert "typedef size_t uintptr_t;" in cdef

    def test_real_header_parses(self) -> None:
        """The actual simlin.h should transform without error."""
        from pathlib import Path

        header_path = Path(__file__).resolve().parents[2] / "libsimlin" / "simlin.h"
        header_text = header_path.read_text()
        cdef = _header_to_cdef(header_text)
        assert "simlin_project_open_xmile" in cdef
        assert "simlin_sim_run_to_end" in cdef
