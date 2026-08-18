#!/usr/bin/env python3
"""Assert that built pysimlin wheels carry the notebook widget's assets.

pattern: Functional Core (``wheel_asset_entries``, ``identical_across``) +
Imperative Shell (zip reading, the import probe)

Two modes, both used by the release workflow's ``test-wheels`` job:

``check_wheel_assets.py WHEEL...``
    Every wheel must contain ``simlin/_widget/widget.js``,
    ``libsimlin-browser.wasm`` and ``ASSETS.json``; each set is extracted and
    put through the same ``verify_staged`` check ``setup.py`` applies at build
    time (present, non-empty, hashes match the manifest, ``--require-opt``
    for a wasm-opt'd engine); and when several wheels are given the assets
    must be byte-identical across all of them -- the release builds them once
    and every platform wheel ships that one set.

``check_wheel_assets.py --installed``
    Run in an environment where a wheel is installed (from a directory that
    does not contain the source package): the package data must resolve on
    disk, and ``simlin.Project.new().main_model.widget()`` must construct a
    ``ModelWidget`` (which raises ``SimlinAssetError`` when the module is
    missing) whose ``_esm`` -- anywidget's name for the module text -- is
    non-empty.

Exit status 1 with the problems on stderr on any failure.
"""

from __future__ import annotations

import argparse
import sys
import tempfile
import zipfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from stage_widget_assets import (
    ASSET_FILES,
    MANIFEST_FILE,
    FileDigest,
    verify_staged,
)

WHEEL_ASSET_DIR = "simlin/_widget/"
REQUIRED_ENTRIES = tuple(WHEEL_ASSET_DIR + name for name in (*ASSET_FILES, MANIFEST_FILE))


# ── functional core ─────────────────────────────────────────────────────


def wheel_asset_entries(names: list[str]) -> tuple[list[str], list[str]]:
    """Split a wheel's entry list into (asset entries present, required
    entries missing)."""
    present = [n for n in names if n.startswith(WHEEL_ASSET_DIR) and not n.endswith("/")]
    missing = [n for n in REQUIRED_ENTRIES if n not in names]
    return present, missing


def identical_across(digests_by_wheel: dict[str, dict[str, FileDigest]]) -> list[str]:
    """Problems when the asset digests differ between wheels (empty when they
    all agree, or when there is at most one wheel)."""
    problems: list[str] = []
    if len(digests_by_wheel) < 2:
        return problems
    wheels = sorted(digests_by_wheel)
    reference = digests_by_wheel[wheels[0]]
    for wheel in wheels[1:]:
        for name in sorted(set(reference) | set(digests_by_wheel[wheel])):
            a = reference.get(name)
            b = digests_by_wheel[wheel].get(name)
            if a != b:
                problems.append(f"{name} differs between {wheels[0]} and {wheel}: {a} vs {b}")
    return problems


# ── imperative shell ────────────────────────────────────────────────────


def check_wheel(wheel: Path, *, require_opt: bool) -> tuple[list[str], dict[str, FileDigest]]:
    problems: list[str] = []
    digests: dict[str, FileDigest] = {}
    with zipfile.ZipFile(wheel) as zf:
        present, missing = wheel_asset_entries(zf.namelist())
        for entry in missing:
            problems.append(f"{wheel.name}: {entry} is not in the wheel")
        with tempfile.TemporaryDirectory() as tmp:
            dest = Path(tmp)
            for entry in present:
                target = dest / entry[len(WHEEL_ASSET_DIR) :]
                target.write_bytes(zf.read(entry))
                digests[target.name] = FileDigest.of_path(target)
            v = verify_staged(dest, require_opt=require_opt)
            problems.extend(f"{wheel.name}: {e}" for e in v.errors)
            for w in v.warnings:
                print(f"warning: {wheel.name}: {w}")
    return problems, digests


def check_wheels(wheels: list[Path], *, require_opt: bool) -> int:
    problems: list[str] = []
    digests_by_wheel: dict[str, dict[str, FileDigest]] = {}
    for wheel in wheels:
        wheel_problems, digests = check_wheel(wheel, require_opt=require_opt)
        problems.extend(wheel_problems)
        digests_by_wheel[wheel.name] = digests
        for name, digest in sorted(digests.items()):
            print(f"{wheel.name}: {name} {digest.size} bytes sha256={digest.sha256}")
    problems.extend(identical_across(digests_by_wheel))
    for p in problems:
        print(f"error: {p}", file=sys.stderr)
    if problems:
        return 1
    print(f"{len(wheels)} wheel(s) carry complete, identical widget assets")
    return 0


def check_installed(*, require_opt: bool) -> int:
    from importlib import resources

    import simlin

    package_file = Path(str(simlin.__file__)).resolve()
    if Path(__file__).resolve().parents[1] in package_file.parents:
        print(
            f"error: `import simlin` resolved to the source checkout ({package_file}), "
            "not an installed wheel; run this from another directory",
            file=sys.stderr,
        )
        return 1

    asset_dir = Path(str(resources.files("simlin") / "_widget"))
    v = verify_staged(asset_dir, require_opt=require_opt)
    for w in v.warnings:
        print(f"warning: {w}")
    for e in v.errors:
        print(f"error: {e}", file=sys.stderr)
    if not v.ok:
        return 1

    project = simlin.Project.new()
    widget = project.main_model.widget()
    esm = getattr(widget, "_esm", "")
    if not isinstance(esm, str) or not esm.strip():
        print("error: ModelWidget._esm is empty; the widget module did not load", file=sys.stderr)
        return 1
    print(f"installed pysimlin at {package_file.parent} constructs a {type(widget).__name__}")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("wheels", nargs="*", type=Path, help="wheel files to inspect")
    parser.add_argument(
        "--installed",
        action="store_true",
        help="probe the installed simlin package instead of inspecting wheel files",
    )
    parser.add_argument(
        "--require-opt", action="store_true", help="the wasm must have been wasm-opt'd"
    )
    args = parser.parse_args(argv)
    if args.installed:
        if args.wheels:
            parser.error("--installed takes no wheel arguments")
        return check_installed(require_opt=args.require_opt)
    if not args.wheels:
        parser.error("give at least one wheel, or --installed")
    return check_wheels(args.wheels, require_opt=args.require_opt)


if __name__ == "__main__":
    sys.exit(main())
