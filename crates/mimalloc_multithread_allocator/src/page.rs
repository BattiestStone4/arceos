use std::sync::atomic::{AtomicUsize, Ordering};
use std::{ptr, mem};

use crate::*;

// 页面中的块定义
#[repr(C)]
pub struct Block {
    next: *mut Block,
}

/// 在页面中按索引定位块
#[inline]
unsafe fn mi_page_block_at(page: &Page, page_start: *mut u8, index: usize) -> *mut Block {    
    page_start.add(index * page.block_size) as *mut Block
}

/// 页面初始化
unsafe fn mi_page_init(heap: &mut Heap, page: &mut Page, size: usize, stats: &mut Stats) {
    page.block_size = size;
    page.used = 0;
    page.free = ptr::null_mut();
    page.local_free = ptr::null_mut();
    page.thread_free.value = 0;
    page.flags.value = 0;
    page.cookie = heap.cookie;
}

/// 设置页面的延迟释放标志
pub unsafe fn mi_page_use_delayed_free(page: &mut Page, enable: bool) {
    let mut tfree;
    let mut tfreex;

    loop {
        tfreex = tfree = page.thread_free;
        tfreex.delayed = if enable { MI_USE_DELAYED_FREE } else { MI_NO_DELAYED_FREE };
        
        if tfree.delayed == MI_DELAYED_FREEING {
            // 等待进行中的延迟释放完成
            atomic::spin_loop_hint();
            continue;
        }
        
        if tfreex.delayed == tfree.delayed {
            // 值已经是期望的,避免原子操作
            break;
        }
        
        if mi_atomic_compare_exchange(
            &mut page.thread_free as *mut _,
            tfreex.value,
            tfree.value,
            Ordering::AcqRel,
            Ordering::Acquire
        ).is_ok() {
            break;
        }
    }
}

/// 收集页面的线程空闲列表
unsafe fn mi_page_thread_free_collect(page: &mut Page) {
    // 原子交换获取头部
    let head = {
        let mut tfree;
        let mut tfreex;
        
        loop {
            tfreex = tfree = page.thread_free;
            let head = (tfree.head as usize) << MI_TF_PTR_SHIFT;
            tfreex.head = 0;
            
            if mi_atomic_compare_exchange(
                &mut page.thread_free as *mut _,
                tfreex.value,
                tfree.value,
                Ordering::AcqRel,
                Ordering::Acquire
            ).is_ok() {
                break head as *mut Block;
            }
        }
    };

    // 如果列表为空则返回
    if head.is_null() {
        return;
    }

    // 找到尾部并计数
    let mut count = 1;
    let mut tail = head;
    while let Some(next) = mi_block_next(page, tail) {
        count += 1;
        tail = next;
    }

    // 添加到空闲列表头部
    mi_block_set_next(page, tail, page.free);
    page.free = head;

    // 更新计数
    page.thread_freed -= count;
    page.used -= count;
}

/// 收集页面中的所有空闲块
pub unsafe fn mi_page_free_collect(page: &mut Page) {
    // 处理本地空闲列表
    if !page.local_free.is_null() {
        if page.free.is_null() {
            // 常见情况
            page.free = page.local_free;
        } else {
            // 找到空闲列表尾部并连接
            let mut tail = page.free;
            while let Some(next) = mi_block_next(page, tail) {
                tail = next;
            }
            mi_block_set_next(page, tail, page.local_free);
        }
        page.local_free = ptr::null_mut();
    }

    // 处理线程空闲列表
    if page.thread_free.head != 0 {
        mi_page_thread_free_collect(page);
    }
}

/// 从段中回收被遗弃的页面
pub unsafe fn mi_page_reclaim(heap: &mut Heap, page: &mut Page) {
    mi_page_free_collect(page);
    let pq = mi_page_queue(heap, page.block_size);
    mi_page_queue_push(heap, pq, page);
}

