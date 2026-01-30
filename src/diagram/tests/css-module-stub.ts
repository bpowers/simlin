// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Stub for CSS modules in Jest tests - returns the property name as the class name
const stub = new Proxy(
  {},
  {
    get: (_target, prop) => (typeof prop === 'string' ? prop : ''),
  },
);

export default stub;
