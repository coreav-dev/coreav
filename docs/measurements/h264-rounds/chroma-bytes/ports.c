// Throughput of single instructions on this CPU: 8 independent chains, N iterations.
#include <stdio.h>
#include <stdint.h>
#include <x86intrin.h>
#include <string.h>
static inline uint64_t now(){ unsigned a; return __rdtscp(&a); }
#define BODY(INS) \
  asm volatile( ".rept 100\n" INS "\n.endr" ::: "xmm0","xmm1","xmm2","xmm3","xmm4","xmm5","xmm6","xmm7","xmm8","xmm9","xmm10","xmm11","xmm12","xmm13","xmm14","xmm15");
#define T(name, INS) { uint64_t best=~0ull; for(int r=0;r<200;r++){ uint64_t t=now(); for(int i=0;i<100;i++){BODY(INS)} uint64_t d=now()-t; if(d<best) best=d;} printf("%-22s %.3f tsc/ins\n", name, best/(100.0*100*8)); }
#define E8(op) op " %%ymm8,%%ymm9,%%ymm0\n" op " %%ymm8,%%ymm9,%%ymm1\n" op " %%ymm8,%%ymm9,%%ymm2\n" op " %%ymm8,%%ymm9,%%ymm3\n" op " %%ymm8,%%ymm9,%%ymm4\n" op " %%ymm8,%%ymm9,%%ymm5\n" op " %%ymm8,%%ymm9,%%ymm6\n" op " %%ymm8,%%ymm9,%%ymm7"
int main(){
  T("vpaddb", E8("vpaddb"))
  T("vpunpcklbw", E8("vpunpcklbw"))
  T("vpunpcklqdq", E8("vpunpcklqdq"))
  T("vpshufb", E8("vpshufb"))
  T("vpackuswb", E8("vpackuswb"))
  T("vpavgb", E8("vpavgb"))
  T("vpblendd", "vpblendd $0xf0,%%ymm8,%%ymm9,%%ymm0\nvpblendd $0xf0,%%ymm8,%%ymm9,%%ymm1\nvpblendd $0xf0,%%ymm8,%%ymm9,%%ymm2\nvpblendd $0xf0,%%ymm8,%%ymm9,%%ymm3\nvpblendd $0xf0,%%ymm8,%%ymm9,%%ymm4\nvpblendd $0xf0,%%ymm8,%%ymm9,%%ymm5\nvpblendd $0xf0,%%ymm8,%%ymm9,%%ymm6\nvpblendd $0xf0,%%ymm8,%%ymm9,%%ymm7")
  T("vinserti128", "vinserti128 $1,%%xmm8,%%ymm9,%%ymm0\nvinserti128 $1,%%xmm8,%%ymm9,%%ymm1\nvinserti128 $1,%%xmm8,%%ymm9,%%ymm2\nvinserti128 $1,%%xmm8,%%ymm9,%%ymm3\nvinserti128 $1,%%xmm8,%%ymm9,%%ymm4\nvinserti128 $1,%%xmm8,%%ymm9,%%ymm5\nvinserti128 $1,%%xmm8,%%ymm9,%%ymm6\nvinserti128 $1,%%xmm8,%%ymm9,%%ymm7")
  T("vpshufd", "vpshufd $0x4e,%%ymm8,%%ymm0\nvpshufd $0x4e,%%ymm8,%%ymm1\nvpshufd $0x4e,%%ymm8,%%ymm2\nvpshufd $0x4e,%%ymm8,%%ymm3\nvpshufd $0x4e,%%ymm8,%%ymm4\nvpshufd $0x4e,%%ymm8,%%ymm5\nvpshufd $0x4e,%%ymm8,%%ymm6\nvpshufd $0x4e,%%ymm8,%%ymm7")
  T("vpshufd xmm", "vpshufd $0x4e,%%xmm8,%%xmm0\nvpshufd $0x4e,%%xmm8,%%xmm1\nvpshufd $0x4e,%%xmm8,%%xmm2\nvpshufd $0x4e,%%xmm8,%%xmm3\nvpshufd $0x4e,%%xmm8,%%xmm4\nvpshufd $0x4e,%%xmm8,%%xmm5\nvpshufd $0x4e,%%xmm8,%%xmm6\nvpshufd $0x4e,%%xmm8,%%xmm7")
  T("vpunpcklbw xmm", "vpunpcklbw %%xmm8,%%xmm9,%%xmm0\nvpunpcklbw %%xmm8,%%xmm9,%%xmm1\nvpunpcklbw %%xmm8,%%xmm9,%%xmm2\nvpunpcklbw %%xmm8,%%xmm9,%%xmm3\nvpunpcklbw %%xmm8,%%xmm9,%%xmm4\nvpunpcklbw %%xmm8,%%xmm9,%%xmm5\nvpunpcklbw %%xmm8,%%xmm9,%%xmm6\nvpunpcklbw %%xmm8,%%xmm9,%%xmm7")
  T("vextracti128", "vextracti128 $1,%%ymm8,%%xmm0\nvextracti128 $1,%%ymm8,%%xmm1\nvextracti128 $1,%%ymm8,%%xmm2\nvextracti128 $1,%%ymm8,%%xmm3\nvextracti128 $1,%%ymm8,%%xmm4\nvextracti128 $1,%%ymm8,%%xmm5\nvextracti128 $1,%%ymm8,%%xmm6\nvextracti128 $1,%%ymm8,%%xmm7")
  T("vpblendvb", "vpblendvb %%ymm10,%%ymm8,%%ymm9,%%ymm0\nvpblendvb %%ymm10,%%ymm8,%%ymm9,%%ymm1\nvpblendvb %%ymm10,%%ymm8,%%ymm9,%%ymm2\nvpblendvb %%ymm10,%%ymm8,%%ymm9,%%ymm3\nvpblendvb %%ymm10,%%ymm8,%%ymm9,%%ymm4\nvpblendvb %%ymm10,%%ymm8,%%ymm9,%%ymm5\nvpblendvb %%ymm10,%%ymm8,%%ymm9,%%ymm6\nvpblendvb %%ymm10,%%ymm8,%%ymm9,%%ymm7")
  T("unpck+shufb mix", "vpunpcklbw %%ymm8,%%ymm9,%%ymm0\nvpshufb %%ymm8,%%ymm9,%%ymm1\nvpunpcklbw %%ymm8,%%ymm9,%%ymm2\nvpshufb %%ymm8,%%ymm9,%%ymm3\nvpaddb %%ymm8,%%ymm9,%%ymm4\nvpaddb %%ymm8,%%ymm9,%%ymm5\nvpaddb %%ymm8,%%ymm9,%%ymm6\nvpaddb %%ymm8,%%ymm9,%%ymm7")
  return 0;
}
