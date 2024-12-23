use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering}; 
use std::ptr::{null, null_mut, NonNull};

use crate::*;

// 段队列操作函数
impl SegmentQueue {
    fn is_empty(&self) -> bool {
        self.first.is_null()
    }
    
    fn mi_segment_is_in_free_queue(segment: *mut Segment, tld: &SegmentsTld) -> bool {
       let in_queue = unsafe {
           (*segment).next.is_null() 
               || (*segment).prev.is_null() 
               || tld.small_free.first == segment
       };
       in_queue
   }
   
    fn remove(&mut self, segment: *mut Segment) {
        unsafe {

            if !(*segment).prev.is_null() {
                (*(*segment).prev).next = (*segment).next;
            }
            if !(*segment).next.is_null() {
                (*(*segment).next).prev = (*segment).prev; 
            }

            if segment == self.first {
                self.first = (*segment).next;
            }
            if segment == self.last {
                self.last = (*segment).prev;
            }

            (*segment).next = std::ptr::null_mut();
            (*segment).prev = std::ptr::null_mut();
        }
    }

    fn enqueue(&mut self, segment: *mut Segment) {
        unsafe {
            (*segment).next = null_mut();
            (*segment).prev = self.last;
            
            if !self.last.is_null() {
                (*self.last).next = segment;
                self.last = segment;
            } else {
                self.last = segment;
                self.first = segment;
            }
        }
    }
}

// 获取页面开始的地址
pub unsafe fn mi_segment_page_start(
    segment: *const Segment, 
    page: *const Page,
    page_size: &mut usize
) -> *mut u8 {
    let page = &*page;
    let segment = &*segment;
    
    // 计算页面大小
    let mut psize = if segment.page_kind == PageKind::Huge {
        segment.segment_size
    } else {
        1 << segment.page_shift  
    };

    // 计算页面起始地址
    let mut p = (segment as *const _ as *mut u8)
        .add(page.segment_idx * psize);
().into()
    // 第一个页面需要跳过段信息大小
    if page.segment_idx == 0 {
        p = p.add(segment.segment_info_size);
        psize -= segment.segment_info_size;
    }

    // 根据安全选项调整大小
    let secure = mi_option_get(mi_option_secure);
    if secure > 1 || (secure == 1 && page.segment_idx == segment.capacity - 1) {
        psize -= mi_os_page_size();
    }

    if !page_size.is_null() {
        *page_size = psize; 
    }

    p
}

// 计算段的大小
fn mi_segment_size(
    capacity: usize, 
    required: usize,
    pre_size: &mut usize,
    info_size: &mut usize
) -> usize {
    // 计算最小所需大小
    let minsize = std::mem::size_of::<Segment>() 
        + (capacity - 1) * std::mem::size_of::<Page>() 
        + 16;
    
    let mut guardsize = 0;
    let mut isize = 0;

    if !mi_option_is_enabled(mi_option_secure) {
        // 普通模式下无保护页
        isize = align_up(minsize, std::cmp::max(16, MI_MAX_ALIGN_SIZE));
    } else {
        // 安全模式下添加保护页
        let page_size = mi_os_page_size();
        isize = align_up(minsize, page_size);
        guardsize = page_size;
        required = align_up(required, page_size);
    }

    *info_size = isize;
    *pre_size = isize + guardsize;

    if required == 0 {
        MI_SEGMENT_SIZE
    } else {
        align_up(required + isize + 2 * guardsize, MI_PAGE_HUGE_ALIGN)
    }
}

// 段缓存大小跟踪
fn mi_segments_track_size(segment_size: isize, tld: &mut SegmentsTld) {
    if segment_size >= 0 {
        mi_stat_increase(&mut tld.stats.segments, 1);
    } else {
        mi_stat_decrease(&mut tld.stats.segments, 1); 
    }

    tld.current_size += segment_size;
    if tld.current_size > tld.peak_size {
        tld.peak_size = tld.current_size;
    }
}

