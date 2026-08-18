#!/usr/bin/env python3
"""Build the pysimlin wheel for the current platform.

Three inputs go into the wheel and this script produces all of them in order:
the libsimlin static library (cargo), the notebook widget assets (the
TypeScript workspace build, staged by ``stage_widget_assets.py``), and the
CFFI extension + Python package (``python -m build``).
"""

import argparse
import json
import os
import platform
import shutil
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]
LIBSIMLIN_DIR = REPO_ROOT / "src" / "libsimlin"
STAGE_ASSETS = Path(__file__).resolve().with_name("stage_widget_assets.py")


def get_platform_tag() -> str:
    """Get the platform tag for the current system."""
    system = platform.system()
    machine = platform.machine()

    if system == "Darwin":
        if machine == "arm64":
            return "macosx_11_0_arm64"
        else:
            return "macosx_10_9_x86_64"
    elif system == "Linux":
        if machine == "aarch64":
            return "manylinux_2_28_aarch64"
        elif machine in ("x86_64", "amd64"):
            return "manylinux_2_28_x86_64"
    else:
        raise RuntimeError(f"Unsupported platform: {system} {machine}")


def cargo_target_dir() -> Path:
    """Ask cargo where build artifacts land.

    libsimlin is a workspace member, so its staticlib is written to the
    *workspace* target directory, not `src/libsimlin/target/`. Deriving the
    path by hand is how this script previously drifted out of sync with
    reality; `cargo metadata` is authoritative and honors CARGO_TARGET_DIR
    and any `build.target-dir` config.
    """
    proc = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--no-deps"],
        cwd=LIBSIMLIN_DIR,
        check=True,
        capture_output=True,
        text=True,
    )
    return Path(json.loads(proc.stdout)["target_directory"])


def build_libsimlin() -> Path:
    """Build the libsimlin static library and return the path to it."""
    print("Building libsimlin...")

    system = platform.system()
    if system not in ("Darwin", "Linux"):
        raise RuntimeError(f"Unsupported platform: {system}")

    # The mimalloc feature swaps in mimalloc as the global allocator: the
    # engine compile path is allocation-heavy and mimalloc roughly halves
    # allocator time on native builds (docs/design/engine-performance.md).
    subprocess.run(
        ["cargo", "build", "--release", "--features", "mimalloc"], cwd=LIBSIMLIN_DIR, check=True
    )

    lib_path = cargo_target_dir() / "release" / "libsimlin.a"
    if not lib_path.exists():
        raise RuntimeError(f"Library not found at {lib_path}")

    print(f"Built {lib_path}")
    return lib_path


def stage_widget_assets(build: bool, require_opt: bool) -> None:
    """Put the notebook widget's JS module and engine wasm into
    ``simlin/_widget/`` so the wheel carries them.

    ``build`` runs the TypeScript workspace build first (engine wasm, diagram,
    widget); ``False`` stages whatever those builds last produced. Either way
    the staging script writes ``ASSETS.json`` and ``setup.py`` refuses the
    wheel if anything is missing or inconsistent, so a wheel produced here can
    never silently lack the widget. ``require_opt`` additionally fails the
    build when the wasm was not wasm-opt'd (a raw wasm is what a
    DISABLE_WASM_OPT development build leaves behind).
    """
    print("Staging notebook widget assets...")
    args = [sys.executable, str(STAGE_ASSETS)]
    if not build:
        args.append("--no-build")
    if require_opt:
        args.append("--require-opt")
    subprocess.run(args, cwd=REPO_ROOT, check=True)


def build_wheel(lib_path: Path) -> None:
    """Build the wheel for the current platform.

    ``lib_path`` is pinned into the CFFI link step via ``SIMLIN_STATIC_LIB``
    so the wheel provably contains the staticlib this script just built,
    rather than whatever ``_ffi_build``'s fallback search happens to find
    first (GH #682).
    """
    print("Building wheel...")

    package_dir = Path(__file__).parent.parent

    # Clean up old builds. `build/` in particular must go: setuptools skips
    # recompiling the CFFI extension when its object files look current, which
    # would relink nothing and ship a stale engine.
    for dir_name in ["build", "dist", "simlin.egg-info"]:
        dir_path = package_dir / dir_name
        if dir_path.exists():
            shutil.rmtree(dir_path)

    # `python -m build`, not `pip wheel`: this script runs under `uv run`, and
    # uv-managed virtualenvs have no pip. `build` is declared in the dev extra
    # and provisions the pyproject build-system requires itself.
    subprocess.run(
        [sys.executable, "-m", "build", "--wheel", "--outdir", "dist"],
        cwd=package_dir,
        check=True,
        env={**os.environ, "SIMLIN_STATIC_LIB": str(lib_path)},
    )

    # Get the built wheel
    dist_dir = package_dir / "dist"
    wheels = list(dist_dir.glob("*.whl"))

    if not wheels:
        raise RuntimeError("No wheel found after build")

    wheel_path = wheels[0]

    # Rename with correct platform tag
    platform_tag = get_platform_tag()
    wheel_name = wheel_path.name

    # Replace the platform tag in the wheel name
    # Format: {name}-{version}-{python}-{abi}-{platform}.whl
    parts = wheel_name.rsplit("-", 1)
    if len(parts) != 2:
        raise RuntimeError(f"Unexpected wheel name format: {wheel_name}")

    new_name = f"{parts[0]}-{platform_tag}.whl"
    new_path = wheel_path.parent / new_name

    wheel_path.rename(new_path)
    print(f"Wheel built: {new_path}")


def main(argv: list[str] | None = None) -> None:
    """Main entry point."""
    parser = argparse.ArgumentParser(description="Build the pysimlin wheel for this platform.")
    parser.add_argument(
        "--no-asset-build",
        action="store_true",
        help="stage the notebook widget assets from the existing TypeScript build outputs "
        "instead of running the pnpm build first",
    )
    parser.add_argument(
        "--require-opt",
        action="store_true",
        help="fail unless the staged wasm was built with wasm-opt (release builds); "
        "off by default so a development build without binaryen still produces a wheel",
    )
    args = parser.parse_args(argv)

    print("Building simlin Python package...")
    print(f"Platform: {platform.system()} {platform.machine()}")

    lib_path = build_libsimlin()
    stage_widget_assets(build=not args.no_asset_build, require_opt=args.require_opt)
    build_wheel(lib_path)

    print("Build complete!")


if __name__ == "__main__":
    main()
