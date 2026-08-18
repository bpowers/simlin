// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Side-effect stylesheet imports (theme.css, katex.min.css) have no JS module
// shape; TypeScript 6's noUncheckedSideEffectImports needs this declaration.
declare module '*.css';
