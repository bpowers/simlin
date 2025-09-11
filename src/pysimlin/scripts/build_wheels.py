#!/usr/bin/env python3
"""Build wheels for multiple platforms."""

import os
import platform
import shutil
import subprocess
import sys
from pathlib import Path
from typing import List, Tuple


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


def build_libsimlin() -> Path:
    """Build the libsimlin static library."""
    print("Building libsimlin...")
    
    # Get the path to the libsimlin directory
    project_root = Path(__file__).parent.parent.parent.parent
    libsimlin_dir = project_root / "libsimlin"
    
    # Build the library
    subprocess.run(
        ["cargo", "build", "--release"],
        cwd=libsimlin_dir,
        check=True
    )
    
    # Return the path to the built library
    target_dir = libsimlin_dir / "target" / "release"
    
    system = platform.system()
    if system == "Darwin":
        lib_path = target_dir / "libsimlin.a"
    elif system == "Linux":
        lib_path = target_dir / "libsimlin.a"
    else:
        raise RuntimeError(f"Unsupported platform: {system}")
    
    if not lib_path.exists():
        raise RuntimeError(f"Library not found at {lib_path}")
    
    return lib_path


def copy_library_to_package(lib_path: Path) -> None:
    """Copy the library to the package directory."""
    print(f"Copying library from {lib_path}...")
    
    # Get the platform-specific directory
    system = platform.system()
    machine = platform.machine()
    
    package_dir = Path(__file__).parent.parent
    
    if system == "Darwin" and machine == "arm64":
        lib_dir = package_dir / "lib" / "darwin_arm64"
    elif system == "Linux":
        if machine == "aarch64":
            lib_dir = package_dir / "lib" / "linux_aarch64"
        elif machine in ("x86_64", "amd64"):
            lib_dir = package_dir / "lib" / "linux_x86_64"
        else:
            raise RuntimeError(f"Unsupported Linux architecture: {machine}")
    else:
        raise RuntimeError(f"Unsupported platform: {system} {machine}")
    
    # Create the directory and copy the library
    lib_dir.mkdir(parents=True, exist_ok=True)
    dest_path = lib_dir / lib_path.name
    shutil.copy2(lib_path, dest_path)
    print(f"Library copied to {dest_path}")


def build_wheel() -> None:
    """Build the wheel for the current platform."""
    print("Building wheel...")
    
    package_dir = Path(__file__).parent.parent
    
    # Clean up old builds
    for dir_name in ["build", "dist", "simlin.egg-info"]:
        dir_path = package_dir / dir_name
        if dir_path.exists():
            shutil.rmtree(dir_path)
    
    # Build the wheel
    subprocess.run(
        [sys.executable, "-m", "pip", "wheel", ".", "--no-deps", "-w", "dist"],
        cwd=package_dir,
        check=True
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


def main() -> None:
    """Main entry point."""
    print("Building simlin Python package...")
    print(f"Platform: {platform.system()} {platform.machine()}")
    
    # Build the library
    lib_path = build_libsimlin()
    
    # Copy to package
    copy_library_to_package(lib_path)
    
    # Build the wheel
    build_wheel()
    
    print("Build complete!")


if __name__ == "__main__":
    main()