use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};
use std::ptr::NonNull;

use crate::*;

// 页面访问器
type HeapPageVisitorFn = fn(heap: &mut Heap, pq: &mut PageQueue, page: &mut Page, arg1: *mut u8, arg2: *mut u8) -> bool;

// 访问堆中的所有页面
unsafe fn mi_heap_visit_pages(
    heap: &mut Heap, 
    visitor: HeapPageVisitorFn,
    arg1: *mut u8,
    arg2: *mut u8
) -> bool {
    if heap.page_count == 0 {
        return true;
    }
    
    let mut count = 0;
    for i in 0..=MI_BIN_FULL {
        let pq = &mut heap.pages[i];
        let mut page = pq.first;
        
        while !page.is_null() {
            let next = (*page).next;
            count += 1;
            
            if !visitor(heap, pq, &mut *page, arg1, arg2) {
                return false;
            }
            
            page = next;
        }
    }
    
    true
}

enum MiCollect {
    Normal,
    Force,
    Abandon
}

unsafe fn mi_heap_page_collect(
    heap: &mut Heap,
    pq: &mut PageQueue,
    page: &mut Page, 
    arg_collect: *mut u8,
    _arg2: *mut u8
) -> bool {
    let collect = *(arg_collect as *const MiCollect);
    
    _mi_page_free_collect(page);
    if mi_page_all_free(page) {
        _mi_page_free(page, pq, collect != MiCollect::Normal);
    } else if collect == MiCollect::Abandon {
        // 仍有使用的块但线程已结束,放弃页面
        _mi_page_abandon(page, pq);
    }
    
    true
}

// 堆回收的扩展实现
unsafe fn mi_heap_collect_ex(heap: &mut Heap, collect: MiCollect) {
    _mi_deferred_free(heap, collect > MiCollect::Normal);
    
    if !mi_heap_is_initialized(heap) {
        return;
    }
    
    // 回收一些被放弃的页面
    if collect >= MiCollect::Normal && !heap.no_reclaim {
        if collect == MiCollect::Normal {
            // 可能释放一些段(也会获取被放弃的页面的所有权)
            _mi_segment_try_reclaim_abandoned(heap, false, &heap.tld.segments);
        }
    }
    
    // 如果正在放弃堆,标记所有完整页面不再添加到 delayed_free
    if collect == MiCollect::Abandon {
        let mut page = heap.pages[MI_BIN_FULL].first;
        while !page.is_null() {
            _mi_page_use_delayed_free(&mut *page, false);
            page = (*page).next;
        }
    }
    
    _mi_heap_delayed_free(heap);
    mi_heap_visit_pages(
        heap,
        mi_heap_page_collect,
        &collect as *const _ as *mut u8,
        null_mut()
    );
    
    // 收集段缓存
    if collect >= MiCollect::Force {
        _mi_segment_thread_collect(&heap.tld.segments);
    }
}

// 放弃堆收集
pub unsafe fn _mi_heap_collect_abandon(heap: &mut Heap) {
    mi_heap_collect_ex(heap, MiCollect::Abandon);
}

// 收集堆
pub unsafe fn mi_heap_collect(heap: &mut Heap, force: bool) {
    mi_heap_collect_ex(heap, if force { MiCollect::Force } else { MiCollect::Normal });
}

// 收集默认堆
pub unsafe fn mi_collect(force: bool) {
    mi_heap_collect(mi_get_default_heap(), force);
}

// 获取默认堆
pub unsafe fn mi_heap_get_default() -> &'static mut Heap {
    mi_thread_init();
    mi_get_default_heap()
}

pub unsafe fn mi_heap_get_backing() -> &'static mut Heap {
    let heap = mi_heap_get_default();
    let bheap = heap.tld.heap_backing;
    bheap
}

pub unsafe fn _mi_heap_random(heap: &mut Heap) -> usize {
    let r = heap.random;
    heap.random = _mi_random_shuffle(r);
    r
}

// 创建新堆
pub unsafe fn mi_heap_new() -> Option<NonNull<Heap>> {
    let bheap = mi_heap_get_backing();
    let heap = mi_heap_malloc_tp(bheap, Heap)?;
    
    // 初始化新堆
    std::ptr::copy_nonoverlapping(&_mi_heap_empty, heap, 1);
    (*heap).tld = bheap.tld;
    (*heap).thread_id = _mi_thread_id();
    (*heap).cookie = ((heap as usize) ^ _mi_heap_random(bheap)) | 1;
    (*heap).random = _mi_heap_random(bheap);
    (*heap).no_reclaim = true; // 不回收被放弃的页面,否则销毁不安全
    
    Some(NonNull::new_unchecked(heap))
}

