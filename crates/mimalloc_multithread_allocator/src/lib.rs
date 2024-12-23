//! Mimalloc (safe multithread) for `no_std` systems.
//! written by rust code

#![feature(allocator_api)]
// #![no_std]

extern crate alloc;

use alloc::alloc::{AllocError, Layout};
use core::cmp::max;
use core::mem::size_of;

mod internal;
mod page_queue;
mod segment;
mod alloc_;
mod page;
mod heap;

use internal::*;
use page_queue::*;
use segment::*;
use alloc_::*;
use page::*;
use heap::*;

pub const MI_SMALL_WSIZE_MAX: usize = 128;

#[cfg(target_pointer_width = "64")]
pub const MI_INTPTR_SHIFT: usize = 3;
#[cfg(target_pointer_width = "32")]
pub const MI_INTPTR_SHIFT: usize = 2;

pub const MI_INTPTR_SIZE: usize = 1 << MI_INTPTR_SHIFT;

// 内部数据结构

pub const MI_SMALL_PAGE_SHIFT: usize = 13 + MI_INTPTR_SHIFT;      // 64kb
pub const MI_LARGE_PAGE_SHIFT: usize = 6 + MI_SMALL_PAGE_SHIFT;   // 4mb
pub const MI_SEGMENT_SHIFT: usize = MI_LARGE_PAGE_SHIFT;          // 4mb

pub const MI_SEGMENT_SIZE: usize = 1 << MI_SEGMENT_SHIFT;
pub const MI_SEGMENT_MASK: usize = MI_SEGMENT_SIZE - 1;

pub const MI_SMALL_PAGE_SIZE: usize = 1 << MI_SMALL_PAGE_SHIFT;
pub const MI_LARGE_PAGE_SIZE: usize = 1 << MI_LARGE_PAGE_SHIFT;

pub const MI_SMALL_PAGES_PER_SEGMENT: usize = MI_SEGMENT_SIZE / MI_SMALL_PAGE_SIZE;
pub const MI_LARGE_PAGES_PER_SEGMENT: usize = MI_SEGMENT_SIZE / MI_LARGE_PAGE_SIZE;

pub const MI_LARGE_SIZE_MAX: usize = MI_LARGE_PAGE_SIZE / 8;  // 64位下为512kb
pub const MI_LARGE_WSIZE_MAX: usize = MI_LARGE_SIZE_MAX >> MI_INTPTR_SHIFT;

pub const MI_BIN_HUGE: usize = 64;

// 所需的最小对齐
pub const MI_MAX_ALIGN_SIZE: usize = 16;   // size_of::<max_align_t>()

// 编码的指针类型
pub type MiEncoded = usize;

// 空闲列表中的块
#[repr(C)]
pub struct Block {
    pub next: MiEncoded,
}

// 延迟释放
pub const MI_NO_DELAYED_FREE: u8 = 0;
pub const MI_USE_DELAYED_FREE: u8 = 1;
pub const MI_DELAYED_FREEING: u8 = 2;

// 页面标志
#[repr(C)]
#[derive(Copy, Clone)]
pub union PageFlags {
    pub value: u16,
    pub flags: PageFlagBits,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct PageFlagBits {
    pub has_aligned: bool,
    pub in_full: bool,
}

// 线程空闲列表
#[repr(C)]
#[derive(Copy, Clone)]
pub union ThreadFree {
    pub value: usize,
    pub delayed: u8,
    #[cfg(target_pointer_width = "64")]
    pub head: u64,      
    #[cfg(target_pointer_width = "32")] 
    pub head: u32,
}

pub const MI_TF_PTR_SHIFT: usize = 2;

// 每个页面有三个空闲块列表:
// - free: 可以被分配的块
// - local_free: 被释放但还不能被mi_malloc使用的块 
// - thread_free: 被其他线程释放的块
#[repr(C)]
pub struct Page {
    pub segment_idx: u8,        // 段中pages数组的索引
    pub segment_in_use: bool,   // 段是否分配了此页
    pub is_reset: bool, 
    
