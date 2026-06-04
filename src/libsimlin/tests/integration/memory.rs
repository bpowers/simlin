// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::os::raw::c_double;

use simlin::*;

use std::mem::align_of;

#[test]
fn test_simlin_malloc_alignment() {
    unsafe {
        let ptr = simlin_malloc(1);
        assert!(!ptr.is_null());
        assert_eq!((ptr as usize) % align_of::<c_double>(), 0);
        simlin_free(ptr);
    }
}