// 重置堆页面
unsafe fn mi_heap_reset_pages(heap: &mut Heap) {
    // 重置直接空闲页面
    std::ptr::write_bytes(&mut heap.pages_free_direct, 0, 1);
    std::ptr::copy_nonoverlapping(&_mi_heap_empty.pages, &mut heap.pages, 1);
    heap.thread_delayed_free = null_mut();
    heap.page_count = 0;
}

// 释放堆内部资源
unsafe fn mi_heap_free(heap: &mut Heap) {
    if mi_heap_is_backing(heap) {
        return; 
    }
    
    if mi_heap_is_default(heap) {
        _mi_heap_default = heap.tld.heap_backing;
    }
    mi_free(heap as *mut _ as *mut u8);
}

// 销毁堆页面
unsafe fn _mi_heap_page_destroy(
    heap: &mut Heap,
    _pq: &mut PageQueue,
    page: &mut Page,
    _arg1: *mut u8, 
    _arg2: *mut u8
) -> bool {
    _mi_page_use_delayed_free(page, false);
    page.used = page.thread_freed as u16;
    _mi_segment_page_free(page, false, &heap.tld.segments);
    
    true
}

// 销毁所有页面
pub unsafe fn _mi_heap_destroy_pages(heap: &mut Heap) {
    mi_heap_visit_pages(heap, _mi_heap_page_destroy, null_mut(), null_mut());
    mi_heap_reset_pages(heap);
}

// 销毁堆
pub unsafe fn mi_heap_destroy(heap: &mut Heap) {
    if !mi_heap_is_initialized(heap) {
        return;
    }
    if !heap.no_reclaim {
        mi_heap_delete(heap);
    } else {
        _mi_heap_destroy_pages(heap);
        mi_heap_free(heap);
    }
}

// 吸收另一个堆的页面 
unsafe fn mi_heap_absorb(heap: &mut Heap, from: &mut Heap) {
    if from.page_count == 0 {
        return;
    }
    
    let mut page = heap.pages[MI_BIN_FULL].first;
    while !page.is_null() {
        let next = (*page).next;
        _mi_page_unfull(&mut *page);
        page = next;
    }
    _mi_heap_delayed_free(from);
    
    for i in 0..MI_BIN_FULL {
        let pq = &mut heap.pages[i];
        let append = &mut from.pages[i];
        _mi_page_queue_append(heap, pq, append);
    }

    mi_heap_reset_pages(from);
}

pub unsafe fn mi_heap_delete(heap: &mut Heap) {
    if !mi_heap_is_initialized(heap) {
        return;
    }
    
    if !mi_heap_is_backing(heap) {
        mi_heap_absorb(heap.tld.heap_backing, heap);
    } else {
        _mi_heap_collect_abandon(heap);
    }

    mi_heap_free(heap);
}

// 设置默认堆
pub unsafe fn mi_heap_set_default(heap: &mut Heap) -> *mut Heap {    
    if !mi_heap_is_initialized(heap) {
        return null_mut();
    }
    
    let old = _mi_heap_default;
    _mi_heap_default = heap;
    old
}

// 获取块所属的堆
unsafe fn mi_heap_of_block(p: *const u8) -> Option<NonNull<Heap>> {
    if p.is_null() {
        return None;
    }
    
    let segment = _mi_ptr_segment(p);
    let valid = _mi_ptr_cookie(segment) == (*segment).cookie;
    if !valid {
        return None; 
    }
    
    Some(NonNull::new_unchecked(
        _mi_segment_page_of(segment, p).heap
    ))
}

// 检查堆是否包含给定块
pub unsafe fn mi_heap_contains_block(heap: &mut Heap, p: *const u8) -> bool {
    if !mi_heap_is_initialized(heap) {
        return false;
    }
    
    match mi_heap_of_block(p) {
        Some(block_heap) => block_heap.as_ptr() == heap,
        None => false
    }
}

// 内部函数,用于检查页面是否包含指定指针
unsafe fn mi_heap_page_check_owned(
    _heap: &mut Heap,
    _pq: &mut PageQueue,
    page: &mut Page, 
    p: *mut u8,
    found: *mut bool
) -> bool {
    let segment = _mi_page_segment(page);
    let mut psize = 0;
    let start = _mi_page_start(segment, page, &mut psize);
    let end = start.add(page.capacity * page.block_size);
    
    *found = p >= start && p < end;
    !*found // 未找到继续搜索
}

// 检查堆是否拥有给定指针
pub unsafe fn mi_heap_check_owned(heap: &mut Heap, p: *const u8) -> bool {
    if !mi_heap_is_initialized(heap) {
        return false;
    }
    
    // 只检查对齐的指针
    if (p as usize) & (MI_INTPTR_SIZE - 1) != 0 {
        return false;  
    }
    
    let mut found = false;
    mi_heap_visit_pages(
        heap,
        mi_heap_page_check_owned,
        p as *mut u8,
        &mut found as *mut bool as *mut u8
    );
    found
}

