use std::ptr::NonNull;
use std::sync::atomic::Ordering;

use crate::*;

// 快速路径
#[inline]
pub unsafe fn mi_page_malloc(heap: &mut Heap, page: &mut Page, size: usize) -> *mut u8 {
    let block = page.free;
    if block.is_null() {
        return mi_malloc_generic(heap, size); // 慢路径
    } 
    page.free = mi_block_next(page, block);
    page.used += 1;
    block as *mut u8
}

// 分配小块
#[inline]
pub unsafe fn mi_heap_malloc_small(heap: &mut Heap, size: usize) -> *mut u8 {
    let page = mi_heap_get_free_small_page(heap, size);
    mi_page_malloc(heap, page, size)
}

#[inline]
pub unsafe fn mi_malloc_small(size: usize) -> *mut u8 {
    mi_heap_malloc_small(mi_get_default_heap(), size)
}

// 分配已初始化为0的小块
pub unsafe fn mi_zalloc_small(size: usize) -> *mut u8 {
    let p = mi_malloc_small(size);
    if !p.is_null() {
        std::ptr::write_bytes(p, 0, size);
    }
    p
}

// 主要的分配函数
#[inline]
pub unsafe fn mi_heap_malloc(heap: &mut Heap, size: usize) -> *mut u8 {
    let p = if size <= MI_SMALL_SIZE_MAX {
        mi_heap_malloc_small(heap, size)
    } else {
        mi_malloc_generic(heap, size)  
    };

    p
}

#[inline]
pub unsafe fn mi_malloc(size: usize) -> *mut u8 {
    mi_heap_malloc(mi_get_default_heap(), size)  
}

// 0初始化的分配
unsafe fn mi_heap_malloc_zero(heap: &mut Heap, size: usize, zero: bool) -> *mut u8 {
    let p = mi_heap_malloc(heap, size);
    if zero && !p.is_null() {
        std::ptr::write_bytes(p, 0, size);
    }
    p
}

#[inline]
pub unsafe fn mi_heap_zalloc(heap: &mut Heap, size: usize) -> *mut u8 {
    mi_heap_malloc_zero(heap, size, true)
}

pub unsafe fn mi_zalloc(size: usize) -> *mut u8 {
    mi_heap_zalloc(mi_get_default_heap(), size)
}

// 多线程释放实现
#[inline(never)]
unsafe fn mi_free_block_mt(page: &mut Page, block: *mut Block) {
    let mut tfree;
    let mut tfreex;
    let mut use_delayed;

    loop {
        tfree = page.thread_free;
        tfreex = tfree;
        use_delayed = tfree.delayed == MI_DELAYED_FREEING;
        
        if use_delayed {
            // 第一次并发释放时才会发生
            tfreex.delayed = MI_DELAYED_FREEING;
        } else {
            mi_block_set_next(page, block, 
                (tfree.head as usize) << MI_TF_PTR_SHIFT);
            tfreex.head = block as usize >> MI_TF_PTR_SHIFT;
        }
        if mi_atomic_compare_exchange(
            &mut page.thread_free as *mut _,
            tfreex.value,
            tfree.value
        ) {
            break;
        }
    } 
        

    if !use_delayed {
        // 增加线程空闲计数并返回
        mi_atomic_increment(&mut page.thread_freed);
    } else {
        // heap的unsafe读取是允许的,因为设置了MI_DELAYED_FREEING
        let heap = page.heap;
        
        if !heap.is_null() {
            let mut dfree;
            loop {
                dfree = heap.thread_delayed_free;
                mi_block_set_nextx(heap.cookie, block, dfree);
            } while !mi_atomic_compare_exchange_ptr(
                &mut heap.thread_delayed_free,
                block,
                dfree
            );
        }

        loop {
            tfreex = tfree = page.thread_free;
            tfreex.delayed = MI_NO_DELAYED_FREE;
        } while !mi_atomic_compare_exchange(
            &mut page.thread_free as *mut _,
            tfreex.value,
            tfree.value
        );
    }
}