fn mi_segment_os_free(
    segment: *mut Segment, 
    segment_size: usize,
    tld: &mut SegmentsTld
) {
    mi_segments_track_size(-(segment_size as isize), tld);
    mi_os_free(segment as *mut _, segment_size, &mut tld.stats);
}

// 查找缓存的段
unsafe fn mi_segment_cache_find(
    tld: &mut SegmentsTld,
    required: usize,
    reverse: bool
) -> *mut Segment {
    let mut segment = if reverse {
        tld.cache.last 
    } else {
        tld.cache.first
    };

    while !segment.is_null() {
        let seg = &*segment;
        if seg.segment_size >= required {
            tld.cache_count -= 1;
            tld.cache_size -= seg.segment_size;
            tld.cache.remove(segment);
            if required == 0 || seg.segment_size == required {
                return segment;
            }

            if required != MI_SEGMENT_SIZE 
                && seg.segment_size - (seg.segment_size/4) <= required {
                return segment;    
            }
            if mi_option_is_enabled(mi_option_secure) {
                mi_os_unprotect(segment as *mut _, seg.segment_size); 
            }

            if mi_os_shrink(segment as *mut _, seg.segment_size, required) {
                tld.current_size -= seg.segment_size;
                tld.current_size += required;
                (*segment).segment_size = required;
                return segment;
            } else {
                mi_segment_os_free(segment, seg.segment_size, tld);
                return std::ptr::null_mut();
            }
        }
        
        segment = if reverse {
            (*segment).prev
        } else {
            (*segment).next
        };
    }

    std::ptr::null_mut()
}

// 缓存淘汰
unsafe fn mi_segment_cache_evict(tld: &mut SegmentsTld) -> *mut Segment {   
    mi_segment_cache_find(tld, 0, true)
}

// 判断缓存是否已满
unsafe fn mi_segment_cache_full(tld: &mut SegmentsTld) -> bool {
    if tld.cache_count < MI_SEGMENT_CACHE_MAX && 
       tld.cache_size * MI_SEGMENT_CACHE_FRACTION < tld.peak_size {
        return false;
    }

    while tld.cache_size * MI_SEGMENT_CACHE_FRACTION >= tld.peak_size + 1 {
        let segment = mi_segment_cache_evict(tld);
        if !segment.is_null() {
            mi_segment_os_free(segment, (*segment).segment_size, tld);
        }
    }

    true
}

// 将段插入缓存
unsafe fn mi_segment_cache_insert(
    segment: *mut Segment, 
    tld: &mut SegmentsTld
) -> bool {
    if mi_segment_cache_full(tld) {
        return false;
    }

    if mi_option_is_enabled(mi_option_cache_reset) && 
       !mi_option_is_enabled(mi_option_page_reset) {
        mi_os_reset(
            (segment as *mut u8).add((*segment).segment_info_size),
            (*segment).segment_size - (*segment).segment_info_size
        );
    }

    let mut seg = tld.cache.first;
    while !seg.is_null() && (*seg).segment_size < (*segment).segment_size {
        seg = (*seg).next;
    }

    tld.cache.insert_before(seg, segment);
    tld.cache_count += 1;
    tld.cache_size += (*segment).segment_size;

    true
}

// 线程结束时回收缓存的段
pub unsafe fn mi_segment_thread_collect(tld: &mut SegmentsTld) {
    while let Some(segment) = mi_segment_cache_find(tld, 0, false) {
        mi_segment_os_free(segment, MI_SEGMENT_SIZE, tld);
    }
}