// 检查默认堆是否拥有给定指针
pub unsafe fn mi_check_owned(p: *const u8) -> bool {
    mi_heap_check_owned(mi_get_default_heap(), p)
}

// 用于堆区域访问的扩展结构
struct MiHeapAreaEx {
    area: MiHeapArea,
    page: *mut Page
}

// 访问堆区域中的所有块
unsafe fn mi_heap_area_visit_blocks(
    xarea: &MiHeapAreaEx,
    visitor: MiBlockVisitFn,
    arg: *mut u8
) -> bool {
    if xarea.page.is_null() {
        return true;
    }
    
    let area = &xarea.area;
    let page = &mut *xarea.page;
    
    _mi_page_free_collect(page);
    if page.used == 0 {
        return true;
    }
    
    let mut psize = 0;
    let pstart = _mi_page_start(
        _mi_page_segment(page),
        page,
        &mut psize
    );

    if page.capacity == 1 {
        return visitor(page.heap, area, pstart, page.block_size, arg);
    }
    
    // 空闲块位图
    const MI_MAX_BLOCKS: usize = MI_SMALL_PAGE_SIZE / std::mem::size_of::<usize>();
    let mut free_map = [0usize; MI_MAX_BLOCKS / std::mem::size_of::<usize>()];
    
    // 标记空闲块
    let mut free_count = 0;
    let mut block = page.free;
    while !block.is_null() {
        free_count += 1;
        let offset = (block as *const u8).offset_from(pstart) as usize;
        let blockidx = offset / page.block_size;
        
        let bitidx = blockidx / std::mem::size_of::<usize>();
        let bit = blockidx - (bitidx * std::mem::size_of::<usize>());
        
        free_map[bitidx] |= 1 << bit;
        block = mi_block_next(page, block);
    }
    
    // 遍历所有块,跳过空闲块
    let mut used_count = 0;
    let mut i = 0;
    while i < page.capacity {
        let bitidx = i / std::mem::size_of::<usize>();
        let bit = i - (bitidx * std::mem::size_of::<usize>());
        let m = free_map[bitidx];
        
        if bit == 0 && m == usize::MAX {
            i += std::mem::size_of::<usize>() - 1;
        } else if (m & (1 << bit)) == 0 {
            used_count += 1;
            let block = pstart.add(i * page.block_size);
            if !visitor(page.heap, area, block, page.block_size, arg) {
                return false;
            }
        }
        i += 1;
    }

    true
}

type MiHeapAreaVisitFn = fn(heap: &mut Heap, area: &MiHeapAreaEx, arg: *mut u8) -> bool;

unsafe fn mi_heap_visit_areas_page(
    heap: &mut Heap,
    _pq: &mut PageQueue,
    page: &mut Page,
    vfun: *mut u8,
    arg: *mut u8
) -> bool {
    let visitor = std::mem::transmute::<*mut u8, MiHeapAreaVisitFn>(vfun);
    let mut xarea = MiHeapAreaEx {
        area: MiHeapArea {
            blocks: _mi_page_start(_mi_page_segment(page), page, null_mut()),
            reserved: page.reserved * page.block_size,
            committed: page.capacity * page.block_size,
            used: page.used - page.thread_freed, // 竞态是可以的
            block_size: page.block_size
        },
        page: page
    };
    visitor(heap, &xarea, arg)
}

// 访问所有堆区域
unsafe fn mi_heap_visit_areas(
    heap: &mut Heap,
    visitor: MiHeapAreaVisitFn,
    arg: *mut u8
) -> bool {
    mi_heap_visit_pages(
        heap,
        mi_heap_visit_areas_page,
        visitor as *mut u8,
        arg
    )
}

// 块访问参数结构
struct MiVisitBlocksArgs {
    visit_blocks: bool,
    visitor: MiBlockVisitFn,
    arg: *mut u8
}

// 区域访问器
unsafe fn mi_heap_area_visitor(
    heap: &mut Heap,
    xarea: &MiHeapAreaEx,
    arg: *mut u8
) -> bool {
    let args = &mut *(arg as *mut MiVisitBlocksArgs);
    
    if !args.visitor(heap, &xarea.area, null_mut(), xarea.area.block_size, args.arg) {
        return false;
    }
    
    if args.visit_blocks {
        mi_heap_area_visit_blocks(xarea, args.visitor, args.arg)
    } else {  
        true
    }
}

// 访问堆中的所有块
pub unsafe fn mi_heap_visit_blocks(
    heap: &mut Heap,
    visit_blocks: bool,
    visitor: MiBlockVisitFn,
    arg: *mut u8
) -> bool {
    let mut args = MiVisitBlocksArgs {
        visit_blocks,
        visitor,
        arg
    };
    mi_heap_visit_areas(
        heap,
        mi_heap_area_visitor,
        &mut args as *mut _ as *mut u8
    )
}