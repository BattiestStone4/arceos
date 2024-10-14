# Week1-4

### 完成的工作：

1.先运行起现有的ArceOS环境。

2.多次运行多线程malloc/free用例，观察所耗费的时间，并与原版mimalloc-bench的时间进行比较。

现有ArceOS测例，rust版本2.56s，C版本4.26s。

mimalloc-bench

3.阅读现有ArceOS中mimalloc代码的实现。

### 下周的预期：

1.尽快添加mimalloc-bench里多线程的测试用例。

2.根据原版mimalloc对多线程分配的处理，先将必要的数据结构构造出来。

### 遇到的问题：

1.zyb学长原版仓库依赖存在问题，arceos无法正常运行。

解决方案：直接用arceos主线的dev-monolithickernel分支。

# Mimalloc简介

为完成原版的mimalloc的移植，本人认为需要对这个内存分配器做一些了解与资料收集。

这个简介分为两个部分：一是熟悉当前已经完成的工作，二是阅读mimalloc中有关多线程的部分，并结合原项目的工作来探索下一步的方向。

### 1.ArceOS中当前已经完成的工作

![image-20240928235853512](C:\Users\pc\AppData\Roaming\Typora\typora-user-images\image-20240928235853512.png)

### 2.mimalloc文档与源码中有关多线程的部分

Mimalloc指出：传统的allocator根据内存块大小分类来管理free list，其中一类大小的堆块会以同个单链表连接起来。优点在于访问确实是O(1)，缺点却是堆块会排布在整个堆上。

为改善局部性，Mimalloc采用了一种叫free list sharding的设计。它先将堆分为一系列固定大小的页，之后每页都通过一个free list管理。

在mimalloc中page归属于线程堆, 线程仅从本地堆中分配内存, 但其它线程同样可以释放本线程的堆. 为避免引入锁, mimalloc再为每页增加一个thread free list用于记录由其它线程释放(本线程申请)的内存, 当非本地释放发生时使用atomic_push(&page->thread_free, p)将其放入thread free list中.

mimalloc中内存块的最小单位是mi_block_t, 区别于ptmalloc中malloc_chunk复杂的结构, mi_block_t只有一个指向(同样大小的)下一空闲内存块的指针.
这是因为在mimalloc中所有内存块都是size classed page中分配的, 不需要对空闲内存块做migrate, 因此不用保存本块大小, (物理连续的块的)状态及大小等信息.

### reference:

1.https://www.microsoft.com/en-us/research/uploads/prod/2019/06/mimalloc-tr-v1.pdf

2.https://www.cnblogs.com/Five100Miles/p/12169392.html