// 分配新段
unsafe fn mi_segment_alloc(
    required: usize,
    page_kind: PageKind,
    page_shift: usize, 
    tld: &mut SegmentsTld,
    os_tld: &mut OsTld
) -> *mut Segment {
    let capacity = if page_kind == PageKind::Huge {
        1
    } else {
        let page_size = 1 << page_shift;
        MI_SEGMENT_SIZE / page_size
    };

    let mut info_size = 0;
    let mut pre_size = 0;
    let segment_size = mi_segment_size(
        capacity, 
        required,
        &mut pre_size,
        &mut info_size
    );

    let mut segment = mi_segment_cache_find(tld, segment_size, false);
    if !segment.is_null() {
        if mi_option_is_enabled(mi_option_secure) && 
           ((*segment).page_kind != page_kind || 
            (*segment).segment_size != segment_size) {
            mi_os_unprotect(segment as *mut _, (*segment).segment_size);
        }
    } else {
        segment = mi_os_alloc_aligned(
            segment_size,
            MI_SEGMENT_SIZE,
            os_tld
        ) as *mut Segment;

        if segment.is_null() {
            return std::ptr::null_mut();
        }
        mi_segments_track_size(segment_size as isize, tld);
    }

    std::ptr::write_bytes(segment as *mut u8, 0, info_size);

    if mi_option_is_enabled(mi_option_secure) {        
        mi_os_protect(
            (segment as *mut u8).add(info_size),
            pre_size - info_size
        );

        let os_page_size = mi_os_page_size();
        if mi_option_get(mi_option_secure) <= 1 {
            mi_os_protect(
                (segment as *mut u8).add(segment_size - os_page_size),
                os_page_size
            );
        } else {
            let page_size = if page_kind == PageKind::Huge {
                segment_size
            } else {
                1 << page_shift
            };
            for i in 0..capacity {
                mi_os_protect(
                    (segment as *mut u8).add((i + 1) * page_size - os_page_size),
                    os_page_size
                );
            }
        }
    }

    (*segment).page_kind = page_kind;
    (*segment).capacity = capacity;
    (*segment).page_shift = page_shift;
    (*segment).segment_size = segment_size;
    (*segment).segment_info_size = pre_size;
    (*segment).thread_id = mi_thread_id();
    (*segment).cookie = mi_ptr_cookie(segment as *const _);

    // 初始化页索引
    for i in 0..capacity {
        (*segment).pages[i].segment_idx = i;
    }

    segment
}

// 释放一个段
unsafe fn mi_segment_free(
    segment: *mut Segment,
    force: bool,
    tld: &mut SegmentsTld 
) {    
    // 如果段在空闲队列中则移除
    if mi_segment_is_in_free_queue(segment, tld) {
        if (*segment).page_kind != PageKind::Small {
            eprintln!(
                "mimalloc: expecting small segment: {:?}, {:?}, {:?}, {:?}",
                (*segment).page_kind,
                (*segment).prev,
                (*segment).next,
                tld.small_free.first
            );
        } else {
            tld.small_free.remove(segment);
        }
    }

    // 更新统计信息
    mi_stat_decrease(&mut tld.stats.committed, (*segment).segment_info_size);
    (*segment).thread_id = 0;

    // 更新重置内存统计
    for i in 0..(*segment).capacity {
        let page = &mut (*segment).pages[i];
        if page.is_reset {
            page.is_reset = false;
            mi_stat_decrease(&mut tld.stats.reset, mi_page_size(page));
        }
    }

    // 尝试将段放入缓存或直接释放
    if !force && mi_segment_cache_insert(segment, tld) {
        // 已放入缓存
    } else {
        // 返回给操作系统
        mi_segment_os_free(segment, (*segment).segment_size, tld);
    }
}

// 查找段中的空闲页
unsafe fn mi_segment_find_free(segment: *mut Segment) -> *mut Page {
    for i in 0..(*segment).capacity {
        let page = &mut (*segment).pages[i];
        if !page.segment_in_use {
            return page;
        }
    }
}

// 检查段是否有空闲页
unsafe fn mi_segment_has_free(segment: *const Segment) -> bool {
    (*segment).used < (*segment).capacity
}

