#include <stdio.h>
typedef void* (*CallBackMalloc)(size_t size);
typedef void* (*CallBackMallocAligned)(size_t size,size_t align);
typedef void (*CallBackFree)(void* ptr,size_t size);
CallBackMalloc cb1_glibc_bench_simple;
CallBackMallocAligned cb2_glibc_bench_simple;
CallBackFree cb3_glibc_bench_simple;

void* glibc_bench_simple_malloc(size_t size){
    return cb1_glibc_bench_simple(size);
}
void* glibc_bench_simple_malloc_aligned(size_t size,size_t align){
    return cb2_glibc_bench_simple(size,align);
}
void glibc_bench_simple_free(void* ptr,size_t size){
    cb3_glibc_bench_simple(ptr,size);
}

