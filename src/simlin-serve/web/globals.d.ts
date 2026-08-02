// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Side-effect CSS imports (katex, styles.css) have no JS module shape;
// this declaration replaces the one `vite/client` used to provide.
declare module '*.css';
