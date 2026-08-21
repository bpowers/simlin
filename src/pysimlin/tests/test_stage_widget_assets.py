"""Tests for the pure helpers of ``scripts/stage_widget_assets.py``: the
manifest, its verification (every error/warning arm), and the up-to-date
short-circuit that makes staging idempotent.

The script is not part of the ``simlin`` package (it stages the package's
data), so it is loaded from the source checkout, located via
``conftest.get_repo_root`` exactly as the model fixtures are.
"""

from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path
from typing import TYPE_CHECKING

import pytest

from .conftest import get_repo_root

if TYPE_CHECKING:
    from types import ModuleType

SCRIPT = get_repo_root() / "src" / "pysimlin" / "scripts" / "stage_widget_assets.py"
PACKAGE_DIR = SCRIPT.parent.parent / "simlin"


def _testing_an_installed_wheel() -> bool:
    """Is the ``simlin`` under test an installed wheel rather than the package
    directory of this checkout?"""
    import simlin

    return Path(simlin.__file__).resolve().parent != PACKAGE_DIR.resolve()


@pytest.fixture(scope="module")
def staging() -> ModuleType:
    spec = importlib.util.spec_from_file_location("stage_widget_assets", SCRIPT)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    # dataclasses resolve string annotations through sys.modules[__module__].
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _digests(staging: ModuleType, contents: dict[str, bytes]) -> dict[str, object]:
    return {name: staging.FileDigest.of_bytes(data) for name, data in contents.items()}


def _write_staged(
    staging: ModuleType,
    dest: Path,
    contents: dict[str, bytes],
    *,
    wasm_mode: str = "opt",
    source_commit: str | None = "abc123",
    source_dirty: bool = False,
) -> str:
    """Populate ``dest`` the way ``stage()`` does and return the manifest text."""
    dest.mkdir(parents=True, exist_ok=True)
    for name, data in contents.items():
        (dest / name).write_bytes(data)
    text: str = staging.manifest_json(
        staging.build_manifest(
            _digests(staging, contents),
            source_commit=source_commit,
            source_dirty=source_dirty,
            wasm_mode=wasm_mode,
        )
    )
    (dest / staging.MANIFEST_FILE).write_text(text, encoding="utf-8")
    return text


GOOD = {"widget.js": b"export default {};\n", "libsimlin-browser.wasm": b"\0asm\1\0\0\0"}


class TestManifest:
    def test_is_deterministic_and_sorted(self, staging: ModuleType) -> None:
        a = staging.build_manifest(
            {
                "widget.js": staging.FileDigest(1, "a"),
                "libsimlin-browser.wasm": staging.FileDigest(2, "b"),
            },
            source_commit="c",
            source_dirty=False,
            wasm_mode="opt",
        )
        b = staging.build_manifest(
            {
                "libsimlin-browser.wasm": staging.FileDigest(2, "b"),
                "widget.js": staging.FileDigest(1, "a"),
            },
            source_commit="c",
            source_dirty=False,
            wasm_mode="opt",
        )
        assert staging.manifest_json(a) == staging.manifest_json(b)
        assert staging.manifest_json(a).endswith("\n")
        assert list(a["files"]) == ["libsimlin-browser.wasm", "widget.js"]
        # No timestamps or host details: the manifest is a function of its inputs.
        assert set(a) == {"schema", "source_commit", "source_dirty", "wasm_opt", "files"}

    def test_round_trips_through_parse(self, staging: ModuleType) -> None:
        digests = _digests(staging, GOOD)
        manifest = staging.build_manifest(
            digests, source_commit=None, source_dirty=True, wasm_mode="raw"
        )
        parsed = staging.parse_manifest(staging.manifest_json(manifest))
        assert parsed is not None
        assert staging.manifest_digests(parsed) == digests
        assert parsed["source_commit"] is None
        assert parsed["source_dirty"] is True
        assert parsed["wasm_opt"] == "raw"

    @pytest.mark.parametrize(
        "text",
        [
            "",
            "not json",
            "[]",
            json.dumps({"schema": 999, "files": {}}),
            json.dumps({"schema": 1}),
            json.dumps({"schema": 1, "files": []}),
            json.dumps({"schema": 1, "files": {"widget.js": {"size": 1}}}),
            json.dumps({"schema": 1, "files": {"widget.js": {"sha256": "x", "size": "1"}}}),
        ],
    )
    def test_rejects_malformed(self, staging: ModuleType, text: str) -> None:
        assert staging.parse_manifest(text) is None

    def test_file_digest_of_path_matches_of_bytes(
        self, staging: ModuleType, tmp_path: Path
    ) -> None:
        data = bytes(range(256)) * 5000  # spans more than one read chunk boundary
        path = tmp_path / "blob"
        path.write_bytes(data)
        assert staging.FileDigest.of_path(path) == staging.FileDigest.of_bytes(data)


