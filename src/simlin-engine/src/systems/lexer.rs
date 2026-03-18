// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Line-oriented lexer for the systems format.
//!
//! Classifies each line as a comment, stock-only declaration, or flow line,
//! and tokenizes rate formulas into expression trees.
