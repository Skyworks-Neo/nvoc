/*
 * probe-macos-rm — IOKit user-client transport to the NullMoth NVRM kext.
 *
 * Mirrors the proven nvrm_xnu.c flow shipped inside nvk/nvk-macos.patch of
 * reverse/nvidia-macos-driver-main (see feasibility report §3.5):
 *   - IOServiceOpen on "NVAccel" with type 0x4E560000|n, falling back to
 *     "NVRM" with plain type n (0 = ctl equivalent, 1 = device equivalent);
 *   - a synthetic fd (open("/dev/null")) bound to the connection via
 *     IOConnectCallScalarMethod(SEL_BINDFD = 3);
 *   - RM escapes via IOConnectCallMethod(SEL_ESC = 1) with the cmd as the
 *     single scalar input and the struct as the struct input/output.
 *
 * On non-Apple builds (NVX_LINT or !__APPLE__) every entry point is a stub
 * so the rest of the suite can be compile-checked off-target.
 */
#ifndef NVX_XNU_H
#define NVX_XNU_H

#include <stdint.h>
#include <stddef.h>
#include "nvtypes.h"

/* fds returned by nvx_open are small integers usable as array indexes. */
int   nvx_open(int ctl);            /* ctl != 0 -> "ctl" connection */
int   nvx_close(int fd);
int   nvx_esc(int fd, uint32_t cmd, void *data, uint32_t size);

/*
 * Map an RM memory object to user space: NV_ESC_RM_MAP_MEMORY (nvos33+fd)
 * on the ctl connection, then IOConnectMapMemory64 on the device connection.
 * Returns NULL on failure. *out_len receives the mapped size.
 */
void *nvx_map_memory(int ctl_fd, int dev_fd, NvHandle hClient, NvHandle hDevice,
                     NvHandle hMemory, NvU64 size, NvU64 *out_len);
/* Read-only RM mapping; status is RM's result, or UINT32_MAX for transport/IOKit failure. */
void *nvx_map_memory_readonly(int ctl_fd, int dev_fd, NvHandle hClient, NvHandle hDevice,
                              NvHandle hMemory, NvU64 size, NvU64 *out_len, NvU32 *status);
void  nvx_unmap_memory(void *addr, size_t len);
/* Explicitly remove an IOKit mapping before freeing its RM backing object. */
int nvx_unmap_memory_fd(int fd, void *addr);

#endif /* NVX_XNU_H */
