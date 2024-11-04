# Week6-7-8

### 1.完成的工作：

添加了几个字段。

### 2.遇到的问题：

全是问题。

1.去锁之后需要选取新的数据结构类型。但是目前没有什么（自带）类型可以满足当前需求。

### 3.一些记录：

根据前人的最终展示ppt：

![image-20241031224246084](C:\Users\stone\AppData\Roaming\Typora\typora-user-images\image-20241031224246084.png)

但目前的实现还是使用了Mutex互斥锁来实现线程安全。

![image-20241031223523159](C:\Users\stone\AppData\Roaming\Typora\typora-user-images\image-20241031223523159.png)

目前所有的线程安全都依靠Mutex互斥锁来实现。我们的目标就是，去掉Mutex。

```
内存维护单元分为：堆（Heap）、段（Segment）、页（Page）、块（Block）
Block：内存块,只记录next指针，最小8字节
Page：内存页，承载若干Block，每个Page上的Block大小都是相同的.可能的Block大小取最高位的3位二进制，这一点与TLSF有一定类似
Page的大小分为3种：small（64KB），Medium（4MB），Huge（仅有1个Block，由其大小决定）
Page需记录：块大小、free_list等信息
```

按照多线程实现，我们需要实现Page里面添加各种字段。

```
Heap：一个线程的mimalloc控制模块
核心为page_heap，为根据块大小维护的若干个Page的链表
额外维护：全满的page、全空的page（尚未指定block大小）等
对于超过4MB的统一用Huge链表管理
Segment：4MB对齐，负责承载Page
每个Segment中的Page大小是相同的（除了第一个Page被元数据挤掉一部分空间）
Huge页所在的Segment仅含一个页
集中管理所有Page的元数据，与相应的数据段分离
需要记录：段大小、页大小、页的元数据等信息
在单线程模式下，Heap仅有一个，藏在第一个Segment的开头
```

同上，添加字段。

![image-20241102091240307](C:\Users\stone\AppData\Roaming\Typora\typora-user-images\image-20241102091240307.png)

这是去掉锁，使用RefCell之后的结果，可以发现确实没有办法继续运行了。

```c
/// Free previously allocated memory.
/// The pointer `p` must have been allocated before (or be \a NULL).
/// @param p  pointer to free, or \a NULL.
void  mi_free(void* p);

/// Allocate \a size bytes.
/// @param size  number of bytes to allocate.
/// @returns pointer to the allocated memory or \a NULL if out of memory.
/// Returns a unique pointer if called with \a size 0.
void* mi_malloc(size_t size);
```

原版mi_malloc和mi_free实现的接口。

```c
/// Initialize mimalloc on a thread.
/// Should not be used as on most systems (pthreads, windows) this is done
/// automatically.
void mi_thread_init(void);

/// Uninitialize mimalloc on a thread.
/// Should not be used as on most systems (pthreads, windows) this is done
/// automatically. Ensures that any memory that is not freed yet (but will
/// be freed by other threads in the future) is properly handled.
void mi_thread_done(void);

/// Print out heap statistics for this thread.
/// @param out An output function or \a NULL for the default.
/// @param arg Optional argument passed to \a out (if not \a NULL)
///
/// Most detailed when using a debug build.
void mi_thread_stats_print_out(mi_output_fun* out, void* arg);
```

有关多线程实现的一些接口，需要注意这些接口在一些系统下并不需要手动调用。

```c
/// Type of first-class heaps.
/// A heap can only be used for allocation in
/// the thread that created this heap! Any allocated
/// blocks can be freed or reallocated by any other thread though.
struct mi_heap_s;

/// Type of first-class heaps.
/// A heap can only be used for (re)allocation in
/// the thread that created this heap! Any allocated
/// blocks can be freed by any other thread though.
typedef struct mi_heap_s mi_heap_t;
```

堆块结构体声明。注意堆块只有当前线程申请的可以使用，但是分配的堆块都可以被其他线程释放。

```c
struct mi_heap_s {
  mi_tld_t*             tld;
  _Atomic(mi_block_t*)  thread_delayed_free;
  mi_threadid_t         thread_id;                           // thread this heap belongs too
  mi_arena_id_t         arena_id;                            // arena id if the heap belongs to a specific arena (or 0)
  mi_random_ctx_t       random;                              // random number context used for secure allocation
  size_t                page_count;                          // total number of pages in the `pages` queues.
  size_t                page_retired_min;                    // smallest retired index (retired pages are fully free, but still in the page queues)
  size_t                page_retired_max;                    // largest retired index into the `pages` array.
  mi_heap_t*            next;                                // list of heaps per thread
  bool                  no_reclaim;                          // `true` if this heap should not reclaim abandoned pages
  uint8_t               tag;                                 // custom tag, can be used for separating heaps based on the object types
  mi_page_t*            pages_free_direct[MI_PAGES_DIRECT];  // optimize: array where every entry points a page with possibly free blocks in the corresponding queue for that size.
  mi_page_queue_t       pages[MI_BIN_FULL + 1];              // queue of pages for each size class (or "bin")
};
```

堆块具体结构的定义，其中堆块含有多个页，这符合先前实现里的定义。

```c
// thread id's
typedef size_t     mi_threadid_t;

// free lists contain blocks
typedef struct mi_block_s {
  mi_encoded_t next;
} mi_block_t;
```

block的定义，这里跟原代码一致，无需再修改。