class TestVerifyStaged:
    def test_complete_staging_is_ok(self, staging: ModuleType, tmp_path: Path) -> None:
        _write_staged(staging, tmp_path, GOOD)
        v = staging.verify_staged(tmp_path, require_opt=True, head_commit="abc123")
        assert v.ok
        assert v.warnings == []

    def test_empty_directory_reports_every_file(self, staging: ModuleType, tmp_path: Path) -> None:
        v = staging.verify_staged(tmp_path)
        assert not v.ok
        joined = "\n".join(v.errors)
        assert "widget.js is missing" in joined
        assert "libsimlin-browser.wasm is missing" in joined
        assert "ASSETS.json is missing" in joined

    @pytest.mark.parametrize("name", ["widget.js", "libsimlin-browser.wasm"])
    def test_missing_asset(self, staging: ModuleType, tmp_path: Path, name: str) -> None:
        _write_staged(staging, tmp_path, GOOD)
        (tmp_path / name).unlink()
        v = staging.verify_staged(tmp_path)
        assert [e for e in v.errors if e.startswith(f"{name} is missing")]

    @pytest.mark.parametrize("name", ["widget.js", "libsimlin-browser.wasm"])
    def test_empty_asset(self, staging: ModuleType, tmp_path: Path, name: str) -> None:
        _write_staged(staging, tmp_path, GOOD)
        (tmp_path / name).write_bytes(b"")
        v = staging.verify_staged(tmp_path)
        assert any(e.startswith(f"{name} in") and e.endswith("is empty") for e in v.errors)

    def test_manifest_hash_mismatch(self, staging: ModuleType, tmp_path: Path) -> None:
        _write_staged(staging, tmp_path, GOOD)
        (tmp_path / "widget.js").write_bytes(b"export default { stale: true };\n")
        v = staging.verify_staged(tmp_path)
        assert v.errors == ["widget.js does not match its ASSETS.json entry (stale staging?)"]

    def test_manifest_missing_entry(self, staging: ModuleType, tmp_path: Path) -> None:
        _write_staged(staging, tmp_path, {"widget.js": GOOD["widget.js"]})
        (tmp_path / "libsimlin-browser.wasm").write_bytes(GOOD["libsimlin-browser.wasm"])
        v = staging.verify_staged(tmp_path)
        assert v.errors == ["ASSETS.json does not describe libsimlin-browser.wasm"]

    def test_unparseable_manifest(self, staging: ModuleType, tmp_path: Path) -> None:
        _write_staged(staging, tmp_path, GOOD)
        (tmp_path / "ASSETS.json").write_text("{", encoding="utf-8")
        v = staging.verify_staged(tmp_path)
        assert len(v.errors) == 1
        assert "asset manifest" in v.errors[0]

    def test_raw_wasm_is_a_warning_unless_required(
        self, staging: ModuleType, tmp_path: Path
    ) -> None:
        _write_staged(staging, tmp_path, GOOD, wasm_mode="raw")
        relaxed = staging.verify_staged(tmp_path, require_opt=False)
        assert relaxed.ok
        assert any("without wasm-opt" in w for w in relaxed.warnings)
        strict = staging.verify_staged(tmp_path, require_opt=True)
        assert not strict.ok
        assert any("without wasm-opt" in e for e in strict.errors)

    def test_other_commit_and_dirty_tree_are_warnings(
        self, staging: ModuleType, tmp_path: Path
    ) -> None:
        _write_staged(staging, tmp_path, GOOD, source_commit="old", source_dirty=True)
        v = staging.verify_staged(tmp_path, head_commit="new")
        assert v.ok
        assert any("staged from commit old, HEAD is new" in w for w in v.warnings)
        assert any("uncommitted changes" in w for w in v.warnings)

    def test_unknown_head_skips_commit_comparison(
        self, staging: ModuleType, tmp_path: Path
    ) -> None:
        _write_staged(staging, tmp_path, GOOD, source_commit="old")
        v = staging.verify_staged(tmp_path, head_commit=None)
        assert v.ok
        assert v.warnings == []