// 普通释放
#[inline]
unsafe fn mi_free_block(page: &mut Page, local: bool, block: *mut Block) {
    if local {
        // 所属线程可以直接释放块
        mi_block_set_next(page, block, page.local_free);
        page.local_free = block;
        page.used -= 1;
        
        if mi_page_all_free(page) {
            mi_page_retire(page);
        } else if page.flags.in_full {
            mi_page_unfull(page);
        }
    } else {
        mi_free_block_mt(page, block);
    }
}

// 调整对齐分配的块到页面中实际块的开始位置
unsafe fn mi_page_ptr_unalign(
    segment: &Segment,
    page: &Page,
    p: *mut u8
) -> *mut Block {
    
    let diff = p as usize - mi_page_start(segment, page, None);
    let adjust = diff % page.block_size;
    
    (p as usize - adjust) as *mut Block
}

// 通用释放
#[inline(never)]
unsafe fn mi_free_generic(
    segment: &Segment,
    page: &mut Page,
    local: bool,
    p: *mut u8
) {
    let block = if page.flags.has_aligned {
        mi_page_ptr_unalign(segment, page, p)
    } else {
        p as *mut Block
    };
    mi_free_block(page, local, block);
}

// 释放一个块
pub unsafe fn mi_free(p: *mut u8) {
    let segment = mi_ptr_segment(p);
    if segment.is_null() {
        return;
    }
    let local = mi_thread_id() == segment.thread_id;
    let page = mi_segment_page_of(segment, p);

    if page.flags.value == 0 {
        let block = p as *mut Block;
        if local {
            // 所属线程可以直接释放块
            mi_block_set_next(page, block, page.local_free);
            page.local_free = block;
            page.used -= 1;
            
            if mi_page_all_free(page) {
                mi_page_retire(page);
            }
        } else {
            mi_free_block_mt(page, block);
        }
    } else {
        mi_free_generic(segment, page, local, p);
    }
}

// 释放延迟块
pub unsafe fn mi_free_delayed_block(block: *mut Block) {    
    let segment = mi_ptr_segment(block as *mut u8);
    let page = mi_segment_page_of(segment, block as *mut u8);
    mi_free_block(page, true, block);
}

// 获取块中可用的字节数
pub unsafe fn mi_usable_size(p: *mut u8) -> usize {
    if p.is_null() {
        return 0;
    }
    
    let segment = mi_ptr_segment(p);
    let page = mi_segment_page_of(segment, p);
    let size = page.block_size;
    
    if page.flags.has_aligned {
        let adjust = (p as usize) - 
            (mi_page_ptr_unalign(segment, page, p) as usize);
        size - adjust
    } else {
        size  
    }
}

pub unsafe fn mi_heap_mallocn(
    heap: &mut Heap,
    count: usize,
    size: usize
) -> *mut u8 {
    match count.checked_mul(size) {
        Some(total) => mi_heap_malloc(heap, total),
        None => std::ptr::null_mut()
    }
}

pub unsafe fn mi_mallocn(count: usize, size: usize) -> *mut u8 {
    mi_heap_mallocn(mi_get_default_heap(), count, size)
}

// calloc - 分配已初始化为0的内存
#[inline]
pub unsafe fn mi_heap_calloc(
    heap: &mut Heap,
    count: usize,
    size: usize
) -> *mut u8 {
    match count.checked_mul(size) {
        Some(total) => mi_heap_zalloc(heap, total),
        None => std::ptr::null_mut()
    }
}

pub unsafe fn mi_calloc(count: usize, size: usize) -> *mut u8 {
    mi_heap_calloc(mi_get_default_heap(), count, size)
}

// 原地扩展或失败
pub unsafe fn mi_expand(p: *mut u8, newsize: usize) -> *mut u8 {
    if p.is_null() {
        return std::ptr::null_mut();
    }
    let size = mi_usable_size(p);
    if newsize > size {
        return std::ptr::null_mut();
    }
    p // 大小合适
}

