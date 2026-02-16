// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Memory management functions for the C FFI / WASM boundary.
//!
//! Provides `simlin_malloc` and `simlin_free` with a header-based layout so
//! that callers (including WASM hosts) can allocate and free memory through
//! the library's own allocator. Also provides `simlin_free_string` for
//! freeing C strings returned by other API functions.

use std::alloc::{alloc, dealloc, Layout};
use std::mem::{align_of, size_of};
use std::os::raw::c_char;

use crate::drop_c_string;

pub(crate) const ALLOC_ALIGN: usize = if align_of::<usize>() > align_of::<f64>() {
    align_of::<usize>()
} else {
    align_of::<f64>()
};
pub(crate) const ALLOC_HEADER_SIZE: usize = 2 * size_of::<usize>();

pub(crate) fn align_up(value: usize, align: usize) -> usize {
    (value + (align - 1)) & !(align - 1)
}

// Memory management functions for WASM
#[no_mangle]
pub extern "C" fn simlin_malloc(size: usize) -> *mut u8 {
    unsafe {
        let total_size = size
            .saturating_add(ALLOC_HEADER_SIZE)
            .saturating_add(ALLOC_ALIGN - 1);
        let layout = Layout::from_size_align_unchecked(total_size, ALLOC_ALIGN);
        let base = alloc(layout);
        if base.is_null() {
            return base;
        }
        let base_addr = base as usize;
        let aligned_addr = align_up(base_addr + ALLOC_HEADER_SIZE, ALLOC_ALIGN);
        let aligned_ptr = aligned_addr as *mut u8;
        let offset = aligned_addr - base_addr;
        let header_ptr = aligned_ptr.sub(ALLOC_HEADER_SIZE);
        *(header_ptr as *mut usize) = size;
        *(header_ptr.add(size_of::<usize>()) as *mut usize) = offset;
        aligned_ptr
    }
}

/// Frees memory allocated by simlin_malloc
///
/// # Safety
/// - `ptr` must be a valid pointer returned by simlin_malloc, or null
/// - The pointer must not be used after calling this function
#[no_mangle]
pub unsafe extern "C" fn simlin_free(ptr: *mut u8) {
    if ptr.is_null() {
        return;
    }
    let header_ptr = ptr.sub(ALLOC_HEADER_SIZE);
    let size = *(header_ptr as *mut usize);
    let offset = *(header_ptr.add(size_of::<usize>()) as *mut usize);
    let total_size = size
        .saturating_add(ALLOC_HEADER_SIZE)
        .saturating_add(ALLOC_ALIGN - 1);
    let layout = Layout::from_size_align_unchecked(total_size, ALLOC_ALIGN);
    let base = ptr.sub(offset);
    dealloc(base, layout);
}

/// Frees a string returned by the API
///
/// # Safety
/// - `s` must be a valid pointer returned by simlin API functions that return strings
#[no_mangle]
pub unsafe extern "C" fn simlin_free_string(s: *mut c_char) {
    drop_c_string(s);
}
