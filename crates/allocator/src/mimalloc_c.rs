//! Mimalloc C memory allocation.
//!
//!

use core::ptr::{self, null};

use super::{AllocError, AllocResult, BaseAllocator, ByteAllocator};
use heap::*;
use mimalloc_c_allocator::*;
use types::mi_heap_t;

/// A byte-granularity memory allocator based on the [tlsf_allocator] written by C code.
pub struct MiCAllocator {
    inner: *mut mi_heap_t,
}

impl MiCAllocator {
    /// Creates a new empty `MiCAllocator`.
    pub const fn new() -> Self {
        Self { inner: ptr::null_mut() }
    }

    fn inner_mut(&mut self) -> &mut mi_heap_t {
        unsafe { self.inner.as_mut().unwrap() }
    }

    fn inner(&self) -> &mi_heap_t {
        unsafe { self.inner.as_ref().unwrap() }
    }
}

impl BaseAllocator for MiCAllocator {
    fn init(&mut self, start: usize, size: usize) {
        self.inner = unsafe { mi_heap_new() };
        unsafe { mi_heap_set_default(self.inner) };
    }

    fn add_memory(&mut self, start: usize, size: usize) -> AllocResult {
        Ok(())
    }
}

impl ByteAllocator for MiCAllocator {
    fn alloc(&mut self, size: usize, align_pow2: usize) -> AllocResult<usize> {
        unsafe { mi_malloc_aligned(size, align_pow2); }
        Ok(1)
    }

    fn dealloc(&mut self, pos: usize, size: usize, align_pow2: usize) {
        unsafe { mi_free(pos as *mut _); }
    }

    fn total_bytes(&self) -> usize {
        0usize
    }

    fn used_bytes(&self) -> usize {
        0usize
    }

    fn available_bytes(&self) -> usize {
        0usize
    }
}
