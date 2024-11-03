# Week3

### 1.完成的工作：

将一些多线程需要使用到的字段加入到对应的数据结构中。

移植一个多线程示例。见allocator_test/glibc_bench_simple.c。

测试发现：system_alloc（此处认为默认实现的内存分配器）在这个测例下的测试时长平均在13秒，有点难以忍受了。

![image-20241014161308180](C:\Users\pc\AppData\Roaming\Typora\typora-user-images\image-20241014161308180.png)

### 2.下周的预期：

读mimalloc源码，写一些原子操作。

page增加thread_id，然后过align_test。

多花点时间在这个上面。忙着做开源操作系统训练营了。

### 3.遇到的问题：

将一些多线程需要使用到的字段加入到对应的数据结构中，但是这样过不了align_test。（发现是page那里的问题，page有一个align_test）

没有找到一个好的评价多线程工作完成的指标。

对原版mimalloc里原子操作，如何实现这个CAS避免竞争，仍然没有一个清晰的概念。换言之，要锻炼自己读长源码的能力。

学长原有的tlsf_c与tlsf_rust都无法过新加入的测试。（但这目前不是我们研究的重点）

### 4.一些记录：

```
每个页维护3个链表：空闲块链表free、本线程回收链表local_free，其他线程回收链表thread_free

维护local_free而不是直接回收到free里应该是基于效率的考虑（并非每次free都要收集，而是定期通过调用page_collect来一次性收集）；thread_free的设立是为了线程安全

需要分配时，先从free中取，若free为空，则再调用page_collect：先去尝试收集thread_free，其次再收集local_free。这一机制可以确保page_collect会被定期调用到，可以同时保证效率和不产生浪费的内存块
```

CAS机制是一种数据更新的方式。在具体讲什么是CAS机制之前，我们先来聊下在多线程环境下，对共享变量进行数据更新的两种模式：悲观锁模式和乐观锁模式。

乐观锁更新方式认为:在更新数据的时候其他线程争抢这个共享变量的概率非常小，所以更新数据的时候不会对共享数据加锁。但是在正式更新数据之前会检查数据是否被其他线程改变过，如果未被其他线程改变过就将共享变量更新成最新值，如果发现共享变量已经被其他线程更新过了，就重试，直到成功为止。CAS机制就是乐观锁的典型实现。

CAS，是Compare and Swap的简称，在这个机制中有三个核心的参数：

- 主内存中存放的共享变量的值：V（一般情况下这个V是内存的地址值，通过这个地址可以获得内存中的值）
- 工作内存中共享变量的副本值，也叫预期值：A
- 需要将共享变量更新到的最新值：B

[![img](https://img2018.cnblogs.com/blog/1775037/202001/1775037-20200106164315461-658325570.jpg)](https://img2018.cnblogs.com/blog/1775037/202001/1775037-20200106164315461-658325570.jpg)

如上图中，主存中保存V值，线程中要使用V值要先从主存中读取V值到线程的工作内存A中，然后计算后变成B值，最后再把B值写回到内存V值中。多个线程共用V值都是如此操作。CAS的核心是在将B值写入到V之前要比较A值和V值是否相同，如果不相同证明此时V值已经被其他线程改变，重新将V值赋给A，并重新计算得到B，如果相同，则将B值赋给V。

值得注意的是CAS机制中的这步步骤是原子性的（从指令层面提供的原子操作），所以CAS机制可以解决多线程并发编程对共享变量读写的原子性问题。