use std::ptr::NonNull;

use crate::*;

// 溢出检测乘法
const MI_MUL_NO_OVERFLOW: usize = 1 << (4 * std::mem::size_of::<usize>()); // sqrt(SIZE_MAX)

#[inline]
pub fn mi_mul_overflow(size: usize, count: usize) -> (bool, usize) {
    let total = size * count;
    let overflow = (size >= MI_MUL_NO_OVERFLOW || count >= MI_MUL_NO_OVERFLOW)
        && size > 0 && (usize::MAX / size) < count;
    (overflow, total)
}

// 将字节大小对齐到机器字长
// 即字节大小 == `wsize * size_of::<usize>()`
#[inline]
pub fn mi_wsize_from_size(size: usize) -> usize {
    (size + std::mem::size_of::<usize>() - 1) / std::mem::size_of::<usize>()
}

// 只读空堆，线程本地默认堆的初始值
static MI_HEAP_EMPTY: Heap = Heap::empty();
// 静态分配的主备份堆
static mut MI_HEAP_MAIN: Heap = Heap::main();

thread_local! {
    // 默认分配堆
    static MI_HEAP_DEFAULT: &'static mut Heap = &mut MI_HEAP_MAIN;
}

#[inline]
pub fn mi_get_default_heap() -> &'static mut Heap {
    MI_HEAP_DEFAULT.with(|heap| heap)
}

#[inline]
pub fn mi_heap_is_default(heap: &Heap) -> bool {
    std::ptr::eq(heap, mi_get_default_heap())
}

#[inline]
pub fn mi_heap_is_backing(heap: &Heap) -> bool {
    heap.tld.heap_backing.as_ref() == Some(heap)
}

#[inline]
pub fn mi_heap_is_initialized(heap: &Heap) -> bool {
    !std::ptr::eq(heap, &MI_HEAP_EMPTY)
}

#[inline]
pub fn mi_heap_get_free_small_page(heap: &Heap, size: usize) -> Option<NonNull<Page>> {
    heap.pages_free_direct[mi_wsize_from_size(size)]
}

// 获取特定大小类的页面
#[inline]
pub fn mi_get_free_small_page(size: usize) -> Option<NonNull<Page>> {
    mi_heap_get_free_small_page(mi_get_default_heap(), size)
}

// 获取包含指针的段
#[inline]
pub fn mi_ptr_segment(p: *const u8) -> *mut Segment {
    (p as usize & !MI_SEGMENT_MASK) as *mut Segment
}

// 获取页面所属的段
#[inline]
pub fn mi_page_segment(page: &Page) -> &mut Segment {
    let segment = mi_ptr_segment(page as *const _ as *const u8);
    unsafe { &mut *segment }
}

// 获取包含指针的页面
#[inline]
pub fn mi_segment_page_of(segment: &Segment, p: *const u8) -> &mut Page {
    let diff = p as usize - segment as *const _ as usize;
    let idx = diff >> segment.page_shift;
    unsafe { &mut (*segment).pages[idx] }
}

// 已初始化页面的快速页面起始地址获取
#[inline]
pub fn mi_page_start(segment: &Segment, page: &Page) -> (*mut u8, usize) {
    mi_segment_page_start(segment, page)
}

// 获取包含指针的页面
#[inline]
pub fn mi_ptr_page(p: *mut u8) -> &'static mut Page {
    let segment = mi_ptr_segment(p);
    unsafe { mi_segment_page_of(&*segment, p) }
}

// 页面是否所有块都已释放
#[inline]
pub fn mi_page_all_free(page: &Page) -> bool {
    page.used - page.thread_freed == 0
}

// 是否有立即可用的块
#[inline]
pub fn mi_page_immediate_available(page: &Page) -> bool {
    !page.free.is_null()
}

// 页面中是否有空闲块
#[inline]
pub fn mi_page_has_free(page: &Page) -> bool {
    let has_free = mi_page_immediate_available(page) 
        || !page.local_free.is_null() 
        || page.thread_free.head != 0;
    has_free
}

// 是否所有块都在使用中
#[inline]
pub fn mi_page_all_used(page: &Page) -> bool {
    !mi_page_has_free(page)
}

// 是否超过 7/8 的页面在使用中
#[inline]
pub fn mi_page_mostly_used(page: &Page) -> bool {
    if page.is_null() {
        return true;
    }
    let frac = page.reserved / 8;
    page.reserved - page.used + page.thread_freed < frac
}

#[inline]
pub fn mi_page_queue(heap: &Heap, size: usize) -> &mut PageQueue {
    &mut heap.pages[mi_bin(size)]
}

// 编码/解码空闲列表的下一个指针

#[inline]
pub fn mi_block_nextx(cookie: usize, block: &Block) -> *mut Block {
    #[cfg(feature = "mi_secure")]
    {
        (block.next ^ cookie) as *mut Block
    }
    #[cfg(not(feature = "mi_secure"))]
    {
        let _ = cookie;
        block.next as *mut Block
    }
}

#[inline]
pub fn mi_block_set_nextx(cookie: usize, block: &mut Block, next: *mut Block) {
    #[cfg(feature = "mi_secure")]
    {
        block.next = next as usize ^ cookie;
    }
    #[cfg(not(feature = "mi_secure"))]
    {
        let _ = cookie;
        block.next = next as usize;
    }
}

#[inline]
pub fn mi_block_next(page: &Page, block: &Block) -> *mut Block {
    mi_block_nextx(page.cookie, block)
}

#[inline]
pub fn mi_block_set_next(page: &Page, block: &mut Block, next: *mut Block) {
    mi_block_set_nextx(page.cookie, block, next)
}