class TestIsUpToDate:
    def _sources(self, tmp_path: Path, contents: dict[str, bytes]) -> dict[str, Path]:
        src_dir = tmp_path / "src"
        src_dir.mkdir()
        for name, data in contents.items():
            (src_dir / name).write_bytes(data)
        return {name: src_dir / name for name in contents}

    def test_true_after_a_faithful_staging(self, staging: ModuleType, tmp_path: Path) -> None:
        sources = self._sources(tmp_path, GOOD)
        dest = tmp_path / "dest"
        text = _write_staged(staging, dest, GOOD)
        assert staging.is_up_to_date(dest, sources, text)

    def test_false_when_a_source_changed(self, staging: ModuleType, tmp_path: Path) -> None:
        sources = self._sources(tmp_path, GOOD)
        dest = tmp_path / "dest"
        text = _write_staged(staging, dest, GOOD)
        sources["widget.js"].write_bytes(b"export default { v: 2 };\n")
        assert not staging.is_up_to_date(dest, sources, text)

    def test_false_when_the_manifest_would_differ(
        self, staging: ModuleType, tmp_path: Path
    ) -> None:
        sources = self._sources(tmp_path, GOOD)
        dest = tmp_path / "dest"
        _write_staged(staging, dest, GOOD, source_commit="old")
        new_text = staging.manifest_json(
            staging.build_manifest(
                _digests(staging, GOOD), source_commit="new", source_dirty=False, wasm_mode="opt"
            )
        )
        assert not staging.is_up_to_date(dest, sources, new_text)

    def test_false_when_dest_is_empty(self, staging: ModuleType, tmp_path: Path) -> None:
        sources = self._sources(tmp_path, GOOD)
        assert not staging.is_up_to_date(tmp_path / "dest", sources, "x")


