"""Tests for ``scripts/check_wheel_assets.py``: the entry classification, the
cross-wheel identity check, and the zip-inspection shell over small wheels
built the way setuptools lays them out (``simlin/_widget/...`` entries).
"""

from __future__ import annotations

import importlib.util
import sys
import zipfile
from typing import TYPE_CHECKING

import pytest

from .conftest import get_repo_root

if TYPE_CHECKING:
    from pathlib import Path
    from types import ModuleType

SCRIPTS = get_repo_root() / "src" / "pysimlin" / "scripts"


def _load(name: str) -> ModuleType:
    spec = importlib.util.spec_from_file_location(name, SCRIPTS / f"{name}.py")
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


@pytest.fixture(scope="module")
def staging() -> ModuleType:
    return _load("stage_widget_assets")


@pytest.fixture(scope="module")
def checker(staging: ModuleType) -> ModuleType:
    return _load("check_wheel_assets")


GOOD = {"widget.js": b"export default {};\n", "libsimlin-browser.wasm": b"\0asm\1\0\0\0"}


def _manifest(staging: ModuleType, contents: dict[str, bytes], wasm_mode: str = "opt") -> bytes:
    text: str = staging.manifest_json(
        staging.build_manifest(
            {n: staging.FileDigest.of_bytes(d) for n, d in contents.items()},
            source_commit="abc",
            source_dirty=False,
            wasm_mode=wasm_mode,
        )
    )
    return text.encode()


def _wheel(path: Path, entries: dict[str, bytes]) -> Path:
    with zipfile.ZipFile(path, "w") as zf:
        zf.writestr("simlin/__init__.py", "")
        for name, data in entries.items():
            zf.writestr(name, data)
    return path


def _good_entries(staging: ModuleType) -> dict[str, bytes]:
    entries = {f"simlin/_widget/{n}": d for n, d in GOOD.items()}
    entries["simlin/_widget/ASSETS.json"] = _manifest(staging, GOOD)
    return entries


class TestWheelAssetEntries:
    def test_complete(self, checker: ModuleType) -> None:
        names = [
            "simlin/__init__.py",
            "simlin/_widget/",
            "simlin/_widget/widget.js",
            "simlin/_widget/libsimlin-browser.wasm",
            "simlin/_widget/ASSETS.json",
            "simlin/_widget/README.md",
        ]
        present, missing = checker.wheel_asset_entries(names)
        assert missing == []
        assert set(present) == {
            "simlin/_widget/widget.js",
            "simlin/_widget/libsimlin-browser.wasm",
            "simlin/_widget/ASSETS.json",
            "simlin/_widget/README.md",
        }

    def test_missing_lists_each_required_entry(self, checker: ModuleType) -> None:
        present, missing = checker.wheel_asset_entries(["simlin/__init__.py"])
        assert present == []
        assert missing == [
            "simlin/_widget/widget.js",
            "simlin/_widget/libsimlin-browser.wasm",
            "simlin/_widget/ASSETS.json",
        ]


class TestIdenticalAcross:
    def test_single_wheel_is_trivially_fine(self, checker: ModuleType, staging: ModuleType) -> None:
        d = {"widget.js": staging.FileDigest(1, "a")}
        assert checker.identical_across({"a.whl": d}) == []

    def test_agreeing_wheels(self, checker: ModuleType, staging: ModuleType) -> None:
        d = {
            "widget.js": staging.FileDigest(1, "a"),
            "libsimlin-browser.wasm": staging.FileDigest(2, "b"),
        }
        assert (
            checker.identical_across({"a.whl": dict(d), "b.whl": dict(d), "c.whl": dict(d)}) == []
        )

    def test_differing_and_missing_files_are_reported(
        self, checker: ModuleType, staging: ModuleType
    ) -> None:
        a = {
            "widget.js": staging.FileDigest(1, "a"),
            "libsimlin-browser.wasm": staging.FileDigest(2, "b"),
        }
        b = {"widget.js": staging.FileDigest(1, "a2")}
        problems = checker.identical_across({"linux.whl": a, "mac.whl": b})
        assert len(problems) == 2
        assert any(p.startswith("libsimlin-browser.wasm differs") for p in problems)
        assert any(p.startswith("widget.js differs") for p in problems)


class TestCheckWheels:
    def test_complete_identical_wheels_pass(
        self, checker: ModuleType, staging: ModuleType, tmp_path: Path
    ) -> None:
        entries = _good_entries(staging)
        wheels = [_wheel(tmp_path / f"pysimlin-1-{tag}.whl", entries) for tag in ("linux", "mac")]
        assert checker.main(["--require-opt", *map(str, wheels)]) == 0

    def test_wheel_without_assets_fails(
        self, checker: ModuleType, staging: ModuleType, tmp_path: Path
    ) -> None:
        wheel = _wheel(tmp_path / "pysimlin-1-linux.whl", {})
        assert checker.main([str(wheel)]) == 1

    def test_wheel_with_empty_asset_fails(
        self, checker: ModuleType, staging: ModuleType, tmp_path: Path
    ) -> None:
        entries = _good_entries(staging)
        entries["simlin/_widget/widget.js"] = b""
        wheel = _wheel(tmp_path / "pysimlin-1-linux.whl", entries)
        assert checker.main([str(wheel)]) == 1

    def test_wheel_whose_manifest_disagrees_fails(
        self, checker: ModuleType, staging: ModuleType, tmp_path: Path
    ) -> None:
        entries = _good_entries(staging)
        entries["simlin/_widget/widget.js"] = b"export default { other: 1 };\n"
        wheel = _wheel(tmp_path / "pysimlin-1-linux.whl", entries)
        assert checker.main([str(wheel)]) == 1

    def test_wheels_with_different_assets_fail(
        self, checker: ModuleType, staging: ModuleType, tmp_path: Path
    ) -> None:
        a = _good_entries(staging)
        other = dict(GOOD, **{"widget.js": b"export default { v: 2 };\n"})
        b = {f"simlin/_widget/{n}": d for n, d in other.items()}
        b["simlin/_widget/ASSETS.json"] = _manifest(staging, other)
        wheels = [
            _wheel(tmp_path / "pysimlin-1-linux.whl", a),
            _wheel(tmp_path / "pysimlin-1-mac.whl", b),
        ]
        # Each wheel is internally consistent...
        assert checker.main([str(wheels[0])]) == 0
        assert checker.main([str(wheels[1])]) == 0
        # ...but the release requires one asset set across all of them.
        assert checker.main([*map(str, wheels)]) == 1

    def test_raw_wasm_fails_only_when_opt_required(
        self, checker: ModuleType, staging: ModuleType, tmp_path: Path
    ) -> None:
        entries = _good_entries(staging)
        entries["simlin/_widget/ASSETS.json"] = _manifest(staging, GOOD, wasm_mode="raw")
        wheel = _wheel(tmp_path / "pysimlin-1-linux.whl", entries)
        assert checker.main([str(wheel)]) == 0
        assert checker.main(["--require-opt", str(wheel)]) == 1

    def test_argument_errors(self, checker: ModuleType, tmp_path: Path) -> None:
        with pytest.raises(SystemExit):
            checker.main([])
        with pytest.raises(SystemExit):
            checker.main(["--installed", str(tmp_path / "x.whl")])
