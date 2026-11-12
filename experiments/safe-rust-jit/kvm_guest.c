// Linux x86_64 KVM proof-of-execution. This low-level C backend is NOT safe Rust.
// The safe Rust benchmark remains separate; KVM ioctls and mmap require unsafe operations.
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <inttypes.h>
#include <linux/kvm.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <time.h>
#include <unistd.h>
static uint64_t ns(void) { struct timespec t; clock_gettime(CLOCK_MONOTONIC_RAW,&t); return (uint64_t)t.tv_sec*1000000000ull+t.tv_nsec; }
static void die(const char *s) { perror(s); exit(1); }
int main(void) {
    int kvm=open("/dev/kvm",O_RDWR|O_CLOEXEC);
    if(kvm<0) { fprintf(stderr,"KVM unavailable: %s\n",strerror(errno)); return 77; }
    int version=ioctl(kvm,KVM_GET_API_VERSION,0);
    if(version!=12) { fprintf(stderr,"KVM API version %d, expected 12\n",version); return 1; }
    int vm=ioctl(kvm,KVM_CREATE_VM,0); if(vm<0)die("KVM_CREATE_VM");
    uint8_t *mem=mmap(NULL,4096,PROT_READ|PROT_WRITE,MAP_PRIVATE|MAP_ANONYMOUS,-1,0);
    if(mem==MAP_FAILED)die("guest memory mmap");
    // mov dx,0xe9; mov al,42; out dx,al; hlt
    const uint8_t code[]={0xba,0xe9,0x00,0xb0,0x2a,0xee,0xf4};
    memcpy(mem,code,sizeof(code));
    struct kvm_userspace_memory_region region={.slot=0,.guest_phys_addr=0,.memory_size=4096,.userspace_addr=(uint64_t)(uintptr_t)mem};
    if(ioctl(vm,KVM_SET_USER_MEMORY_REGION,&region)<0)die("KVM_SET_USER_MEMORY_REGION");
    int vcpu=ioctl(vm,KVM_CREATE_VCPU,0); if(vcpu<0)die("KVM_CREATE_VCPU");
    int size=ioctl(kvm,KVM_GET_VCPU_MMAP_SIZE,0); if(size<(int)sizeof(struct kvm_run))die("KVM_GET_VCPU_MMAP_SIZE");
    struct kvm_run *run=mmap(NULL,(size_t)size,PROT_READ|PROT_WRITE,MAP_SHARED,vcpu,0);
    if(run==MAP_FAILED)die("vcpu mmap");
    struct kvm_sregs sregs;
    if(ioctl(vcpu,KVM_GET_SREGS,&sregs)<0)die("KVM_GET_SREGS");
    sregs.cs.base=0;sregs.cs.selector=0;
    if(ioctl(vcpu,KVM_SET_SREGS,&sregs)<0)die("KVM_SET_SREGS");
    int n=1000; const char *e=getenv("KVM_ITERATIONS"); if(e){n=atoi(e);if(n<1||n>1000000)return 2;}
    uint64_t start=ns();
    for(int i=0;i<n;i++){
        struct kvm_regs regs={.rip=0,.rflags=2};
        if(ioctl(vcpu,KVM_SET_REGS,&regs)<0)die("KVM_SET_REGS");
        if(ioctl(vcpu,KVM_RUN,0)<0)die("KVM_RUN");
        if(run->exit_reason!=KVM_EXIT_IO || run->io.direction!=KVM_EXIT_IO_OUT || run->io.port!=0xe9 || run->io.size!=1 || run->io.count!=1){fprintf(stderr,"Unexpected exit reason=%u\n",run->exit_reason);return 3;}
        uint8_t value=*((uint8_t *)run+run->io.data_offset);
        if(value!=42){fprintf(stderr,"Unexpected guest value %u\n",value);return 4;}
    }
    double per=(double)(ns()-start)/n;
    printf("kvm_real_mode_io_exit,%d,%.2f\n",n,per);
    munmap(run,(size_t)size);close(vcpu);munmap(mem,4096);close(vm);close(kvm);
    return 0;
}
