/* Thin read-only ABI over NVOC's RM transport. NVIDIA layouts come from OGKM,
 * not handwritten Rust copies. No probe CLI or performance SET calls. */
#include "nvx_xnu.h"
#include "nvx_rm.h"
#include "nvx_defs.h"
#include "nv-ioctl.h"
#include "nv_escape.h"
#include "class/cl0080.h"
#include "class/cl2080.h"
#include "ctrl/ctrl0000/ctrl0000gpu.h"
#include "ctrl/ctrl0080/ctrl0080gpu.h"
#include "ctrl/ctrl2080/ctrl2080gpu.h"
#include "ctrl/ctrl2080/ctrl2080fb.h"
#include "ctrl/ctrl2080/ctrl2080bus.h"
#include "ctrl/ctrl2080/ctrl2080bios.h"
#include "ctrl/ctrl2080/ctrl2080perf.h"
#include <stdio.h>
#include <string.h>
#include <time.h>
#include "rusd_read.h"

enum { RUSD_CLOCK, RUSD_TEMP, RUSD_POWER, RUSD_LIMIT, RUSD_FAN, RUSD_UTIL, RUSD_FIELDS };
typedef struct {
    uint32_t status, valid, graphics_khz, memory_khz;
    int32_t temperature_fx256;
    uint32_t power_mw, limit_mw, fan_rpm, utilization_percent, reserved;
    uint64_t timestamps[RUSD_FIELDS];
} nvoc_rm_telemetry;

typedef struct {
    uint32_t abi_version, rm_gpu_id, domain, bus, slot, function;
    uint32_t vendor_id, device_id, subsystem_id, revision, board_id, class_count;
    uint64_t total_bytes, usable_bytes, bar1_bytes, l2_bytes;
    uint32_t pstate, memory_status, pstate_status, pci_status, bios_status, name_status, class_status;
    char driver[64], name[64], sku[16], project[16];
    nvoc_rm_telemetry telemetry;
} nvoc_rm_snapshot;

_Static_assert(sizeof(nvoc_rm_snapshot) == 360, "Rust/C snapshot ABI mismatch");

/* Return nonzero only when connection teardown must reclaim a failed unmap. */
static int read_telemetry(int ctl, NvHandle client, NvHandle subdevice, nvoc_rm_telemetry *out)
{
    NV00DE_ALLOC_PARAMETERS ap = {0};
    ap.polledDataMask = NV00DE_RUSD_POLL_CLOCK | NV00DE_RUSD_POLL_PERF |
        NV00DE_RUSD_POLL_POWER | NV00DE_RUSD_POLL_THERMAL | NV00DE_RUSD_POLL_FAN;
    NvHandle shared = nvx_rm_alloc(ctl, client, subdevice, 0xcafe0003,
        RM_USER_SHARED_DATA, &ap, sizeof ap, &out->status);
    if (!shared) return 0;
    NvU64 length = 0;
    /* System memory requires ctl fd both for RM context and IOKit mapping. */
    NV00DE_SHARED_DATA *data = nvx_map_memory_readonly(ctl, ctl, client, subdevice,
        shared, sizeof(*data), &length, &out->status);
    if (!data) goto done;
    if (length < sizeof(*data)) { out->status = UINT32_MAX; goto unmap; }
    const NvU64 baseline[RUSD_FIELDS] = {
        data->clkPublicDomainInfos.lastModifiedTimestamp,
        data->temperatures[RUSD_TEMPERATURE_TYPE_GPU].lastModifiedTimestamp,
        data->instPowerUsage.lastModifiedTimestamp,
        data->powerLimitGpu.lastModifiedTimestamp,
        data->fanCoolerStatus.lastModifiedTimestamp,
        data->perfDevUtil.lastModifiedTimestamp,
    };
    /* Wait at most 1.5 s for the default polling interval, without changing it.
     * All available polled sections must advance in this session. */
    for (unsigned attempt = 0; attempt < 16; attempt++) {
        RUSD_CLK_PUBLIC_DOMAIN_INFOS clocks;
        RUSD_TEMPERATURE temp;
        RUSD_INST_POWER_USAGE power;
        RUSD_POWER_LIMITS limit;
        RUSD_FAN_COOLER_STATUS fan;
        RUSD_PERF_DEVICE_UTILIZATION util;
#define READ(field, dest, index) nvoc_rusd_read(&(field).lastModifiedTimestamp, &(field), \
    &(dest), sizeof(dest), baseline[index], &out->timestamps[index])
        /* Reset validity on each attempt; never keep a value whose current
         * sequence became invalid or whose data could no longer be read. */
        out->valid = 0;
        if (READ(data->clkPublicDomainInfos, clocks, RUSD_CLOCK)) {
            out->graphics_khz = clocks.info[RUSD_CLK_PUBLIC_DOMAIN_GRAPHICS].actualClkKHz;
            out->memory_khz = clocks.info[RUSD_CLK_PUBLIC_DOMAIN_MEMORY].actualClkKHz;
            out->valid |= 1u << RUSD_CLOCK;
        }
        if (READ(data->temperatures[RUSD_TEMPERATURE_TYPE_GPU], temp, RUSD_TEMP)) {
            out->temperature_fx256 = temp.temperature;
            out->valid |= 1u << RUSD_TEMP;
        }
        if (READ(data->instPowerUsage, power, RUSD_POWER)) {
            out->power_mw = power.info.instGpuPower;
            out->valid |= 1u << RUSD_POWER;
        }
        if (READ(data->powerLimitGpu, limit, RUSD_LIMIT)) {
            out->limit_mw = limit.info.enforcedmW;
            out->valid |= 1u << RUSD_LIMIT;
        }
        if (READ(data->fanCoolerStatus, fan, RUSD_FAN)) {
            out->fan_rpm = 0;
            for (unsigned i = 0; i < RUSD_FAN_COOLER_MAX_COOLERS; i++)
                if (fan.info.rpmCurr[i] > out->fan_rpm) out->fan_rpm = fan.info.rpmCurr[i];
            out->valid |= 1u << RUSD_FAN;
        }
        if (READ(data->perfDevUtil, util, RUSD_UTIL)) {
            out->utilization_percent = util.info.gpuPercentBusy;
            out->valid |= 1u << RUSD_UTIL;
        }
#undef READ
        if (out->valid == (1u << RUSD_FIELDS) - 1) break;
        if (attempt < 15) {
            struct timespec delay = { .tv_nsec = 100000000 };
            nanosleep(&delay, NULL);
        }
    }
