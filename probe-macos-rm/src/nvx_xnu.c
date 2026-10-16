#include "nvx_xnu.h"

#include <stdio.h>
#include <string.h>
#include <errno.h>

#if defined(__APPLE__) && !defined(NVX_LINT)

#include <IOKit/IOKitLib.h>
#include <mach/mach.h>
#include <fcntl.h>
#include <unistd.h>
#include "nv-ioctl.h"
#include "nv_escape.h"
#include "nv-unix-nvos-params-wrappers.h"

#define NVX_MAX_FD   32768
enum { SEL_GO = 0, SEL_ESC = 1, SEL_GETFD = 2, SEL_BINDFD = 3 };

static io_connect_t gConn[NVX_MAX_FD];

static io_connect_t conn_of(int fd)
{
    return (fd >= 0 && fd < NVX_MAX_FD) ? gConn[fd] : 0;
}

static int open_service(const char *name, uint32_t type, io_connect_t *out)
{
    io_service_t svc = IOServiceGetMatchingService(kIOMainPortDefault, IOServiceMatching(name));
    if (!svc) return 0;
    kern_return_t kr = IOServiceOpen(svc, mach_task_self(), type, out);
    IOObjectRelease(svc);
    return kr == KERN_SUCCESS;
}

int nvx_open(int ctl)
{
    uint32_t type = ctl ? 0u : 1u;   /* 0 = ctl-equivalent, 1 = device-equivalent */
    io_connect_t conn = 0;

    /* NVAccel first (0x4E560000|type), NVRM fallback (plain type) — the
     * exact preference order nvrm_xnu_open() uses. */
    if (open_service("NVAccel", 0x4E560000u | type, &conn) ||
        open_service("NVRM", type, &conn)) {
        /* fall through */
    } else {
        fprintf(stderr, "nvx: no NVAccel and no NVRM service is present\n");
        errno = ENODEV;
        return -1;
    }

    int fd = open("/dev/null", O_RDONLY | O_CLOEXEC);
    if (fd < 0 || fd >= NVX_MAX_FD) {
        if (fd >= 0) close(fd);
        IOServiceClose(conn);
        errno = EMFILE;
        return -1;
    }
    uint64_t in = (uint64_t)fd;
    kern_return_t kr = IOConnectCallScalarMethod(conn, SEL_BINDFD, &in, 1, NULL, NULL);
    if (kr != KERN_SUCCESS) {
        fprintf(stderr, "nvx: bind fd -> kr 0x%08x\n", kr);
        close(fd);
        IOServiceClose(conn);
        errno = EIO;
        return -1;
    }
    gConn[fd] = conn;
    return fd;
}

int nvx_close(int fd)
{
    io_connect_t conn = conn_of(fd);
    if (conn) {
        uint64_t in = (uint64_t)(int64_t)-2;   /* unbind, mirroring nvrm_xnu_close */
        IOConnectCallScalarMethod(conn, SEL_BINDFD, &in, 1, NULL, NULL);
        IOServiceClose(conn);
        gConn[fd] = 0;
    }
    return close(fd);
}

int nvx_esc(int fd, uint32_t cmd, void *data, uint32_t size)
{
    io_connect_t conn = conn_of(fd);
    if (!conn) return EBADF;
    uint64_t in = cmd;
    size_t out = size;
    kern_return_t kr = IOConnectCallMethod(conn, SEL_ESC, &in, 1,
                                           data, size, NULL, NULL, data, &out);
    if (kr != KERN_SUCCESS) {
        fprintf(stderr, "nvx: esc 0x%02x (%u bytes) -> kr 0x%08x\n", cmd, size, kr);
        return EINVAL;
    }
    return 0;
}

