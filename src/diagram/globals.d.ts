// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Plain (non-module) stylesheets are imported for their side effects only --
// the bundler extracts them. TypeScript 6 turned on noUncheckedSideEffectImports
// by default, which rejects a side-effect import of a module it cannot resolve,
// so declare the shape here. The more specific '*.module.css' declaration in
// css-modules.d.ts still wins for CSS modules.
declare module '*.css';
