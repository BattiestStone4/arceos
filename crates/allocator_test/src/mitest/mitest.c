#include <stdio.h>
#include <assert.h>
#include "mitest.h"

void test_large() {
  const size_t N = 1000;

  for (size_t i = 0; i < N; ++i) {
    size_t sz = 1ull << 21;
    char *a = mi_malloc_(sz);
    for (size_t k = 0; k < sz; k++) { a[k] = 'x'; }
    mi_free_(a,sz);
  }
}

void mi_test_start(CallBackMalloc _cb1,CallBackMallocAligned _cb2,CallBackFree _cb3) {
  cb1_mi_test = _cb1;
  cb2_mi_test = _cb2;
  cb3_mi_test = _cb3;
  void* p1 = mi_malloc_(16);
  void* p2 = mi_malloc_(1000000);
  mi_free_(p1,16);
  mi_free_(p2,1000000);
  p1 = mi_malloc_(16);
  p2 = mi_malloc_(16);
  mi_free_(p1,16);
  mi_free_(p2,16);

  p1 = mi_malloc_aligned_(64, 8);
  p2 = mi_malloc_aligned_(160,8);
  mi_free_(p2,160);
  mi_free_(p1,64);
  test_large();
}
