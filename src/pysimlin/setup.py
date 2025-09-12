#!/usr/bin/env python
"""Setup for simlin Python package (CFFI extension)."""

from setuptools import setup

setup(
    name="simlin",
    cffi_modules=["simlin/_ffi_build.py:ffibuilder"],
)
