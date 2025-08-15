#!/usr/bin/env bash

# Copyright 2025 The Simlin Authors. All rights reserved.
# Use of this source code is governed by the Apache License,
# Version 2.0, that can be found in the LICENSE file.

set -euxo pipefail

# Build engine2 as a wasm module
# Using wasm32-wasip1 for better stdlib support
cargo build \
    --target wasm32-wasip1 \
    --release \
    --package engine2

# Copy the wasm file to the current directory
cp ../../target/wasm32-wasip1/release/engine2.wasm .

# Report the size
ls -lh engine2.wasm