/// 从段中分配新页面
unsafe fn mi_page_fresh_alloc(
    heap: &mut Heap,
    pq: &mut PageQueue,
    block_size: usize
) -> *mut Page {    
    // 从段中分配新页面
    let page = mi_segment_page_alloc(
        block_size,
        &mut heap.tld.segments,
        &mut heap.tld.os
    );
    
    if page.is_null() {
        return ptr::null_mut();
    }
    mi_page_init(heap, &mut *page, block_size, &mut heap.tld.stats);
    mi_page_queue_push(heap, pq, page);
    
    page
}

/// 获取新的可用页面
unsafe fn mi_page_fresh(heap: &mut Heap, pq: &mut PageQueue) -> *mut Page {
    let mut page = pq.first;
    if !heap.no_reclaim && 
       mi_segment_try_reclaim_abandoned(heap, false, &mut heap.tld.segments) &&
       page != pq.first 
    {
        // 成功回收并且找到了合适的页面
        page = pq.first;
        if !(*page).free.is_null() {
            return page;
        }
    }
    // 分配新页面
    page = mi_page_fresh_alloc(heap, pq, pq.block_size);
    if page.is_null() {
        return ptr::null_mut();
    }
    page
}

/// 处理堆的延迟释放
pub unsafe fn mi_heap_delayed_free(heap: &mut Heap) {
    let mut block = {
        loop {
            let block = heap.thread_delayed_free;
            if block.is_null() {
                return;
            }
            
            if mi_atomic_compare_exchange_ptr(
                &mut heap.thread_delayed_free,
                ptr::null_mut(),
                block,
                Ordering::AcqRel,
                Ordering::Acquire
            ).is_ok() {
                break block;
            }
        }
    };

    // 释放所有延迟块
    while !block.is_null() {
        let next = mi_block_nextx(heap.cookie, block);
        mi_free_delayed_block(block);
        block = next;
    } 
}

/// 将页面从满页面列表移回普通列表
pub unsafe fn mi_page_unfull(page: &mut Page) {
    mi_page_use_delayed_free(page, false);
    if !page.flags.in_full {
        return;
    }

    let heap = page.heap;
    let pqfull = &mut heap.pages[MI_BIN_FULL];
    
    page.flags.in_full = false;
    let pq = mi_heap_page_queue_of(heap, page);
    page.flags.in_full = true;
    
    mi_page_queue_enqueue_from(pq, pqfull, page);
}

/// 将页面移入满页面列表
unsafe fn mi_page_to_full(page: &mut Page, pq: &mut PageQueue) {
    // 启用延迟释放
    mi_page_use_delayed_free(page, true);
    if page.flags.in_full {
        return;
    }
    mi_page_queue_enqueue_from(
        &mut page.heap.pages[MI_BIN_FULL],
        pq,
        page
    );
    
    mi_page_thread_free_collect(page);
}

pub unsafe fn mi_page_abandon(page: &mut Page, pq: &mut PageQueue) {
    let segments_tld = &mut page.heap.tld.segments;
    mi_page_queue_remove(pq, page);

    mi_segment_page_abandon(page, segments_tld);
}

/// 释放一个没有已用块的页面
pub unsafe fn mi_page_free(page: &mut Page, pq: &mut PageQueue, force: bool) {
    page.flags.has_aligned = false;

    if page.block_size > MI_LARGE_SIZE_MAX {
        mi_heap_stat_decrease(page.heap, huge, page.block_size);
    }

    let segments_tld = &mut page.heap.tld.segments;
    mi_page_queue_remove(pq, page);
    mi_segment_page_free(page, force, segments_tld);
}

/// 回收一个没有已用块的页面
pub unsafe fn mi_page_retire(page: &mut Page) {
    page.flags.has_aligned = false;

    // 对于小页面,如果邻近页面使用率高就不回收 
    if page.block_size <= MI_LARGE_SIZE_MAX {
        if mi_page_mostly_used(page.prev) && mi_page_mostly_used(page.next) {
            return;
        }
    }

    mi_page_free(page, mi_page_queue_of(page), false);
}

// 常量定义
const MI_MAX_SLICE_SHIFT: usize = 6;   // 最多 64 个分片
const MI_MAX_SLICES: usize = 1 << MI_MAX_SLICE_SHIFT;
const MI_MIN_SLICES: usize = 2;