// 废弃一个段
static mut ABANDONED: *mut Segment = std::ptr::null_mut();
static mut ABANDONED_COUNT: AtomicUsize = AtomicUsize::new(0);

unsafe fn mi_segment_abandon(segment: *mut Segment, tld: &mut SegmentsTld) {
    if mi_segment_is_in_free_queue(segment, tld) {
        mi_segment_queue_remove(&mut tld.small_free, segment);
    }

    (*segment).thread_id = 0;
    loop {
        (*segment).abandoned_next = ABANDONED;
        let success = AtomicPtr::new(ABANDONED).compare_exchange(
            (*segment).abandoned_next,
            segment,
            Ordering::SeqCst,
            Ordering::SeqCst
        ).is_ok();
        
        if success {
            break;
        }
    }
    
    ABANDONED_COUNT.fetch_add(1, Ordering::SeqCst);
    mi_stat_increase(&mut tld.stats.segments_abandoned, 1);
}

// 废弃一个页面
pub unsafe fn mi_segment_page_abandon(page: *mut Page, tld: &mut SegmentsTld) {
    let segment = _mi_page_segment(page);
    (*segment).abandoned += 1;
    mi_stat_increase(&mut tld.stats.pages_abandoned, 1);
    
    if (*segment).used == (*segment).abandoned {
        mi_segment_abandon(segment, tld);
    }
}

// 尝试回收废弃的段
pub unsafe fn mi_segment_try_reclaim_abandoned(
    heap: *mut Heap,
    try_all: bool,
    tld: &mut SegmentsTld
) -> bool {
    let mut reclaimed = 0;
    let atmost = if try_all {
        ABANDONED_COUNT.load(Ordering::SeqCst) + 16
    } else {
        std::cmp::max(ABANDONED_COUNT.load(Ordering::SeqCst) / 8, 8)
    };

    // 循环处理废弃的段
    while atmost > reclaimed {
        let mut segment;
        loop {
            segment = ABANDONED;
            if segment.is_null() {
                break;
            }
            let success = AtomicPtr::new(ABANDONED).compare_exchange(
                segment,
                (*segment).abandoned_next,
                Ordering::SeqCst,
                Ordering::SeqCst
            ).is_ok();
            if success {
                break;
            }
        }
        if segment.is_null() {
            break;
        }

        ABANDONED_COUNT.fetch_sub(1, Ordering::SeqCst);
        (*segment).thread_id = mi_thread_id();
        (*segment).abandoned_next = std::ptr::null_mut();
        mi_segments_track_size((*segment).segment_size as isize, tld);
        mi_stat_decrease(&mut tld.stats.segments_abandoned, 1);

        if (*segment).page_kind == PageKind::Small && mi_segment_has_free(segment) {
            tld.small_free.enqueue(segment);
        }

        for i in 0..(*segment).capacity {
            let page = &mut (*segment).pages[i];
            if page.segment_in_use {
                (*segment).abandoned -= 1;
                mi_stat_decrease(&mut tld.stats.pages_abandoned, 1);
                
                if mi_page_all_free(page) {
                    mi_segment_page_clear(segment, page, &mut tld.stats);
                } else {
                    mi_page_reclaim(heap, page);
                }
            }
        }
        if (*segment).used == 0 {
            mi_segment_free(segment, false, tld);
        } else {
            reclaimed += 1;
        }
    }

    reclaimed > 0
}

// 在段内分配小页面
unsafe fn mi_segment_small_page_alloc_in(
    segment: *mut Segment,
    tld: &mut SegmentsTld
) -> *mut Page {
    let page = mi_segment_find_free(segment);
    (*page).segment_in_use = true;
    
    // 更新段的使用计数
    (*segment).used += 1;
    if (*segment).used == (*segment).capacity {
        tld.small_free.remove(segment);
    }

    page
}