static void *map_memory(int ctl_fd, int dev_fd, NvHandle hClient, NvHandle hDevice,
                         NvHandle hMemory, NvU64 size, NvU64 *out_len,
                         NvU32 flags, NvU32 *status)
{
    if (out_len) *out_len = 0;
    if (status) *status = UINT32_MAX;
    io_connect_t devConn = conn_of(dev_fd);
    if (!devConn || !conn_of(ctl_fd)) { errno = EBADF; return NULL; }

    nv_ioctl_nvos33_parameters_with_fd m;
    memset(&m, 0, sizeof m);
    m.params.hClient = hClient;
    m.params.hDevice = hDevice;
    m.params.hMemory = hMemory;
    m.params.length  = size;
    m.params.flags = flags;
    m.fd = dev_fd;
    if (nvx_esc(ctl_fd, NV_ESC_RM_MAP_MEMORY, &m, sizeof m) != 0)
        return NULL;
    if (m.params.status != 0) {
        if (status) *status = m.params.status;
        fprintf(stderr, "nvx: MAP_MEMORY status 0x%x\n", m.params.status);
        return NULL;
    }
    mach_vm_address_t addr = 0;
    mach_vm_size_t len = 0;
    kern_return_t kr = IOConnectMapMemory64(devConn, 0, mach_task_self(),
                                            &addr, &len, kIOMapAnywhere);
    if (kr != KERN_SUCCESS) {
        fprintf(stderr, "nvx: IOConnectMapMemory64 -> 0x%08x\n", kr);
        return NULL;
    }
    if (out_len) *out_len = (NvU64)len;
    if (status) *status = 0;
    return (void *)(uintptr_t)addr;
}

void *nvx_map_memory(int ctl_fd, int dev_fd, NvHandle hClient, NvHandle hDevice,
                     NvHandle hMemory, NvU64 size, NvU64 *out_len)
{
    return map_memory(ctl_fd, dev_fd, hClient, hDevice, hMemory, size, out_len, 0, NULL);
}

void *nvx_map_memory_readonly(int ctl_fd, int dev_fd, NvHandle hClient, NvHandle hDevice,
                              NvHandle hMemory, NvU64 size, NvU64 *out_len, NvU32 *status)
{
    return map_memory(ctl_fd, dev_fd, hClient, hDevice, hMemory, size, out_len,
                      NVOS33_FLAGS_ACCESS_READ_ONLY, status);
}

void nvx_unmap_memory(void *addr, size_t len)
{
    (void)len;
    /* The device connection is closed by nvx_close, which tears the mapping
     * down; v1 does not track individual mappings (suite, not a library). */
    (void)addr;
}

int nvx_unmap_memory_fd(int fd, void *addr)
{
    io_connect_t conn = conn_of(fd);
    if (!conn) return EBADF;
    return IOConnectUnmapMemory64(conn, 0, mach_task_self(),
                                  (mach_vm_address_t)(uintptr_t)addr) == KERN_SUCCESS ? 0 : EIO;
}

#else /* !__APPLE__ || NVX_LINT — off-target stubs, compile-check only */

int nvx_open(int ctl)            { (void)ctl; return -1; }
int nvx_close(int fd)            { (void)fd; return 0; }
int nvx_esc(int fd, uint32_t cmd, void *data, uint32_t size)
{
    (void)fd; (void)cmd; (void)data; (void)size;
    return EINVAL;
}
void *nvx_map_memory(int ctl_fd, int dev_fd, NvHandle hClient, NvHandle hDevice,
                     NvHandle hMemory, NvU64 size, NvU64 *out_len)
{
    (void)ctl_fd; (void)dev_fd; (void)hClient; (void)hDevice;
    (void)hMemory; (void)size; (void)out_len;
    return NULL;
}
void nvx_unmap_memory(void *addr, size_t len) { (void)addr; (void)len; }
int nvx_unmap_memory_fd(int fd, void *addr) { (void)fd; (void)addr; return EINVAL; }

void *nvx_map_memory_readonly(int ctl_fd, int dev_fd, NvHandle hClient, NvHandle hDevice,
                              NvHandle hMemory, NvU64 size, NvU64 *out_len, NvU32 *status)
{
    if (out_len) *out_len = 0;
    if (status) *status = UINT32_MAX;
    return nvx_map_memory(ctl_fd, dev_fd, hClient, hDevice, hMemory, size, out_len);
}

#endif
