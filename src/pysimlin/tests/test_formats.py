"""Tests for the single on-disk format table (``simlin._formats``).

The table is the one owner of "which bytes go with which suffix" for
``simlin.open()``, ``simlin.load()``, ``Project.save()`` and
``Project.save_as()``.  These tests enumerate every suffix arm and every
content-sniff arm so a divergence between readers and writers cannot hide.
"""

from __future__ import annotations

from pathlib import Path

import pytest

from simlin import SimlinImportError
from simlin._formats import (
    FileFormat,
    format_for_suffix,
    resolve_read_format,
    resolve_write_format,
    sniff_format,
)

NATIVE_JSON = b'{"name": "p", "simSpecs": {"startTime": 0, "endTime": 1}, "models": []}'
SDAI_JSON = b'{"variables": [{"type": "variable", "name": "x", "equation": "1"}]}'
XMILE = b'<?xml version="1.0" encoding="utf-8"?><xmile version="1.0"></xmile>'
XMILE_NO_DECL = b'<xmile xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0"></xmile>'
MDL_UTF8 = b"{UTF-8}\nx = 1\n\t~\t\n\t~\t\t|\n"
MDL_BARE = b"x = 1\n\t~\t\n\t~\t\t|\n\\\\\\---/// Sketch information\n"


class TestFormatForSuffix:
    """Every suffix arm of the table, plus case-insensitivity."""

    @pytest.mark.parametrize(
        ("name", "expected"),
        [
            ("m.stmx", FileFormat.XMILE),
            ("m.xmile", FileFormat.XMILE),
            ("m.xml", FileFormat.XMILE),
            ("m.STMX", FileFormat.XMILE),
            ("m.mdl", FileFormat.MDL),
            ("m.MDL", FileFormat.MDL),
            ("m.vpm", FileFormat.MDL),
            ("m.sd.json", FileFormat.NATIVE_JSON),
            ("m.json", FileFormat.NATIVE_JSON),
            ("m.JSON", FileFormat.NATIVE_JSON),
            ("m.pb", FileFormat.PROTOBUF),
            ("m.bin", FileFormat.PROTOBUF),
            ("m.proto", FileFormat.PROTOBUF),
            ("m.txt", None),
            ("m", None),
            ("m.stmx.bak", None),
        ],
    )
    def test_suffix_table(self, name: str, expected: FileFormat | None) -> None:
        assert format_for_suffix(Path(name)) == expected
        assert format_for_suffix(name) == expected


class TestSniffFormat:
    """Every content arm: XML, JSON (native / SD-AI / neither), Vensim, unknown."""

    @pytest.mark.parametrize(
        ("data", "expected"),
        [
            (XMILE, FileFormat.XMILE),
            (XMILE_NO_DECL, FileFormat.XMILE),
            (b"\xef\xbb\xbf" + XMILE, FileFormat.XMILE),
            (b"  \n" + NATIVE_JSON, FileFormat.NATIVE_JSON),
            (SDAI_JSON, FileFormat.SDAI_JSON),
            (MDL_UTF8, FileFormat.MDL),
            (MDL_BARE, FileFormat.MDL),
            (b'{"neither": 1}', None),
            (b"{not json", None),
            (b"", None),
            (b"\x00\x01\x02", None),
            (b"hello world", None),
        ],
    )
    def test_sniff(self, data: bytes, expected: FileFormat | None) -> None:
        assert sniff_format(data) == expected


class TestResolveReadFormat:
    """Suffix wins when it is unambiguous; JSON suffixes sniff the payload;
    unknown suffixes fall back to sniffing; nothing recognisable raises
    ``SimlinImportError`` naming the path."""

    def test_known_suffix_wins_over_content(self) -> None:
        # A .stmx that does not look like XML is still handed to the XMILE
        # reader: the engine reports the parse failure, not the table.
        assert resolve_read_format(Path("m.stmx"), b"garbage") == FileFormat.XMILE
        assert resolve_read_format(Path("m.mdl"), NATIVE_JSON) == FileFormat.MDL
        assert resolve_read_format(Path("m.pb"), b"\x00") == FileFormat.PROTOBUF

    def test_json_suffix_sniffs_native_vs_sdai(self) -> None:
        assert resolve_read_format(Path("m.json"), NATIVE_JSON) == FileFormat.NATIVE_JSON
        assert resolve_read_format(Path("m.json"), SDAI_JSON) == FileFormat.SDAI_JSON
        # .sd.json is the native suffix, but an SD-AI payload under it (as in
        # test/sd-ai-simple.sd.json) is still read as SD-AI.
        assert resolve_read_format(Path("m.sd.json"), NATIVE_JSON) == FileFormat.NATIVE_JSON
        assert resolve_read_format(Path("m.sd.json"), SDAI_JSON) == FileFormat.SDAI_JSON

    def test_json_suffix_with_unparseable_json_defers_to_engine(self) -> None:
        # Invalid JSON under a JSON suffix is the engine's error to report,
        # so it resolves to native JSON rather than raising here.
        assert resolve_read_format(Path("m.json"), b"{not json") == FileFormat.NATIVE_JSON

    def test_json_suffix_with_unrecognised_keys_raises(self) -> None:
        with pytest.raises(SimlinImportError, match=r"m\.json.*models.*variables"):
            resolve_read_format(Path("m.json"), b'{"neither": 1}')

    def test_unknown_suffix_sniffs_content(self) -> None:
        assert resolve_read_format(Path("m.txt"), XMILE) == FileFormat.XMILE
        assert resolve_read_format(Path("m"), NATIVE_JSON) == FileFormat.NATIVE_JSON
        assert resolve_read_format(Path("m.dat"), SDAI_JSON) == FileFormat.SDAI_JSON
        assert resolve_read_format(Path("m.model"), MDL_UTF8) == FileFormat.MDL

    def test_unknown_suffix_unrecognisable_content_raises_naming_path(self) -> None:
        with pytest.raises(SimlinImportError, match=r"mystery\.dat"):
            resolve_read_format(Path("/some/dir/mystery.dat"), b"\x00\x01garbage")


class TestResolveWriteFormat:
    """An explicit format always wins; otherwise the suffix decides; a path
    with an unknown suffix and no explicit format is an error."""

    def test_explicit_format_wins(self) -> None:
        assert resolve_write_format(Path("m.txt"), FileFormat.XMILE) == FileFormat.XMILE
        assert resolve_write_format(Path("m.stmx"), FileFormat.NATIVE_JSON) == (
            FileFormat.NATIVE_JSON
        )

    @pytest.mark.parametrize(
        ("name", "expected"),
        [
            ("m.stmx", FileFormat.XMILE),
            ("m.xmile", FileFormat.XMILE),
            ("m.mdl", FileFormat.MDL),
            ("m.sd.json", FileFormat.NATIVE_JSON),
            ("m.json", FileFormat.NATIVE_JSON),
            ("m.pb", FileFormat.PROTOBUF),
        ],
    )
    def test_suffix_decides(self, name: str, expected: FileFormat) -> None:
        assert resolve_write_format(Path(name), None) == expected

    def test_unknown_suffix_without_format_raises(self) -> None:
        with pytest.raises(ValueError, match=r"m\.txt"):
            resolve_write_format(Path("m.txt"), None)
