#include <stdio.h>
typedef void* (*CallBackMalloc)(size_t size);
typedef void* (*CallBackMallocAligned)(size_t size,size_t align);
typedef void (*CallBackFree)(void* ptr,size_t size);
CallBackMalloc cb1_mi_test;
CallBackMallocAligned cb2_mi_test;
CallBackFree cb3_mi_test;

void* mi_malloc_(size_t size){
    return cb1_mi_test(size);
}
void* mi_malloc_aligned_(size_t size,size_t align){
    return cb2_mi_test(size,align);
}
void mi_free_(void* ptr,size_t size){
    cb3_mi_test(ptr,size);
}