const MI_MAX_EXTEND_SIZE: usize = 4 * 1024; 
const MI_MIN_EXTEND: usize = if cfg!(feature = "secure") { 8 } else { 1 };

// 在页面中初始化最初的空闲列表
unsafe fn mi_page_free_list_extend(
    heap: &mut Heap,
    page: &mut Page,
    extend: usize,
    stats: &mut Stats
) {
    let page_area = mi_page_start(mi_page_segment(page), page, None);
    let bsize = page.block_size;
    let start = mi_page_block_at(page, page_area, page.capacity);

    if extend < MI_MIN_SLICES || !mi_option_is_enabled(mi_option_secure) {
        // 初始化一个顺序的空闲列表
        let end = mi_page_block_at(page, page_area, page.capacity + extend - 1);
        let mut block = start;
        
        for _ in 0..extend {
            let next = (block as *mut u8).add(bsize) as *mut Block;
            mi_block_set_next(page, block, next);
            block = next;
        }
        
        mi_block_set_next(page, end, std::ptr::null_mut());
        page.free = start;
    } else {
        // 初始化一个随机的空闲列表
        // 设置 `slice_count` 个分片来交替
        let mut shift = MI_MAX_SLICE_SHIFT;
        while (extend >> shift) == 0 {
            shift -= 1;
        }
        
        let slice_count = 1usize << shift;
        let slice_extend = extend / slice_count;
        
        let mut blocks = vec![std::ptr::null_mut(); MI_MAX_SLICES];
        let mut counts = vec![0usize; MI_MAX_SLICES];
        
        for i in 0..slice_count {
            blocks[i] = mi_page_block_at(
                page,
                page_area,
                page.capacity + i * slice_extend
            );
            counts[i] = slice_extend;
        }
        
        // 最后一个分片也保存余数
        counts[slice_count - 1] += extend % slice_count;

        // 通过随机穿过它们来初始化空闲列表
        // 设置第一个元素
        let mut current = mi_heap_random(heap) % slice_count;
        counts[current] -= 1;
        page.free = blocks[current];

        // 遍历剩余部分
        let mut rnd = heap.random;
        for i in 1..extend {
            // 每个 INTPTR_SIZE 轮才调用 random_shuffle
            let round = i % std::mem::size_of::<usize>();
            if round == 0 {
                rnd = mi_random_shuffle(rnd);
            }
            
            // 选择一个随机的下一个分片索引
            let mut next = ((rnd >> (8 * round)) & (slice_count - 1)) as usize;
            while counts[next] == 0 {
                next += 1;
                if next == slice_count {
                    next = 0;
                }
            }
            
            // 链接当前块到下一个
            counts[next] -= 1;
            let block = blocks[current];
            blocks[current] = (block as *mut u8).add(bsize) as *mut Block;
            mi_block_set_next(page, block, blocks[next]);
            current = next;
        }
        
        mi_block_set_next(page, blocks[current], std::ptr::null_mut());
        heap.random = mi_random_shuffle(rnd);
    }

    // 启用新的空闲列表
    page.capacity += extend as u16;
    mi_stat_increase(&mut stats.committed, extend * page.block_size);
}

// 扩展容量
unsafe fn mi_page_extend_free(
    heap: &mut Heap, 
    page: &mut Page,
    stats: &mut Stats
) { 
    if !page.free.is_null() {
        return;
    } 
    if page.capacity >= page.reserved {
        return;
    }

    let page_size = mi_page_size(mi_page_segment(page), page);
    if page.is_reset {
        page.is_reset = false;
        mi_stat_decrease(&mut stats.reset, page_size);
    }
    mi_stat_increase(&mut stats.pages_extended, 1);

    let mut extend = page.reserved - page.capacity;
    let max_extend = MI_MAX_EXTEND_SIZE / page.block_size;
    let max_extend = if max_extend < MI_MIN_EXTEND { MI_MIN_EXTEND } else { max_extend };

    if extend > max_extend {
        extend = if max_extend == 0 { 1 } else { max_extend };
    }
    mi_page_free_list_extend(heap, page, extend, stats);
}