unmap:
    /* If unmapping fails, keep RM backing alive until connection close. */
    if (nvx_unmap_memory_fd(ctl, data) != 0) { out->valid = 0; out->status = UINT32_MAX; return 1; }
done:
    nvx_rm_free(ctl, client, subdevice, shared);
    return 0;
}

static uint32_t failure(char *error, size_t cap, const char *operation, uint32_t status)
{
    snprintf(error, cap, "%s: 0x%08x (%s)", operation, status, nvx_status_name(status));
    return status ? status : UINT32_MAX;
}

/* Must be serialized by the Rust caller: nvx_xnu owns a process-global fd table.
 * NullMoth's CARD_INFO currently describes the first matching service only. */
uint32_t nvoc_macos_rm_snapshot(nvoc_rm_snapshot *out, size_t size, uint32_t telemetry,
                               char *error, size_t cap)
{
    if (size != sizeof(*out)) return failure(error, cap, "snapshot ABI size", UINT32_MAX);
    memset(out, 0, sizeof(*out));
    out->abi_version = 2;
    int ctl = nvx_open(1), dev = -1, abandon_chain = 0;
    NvHandle client = 0, device = 0, subdevice = 0;
    uint32_t result = UINT32_MAX, st = UINT32_MAX;
    const char *operation = "open NullMoth IOKit control service (driver loaded?)";
    if (ctl < 0) goto done;
    operation = "CARD_INFO";
    nv_ioctl_card_info_t card = {0};
    if (nvx_esc(ctl, NV_ESC_CARD_INFO, &card, sizeof card) || !card.valid) goto done;
    out->rm_gpu_id = card.gpu_id;
    out->domain = card.pci_info.domain; out->bus = card.pci_info.bus;
    out->slot = card.pci_info.slot; out->function = card.pci_info.function;
    out->vendor_id = card.pci_info.vendor_id; out->device_id = card.pci_info.device_id;
    operation = "RM version handshake";
    nv_ioctl_rm_api_version_t version = {0};
    version.cmd = NV_RM_API_VERSION_CMD_QUERY;
    snprintf(version.versionString, sizeof version.versionString, "610.57.04");
    if (nvx_esc(ctl, NV_ESC_CHECK_VERSION_STR, &version, sizeof version) ||
        version.reply != NV_RM_API_VERSION_REPLY_RECOGNIZED ||
        strncmp(version.versionString, "610.57.04", sizeof version.versionString)) goto done;
    snprintf(out->driver, sizeof out->driver, "%s", version.versionString);
    operation = "open NullMoth IOKit device service";
    dev = nvx_open(0);
    if (dev < 0) goto done;
    operation = "REGISTER_FD";
    nv_ioctl_register_fd_t rf = {0}; rf.ctl_fd = ctl;
    if (nvx_esc(dev, NV_ESC_REGISTER_FD, &rf, sizeof rf)) goto done;
    operation = "allocate root client";
    client = nvx_rm_alloc(ctl, 0, 0, 0xcafe0000, NVX_CLASS_ROOT_CLIENT, NULL, 0, &st);
    if (!client) goto done;
    operation = "attach GPU";
    NV0000_CTRL_GPU_ATTACH_IDS_PARAMS attach = {0};
    attach.gpuIds[0] = card.gpu_id; attach.gpuIds[1] = NV0000_CTRL_GPU_INVALID_ID;
    st = nvx_rm_control(ctl, client, client, NV0000_CTRL_CMD_GPU_ATTACH_IDS, &attach, sizeof attach);
    if (st) goto done;
    operation = "GPU device instance";
    NV0000_CTRL_GPU_GET_ID_INFO_V2_PARAMS id = {0}; id.gpuId = card.gpu_id;
    st = nvx_rm_control(ctl, client, client, NV0000_CTRL_CMD_GPU_GET_ID_INFO_V2, &id, sizeof id);
    if (st) goto done;
    operation = "allocate device";
    NV0080_ALLOC_PARAMETERS dp = {0}; dp.deviceId = id.deviceInstance; dp.hClientShare = client;
    device = nvx_rm_alloc(ctl, client, client, 0xcafe0001, NV01_DEVICE_0, &dp, sizeof dp, &st);
    if (!device) goto done;
    operation = "allocate subdevice";
    NV2080_ALLOC_PARAMETERS sp = {0};
    subdevice = nvx_rm_alloc(ctl, client, device, 0xcafe0002, NV20_SUBDEVICE_0, &sp, sizeof sp, &st);
    if (!subdevice) goto done;
#define QUERY(cmd, p) nvx_rm_control(ctl, client, subdevice, cmd, &(p), sizeof(p))
    NV2080_CTRL_GPU_GET_NAME_STRING_PARAMS name = {0};
    out->name_status = QUERY(NV2080_CTRL_CMD_GPU_GET_NAME_STRING, name);
    if (!out->name_status) {
        memcpy(out->name, name.gpuNameString.ascii, sizeof out->name - 1);
    }
    NV2080_CTRL_BUS_GET_PCI_INFO_PARAMS pci = {0};
    out->pci_status = QUERY(NV2080_CTRL_CMD_BUS_GET_PCI_INFO, pci);
    if (!out->pci_status) { out->subsystem_id = pci.pciSubSystemId; out->revision = pci.pciRevisionId; }
    NV2080_CTRL_BIOS_GET_SKU_INFO_PARAMS bios = {0};
    out->bios_status = QUERY(NV2080_CTRL_CMD_BIOS_GET_SKU_INFO, bios);
    if (!out->bios_status) {
        out->board_id = bios.BoardID;
        memcpy(out->sku, bios.chipSKU, sizeof bios.chipSKU);
        memcpy(out->project, bios.project, sizeof bios.project);
    }
    const NvU32 wanted[] = { NV2080_CTRL_FB_INFO_INDEX_TOTAL_RAM_SIZE,
        NV2080_CTRL_FB_INFO_INDEX_USABLE_RAM_SIZE, NV2080_CTRL_FB_INFO_INDEX_BAR1_SIZE,
        NV2080_CTRL_FB_INFO_INDEX_L2CACHE_SIZE };
    NV2080_CTRL_FB_GET_INFO_V2_PARAMS fb = {0}; fb.fbInfoListSize = 4;
    for (unsigned i = 0; i < 4; i++) fb.fbInfoList[i].index = wanted[i];
    out->memory_status = QUERY(NV2080_CTRL_CMD_FB_GET_INFO_V2, fb);
    if (!out->memory_status) {
        out->total_bytes = (uint64_t)fb.fbInfoList[0].data * 1024;
        out->usable_bytes = (uint64_t)fb.fbInfoList[1].data * 1024;
        out->bar1_bytes = (uint64_t)fb.fbInfoList[2].data * 1024;
        out->l2_bytes = fb.fbInfoList[3].data;
    }
    NV2080_CTRL_PERF_GET_CURRENT_PSTATE_PARAMS ps = {0};
    out->pstate_status = QUERY(NV2080_CTRL_CMD_PERF_GET_CURRENT_PSTATE, ps);
    if (!out->pstate_status) out->pstate = ps.currPstate;
    NV0080_CTRL_GPU_GET_CLASSLIST_V2_PARAMS classes = {0};
    out->class_status = nvx_rm_control(ctl, client, device,
        NV0080_CTRL_CMD_GPU_GET_CLASSLIST_V2, &classes, sizeof classes);
    if (!out->class_status) out->class_count = classes.numClasses;
    if (telemetry) abandon_chain = read_telemetry(ctl, client, subdevice, &out->telemetry);
    result = 0;
done:
    if (result) result = failure(error, cap, operation, st);
    /* Free partially-created chains as well as successful sessions. */
    if (!abandon_chain) nvx_rm_chain_close(ctl, client, device, subdevice);
    if (dev >= 0) nvx_close(dev);
    if (ctl >= 0) nvx_close(ctl);
    return result;
}
