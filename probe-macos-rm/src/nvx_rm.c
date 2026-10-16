#include "nvx_rm.h"
#include "nvx_defs.h"
#include "nvx_xnu.h"

#include "nvos.h"
#include "nvstatus.h"
#include "nv_escape.h"
#include "class/cl0080.h"
#include "class/cl2080.h"
#include "ctrl/ctrl0000/ctrl0000gpu.h"

#include <stdio.h>
#include <string.h>

NvU32 nvx_rm_control(int ctl_fd, NvHandle hClient, NvHandle hObject,
                     NvU32 cmd, void *params, NvU32 paramsSize)
{
    NVOS54_PARAMETERS p;
    memset(&p, 0, sizeof p);
    p.hClient = hClient;
    p.hObject = hObject;
    p.cmd = cmd;
    p.flags = NVOS54_FLAGS_NONE;
    /* NvP64 direct cast, same as the NVK shim's nvRmApiControl. */
    p.params = (NvP64)(uintptr_t)params;
    p.paramsSize = paramsSize;
    if (nvx_esc(ctl_fd, NV_ESC_RM_CONTROL, &p, sizeof p) != 0)
        return (NvU32)-1;
    return (NvU32)p.status;
}

NvHandle nvx_rm_alloc(int ctl_fd, NvHandle hRoot, NvHandle hParent,
                      NvHandle hObjectNew, NvU32 hClass,
                      void *allocParms, NvU32 paramsSize, NvU32 *st)
{
    NVOS21_PARAMETERS p;
    memset(&p, 0, sizeof p);
    p.hRoot = hRoot;
    p.hObjectParent = hParent;
    p.hObjectNew = hObjectNew;
    p.hClass = (NvV32)hClass;
    p.pAllocParms = (NvP64)(uintptr_t)allocParms;
    p.paramsSize = paramsSize;
    if (nvx_esc(ctl_fd, NV_ESC_RM_ALLOC, &p, sizeof p) != 0) {
        if (st) *st = (NvU32)-1;
        return 0;
    }
    if (st) *st = (NvU32)p.status;
    return p.status == 0 ? p.hObjectNew : 0;
}

NvU32 nvx_rm_free(int ctl_fd, NvHandle hRoot, NvHandle hParent, NvHandle hObjectOld)
{
    NVOS00_PARAMETERS p;
    memset(&p, 0, sizeof p);
    p.hRoot = hRoot;
    p.hObjectParent = hParent;
    p.hObjectOld = hObjectOld;
    if (nvx_esc(ctl_fd, NV_ESC_RM_FREE, &p, sizeof p) != 0)
        return (NvU32)-1;
    return (NvU32)p.status;
}

int nvx_rm_chain_open(int ctl_fd, NvU32 gpuId,
                      NvHandle *hClient, NvHandle *hDevice, NvHandle *hSubdevice)
{
    NvU32 st = 0;
    NvHandle c = nvx_rm_alloc(ctl_fd, 0, 0, 0xcafe0000, NVX_CLASS_ROOT_CLIENT,
                              NULL, 0, &st);
    if (!c) { fprintf(stderr, "nvx: NV01_ROOT_CLIENT alloc failed (0x%x)\n", st); return -1; }

    NV0000_CTRL_GPU_ATTACH_IDS_PARAMS at;
    memset(&at, 0, sizeof at);
    at.gpuIds[0] = gpuId;
    at.gpuIds[1] = NV0000_CTRL_GPU_INVALID_ID;
    st = nvx_rm_control(ctl_fd, c, c, NV0000_CTRL_CMD_GPU_ATTACH_IDS, &at, sizeof at);
    if (st != 0) { fprintf(stderr, "nvx: GPU_ATTACH_IDS failed (0x%x)\n", st); return -1; }

    NV0080_ALLOC_PARAMETERS dp;
    memset(&dp, 0, sizeof dp);
    dp.deviceId = 0;
    dp.hClientShare = c;
    NvHandle d = nvx_rm_alloc(ctl_fd, c, c, 0xcafe0001, NV01_DEVICE_0,
                              &dp, sizeof dp, &st);
    if (!d) { fprintf(stderr, "nvx: NV01_DEVICE_0 alloc failed (0x%x)\n", st); return -1; }

    NV2080_ALLOC_PARAMETERS sp;
    memset(&sp, 0, sizeof sp);
    NvHandle s = nvx_rm_alloc(ctl_fd, c, d, 0xcafe0002, NV20_SUBDEVICE_0,
                              &sp, sizeof sp, &st);
    if (!s) { fprintf(stderr, "nvx: NV20_SUBDEVICE_0 alloc failed (0x%x)\n", st); return -1; }

    *hClient = c; *hDevice = d; *hSubdevice = s;
    return 0;
}

void nvx_rm_chain_close(int ctl_fd, NvHandle hClient, NvHandle hDevice, NvHandle hSubdevice)
{
    if (hSubdevice) nvx_rm_free(ctl_fd, hClient, hDevice, hSubdevice);
    if (hDevice)    nvx_rm_free(ctl_fd, hClient, hClient, hDevice);
    if (hClient)    nvx_rm_free(ctl_fd, 0, 0, hClient);
}

/* NV_STATUS name via the nvstatuscodes.h X-macro list. */
const char *nvx_status_name(NvU32 status)
{
    switch (status) {
/* nvstatus.h already expanded this guarded X-macro header into the enum. */
#undef SDK_NVSTATUSCODES_H
#define NV_STATUS_CODE(name, code, string) case name: return #name;
#include "nvstatuscodes.h"
#undef NV_STATUS_CODE
    default:
        return "NV_STATUS_UNKNOWN";
    }
}
