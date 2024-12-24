use std::{ptr::NonNull, sync::atomic::{AtomicPtr, AtomicUsize, Ordering}};

use crate::*;

const MI_MAX_ALIGN_SIZE: usize = 16;
const MI_INTPTR_SIZE: usize = std::mem::size_of::<usize>();

#[cfg(target_pointer_width = "64")]
const MI_LARGE_SIZE_MAX: usize = 16777215; // 最大的大块大小

// 页队列结构体
pub struct PageQueue {
    first: Option<NonNull<Page>>,
    last: Option<NonNull<Page>>,
    block_size: usize,
}

impl PageQueue {
    // 判断是否为大页队列
    pub fn is_huge(&self) -> bool {
        self.block_size == MI_LARGE_SIZE_MAX + std::mem::size_of::<usize>() 
    }

    // 判断是否为已满页队列
    pub fn is_full(&self) -> bool {
        self.block_size == MI_LARGE_SIZE_MAX + (2 * std::mem::size_of::<usize>())
    }

    // 判断是否为特殊页队列(大或已满)
    pub fn is_special(&self) -> bool {
        self.block_size > MI_LARGE_SIZE_MAX
    }
}

// 返回最高位的索引
#[inline]
fn mi_bsr32(mut x: u32) -> u8 {
    (31 - x.leading_zeros()) as u8    
}

// 返回最高位索引
fn mi_bsr(x: usize) -> u8 {
    if x == 0 {
        return 0;
    }
    #[cfg(target_pointer_width = "64")] {
        let hi = (x >> 32) as u32;
        if hi == 0 {
            mi_bsr32(x as u32)
        } else {
            32 + mi_bsr32(hi)
        }
    }
    #[cfg(target_pointer_width = "32")] {
        mi_bsr32(x as u32)
    }
}

// 为给定大小返回分桶索引
// 如果尺寸太大则返回MI_BIN_HUGE
pub fn mi_bin(size: usize) -> u8 {
    let wsize = size / std::mem::size_of::<usize>();
    
    if wsize <= 1 {
        return 1;
    }
    
    #[cfg(any(MI_ALIGN4W))]
    if wsize <= 4 {
        return ((wsize + 1) & !1) as u8; // 向双字对齐取整
    }
    
    #[cfg(any(MI_ALIGN2W))]
    if wsize <= 8 {
        return ((wsize + 1) & !1) as u8; // 向双字对齐取整
    }

    if wsize <= 8 {
        return wsize as u8;
    }

    if wsize > MI_LARGE_WSIZE_MAX {
        return MI_BIN_HUGE.try_into().unwrap();
    }
    
    #[cfg(MI_ALIGN4W)]
    let wsize = if wsize <= 16 { (wsize + 3) & !3 } else { wsize }; // 4字对齐
    
    let wsize = wsize - 1;
    let b = mi_bsr32(wsize as u32);
    ((b << 2) + ((wsize >> (b - 2)) & 0x03) as u8) - 3
}

// 返回分桶对应的块大小
pub fn mi_bin_size(bin: u8) -> usize {
    EMPTY_HEAP.pages[bin as usize].block_size
}

// 获取合适的分配大小
pub fn mi_good_size(size: usize) -> usize {
    if size <= MI_LARGE_SIZE_MAX {
        mi_bin_size(mi_bin(size))
    } else {
        align_up(size, MI_PAGE_SIZE)
    }
}

// 获取包含指定页面的队列
fn mi_page_queue_of(page: &Page) -> &PageQueue {
    let bin = if page.is_in_full() {
        MI_BIN_FULL 
    } else {
        mi_bin(page.block_size)
    };
    &page.heap.pages[bin as usize]
}

// 从队列中移除页面
pub fn mi_page_queue_remove(queue: &mut PageQueue, page: &mut Page) {        
    // 处理前后链接
    if let Some(prev) = page.prev {
        unsafe { (*prev.as_ptr()).next = page.next; }
    }
    if let Some(next) = page.next {
        unsafe { (*next.as_ptr()).prev = page.prev; }
    }

    // 处理队列首尾
    if page.prev.is_none() {
        queue.first = page.next;
        // 更新首个页面
        mi_heap_queue_first_update(page.heap, queue);
    }
    if page.next.is_none() {
        queue.last = page.prev;
    }

    // 清理页面状态
    page.heap.page_count -= 1;
    page.next = None;
    page.prev = None;
    page.heap = None;
    page.flags.set_in_full(false);
}

// 将页面添加到队列头部
pub fn mi_page_queue_push(heap: &mut Heap, queue: &mut PageQueue, page: &mut Page) {
    page.flags.set_in_full(queue.is_full());
    page.heap = Some(heap);
    
    // 链接到队列头部
    page.next = queue.first;
    page.prev = None;
    
    if let Some(first) = queue.first {
        unsafe { 
            (*first.as_ptr()).prev = Some(page.into());
        }
        queue.first = Some(page.into());
    } else {
        queue.first = Some(page.into());
        queue.last = Some(page.into());
    }

    mi_heap_queue_first_update(heap, queue);
    heap.page_count += 1;
}

// 将页面从源队列转移到目标队列
pub fn mi_page_queue_enqueue_from(to: &mut PageQueue, from: &mut PageQueue, page: &mut Page) {
    if let Some(prev) = page.prev {
        unsafe { (*prev.as_ptr()).next = page.next; }
    }
    if let Some(next) = page.next {
        unsafe { (*next.as_ptr()).prev = page.prev; }
    }
    if page.prev.is_none() {
        from.first = page.next;
        mi_heap_queue_first_update(page.heap.unwrap(), from);
    }
    if page.next.is_none() {
        from.last = page.prev;
    }

    // 添加到目标队列尾部
    page.prev = to.last;
    page.next = None;
    if let Some(last) = to.last {
        unsafe {
            (*last.as_ptr()).next = Some(page.into());
        }
        to.last = Some(page.into());
    } else {
        to.first = Some(page.into());
        to.last = Some(page.into());
        mi_heap_queue_first_update(page.heap.unwrap(), to);
    }

    page.flags.set_in_full(to.is_full());
}

// 将一个队列附加到另一个队列末尾
pub fn mi_page_queue_append(heap: &mut Heap, pq: &mut PageQueue, append: &mut PageQueue) {
    if append.first.is_none() {
        return;
    }

    // 设置附加页面的堆指针
    let mut current = append.first;
    while let Some(page) = current {
        unsafe {
            (*page.as_ptr()).heap = Some(heap);
            current = (*page.as_ptr()).next;
        }
    }

    if pq.last.is_none() {
        pq.first = append.first;
        pq.last = append.last;
        mi_heap_queue_first_update(heap, pq);
    } else {
        unsafe {
            let last = pq.last.unwrap();
            let first = append.first.unwrap();
            (*last.as_ptr()).next = Some(first);
            (*first.as_ptr()).prev = Some(last);
            pq.last = append.last;
        }
    }
}