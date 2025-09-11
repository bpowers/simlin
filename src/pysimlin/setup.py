#!/usr/bin/env python
"""Setup for simlin Python package (CFFI extension)."""

from setuptools import setup

# Read version from pyproject.toml using stdlib tomllib (Python 3.11+)
import tomllib

with open("pyproject.toml", "rb") as f:
    pyproject = tomllib.load(f)
    version = pyproject["project"]["version"]

setup(
    name="simlin",
    version=version,
    cffi_modules=["simlin/_ffi_build.py:ffibuilder"],
)