// 分配小页面
unsafe fn mi_segment_small_page_alloc(
    tld: &mut SegmentsTld,
    os_tld: &mut OsTld
) -> *mut Page {
    if tld.small_free.is_empty() {
        let segment = mi_segment_alloc(
            0,
            PageKind::Small,
            MI_SMALL_PAGE_SHIFT,
            tld,
            os_tld
        );
        if segment.is_null() {
            return std::ptr::null_mut();
        }
        tld.small_free.enqueue(segment);
    }
    mi_segment_small_page_alloc_in(tld.small_free.first, tld)
}

// 分配大页面
unsafe fn mi_segment_large_page_alloc(
    tld: &mut SegmentsTld,
    os_tld: &mut OsTld
) -> *mut Page {
    let segment = mi_segment_alloc(
        0,
        PageKind::Large,
        MI_LARGE_PAGE_SHIFT,
        tld,
        os_tld
    );
    
    if segment.is_null() {
        return std::ptr::null_mut();
    }

    (*segment).used = 1;
    let page = &mut (*segment).pages[0];
    page.segment_in_use = true;
    
    page
}

// 分配超大页面
unsafe fn mi_segment_huge_page_alloc(
    size: usize,
    tld: &mut SegmentsTld,  
    os_tld: &mut OsTld
) -> *mut Page {
    let segment = mi_segment_alloc(
        size,
        PageKind::Huge,
        MI_SEGMENT_SHIFT,
        tld,
        os_tld
    );
    if segment.is_null() {
        return std::ptr::null_mut();
    }
    (*segment).used = 1;
    let page = &mut (*segment).pages[0];
    page.segment_in_use = true;
    
    page
}

// 分配页面
pub unsafe fn mi_segment_page_alloc(
    block_size: usize,
    tld: &mut SegmentsTld,
    os_tld: &mut OsTld
) -> *mut Page {
    let page = if block_size < MI_SMALL_PAGE_SIZE / 8 {
        mi_segment_small_page_alloc(tld, os_tld)
    } else if block_size < (MI_LARGE_SIZE_MAX - std::mem::size_of::<Segment>()) {
        mi_segment_large_page_alloc(tld, os_tld)
    } else {
        mi_segment_huge_page_alloc(block_size, tld, os_tld)
    };

    page
}

// 清理页面
unsafe fn mi_segment_page_clear(
    segment: *mut Segment,
    page: *mut Page,
    stats: &mut Stats
) {
    let inuse = (*page).capacity * (*page).block_size;
    mi_stat_decrease(&mut stats.committed, inuse);
    mi_stat_decrease(&mut stats.pages, 1);

    if !(*page).is_reset && mi_option_is_enabled(mi_option_page_reset) {
        let mut psize = 0;
        let start = mi_segment_page_start(segment, page, &mut psize);

        mi_stat_increase(&mut stats.reset, psize);
        (*page).is_reset = true;

        if inuse > 0 {
            mi_os_reset(start, inuse);
        }
    }

    let idx = (*page).segment_idx;
    let is_reset = (*page).is_reset;
    std::ptr::write_bytes(page as *mut u8, 0, std::mem::size_of::<Page>());
    (*page).segment_idx = idx;
    (*page).segment_in_use = false;
    (*page).is_reset = is_reset;
    
    (*segment).used -= 1;
}

// 释放页面
pub unsafe fn mi_segment_page_free(
    page: *mut Page,
    force: bool,
    tld: &mut SegmentsTld
) {
    let segment = _mi_page_segment(page);

    // 标记为空闲
    mi_segment_page_clear(segment, page, &mut tld.stats);
    if (*segment).used == 0 {
        mi_segment_free(segment, force, tld);
    } else if (*segment).used == (*segment).abandoned {
        mi_segment_abandon(segment, tld);
    } else if (*segment).used + 1 == (*segment).capacity {
        tld.small_free.enqueue(segment);
    }
}