    pub flags: PageFlags,
    pub capacity: u16,          // 已提交的块数
    pub reserved: u16,          // 内存中保留的块数

    pub free: *mut Block,       // 可用空闲块列表
    pub cookie: usize,
    pub used: usize,           // 使用中的块数

    pub local_free: *mut Block, // 此线程延迟释放的块列表
    pub thread_freed: usize,    // thread_free中至少有这么多块
    pub thread_free: ThreadFree,// 其他线程延迟释放的块列表

    pub block_size: usize,      // 每个块中可用的大小
    pub heap: *mut Heap,        // 所属堆
    pub next: *mut Page,        // 相同block_size的下一页
    pub prev: *mut Page,        // 相同block_size的上一页
}

// 页面类型
#[derive(Copy, Clone, PartialEq, Debug)]
pub enum PageKind {
    Small,    // 小块进入段内的64kb页面
    Large,    // 较大的块进入跨越整个段的单个页面
    Huge,     // 巨大块(>512kb)放入精确大小的段中的单页(但仍2mb对齐)
}

// 段是从操作系统分配的大内存块(64位下2mb)
// 在段内我们分配固定大小的页面，其中包含块
#[repr(C)]
pub struct Segment {
    pub next: *mut Segment,
    pub prev: *mut Segment, 
    pub abandoned_next: *mut Segment,
    pub abandoned: usize,    // 已废弃的页面数
    pub used: usize,        // 使用中的页面数
    pub capacity: usize,    // 可用页面数
    pub segment_size: usize,
    pub segment_info_size: usize,
    pub cookie: usize,

    pub page_shift: usize,  // 1 << page_shift
    pub thread_id: usize,   // 线程id
    pub page_kind: PageKind,// 页面类型
    pub pages: [Page; 1],   // 最多MI_SMALL_PAGES_PER_SEGMENT个页面
}

pub type Tld = TldData;

// 特定块大小的页面队列
#[repr(C)]
pub struct PageQueue {
    pub first: *mut Page,
    pub last: *mut Page,
    pub block_size: usize,
}

pub const MI_BIN_FULL: usize = MI_BIN_HUGE + 1;

#[repr(C)]
pub struct Heap {
    pub tld: *mut Tld,
    pub pages_free_direct: [*mut Page; MI_SMALL_WSIZE_MAX + 2],  // 优化:数组中每个条目指向可能有对应大小空闲块的页面
    pub pages: [PageQueue; MI_BIN_FULL + 1],                     // 每个大小类的页面队列
    pub thread_delayed_free: *mut Block,
    pub thread_id: usize,                                        // 此堆所属的线程
    pub cookie: usize,
    pub random: usize,                                          // 用于安全分配的随机数
    pub page_count: usize,                                      // pages队列中的总页面数
    pub no_reclaim: bool,                                       // 此堆是否不应回收废弃页面
}

// 段队列
#[repr(C)]
pub struct SegmentQueue {
    pub first: *mut Segment,
    pub last: *mut Segment,
}

// 段线程
#[repr(C)]
pub struct SegmentsTld {
    pub small_free: SegmentQueue,  // 有空闲小页面的段队列
    pub current_size: usize,       // 所有段的当前大小
    pub peak_size: usize,          // 所有段的峰值大小
    pub cache_count: usize,        // 缓存中的段数
    pub cache_size: usize,         // 缓存中所有段的总大小
    pub cache: SegmentQueue,       // 小页面和大页面的段缓存(避免重复mmap调用)
}

// OS线程
#[repr(C)]
pub struct OsTld {
    pub mmap_next_probable: usize, // mmap可能分配的下一个地址 
    pub mmap_previous: *mut u8,// mmap返回的上一个地址
    pub pool: *mut u8,             // 一些平台上用于减少mmap调用的段池
    pub pool_available: usize,     // 池中可用字节数
}

// 线程本地数据
#[repr(C)]
pub struct TldData {
    pub heartbeat: u64,           // 单调递增的心跳计数
    pub heap_backing: *mut Heap,  // 此线程的后备堆(不能删除)
    pub segments: SegmentsTld,    // 段tld
    pub os: OsTld,                // os tld 
}