class TestStageAndCheck:
    """The shell over a temp destination: ``stage`` copies + writes the
    manifest, a rerun is a no-op that leaves mtimes alone, and ``check``
    agrees with ``verify_staged``. Sources are redirected to fixtures so no
    build is involved."""

    @pytest.fixture
    def redirected(
        self, staging: ModuleType, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
    ) -> Path:
        src = tmp_path / "build"
        src.mkdir()
        (src / "widget.js").write_bytes(GOOD["widget.js"])
        (src / "libsimlin-browser.wasm").write_bytes(GOOD["libsimlin-browser.wasm"])
        (src / "libsimlin-browser.wasm.mode").write_text("opt\n")
        monkeypatch.setattr(staging, "WIDGET_DIST_JS", src / "widget.js")
        monkeypatch.setattr(staging, "ENGINE_WASM", src / "libsimlin-browser.wasm")
        monkeypatch.setattr(staging, "ENGINE_WASM_MODE", src / "libsimlin-browser.wasm.mode")
        return tmp_path / "dest"

    def test_stage_then_check(self, staging: ModuleType, redirected: Path) -> None:
        assert staging.main(["--no-build", "--dest", str(redirected)]) == 0
        assert (redirected / "widget.js").read_bytes() == GOOD["widget.js"]
        assert (redirected / "libsimlin-browser.wasm").read_bytes() == GOOD[
            "libsimlin-browser.wasm"
        ]
        manifest = staging.parse_manifest((redirected / "ASSETS.json").read_text())
        assert manifest is not None
        assert manifest["wasm_opt"] == "opt"
        assert staging.main(["--check", "--require-opt", "--dest", str(redirected)]) == 0

    def test_rerun_is_a_no_op(self, staging: ModuleType, redirected: Path) -> None:
        assert staging.main(["--no-build", "--dest", str(redirected)]) == 0
        before = {p.name: p.stat().st_mtime_ns for p in redirected.iterdir()}
        assert staging.main(["--no-build", "--dest", str(redirected)]) == 0
        after = {p.name: p.stat().st_mtime_ns for p in redirected.iterdir()}
        assert after == before

    def test_require_opt_refuses_a_raw_wasm(self, staging: ModuleType, redirected: Path) -> None:
        staging.ENGINE_WASM_MODE.write_text("raw\n")
        assert staging.main(["--no-build", "--require-opt", "--dest", str(redirected)]) == 1
        assert not redirected.exists()
        # Without the flag it stages and records the mode; check then warns
        # but passes, and fails only when opt is required.
        assert staging.main(["--no-build", "--dest", str(redirected)]) == 0
        assert staging.main(["--check", "--dest", str(redirected)]) == 0
        assert staging.main(["--check", "--require-opt", "--dest", str(redirected)]) == 1

    def test_missing_build_output_fails(self, staging: ModuleType, redirected: Path) -> None:
        staging.WIDGET_DIST_JS.unlink()
        assert staging.main(["--no-build", "--dest", str(redirected)]) == 1

    def test_check_fails_on_an_empty_dest(self, staging: ModuleType, redirected: Path) -> None:
        assert staging.main(["--check", "--dest", str(redirected)]) == 1


class TestPackagingGuard:
    """The bdist_wheel/sdist decision ``setup.py`` executes."""

    def test_staged_dir_may_proceed(self, staging: ModuleType, tmp_path: Path) -> None:
        _write_staged(staging, tmp_path, GOOD)
        messages, refusal = staging.packaging_guard(
            tmp_path, command="bdist_wheel", head_commit="abc123", allow_missing=False
        )
        assert refusal is None
        assert messages == []

    def test_other_commit_proceeds_with_a_warning(
        self, staging: ModuleType, tmp_path: Path
    ) -> None:
        _write_staged(staging, tmp_path, GOOD, source_commit="old")
        messages, refusal = staging.packaging_guard(
            tmp_path, command="sdist", head_commit="new", allow_missing=False
        )
        assert refusal is None
        assert any("staged from commit old" in m for m in messages)

    def test_empty_dir_is_refused(self, staging: ModuleType, tmp_path: Path) -> None:
        _, refusal = staging.packaging_guard(
            tmp_path, command="bdist_wheel", head_commit=None, allow_missing=False
        )
        assert refusal is not None
        assert refusal.startswith("refusing to run bdist_wheel")
        assert "widget.js is missing" in refusal
        assert "make assets" in refusal
        assert staging.ALLOW_MISSING_ENV in refusal

    def test_mismatched_asset_is_refused(self, staging: ModuleType, tmp_path: Path) -> None:
        _write_staged(staging, tmp_path, GOOD)
        (tmp_path / "widget.js").write_bytes(b"export default { stale: true };\n")
        _, refusal = staging.packaging_guard(
            tmp_path, command="sdist", head_commit=None, allow_missing=False
        )
        assert refusal is not None
        assert "does not match its ASSETS.json entry" in refusal

    def test_bypass_proceeds_but_says_so(self, staging: ModuleType, tmp_path: Path) -> None:
        messages, refusal = staging.packaging_guard(
            tmp_path, command="bdist_wheel", head_commit=None, allow_missing=True
        )
        assert refusal is None
        assert messages == [
            f"warning: {staging.ALLOW_MISSING_ENV}=1: bdist_wheel without checking widget assets"
        ]


