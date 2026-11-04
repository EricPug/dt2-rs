#include <unicorn/unicorn.h>
#include <stdio.h>
#include <stdlib.h>
#include <time.h>
#include <string.h>
static uint8_t *ld(const char*p,size_t n){FILE*f=fopen(p,"rb");uint8_t*b=malloc(n);if(fread(b,1,n,f)!=n){puts("short");exit(1);}fclose(f);return b;}
static uint64_t mm_r(uc_engine*u,uint64_t o,unsigned sz,void*ud){return 0;}
static void mm_w(uc_engine*u,uint64_t o,unsigned sz,uint64_t v,void*ud){}
static bool bad(uc_engine*u,uc_mem_type t,uint64_t a,int sz,int64_t v,void*ud){printf("unmapped t=%d addr=%llx sz=%d\n",t,(unsigned long long)a,sz);return false;}
int main(int argc,char**argv){
 uc_engine*uc; uc_open(UC_ARCH_M68K,UC_MODE_BIG_ENDIAN,&uc);
 uc_ctl_set_cpu_model(uc,UC_CPU_M68K_CFV4E);
 uc_mem_map(uc,0x40000000,0x10000000,UC_PROT_ALL);
 uc_mem_map(uc,0x80000000,0x10000,UC_PROT_ALL);
 uc_mem_map(uc,0,0x10000,UC_PROT_ALL);uc_mmio_map(uc,0xEC000000,0x1000000,mm_r,0,mm_w,0);uc_mmio_map(uc,0xFC000000,0x1000000,mm_r,0,mm_w,0);uc_mmio_map(uc,0x8C000000,0x1000000,mm_r,0,mm_w,0);
 char p[64]; for(int i=0;i<4;i++){sprintf(p,"/tmp/ucw/ddr%d.bin",i);uint8_t*b=ld(p,0x4000000);uc_mem_write(uc,0x40000000+i*0x4000000,b,0x4000000);free(b);}
 uint8_t*s=ld("/tmp/ucw/sram.bin",0x10000);uc_mem_write(uc,0x80000000,s,0x10000);
 uint32_t d[8]={1,0xffffc400,0x800069b8,0x80005fe2,2,0,0,0};
 uint32_t a[8]={0x80005fe2,0x80006a1c,0x800047ee,0x80004860,0x400d94b8,0x400d94b8,4,0x429489b8};
 uint32_t pc=0x400d9592,sr=0x2504;uc_reg_write(uc,UC_M68K_REG_SR,&sr);
 for(int i=0;i<8;i++){uc_reg_write(uc,UC_M68K_REG_D0+i,&d[i]);uc_reg_write(uc,UC_M68K_REG_A0+i,&a[i]);}
 uc_reg_write(uc,UC_M68K_REG_PC,&pc);
 uc_hook h1;uc_hook_add(uc,&h1,UC_HOOK_MEM_UNMAPPED,bad,0,1,0);
 size_t n=argc>1?atol(argv[1]):1000000;
 struct timespec t0,t1;clock_gettime(CLOCK_MONOTONIC,&t0);
 uc_err e=uc_emu_start(uc,pc,0,0,n);
 clock_gettime(CLOCK_MONOTONIC,&t1);
 double dt=(t1.tv_sec-t0.tv_sec)+(t1.tv_nsec-t0.tv_nsec)/1e9;
 uc_reg_read(uc,UC_M68K_REG_PC,&pc);uc_reg_read(uc,UC_M68K_REG_SR,&sr);
 printf("err=%d (%s) %.4fs pc=%08x sr=%04x\n",e,uc_strerror(e),dt,pc,sr);
 for(int i=0;i<8;i++){uc_reg_read(uc,UC_M68K_REG_D0+i,&d[i]);uc_reg_read(uc,UC_M68K_REG_A0+i,&a[i]);}
 for(int i=0;i<8;i++)printf("D%d=%08x A%d=%08x\n",i,d[i],i,a[i]);
 return 0;}