```c
typedef struct mi_page_s {
  // "owned" by the segment
  uint8_t               segment_idx;       // index in the segment `pages` array, `page == &segment->pages[page->segment_idx]`
  uint8_t               segment_in_use:1;  // `true` if the segment allocated this page
  uint8_t               is_committed:1;    // `true` if the page virtual memory is committed
  uint8_t               is_zero_init:1;    // `true` if the page was initially zero initialized
  uint8_t               is_huge:1;         // `true` if the page is in a huge segment

  // layout like this to optimize access in `mi_malloc` and `mi_free`
  uint16_t              capacity;          // number of blocks committed, must be the first field, see `segment.c:page_clear`
  uint16_t              reserved;          // number of blocks reserved in memory
  mi_page_flags_t       flags;             // `in_full` and `has_aligned` flags (8 bits)
  uint8_t               free_is_zero:1;    // `true` if the blocks in the free list are zero initialized
  uint8_t               retire_expire:7;   // expiration count for retired blocks

  mi_block_t*           free;              // list of available free blocks (`malloc` allocates from this list)
  mi_block_t*           local_free;        // list of deferred free blocks by this thread (migrates to `free`)
  uint16_t              used;              // number of blocks in use (including blocks in `thread_free`)
  uint8_t               block_size_shift;  // if not zero, then `(1 << block_size_shift) == block_size` (only used for fast path in `free.c:_mi_page_ptr_unalign`)
  uint8_t               heap_tag;          // tag of the owning heap, used to separate heaps by object type
                                           // padding
  size_t                block_size;        // size available in each block (always `>0`)
  uint8_t*              page_start;        // start of the page area containing the blocks

  #if (MI_ENCODE_FREELIST || MI_PADDING)
  uintptr_t             keys[2];           // two random keys to encode the free lists (see `_mi_block_next`) or padding canary
  #endif

  _Atomic(mi_thread_free_t) xthread_free;  // list of deferred free blocks freed by other threads
  _Atomic(uintptr_t)        xheap;

  struct mi_page_s*     next;              // next page owned by the heap with the same `block_size`
  struct mi_page_s*     prev;              // previous page owned by the heap with the same `block_size`

  #if MI_INTPTR_SIZE==4                    // pad to 12 words on 32-bit
  void* padding[1];
  #endif
} mi_page_t;
```

mi_page的定义，可以参考数据结构来修改，增加多线程字段。

```c
typedef struct mi_segment_s {
  // constant fields
  mi_memid_t           memid;            // memory id to track provenance
  bool                 allow_decommit;
  bool                 allow_purge;
  size_t               segment_size;     // for huge pages this may be different from `MI_SEGMENT_SIZE`
  mi_subproc_t*        subproc;          // segment belongs to sub process

  // segment fields
  struct mi_segment_s* next;             // must be the first (non-constant) segment field  -- see `segment.c:segment_init`
  struct mi_segment_s* prev;
  bool                 was_reclaimed;    // true if it was reclaimed (used to limit on-free reclamation)

  size_t               abandoned;        // abandoned pages (i.e. the original owning thread stopped) (`abandoned <= used`)
  size_t               abandoned_visits; // count how often this segment is visited for reclaiming (to force reclaim if it is too long)

  size_t               used;             // count of pages in use (`used <= capacity`)
  size_t               capacity;         // count of available pages (`#free + used`)
  size_t               segment_info_size;// space we are using from the first page for segment meta-data and possible guard pages.
  uintptr_t            cookie;           // verify addresses in secure mode: `_mi_ptr_cookie(segment) == segment->cookie`

  struct mi_segment_s* abandoned_os_next; // only used for abandoned segments outside arena's, and only if `mi_option_visit_abandoned` is enabled
  struct mi_segment_s* abandoned_os_prev;

  // layout like this to optimize access in `mi_free`
  _Atomic(mi_threadid_t) thread_id;      // unique id of the thread owning this segment
  size_t               page_shift;       // `1 << page_shift` == the page sizes == `page->block_size * page->reserved` (unless the first page, then `-segment_info_size`).
  mi_page_kind_t       page_kind;        // kind of pages: small, medium, large, or huge
  mi_page_t            pages[1];         // up to `MI_SMALL_PAGES_PER_SEGMENT` pages
} mi_segment_t;
```

mi_segment的定义，同上。

![image-20241102213008965](C:\Users\stone\AppData\Roaming\Typora\typora-user-images\image-20241102213008965.png)

增加字段后首先无法通过align_test，故尝试各种办法修复。

![image-20241103005500002](C:\Users\stone\AppData\Roaming\Typora\typora-user-images\image-20241103005500002.png)

修改padding即可。

![image-20241103134255730](C:\Users\stone\AppData\Roaming\Typora\typora-user-images\image-20241103134255730.png)

没有实现Clone和Copy。

Copy本身就是无法实现的，对于原子操作来说。

所以只能先实现Clone。Clone的话我们新建一个类型叫做AtomicBlockPtr，其实就是封装了一下Atomic<BlockPointer>。总之还是跑起来了。

卡在这里了，接下来应该如何选择？

Refcell是目前最适合的一个，但是问题在于有多个线程同时访问一个内存分配器的问题。

不能用Refcell。Miallocator应该自己实现一个数据结构。故，需要一个Miallocator和MiallocatorInner，而外面的这一层负责并发访问。