// 重新分配内存
unsafe fn mi_realloc_zero(
    p: *mut u8,
    newsize: usize,
    zero: bool
) -> *mut u8 {
    if p.is_null() {
        return mi_heap_malloc_zero(mi_get_default_heap(), newsize, zero);
    }

    let size = mi_usable_size(p);
    if newsize <= size && newsize >= (size / 2) {
        return p; // 重新分配仍然适合且浪费不超过50%
    }

    let newp = mi_malloc(newsize);
    if !newp.is_null() {
        if zero && newsize > size {
            // 确保任何填充都被初始化为零
            let start = if size >= std::mem::size_of::<usize>() {
                size - std::mem::size_of::<usize>()
            } else {
                0
            };
            std::ptr::write_bytes(
                newp.add(start),
                0,
                newsize - start
            );
        }
        std::ptr::copy_nonoverlapping(
            p,
            newp,
            std::cmp::min(size, newsize)
        );
        mi_free(p);
    }
    newp
}

pub unsafe fn mi_realloc(p: *mut u8, newsize: usize) -> *mut u8 {
    mi_realloc_zero(p, newsize, false)
}

pub unsafe fn mi_rezalloc(p: *mut u8, newsize: usize) -> *mut u8 {
    mi_realloc_zero(p, newsize, true)
}

// recalloc - 重新分配并初始化为0 
pub unsafe fn mi_recalloc(
    p: *mut u8,
    count: usize,
    size: usize
) -> *mut u8 {
    match count.checked_mul(size) {
        Some(total) => mi_rezalloc(p, total),
        None => std::ptr::null_mut()
    }
}

// reallocn - 重新分配count * size大小的内存
pub unsafe fn mi_reallocn(
    p: *mut u8,
    count: usize,
    size: usize
) -> *mut u8 {
    match count.checked_mul(size) {
        Some(total) => mi_realloc(p, total),
        None => std::ptr::null_mut()
    }
}

// reallocf - 重新分配,如果失败则释放p
pub unsafe fn mi_reallocf(p: *mut u8, newsize: usize) -> *mut u8 {
    let newp = mi_realloc(p, newsize);
    if newp.is_null() && !p.is_null() {
        mi_free(p);
    }
    newp
}

// strdup - 使用mi_malloc复制字符串
pub unsafe fn mi_heap_strdup(
    heap: &mut Heap,
    s: *const i8
) -> *mut i8 {
    if s.is_null() {
        return std::ptr::null_mut();
    }
    
    let n = libc::strlen(s);
    let t = mi_heap_malloc(heap, n + 1) as *mut i8;
    if !t.is_null() {
        std::ptr::copy_nonoverlapping(s, t, n + 1);
    }
    t
}

pub unsafe fn mi_strdup(s: *const i8) -> *mut i8 {
    mi_heap_strdup(mi_get_default_heap(), s)
}

// strndup - 使用mi_malloc复制最多n个字符
pub unsafe fn mi_heap_strndup(
    heap: &mut Heap,
    s: *const i8,
    n: usize
) -> *mut i8 {
    if s.is_null() {
        return std::ptr::null_mut();
    }
    
    let m = libc::strlen(s);
    let n = std::cmp::min(m, n);
    let t = mi_heap_malloc(heap, n + 1) as *mut i8;
    if t.is_null() {
        return std::ptr::null_mut();
    }
    
    std::ptr::copy_nonoverlapping(s, t, n);
    *t.add(n) = 0;
    t
}

pub unsafe fn mi_strndup(s: *const i8, n: usize) -> *mut i8 {
    mi_heap_strndup(mi_get_default_heap(), s, n)
}

pub unsafe fn mi_heap_realpath(
    heap: &mut Heap,
    fname: *const i8,
    resolved_name: *mut i8
) -> *mut i8 {
    if !resolved_name.is_null() {
        libc::realpath(fname, resolved_name)
    } else {
        const PATH_MAX: usize = 260;
        let mut buf = [0i8; PATH_MAX + 1];
        let rname = libc::realpath(fname, buf.as_mut_ptr());
        mi_heap_strndup(heap, rname, PATH_MAX)
    }
}

pub unsafe fn mi_realpath(
    fname: *const i8,
    resolved_name: *mut i8
) -> *mut i8 {
    mi_heap_realpath(mi_get_default_heap(), fname, resolved_name)
}