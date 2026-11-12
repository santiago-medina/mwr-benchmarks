// Comparable state-dependent 16-bit recurrence across direct execution and KVM.
// C is used for the privileged KVM boundary; Rust code remains forbid(unsafe_code).
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <linux/kvm.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <sys/types.h>
#include <time.h>
#include <unistd.h>
static uint64_t ns(void){struct timespec t;clock_gettime(CLOCK_MONOTONIC_RAW,&t);return (uint64_t)t.tv_sec*1000000000ull+t.tv_nsec;}
static void die(const char*s){perror(s);exit(1);}
__attribute__((noinline)) static uint16_t kernel(uint16_t seed,uint16_t batch){volatile uint16_t state=seed;for(uint16_t i=0;i<batch;i++){uint16_t x=state;state=(uint16_t)(((uint16_t)(x<<1)|(x>>15))^0xb400u);}return state;}
static int compare(const void*a,const void*b){double x=*(const double*)a,y=*(const double*)b;return (x>y)-(x<y);}
static double median(double *v,int n){qsort(v,(size_t)n,sizeof(double),compare);return v[n/2];}
int main(void){
 int kvm=open("/dev/kvm",O_RDWR|O_CLOEXEC);
 if(kvm<0){fprintf(stderr,"KVM unavailable: %s\n",strerror(errno));return 77;}
 if(ioctl(kvm,KVM_GET_API_VERSION,0)!=12){fprintf(stderr,"unsupported KVM API\n");return 2;}
 int vm=ioctl(kvm,KVM_CREATE_VM,0);if(vm<0)die("create vm");
 uint8_t*mem=mmap(NULL,4096,PROT_READ|PROT_WRITE,MAP_PRIVATE|MAP_ANONYMOUS,-1,0);if(mem==MAP_FAILED)die("memory");
 // mov ax,seed; mov cx,batch; rol ax,1; xor ax,0xb400; loop -7; mov dx,0xe9; out dx,ax; hlt
 const uint8_t code[]={0xb8,0x00,0x00,0xb9,0x01,0x00,0xd1,0xc0,0x35,0x00,0xb4,0xe2,0xf9,0xba,0xe9,0x00,0xef,0xf4};
 memcpy(mem,code,sizeof(code));
 struct kvm_userspace_memory_region region={.slot=0,.guest_phys_addr=0,.memory_size=4096,.userspace_addr=(uint64_t)(uintptr_t)mem};
 if(ioctl(vm,KVM_SET_USER_MEMORY_REGION,&region)<0)die("memory region");
 int vcpu=ioctl(vm,KVM_CREATE_VCPU,0);if(vcpu<0)die("vcpu");
 int sz=ioctl(kvm,KVM_GET_VCPU_MMAP_SIZE,0);if(sz<(int)sizeof(struct kvm_run))die("mmap size");
 struct kvm_run*run=mmap(NULL,(size_t)sz,PROT_READ|PROT_WRITE,MAP_SHARED,vcpu,0);if(run==MAP_FAILED)die("run mmap");
 struct kvm_sregs sr;if(ioctl(vcpu,KVM_GET_SREGS,&sr)<0)die("get sregs");sr.cs.base=0;sr.cs.selector=0;if(ioctl(vcpu,KVM_SET_SREGS,&sr)<0)die("set sregs");
 int iterations=1000,reps=9;
 const char*e=getenv("ITERATIONS");if(e)iterations=atoi(e);
 e=getenv("REPETITIONS");if(e)reps=atoi(e);
 if(iterations<1||iterations>100000||reps<1||reps>99)return 2;
 printf("mode,batch,median_ns_per_request,median_ns_per_compute\n");
 volatile uint16_t sink=0;
 for(int batchidx=0;batchidx<3;batchidx++){
   uint16_t batch=(uint16_t)(batchidx==0?1:batchidx==1?16:256);
   double native[99],kvm_samples[99];
   mem[4]=(uint8_t)batch;mem[5]=(uint8_t)(batch>>8);
   for(int rep=0;rep<=reps;rep++){
     uint64_t t=ns();
     for(int i=0;i<iterations;i++){uint16_t seed=(uint16_t)(7u+(unsigned)i*251u+(unsigned)rep*17u);sink=kernel(seed,batch);}
     double a=(double)(ns()-t)/iterations;
     t=ns();
     for(int i=0;i<iterations;i++){
       uint16_t seed=(uint16_t)(7u+(unsigned)i*251u+(unsigned)rep*17u),expected=kernel(seed,batch);
       mem[1]=(uint8_t)seed;mem[2]=(uint8_t)(seed>>8);
       struct kvm_regs regs={.rip=0,.rflags=2};
       if(ioctl(vcpu,KVM_SET_REGS,&regs)<0)die("set regs");
       if(ioctl(vcpu,KVM_RUN,0)<0)die("run");
       if(run->exit_reason!=KVM_EXIT_IO||run->io.direction!=KVM_EXIT_IO_OUT||run->io.port!=0xe9||run->io.size!=2||run->io.count!=1){fprintf(stderr,"KVM unexpected exit %u\n",run->exit_reason);return 4;}
       uint16_t got;memcpy(&got,(uint8_t*)run+run->io.data_offset,sizeof(got));
       if(got!=expected){fprintf(stderr,"KVM mismatch: %u expected %u\n",got,expected);return 5;}
       sink=got;
     }
     double c=(double)(ns()-t)/iterations;
     if(rep){native[rep-1]=a;kvm_samples[rep-1]=c;}
   }
   double a=median(native,reps),c=median(kvm_samples,reps);
   printf("native,%u,%.2f,%.2f\n",batch,a,a/batch);
   printf("kvm_io_exit,%u,%.2f,%.2f\n",batch,c,c/batch);
   fflush(stdout);
 }
 (void)sink;munmap(run,(size_t)sz);close(vcpu);munmap(mem,4096);close(vm);close(kvm);return 0;
}