// 找到具有空闲块的页面
unsafe fn mi_page_queue_find_free_ex(
    heap: &mut Heap,
    pq: &mut PageQueue
) -> *mut Page {
    let mut rpage = std::ptr::null_mut();
    let mut count = 0;
    let mut page_free_count = 0;
    let mut page = pq.first;

    while !page.is_null() {
        let next = (*page).next;
        count += 1;

        // 收集由我们和其他线程释放的块
        mi_page_free_collect(page);

        // 如果页面包含空闲块，我们完成了
        if mi_page_immediate_available(page) {
            // 如果所有块都是空闲的，我们可能会替换掉这个页面
            // 最多执行8次以限制分配时间
            if page_free_count < 8 && mi_page_all_free(page) {
                page_free_count += 1;
                if !rpage.is_null() {
                    mi_page_free(rpage, pq, false);
                }
                rpage = page;
                page = next;
                continue;
            } else {
                break;
            }
        }

        // 尝试扩展
        if (*page).capacity < (*page).reserved {
            mi_page_extend_free(heap, &mut *page, &mut heap.tld.stats);
            break;
        }
        mi_page_to_full(page, pq);

        page = next;
    }

    mi_stat_counter_increase(&mut heap.tld.stats.searches, count);

    if page.is_null() {
        page = rpage;
        rpage = std::ptr::null_mut();
    }
    if !rpage.is_null() {
        mi_page_free(rpage, pq, false);
    }
    if page.is_null() {
        page = mi_page_fresh(heap, pq);
    }

    page
}

// 找到一个具有指定大小空闲块的页面
#[inline]
unsafe fn mi_find_free_page(
    heap: &mut Heap,
    size: usize
) -> *mut Page {
    mi_heap_delayed_free(heap);
    let pq = mi_page_queue(heap, size);
    let mut page = (*pq).first;
    
    if !page.is_null() {
        if mi_option_get(mi_option_secure) >= 3
            && (*page).capacity < (*page).reserved 
            && (mi_heap_random(heap) & 1) == 1 
        {
            mi_page_extend_free(heap, &mut *page, &mut heap.tld.stats);
        } else {
            mi_page_free_collect(page);
        }
        
        if mi_page_immediate_available(page) {
            return page; // 快速路径
        }
    }
    
    mi_page_queue_find_free_ex(heap, pq)
}

// 注册延迟释放函数接口
static mut DEFERRED_FREE: Option<fn(bool, u64)> = None;

unsafe fn mi_deferred_free(heap: &mut Heap, force: bool) {
    heap.tld.heartbeat += 1;
    if let Some(deferred_free) = DEFERRED_FREE {
        deferred_free(force, heap.tld.heartbeat);
    }
}

pub fn mi_register_deferred_free(fun: fn(bool, u64)) {
    unsafe {
        DEFERRED_FREE = Some(fun);
    }
}

// 分配一个 huge 页面
unsafe fn mi_huge_page_alloc(
    heap: &mut Heap,
    size: usize
) -> *mut Page {
    let block_size = mi_wsize_from_size(size) * std::mem::size_of::<usize>();
    let pq = mi_page_queue(heap, block_size);
    
    let page = mi_page_fresh_alloc(heap, pq, block_size);
    if !page.is_null() {
        mi_heap_stat_increase(&mut heap.tld.stats.huge, block_size);
    }
    page
}

// 通用分配函数
pub unsafe extern "C" fn mi_malloc_generic(
    heap: &mut Heap,
    size: usize
) -> *mut u8 {
    if !mi_heap_is_initialized(heap) {
        mi_thread_init();
        heap = mi_get_default_heap();
    }

    mi_deferred_free(heap, false);
    let page = if size > MI_LARGE_SIZE_MAX {
        mi_huge_page_alloc(heap, size)
    } else {
        // 否则在我们的大小分离队列中找到一个具有空闲块的页面
        mi_find_free_page(heap, size)
    };

    if page.is_null() {
        return std::ptr::null_mut();
    }
    mi_page_malloc(heap, page, size)
}