class TestSetupPyGuard:
    """``setup.py`` itself: its ``bdist_wheel``/``sdist`` cmdclasses must call the
    guard before running. Loaded with ``sys.argv = ['setup.py', '--name']`` so
    ``setup()`` only answers a query, then the command classes are exercised
    with the asset directory pointed at a temp dir. A base ``run`` that raises
    ``_Ran`` shows the guard passed; a ``SystemExit`` shows it refused."""

    class _Ran(Exception):
        pass

    @pytest.fixture
    def setup_module(self, monkeypatch: pytest.MonkeyPatch) -> ModuleType:
        # setup.py belongs to the checkout, not to the wheel, and loading it
        # runs setup() -- which needs the build environment the dev extra and
        # `make build` provide: setuptools >= 70.1 (setup.py subclasses
        # setuptools' own bdist_wheel, which older versions do not carry) and,
        # since cffi_modules makes cffi import simlin/_ffi_build.py, a built
        # libsimlin.a. Guard on the one fact that decides all of them rather
        # than on each build input in turn: which inputs an environment
        # happens to supply varies per runner image (actions/setup-python
        # bundles setuptools for 3.11 but not for 3.12+), so enumerating them
        # makes every future build input a new way for this to fail in a job
        # that cannot fix it. The pure guard is covered above regardless.
        if _testing_an_installed_wheel():
            pytest.skip("setup.py is loadable only from a built source checkout")
        setup_py = SCRIPT.parent.parent / "setup.py"
        monkeypatch.setattr(sys, "argv", ["setup.py", "--name"])
        monkeypatch.chdir(setup_py.parent)
        spec = importlib.util.spec_from_file_location("pysimlin_setup", setup_py)
        assert spec is not None
        assert spec.loader is not None
        module = importlib.util.module_from_spec(spec)
        sys.modules[spec.name] = module
        spec.loader.exec_module(module)
        return module

    @pytest.fixture
    def commands(self, setup_module: ModuleType, monkeypatch: pytest.MonkeyPatch) -> list[object]:
        for name in ("bdist_wheel", "sdist"):
            cls = getattr(setup_module, name)
            monkeypatch.setattr(
                cls.__mro__[1], "run", lambda self: (_ for _ in ()).throw(TestSetupPyGuard._Ran())
            )
        # Distribution-free instances: only ``run`` is exercised.
        return [object.__new__(setup_module.bdist_wheel), object.__new__(setup_module.sdist)]

    @pytest.mark.parametrize("index", [0, 1], ids=["bdist_wheel", "sdist"])
    def test_refuses_without_assets(
        self,
        setup_module: ModuleType,
        commands: list[object],
        tmp_path: Path,
        monkeypatch: pytest.MonkeyPatch,
        index: int,
    ) -> None:
        monkeypatch.setattr(setup_module, "ASSET_DIR", tmp_path)
        monkeypatch.delenv(setup_module.ALLOW_MISSING_ENV, raising=False)
        with pytest.raises(SystemExit, match="refusing to run"):
            commands[index].run()  # type: ignore[attr-defined]

    @pytest.mark.parametrize("index", [0, 1], ids=["bdist_wheel", "sdist"])
    def test_runs_with_staged_assets(
        self,
        setup_module: ModuleType,
        staging: ModuleType,
        commands: list[object],
        tmp_path: Path,
        monkeypatch: pytest.MonkeyPatch,
        index: int,
    ) -> None:
        _write_staged(staging, tmp_path, GOOD)
        monkeypatch.setattr(setup_module, "ASSET_DIR", tmp_path)
        monkeypatch.delenv(setup_module.ALLOW_MISSING_ENV, raising=False)
        with pytest.raises(TestSetupPyGuard._Ran):
            commands[index].run()  # type: ignore[attr-defined]

    def test_bypass_env_runs_without_assets(
        self,
        setup_module: ModuleType,
        commands: list[object],
        tmp_path: Path,
        monkeypatch: pytest.MonkeyPatch,
    ) -> None:
        monkeypatch.setattr(setup_module, "ASSET_DIR", tmp_path)
        monkeypatch.setenv(setup_module.ALLOW_MISSING_ENV, "1")
        with pytest.raises(TestSetupPyGuard._Ran):
            commands[0].run()  # type: ignore[attr-defined]
