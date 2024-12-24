#![no_std]
#![allow(nonstandard_style)]
use cty::{c_void};

extern "C" {
    pub fn mi_free(p: *mut c_void);
    pub fn mi_malloc(size: usize) -> *mut c_void;
    pub fn mi_malloc_aligned(size: usize, alignment: usize) -> *mut c_void;
}

pub mod heap;
pub mod types;
pub mod utils;
