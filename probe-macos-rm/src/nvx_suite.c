#include "nvx_suite.h"
#include "nvx_defs.h"
#include "nvx_xnu.h"
#include "nvx_rm.h"

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdarg.h>
#include <unistd.h>
#include <time.h>
#include <stdatomic.h>

#include "nvos.h"
#include "nvstatus.h"
#include "nvmisc.h"
#include "nv-ioctl.h"
#include "nv_escape.h"
#include "nv-unix-nvos-params-wrappers.h"
#include "class/cl0080.h"
#include "class/cl2080.h"
#include "class/cl00de.h"
#include "class/clc361.h"
#include "class/cl2080_notification.h"
#include "ctrl/ctrl0000/ctrl0000gpu.h"
#include "ctrl/ctrl0000/ctrl0000client.h"
#include "ctrl/ctrl0080/ctrl0080gpu.h"
#include "ctrl/ctrl2080/ctrl2080fb.h"
#include "ctrl/ctrl2080/ctrl2080clk.h"
#include "ctrl/ctrl2080/ctrl2080perf.h"
#include "ctrl/ctrl2080/ctrl2080gpumon.h"
#include "ctrl/ctrl2080/ctrl2080internal.h"
#include "ctrl/ctrl2080/ctrl2080bios.h"
#include "ctrl/ctrl2080/ctrl2080bus.h"
#include "ctrl/ctrl2080/ctrl2080i2c.h"
#include "ctrl/ctrla06f/ctrla06fgpfifo.h"
#include "ctrl/ctrlc36f.h"
#include "alloc/alloc_channel.h"

/* ------------------------------------------------------------------ */
/* result plumbing                                                     */
/* ------------------------------------------------------------------ */

static const char *verdict_name(int v)
{
    switch (v) {
    case NVX_V_OK:       return "OK";
    case NVX_V_REJECTED: return "REJECTED";
    case NVX_V_TRANSPORT:return "TRANSPORT";
    case NVX_V_SKIP:     return "SKIP";
    case NVX_V_GATED:    return "GATED";
    case NVX_V_HARD:     return "HARD";
    default:             return "?";
    }
}

static void jsonl_escape_into(char *out, size_t cap, const char *s)
{
    size_t o = 0;
    for (const char *p = s; *p && o + 6 < cap; p++) {
        unsigned char ch = (unsigned char)*p;
        if (ch == '"' || ch == '\\') { out[o++] = '\\'; out[o++] = ch; }
        else if (ch == '\n') { out[o++] = '\\'; out[o++] = 'n'; }
        else if (ch < 0x20)  { out[o++] = ' '; }
        else                  { out[o++] = ch; }
    }
    out[o] = 0;
}

void nvx_emit(nvx_ctx *c, const char *id, const char *name,
              NvU32 status, int verdict, const char *fmt, ...)
{
    nvx_res_t r;
    memset(&r, 0, sizeof r);
    snprintf(r.id, sizeof r.id, "%s", id);
    snprintf(r.name, sizeof r.name, "%s", name);
    r.status = status;
    r.verdict = verdict;
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(r.detail, sizeof r.detail, fmt, ap);
    va_end(ap);

    if (!c->quiet)
        printf("[%s] %-7s %-34s status 0x%08x (%s)  %s\n",
               verdict_name(verdict), id, name, status,
               nvx_status_name(status), r.detail);

    if (c->jsonl) {
        char det[300], nm[60];
        jsonl_escape_into(det, sizeof det, r.detail);
        jsonl_escape_into(nm, sizeof nm, name);
        fprintf(c->jsonl,
                "{\"time\":%lld,\"id\":\"%s\",\"name\":\"%s\",\"status\":\"0x%08x\","
                "\"statusName\":\"%s\",\"verdict\":\"%s\",\"detail\":\"%s\"}\n",
                (long long)time(NULL), id, nm, status,
                nvx_status_name(status), verdict_name(verdict), det);
        fflush(c->jsonl);
    }
}

/* ------------------------------------------------------------------ */
/* RUSD helpers                                                        */
/* ------------------------------------------------------------------ */

/* Seq-lock read of one RUSD field (same discipline as cl00de.h macros). */
static int rusd_copy(const volatile NvU64 *ts, const void *field, void *dst, size_t sz)
{
    NvU64 t0 = *ts;
    atomic_thread_fence(memory_order_acquire);
    if (t0 == RUSD_TIMESTAMP_INVALID || t0 == RUSD_TIMESTAMP_WRITE_IN_PROGRESS)
        return -1;
    memcpy(dst, (const void *)field, sz);
    atomic_thread_fence(memory_order_acquire);
    NvU64 t1 = *ts;
    return (t0 == t1 && RUSD_SEQ_DATA_VALID(t0)) ? 0 : -1;
}

/* ------------------------------------------------------------------ */
/* shared sampling snapshot                                            */
/* ------------------------------------------------------------------ */

static NvU32 ctl_pstate(nvx_ctx *c, NvU32 *pstate)
{
    NV2080_CTRL_PERF_GET_CURRENT_PSTATE_PARAMS p;
    memset(&p, 0, sizeof p);
    NvU32 st = nvx_rm_control(c->ctl_fd, c->hClient, c->hSubdevice,
                              NV2080_CTRL_CMD_PERF_GET_CURRENT_PSTATE, &p, sizeof p);
    if (st == 0) *pstate = p.currPstate;
    return st;
}

static NvU32 pmumon_tail(nvx_ctx *c, NvU32 *gpcKHz, NvU32 *dramKHz)
{
    static NV2080_CTRL_CLK_PMUMON_CLK_DOMAINS_GET_SAMPLES_PARAMS p; /* ~4 KB */
    memset(&p, 0, sizeof p);
    NvU32 st = nvx_rm_control(c->ctl_fd, c->hClient, c->hSubdevice,
                              NV2080_CTRL_CMD_CLK_PMUMON_CLK_DOMAINS_GET_SAMPLES,
                              &p, sizeof p);
    if (st == 0 && p.super.numSamples > 0) {
        NV2080_CTRL_CLK_PMUMON_CLK_DOMAINS_SAMPLE *s =
            &p.samples[p.super.numSamples - 1];
        *gpcKHz  = s->gpcClkFreqKHz;
        *dramKHz = s->dramClkFreqKHz;
    }
    return st;
}

void nvx_sample_once(nvx_ctx *c, const char *tag)
{
    NvU32 pstate = 0xffffffff, gpc = 0, dram = 0;
    NvU32 s1 = ctl_pstate(c, &pstate);
    NvU32 s2 = pmumon_tail(c, &gpc, &dram);

    long tempC_x256 = -256, instMW = -1, enforcedMW = -1, rpm = -1, gpuBusy = -1;
    int haveRusd = 0;

    if (c->rusd) {
        NV00DE_SHARED_DATA *sd = (NV00DE_SHARED_DATA *)c->rusd;
        RUSD_TEMPERATURE t;
        RUSD_INST_POWER_USAGE pw;
        RUSD_POWER_LIMITS pl;
        RUSD_FAN_COOLER_STATUS fan;
        RUSD_PERF_DEVICE_UTILIZATION util;
        RUSD_CLK_PUBLIC_DOMAIN_INFOS clocks;
        if (rusd_copy(&sd->clkPublicDomainInfos.lastModifiedTimestamp,
                      &sd->clkPublicDomainInfos, &clocks, sizeof clocks) == 0) {
            gpc = clocks.info[RUSD_CLK_PUBLIC_DOMAIN_GRAPHICS].actualClkKHz;
            dram = clocks.info[RUSD_CLK_PUBLIC_DOMAIN_MEMORY].actualClkKHz;
            s2 = 0;
        }
        if (rusd_copy(&sd->temperatures[RUSD_TEMPERATURE_TYPE_GPU].lastModifiedTimestamp,
                      &sd->temperatures[RUSD_TEMPERATURE_TYPE_GPU], &t, sizeof t) == 0)
            { tempC_x256 = (long)t.temperature; haveRusd = 1; } /* NvTemp = SFXP24.8 */
        if (rusd_copy(&sd->instPowerUsage.lastModifiedTimestamp,
                      &sd->instPowerUsage, &pw, sizeof pw) == 0)
            instMW = (long)pw.info.instGpuPower;
        if (rusd_copy(&sd->powerLimitGpu.lastModifiedTimestamp,
                      &sd->powerLimitGpu, &pl, sizeof pl) == 0)
            enforcedMW = (long)pl.info.enforcedmW;
        if (rusd_copy(&sd->fanCoolerStatus.lastModifiedTimestamp,
                      &sd->fanCoolerStatus, &fan, sizeof fan) == 0) {
            rpm = 0;
            for (unsigned i = 0; i < RUSD_FAN_COOLER_MAX_COOLERS; i++)
                if ((long)fan.info.rpmCurr[i] > rpm) rpm = (long)fan.info.rpmCurr[i];
        }
        if (rusd_copy(&sd->perfDevUtil.lastModifiedTimestamp,
                      &sd->perfDevUtil, &util, sizeof util) == 0)
            gpuBusy = (long)util.info.gpuPercentBusy;
    }

    if (!c->quiet)
        printf("  %-6s pstate %u  gpc %u kHz  dram %u kHz  temp %ld.%ld C  "
               "pwr %ld mW  lim %ld mW  fan %ld rpm  busy %ld%%\n",
               tag,
               s1 == 0 ? pstate : 0xffffffffu,
               s2 == 0 ? gpc : 0, s2 == 0 ? dram : 0,
               tempC_x256 >> 8, (tempC_x256 & 0xff) * 100 / 256,
               instMW, enforcedMW, rpm, gpuBusy);

    if (c->jsonl)
        fprintf(c->jsonl,
                "{\"time\":%lld,\"sample\":{\"tag\":\"%s\",\"pstate\":%u,"
                "\"gpcKHz\":%u,\"dramKHz\":%u,\"tempFx256\":%ld,\"instmW\":%ld,"
                "\"limitmW\":%ld,\"fanRpm\":%ld,\"gpuBusy\":%ld,"
                "\"rusd\":%s}}\n",
                (long long)time(NULL), tag,
                s1 == 0 ? pstate : 0xffffffffu,
                s2 == 0 ? gpc : 0, s2 == 0 ? dram : 0,
                tempC_x256, instMW, enforcedMW, rpm, gpuBusy,
                haveRusd ? "true" : "false");
}

/* ------------------------------------------------------------------ */
/* stage 0 — transport                                                 */
/* ------------------------------------------------------------------ */

int nvx_stage0(nvx_ctx *c)
{
    c->ctl_fd = nvx_open(1);
    if (c->ctl_fd < 0) {
        nvx_emit(c, "V0.1", "open-ctl", 0, NVX_V_HARD,
                 "no NVAccel/NVRM user client — driver loaded? (kmutil showloaded | grep nullmoth)");
        return -1;
    }
    nvx_emit(c, "V0.1", "open-ctl", 0, NVX_V_OK, "ctl connection opened (fd %d)", c->ctl_fd);

    c->dev_fd = nvx_open(0);
    if (c->dev_fd < 0) {
        nvx_emit(c, "V0.2", "open-dev", 0, NVX_V_SKIP, "no device connection; RUSD map tests limited");
    } else {
        nvx_emit(c, "V0.2", "open-dev", 0, NVX_V_OK, "device connection opened (fd %d)", c->dev_fd);
    }

    /* NV_ESC_CARD_INFO needs the ctl connection; also proves esc works. */
    nv_ioctl_card_info_t ci;
    memset(&ci, 0, sizeof ci);
    if (nvx_esc(c->ctl_fd, NV_ESC_CARD_INFO, &ci, sizeof ci) != 0) {
        nvx_emit(c, "V0.3", "card-info", 0, NVX_V_TRANSPORT, "esc rejected");
        return -1;
    }
    if (!ci.valid) {
        nvx_emit(c, "V0.3", "card-info", 0, NVX_V_HARD,
                 "card info invalid — adapter never came up. Boot without -nvrmnogo, "
                 "or run 'nvrmctl go 2' (see feasibility report §3.2)");
        return -1;
    }
    c->gpuId = ci.gpu_id;
    nvx_emit(c, "V0.3", "card-info", 0, NVX_V_OK,
             "gpu_id 0x%x  reg 0x%llx/%lluMB  fb 0x%llx/%lluMB  irq %u",
             ci.gpu_id,
             (unsigned long long)ci.reg_address, (unsigned long long)(ci.reg_size >> 20),
             (unsigned long long)ci.fb_address, (unsigned long long)(ci.fb_size >> 20),
             ci.interrupt_line);

    if (c->dev_fd >= 0) {
        nv_ioctl_register_fd_t rf;
        memset(&rf, 0, sizeof rf);
        rf.ctl_fd = c->ctl_fd;
        NvU32 st = nvx_esc(c->dev_fd, NV_ESC_REGISTER_FD, &rf, sizeof rf) == 0 ? 0 : 0xffffffffu;
        nvx_emit(c, "V0.4", "register-fd", st,
                 st == 0 ? NVX_V_OK : NVX_V_REJECTED,
                 st == 0 ? "device connection bound to ctl fd" : "REGISTER_FD refused");
    }

    /* RM API version handshake (Linux nv.c CHECK_VERSION_STR semantics). */
    nv_ioctl_rm_api_version_t v;
    memset(&v, 0, sizeof v);
    v.cmd = NV_RM_API_VERSION_CMD_QUERY;
    strncpy(v.versionString, "610.57.04", sizeof v.versionString - 1);
    if (nvx_esc(c->ctl_fd, NV_ESC_CHECK_VERSION_STR, &v, sizeof v) != 0) {
        nvx_emit(c, "V0.5", "version-check", 0, NVX_V_TRANSPORT, "esc rejected");
    } else {
        nvx_emit(c, "V0.5", "version-check", 0,
                 v.reply == NV_RM_API_VERSION_REPLY_RECOGNIZED ? NVX_V_OK : NVX_V_REJECTED,
                 "reply %u, driver reports \"%.64s\"", v.reply, v.versionString);
    }
    return 0;
}

/* ------------------------------------------------------------------ */
/* stage 1 — RM object chain                                           */
/* ------------------------------------------------------------------ */

/* 64 KB alloc + MAP_MEMORY + pattern write/readback. vidmem=1 exercises the
 * VRAM path, vidmem=0 the sysmem/BAR1 path. */
static int mem_smoke(nvx_ctx *c, int vidmem)
{
    NvU32 st = 0;
    NV_MEMORY_ALLOCATION_PARAMS mp;
    memset(&mp, 0, sizeof mp);
    mp.owner = c->hClient;
    mp.type = NVOS32_TYPE_IMAGE;
    mp.size = 0x10000;
    mp.alignment = 0x10000;
    mp.attr = DRF_DEF(OS32, _ATTR, _PAGE_SIZE, _4KB) |
              (vidmem ? (DRF_DEF(OS32, _ATTR, _LOCATION, _VIDMEM) |
                         DRF_DEF(OS32, _ATTR, _COHERENCY, _UNCACHED))
                      : (DRF_DEF(OS32, _ATTR, _LOCATION, _PCI) |
                         DRF_DEF(OS32, _ATTR, _COHERENCY, _CACHED)));
    NvHandle hMem = nvx_rm_alloc(c->ctl_fd, c->hClient, c->hDevice,
                                 vidmem ? 0xcafe0021 : 0xcafe0020,
                                 vidmem ? NVX_CLASS_MEMORY_LOCAL_USER
                                        : NVX_CLASS_MEMORY_SYSTEM,
                                 &mp, sizeof mp, &st);
    if (!hMem) {
        nvx_emit(c, vidmem ? "V1.9b" : "V1.9a",
                 vidmem ? "mem-smoke-vidmem" : "mem-smoke-sysmem",
                 st, NVX_V_REJECTED, "alloc rejected");
        return -1;
    }
    NvU64 len = 0;
    volatile uint8_t *p = nvx_map_memory(c->ctl_fd, c->dev_fd, c->hClient,
                                         c->hSubdevice, hMem, 0x10000, &len);
    if (!p) {
        nvx_emit(c, vidmem ? "V1.9b" : "V1.9a",
                 vidmem ? "mem-smoke-vidmem" : "mem-smoke-sysmem",
                 0, NVX_V_REJECTED, "alloc 0x%08x ok, map failed", hMem);
        nvx_rm_free(c->ctl_fd, c->hClient, c->hDevice, hMem);
        return -1;
    }
    const NvU32 pat = vidmem ? 0xA5A5C3C3u : 0x5A5A3C3Cu;
    *(volatile NvU32 *)(p + 0x10) = pat;
    *(volatile NvU32 *)(p + len - 4) = ~pat;
    NvU32 r0 = *(volatile NvU32 *)(p + 0x10);
    NvU32 r1 = *(volatile NvU32 *)(p + len - 4);
    int ok = (r0 == pat && r1 == ~pat);
    nvx_emit(c, vidmem ? "V1.9b" : "V1.9a",
             vidmem ? "mem-smoke-vidmem" : "mem-smoke-sysmem",
             0, ok ? NVX_V_OK : NVX_V_REJECTED,
             "%lluKB mapped at %p, readback %s",
             (unsigned long long)(len >> 10), (const void *)p,
             ok ? "OK" : "MISMATCH");
    nvx_rm_free(c->ctl_fd, c->hClient, c->hDevice, hMem);
    return ok ? 0 : -1;
}

int nvx_stage1(nvx_ctx *c, NvU32 gpuId)
{
    NvU32 st = 0;
    NvHandle cl = nvx_rm_alloc(c->ctl_fd, 0, 0, 0xcafe0000, NVX_CLASS_ROOT_CLIENT,
                               NULL, 0, &st);
    if (!cl) {
        nvx_emit(c, "V1.1", "alloc-root-client", st, NVX_V_HARD,
                 "NV01_ROOT_CLIENT (0x41) rejected");
        return -1;
    }
    c->hClient = cl;
    nvx_emit(c, "V1.1", "alloc-root-client", st, NVX_V_OK, "handle 0x%08x", cl);

    NV0000_CTRL_GPU_GET_ATTACHED_IDS_PARAMS ga;
    memset(&ga, 0, sizeof ga);
    st = nvx_rm_control(c->ctl_fd, cl, cl, NV0000_CTRL_CMD_GPU_GET_ATTACHED_IDS,
                        &ga, sizeof ga);
    nvx_emit(c, "V1.2", "get-attached-ids", st,
             st == 0 ? NVX_V_OK : NVX_V_REJECTED,
             st == 0 ? "attached gpuIds 0x%x 0x%x" : "-",
             st == 0 ? ga.gpuIds[0] : 0, st == 0 ? ga.gpuIds[1] : 0);

    NV0000_CTRL_GPU_ATTACH_IDS_PARAMS at;
    memset(&at, 0, sizeof at);
    at.gpuIds[0] = gpuId;
    at.gpuIds[1] = NV0000_CTRL_GPU_INVALID_ID;
    st = nvx_rm_control(c->ctl_fd, cl, cl, NV0000_CTRL_CMD_GPU_ATTACH_IDS, &at, sizeof at);
    if (st != 0) {
        nvx_emit(c, "V1.3", "attach-ids", st, NVX_V_HARD, "gpuId 0x%x", gpuId);
        return -1;
    }
    nvx_emit(c, "V1.3", "attach-ids", st, NVX_V_OK, "gpuId 0x%x", gpuId);

    NV0080_ALLOC_PARAMETERS dp;
    memset(&dp, 0, sizeof dp);
    dp.deviceId = 0;
    dp.hClientShare = cl;
    NvHandle dv = nvx_rm_alloc(c->ctl_fd, cl, cl, 0xcafe0001, NV01_DEVICE_0,
                               &dp, sizeof dp, &st);
    if (!dv) {
        nvx_emit(c, "V1.4", "alloc-device", st, NVX_V_HARD, "NV01_DEVICE_0 rejected");
        return -1;
    }
    c->hDevice = dv;
    nvx_emit(c, "V1.4", "alloc-device", st, NVX_V_OK, "handle 0x%08x", dv);

    NV2080_ALLOC_PARAMETERS sp;
    memset(&sp, 0, sizeof sp);
    NvHandle sd = nvx_rm_alloc(c->ctl_fd, cl, dv, 0xcafe0002, NV20_SUBDEVICE_0,
                               &sp, sizeof sp, &st);
    if (!sd) {
        nvx_emit(c, "V1.5", "alloc-subdevice", st, NVX_V_HARD, "NV20_SUBDEVICE_0 rejected");
        return -1;
    }
    c->hSubdevice = sd;
    nvx_emit(c, "V1.5", "alloc-subdevice", st, NVX_V_OK, "handle 0x%08x", sd);

    /* Device-scope control: proves the device object dispatches controls. */
    static NV0080_CTRL_GPU_GET_CLASSLIST_V2_PARAMS cls; /* 804 B */
    memset(&cls, 0, sizeof cls);
    st = nvx_rm_control(c->ctl_fd, cl, dv, NV0080_CTRL_CMD_GPU_GET_CLASSLIST_V2,
                        &cls, sizeof cls);
    nvx_emit(c, "V1.6", "gpu-classlist-v2", st,
             st == 0 ? NVX_V_OK : NVX_V_REJECTED,
             st == 0 ? "%u classes (NVK-proven path)" : "-", st == 0 ? cls.numClasses : 0);

    /* Privilege introspection: which access rights did RM grant our plain
     * client? On the kext os_check_access() is hard-wired NV_TRUE, so this
     * shows what the RM face actually hands out. */
    NV0000_CTRL_CLIENT_GET_ACCESS_RIGHTS_PARAMS ar;
    memset(&ar, 0, sizeof ar);
    ar.hObject = cl;
    ar.hClient = cl;
    st = nvx_rm_control(c->ctl_fd, cl, cl, NV0000_CTRL_CMD_CLIENT_GET_ACCESS_RIGHTS,
                        &ar, sizeof ar);
    nvx_emit(c, "V1.7", "client-access-rights", st,
             st == 0 ? NVX_V_OK : NVX_V_REJECTED,
             st == 0 ? "mask raw 0x%08x 0x%08x 0x%08x 0x%08x" : "-",
             st == 0 ? ((NvU32 *)&ar.maskResult)[0] : 0,
             st == 0 ? ((NvU32 *)&ar.maskResult)[1] : 0,
             st == 0 ? ((NvU32 *)&ar.maskResult)[2] : 0,
             st == 0 ? ((NvU32 *)&ar.maskResult)[3] : 0);

    NV0000_CTRL_GPU_GET_ID_INFO_V2_PARAMS idv;
    memset(&idv, 0, sizeof idv);
    idv.gpuId = gpuId;
    st = nvx_rm_control(c->ctl_fd, cl, cl, NV0000_CTRL_CMD_GPU_GET_ID_INFO_V2,
                        &idv, sizeof idv);
    nvx_emit(c, "V1.8", "gpu-id-info-v2", st,
             st == 0 ? NVX_V_OK : NVX_V_REJECTED,
             st == 0 ? "flags 0x%x devInst %u boardId 0x%x numa %d" : "-",
             st == 0 ? idv.gpuFlags : 0, st == 0 ? idv.deviceInstance : 0,
             st == 0 ? idv.boardId : 0, st == 0 ? idv.numaId : 0);

    /* Memory smoke: BAR1 mapping of a sysmem + a vidmem allocation with a
     * write/readback pattern — the NV01_MEMORY_* -> MAP_MEMORY -> mmap path
     * the channel smoke and any future pushbuffer work depends on. */
    if (NVX_IDLE_ONLY)
        nvx_emit(c, "V1.9", "memory-smoke", 0, NVX_V_SKIP,
                 "idle-only build: memory smoke disabled");
    else if (mem_smoke(c, 0) == 0 && mem_smoke(c, 1) == 0)
        nvx_emit(c, "V1.9", "memory-smoke", 0, NVX_V_OK,
                 "sysmem+vidmem 64KB mapped, pattern verified");
    else
        nvx_emit(c, "V1.9", "memory-smoke", 0, NVX_V_REJECTED,
                 "see mem-smoke items above");
    return 0;
}

/* ------------------------------------------------------------------ */
/* stage 2 — read-only battery                                         */
/* ------------------------------------------------------------------ */

#define SUBCTL(c, cmd, params) \
    nvx_rm_control((c)->ctl_fd, (c)->hClient, (c)->hSubdevice, (cmd), (params), sizeof *(params))

static void item_fb_info(nvx_ctx *c)
{
    static const NvU32 want[] = {
        NV2080_CTRL_FB_INFO_INDEX_TOTAL_RAM_SIZE,
        NV2080_CTRL_FB_INFO_INDEX_USABLE_RAM_SIZE,
        NV2080_CTRL_FB_INFO_INDEX_BAR1_SIZE,
        NV2080_CTRL_FB_INFO_INDEX_BAR1_AVAIL_SIZE,
        NV2080_CTRL_FB_INFO_INDEX_L2CACHE_SIZE,
        NV2080_CTRL_FB_INFO_INDEX_RAM_TYPE,
        NV2080_CTRL_FB_INFO_INDEX_HEAP_SIZE,
        NV2080_CTRL_FB_INFO_INDEX_MAPPABLE_HEAP_SIZE,
    };
    NV2080_CTRL_FB_GET_INFO_V2_PARAMS p;
    memset(&p, 0, sizeof p);
    const NvU32 n = (NvU32)(sizeof want / sizeof want[0]);
    for (NvU32 i = 0; i < n; i++) p.fbInfoList[i].index = want[i];
    p.fbInfoListSize = n;
    NvU32 st = SUBCTL(c, NV2080_CTRL_CMD_FB_GET_INFO_V2, &p);
    if (st == 0) {
        nvx_emit(c, "V2.1", "fb-get-info-v2", st, NVX_V_OK,
                 "ram %uMB usable %uMB bar1 %uMB avail %uMB l2 %uKB heap %uKB mappable %uKB ramtype 0x%x (NVK-proven path)",
                 p.fbInfoList[0].data >> 10, p.fbInfoList[1].data >> 10,
                 p.fbInfoList[2].data >> 10, p.fbInfoList[3].data >> 10,
                 p.fbInfoList[4].data >> 10, p.fbInfoList[6].data,
                 p.fbInfoList[7].data, p.fbInfoList[5].data);
    } else {
        nvx_emit(c, "V2.1", "fb-get-info-v2", st, NVX_V_REJECTED, "-");
    }
}

static void item_fb_stats(nvx_ctx *c)
{
    NV2080_CTRL_CMD_FB_STATS_GET_PARAMS p;
    memset(&p, 0, sizeof p);
    NvU32 st = SUBCTL(c, NV2080_CTRL_CMD_FB_STATS_GET, &p);
    /* Layout beyond status is not interpreted in v1; record raw first dword. */
    nvx_emit(c, "V2.2", "fb-stats-get", st, st == 0 ? NVX_V_OK : NVX_V_REJECTED,
             "raw[0..1]=0x%08x 0x%08x", ((NvU32 *)&p)[0], ((NvU32 *)&p)[1]);
}

static void item_powerstate_get(nvx_ctx *c)
{
    NV2080_CTRL_PERF_GET_POWERSTATE_PARAMS p;
    memset(&p, 0, sizeof p);
    NvU32 st = SUBCTL(c, NV2080_CTRL_CMD_PERF_GET_POWERSTATE, &p);
    nvx_emit(c, "V2.3", "perf-get-powerstate", st,
             st == 0 ? NVX_V_OK : NVX_V_REJECTED,
             st == 0 ? "powerState %u (0=AC, 1=battery)" : "-", p.powerStateInfo.powerState);
}

static void item_rated_tdp_get(nvx_ctx *c)
{
    NV2080_CTRL_PERF_RATED_TDP_CONTROL_PARAMS p;
    memset(&p, 0, sizeof p);
    NvU32 st = SUBCTL(c, NV2080_CTRL_CMD_PERF_RATED_TDP_GET_CONTROL, &p);
    nvx_emit(c, "V2.4", "rated-tdp-get-control", st,
             st == 0 ? NVX_V_OK : NVX_V_REJECTED,
             st == 0 ? "raw[0..2]=0x%08x 0x%08x 0x%08x (client/input/vpstate echo)" : "-",
             ((NvU32 *)&p)[0], ((NvU32 *)&p)[1], ((NvU32 *)&p)[2]);
}

static void item_edpp_get(nvx_ctx *c, NvU32 *curr_out)
{
    NV2080_CTRL_CMD_INTERNAL_PMGR_PFM_REQ_HNDLR_GET_EDPP_LIMIT_INFO_PARAMS p;
    memset(&p, 0, sizeof p);
    NvU32 st = SUBCTL(c, NV2080_CTRL_CMD_INTERNAL_PMGR_PFM_REQ_HNDLR_GET_EDPP_LIMIT_INFO, &p);
    if (st == 0 && curr_out) *curr_out = p.limitCurr;
    nvx_emit(c, "V2.5", "edpp-limit-info", st,
             st == 0 ? NVX_V_OK : NVX_V_REJECTED,
             st == 0 ? "min %u  rated %u  max %u  curr %u mW (power-limit read face)"
                     : "-",
             st == 0 ? p.limitMin : 0, st == 0 ? p.limitRated : 0,
             st == 0 ? p.limitMax : 0, st == 0 ? p.limitCurr : 0);
}

static void item_gpumon(nvx_ctx *c)
{
    static NV2080_CTRL_PERF_GET_GPUMON_PERFMON_UTIL_SAMPLES_V2_PARAMS p;
    memset(&p, 0, sizeof p);
    p.type = NV2080_CTRL_GPUMON_SAMPLE_TYPE_PERFMON_UTIL;
    p.bufSize = (NvU32)sizeof p.samples;
    p.count = NV2080_CTRL_PERF_GPUMON_SAMPLE_COUNT_PERFMON_UTIL;
    NvU32 st = SUBCTL(c, NV2080_CTRL_CMD_PERF_GET_GPUMON_PERFMON_UTIL_SAMPLES_V2, &p);
    nvx_emit(c, "V2.6", "gpumon-util-samples-v2", st,
             st == 0 ? NVX_V_OK : NVX_V_REJECTED,
             st == 0 ? "tracker %u count %u buffer %u bytes (ring interpretation pending; use RUSD for utilization)" : "-",
             st == 0 ? p.tracker : 0, st == 0 ? p.count : 0, st == 0 ? p.bufSize : 0);
}

static void item_pmumon(nvx_ctx *c)
{
    NvU32 gpc = 0, dram = 0;
    NvU32 st = pmumon_tail(c, &gpc, &dram);
    nvx_emit(c, "V2.7", "pmumon-clk-samples", st,
             st == 0 ? NVX_V_OK : NVX_V_REJECTED,
             st == 0 ? "tail gpc %u kHz  dram %u kHz" : "-", gpc, dram);
}

static void item_pstate(nvx_ctx *c)
{
    NvU32 ps = 0xffffffff;
    NvU32 st = ctl_pstate(c, &ps);
    nvx_emit(c, "V2.8", "perf-current-pstate", st,
             st == 0 ? NVX_V_OK : NVX_V_REJECTED,
             st == 0 ? "P%u" : "-", st == 0 ? ps : 0xffffffffu);
}

static void item_legacy_probes(nvx_ctx *c)
{
    /* Tier-B GET probes: IDs are campaign-sourced (see nvx_defs.h), layouts
     * partially decoded. Read-only; verdicts measure firmware acceptance. */
    static uint8_t buf[32 * NVX_VFPOINTS_RECORD_STRIDE];

    memset(buf, 0, NVX_RAILS_INFO_STRIDE);
    NvU32 st = nvx_rm_control(c->ctl_fd, c->hClient, c->hSubdevice,
                              NVX_RM_CMD_RAILS_GET_INFO, buf, NVX_RAILS_INFO_STRIDE);
    nvx_emit(c, "V2.9", "rails-get-info(legacy)", st,
             st == 0 ? NVX_V_OK : NVX_V_REJECTED,
             st == 0 ? "type@0x44=0x%02x class@0x77=0x%02x" : "-",
             buf[0x44], buf[0x77]);

    memset(buf, 0, NVX_RAILS_STATUS_STRIDE);
    st = nvx_rm_control(c->ctl_fd, c->hClient, c->hSubdevice,
                        NVX_RM_CMD_RAILS_GET_STATUS, buf, NVX_RAILS_STATUS_STRIDE);
    nvx_emit(c, "V2.10", "rails-get-status(legacy)", st,
             st == 0 ? NVX_V_OK : NVX_V_REJECTED,
             st == 0 ? "type@88=0x%02x" : "-", buf[88]);

    memset(buf, 0, sizeof buf);
    st = nvx_rm_control(c->ctl_fd, c->hClient, c->hSubdevice,
                        NVX_RM_CMD_VFPOINTS_GET, buf, 10 * NVX_VFPOINTS_RECORD_STRIDE);
    nvx_emit(c, "V2.11", "vfpoints-get(legacy)", st,
             st == 0 ? NVX_V_OK : NVX_V_REJECTED,
             st == 0 ? "rec0 raw 0x%08x 0x%08x 0x%08x" : "-",
             st == 0 ? ((NvU32 *)buf)[0] : 0, st == 0 ? ((NvU32 *)buf)[1] : 0,
             st == 0 ? ((NvU32 *)buf)[2] : 0);
}

static void item_bios_sku(nvx_ctx *c)
{
    NV2080_CTRL_BIOS_GET_SKU_INFO_PARAMS p;
    memset(&p, 0, sizeof p);
    NvU32 st = SUBCTL(c, NV2080_CTRL_CMD_BIOS_GET_SKU_INFO, &p);
    nvx_emit(c, "V2.13", "bios-sku-info", st,
             st == 0 ? NVX_V_OK : NVX_V_REJECTED,
             st == 0 ? "board 0x%x sku \"%.8s\" mod \"%.4s\" project \"%.4s\"" : "-",
             st == 0 ? p.BoardID : 0, st == 0 ? p.chipSKU : "?",
             st == 0 ? p.chipSKUMod : "?", st == 0 ? p.project : "?");
}

static void item_bus(nvx_ctx *c)
{
    NV2080_CTRL_BUS_GET_PCI_INFO_PARAMS p;
    memset(&p, 0, sizeof p);
    NvU32 st = SUBCTL(c, NV2080_CTRL_CMD_BUS_GET_PCI_INFO, &p);
    nvx_emit(c, "V2.14", "bus-get-pci-info", st,
             st == 0 ? NVX_V_OK : NVX_V_REJECTED,
             st == 0 ? "deviceId 0x%08x subsys 0x%08x rev 0x%x ext 0x%x" : "-",
             st == 0 ? p.pciDeviceId : 0, st == 0 ? p.pciSubSystemId : 0,
             st == 0 ? p.pciRevisionId : 0, st == 0 ? p.pciExtDeviceId : 0);
}

static void item_perf_levels(nvx_ctx *c)
{
    static NV2080_CTRL_PERF_GET_LEVEL_INFO_V2_PARAMS p;
    memset(&p, 0, sizeof p);
    p.level = 0; /* lowest level first; ranges come back per domain */
    p.perfGetClkInfoListSize = NV2080_CTRL_PERF_CLK_MAX_DOMAINS;
    NvU32 st = SUBCTL(c, NV2080_CTRL_CMD_PERF_GET_LEVEL_INFO_V2, &p);
    if (st == 0 && p.perfGetClkInfoListSize >= 2) {
        nvx_emit(c, "V2.15", "perf-level-info-v2", st, NVX_V_OK,
                 "lvl%u dom%u cur %u kHz (min %u max %u) | dom%u cur %u kHz (min %u max %u)",
                 p.level,
                 p.perfGetClkInfoList[0].domain, p.perfGetClkInfoList[0].currentFreq,
                 p.perfGetClkInfoList[0].minFreq, p.perfGetClkInfoList[0].maxFreq,
                 p.perfGetClkInfoList[1].domain, p.perfGetClkInfoList[1].currentFreq,
                 p.perfGetClkInfoList[1].minFreq, p.perfGetClkInfoList[1].maxFreq);
    } else {
        nvx_emit(c, "V2.15", "perf-level-info-v2", st,
                 st == 0 ? NVX_V_OK : NVX_V_REJECTED,
                 st == 0 ? "listSize %u" : "-", st == 0 ? p.perfGetClkInfoListSize : 0);
    }
}

static void item_i2c_probe(nvx_ctx *c)
{
    /* Dispatch-reachability probe with zeroed params: an I2C transaction on
     * port 0 with no entries should be rejected by argument validation if the
     * FINN I2C family is wired up — any well-formed NV_ERR_* is informative,
     * silence (transport failure) is not expected. READ face only. */
    NV2080_CTRL_I2C_READ_BUFFER_PARAMS p;
    memset(&p, 0, sizeof p);
    NvU32 st = SUBCTL(c, NV2080_CTRL_CMD_I2C_READ_BUFFER, &p);
    nvx_emit(c, "V2.16", "i2c-read-buffer", st,
             st == 0 ? NVX_V_OK : NVX_V_REJECTED,
             "zeroed-params probe (fan/sensor I2C reachability)");
}

static void item_rusd(nvx_ctx *c)
{
    if (c->dev_fd < 0) {
        nvx_emit(c, "V2.12", "rusd-page", 0, NVX_V_SKIP, "no device connection");
        return;
    }
    NvU32 st = 0;
    NV00DE_ALLOC_PARAMETERS ap;
    memset(&ap, 0, sizeof ap);
    ap.polledDataMask = NVX_RUSD_POLL_MASK;
    NvHandle hRusd = nvx_rm_alloc(c->ctl_fd, c->hClient, c->hSubdevice,
                                  0xcafe0003, NVX_CLASS_USER_SHARED_DATA,
                                  &ap, sizeof ap, &st);
    if (!hRusd) {
        nvx_emit(c, "V2.12", "rusd-page", st, NVX_V_REJECTED,
                 "NV00DE alloc rejected");
        return;
    }

    NvU64 len = 0;
    /* RUSD is SYSMEM: RM binds its mapping context to the control fd. */
    void *page = nvx_map_memory_readonly(c->ctl_fd, c->ctl_fd, c->hClient, c->hSubdevice,
                                         hRusd, sizeof(NV00DE_SHARED_DATA), &len, &st);
    if (!page) {
        nvx_emit(c, "V2.12", "rusd-page", st, NVX_V_REJECTED,
                 "NV00DE alloc ok (0x%08x) but read-only mapping failed (RM status or transport/IOKit failure)",
                 hRusd);
        nvx_rm_free(c->ctl_fd, c->hClient, c->hSubdevice, hRusd);
        return;
    }
    if (len < sizeof(NV00DE_SHARED_DATA)) {
        nvx_emit(c, "V2.12", "rusd-page", 0, NVX_V_REJECTED,
                 "mapping too short: %llu bytes, need %zu",
                 (unsigned long long)len, sizeof(NV00DE_SHARED_DATA));
        return;
    }
    c->rusd = page;
    c->rusd_len = len;
    nvx_emit(c, "V2.12", "rusd-page", 0, NVX_V_OK,
             "NV00DE 0x%08x mapped at %p len %lluKB — poll mask 0x%x (field validity checked separately)",
             hRusd, page, (unsigned long long)(len >> 10), NVX_RUSD_POLL_MASK);

    /* Full RUSD readout: every polled section with a stable timestamp. */
    {
        NV00DE_SHARED_DATA *sd = (NV00DE_SHARED_DATA *)page;
        RUSD_CLK_PUBLIC_DOMAIN_INFOS clks;
        RUSD_PERF_CURRENT_PSTATE pst;
        RUSD_POWER_LIMITS pl;
        RUSD_INST_POWER_USAGE ipw;
        RUSD_AVG_POWER_USAGE apw;
        RUSD_POWER_POLICY_STATUS pol;
        RUSD_FAN_COOLER_STATUS fan;
        RUSD_TEMPERATURE temps[RUSD_TEMPERATURE_TYPE_MAX];
        char tbuf[96];
        size_t o = 0;
        tbuf[0] = 0;

        for (int i = 0; i < RUSD_TEMPERATURE_TYPE_MAX; i++) {
            if (rusd_copy(&sd->temperatures[i].lastModifiedTimestamp,
                          &sd->temperatures[i], &temps[i], sizeof temps[i]) != 0)
                continue;
            int n = snprintf(tbuf + o, sizeof tbuf - o, "%s%s=%ld.%ldC",
                             o ? " " : "",
                             i == RUSD_TEMPERATURE_TYPE_GPU ? "gpu" :
                             i == RUSD_TEMPERATURE_TYPE_MEMORY ? "mem" :
                             i == RUSD_TEMPERATURE_TYPE_BOARD ? "board" :
                             i == RUSD_TEMPERATURE_TYPE_POWER_SUPPLY ? "psu" : "hbm",
                             (long)temps[i].temperature >> 8,
                             ((long)temps[i].temperature & 0xff) * 100 / 256);
            if (n > 0 && o + n < sizeof tbuf) o += (size_t)n;
        }

        long fanMax = -1, instMW = -1, avgMW = -1, enforcedMW = -1, tgpmW = -1;
        NvU32 pstate = 0xffffffff;
        NvU32 gpcKHz = 0, memKHz = 0, smKHz = 0;
        if (rusd_copy(&sd->fanCoolerStatus.lastModifiedTimestamp,
                      &sd->fanCoolerStatus, &fan, sizeof fan) == 0)
            for (unsigned i = 0; i < RUSD_FAN_COOLER_MAX_COOLERS; i++)
                if ((long)fan.info.rpmCurr[i] > fanMax) fanMax = (long)fan.info.rpmCurr[i];
        if (rusd_copy(&sd->instPowerUsage.lastModifiedTimestamp,
                      &sd->instPowerUsage, &ipw, sizeof ipw) == 0)
            instMW = (long)ipw.info.instGpuPower;
        if (rusd_copy(&sd->avgPowerUsage.lastModifiedTimestamp,
                      &sd->avgPowerUsage, &apw, sizeof apw) == 0)
            avgMW = (long)apw.info.averageGpuPower;
        if (rusd_copy(&sd->powerLimitGpu.lastModifiedTimestamp,
                      &sd->powerLimitGpu, &pl, sizeof pl) == 0)
            enforcedMW = (long)pl.info.enforcedmW;
        if (rusd_copy(&sd->powerPolicyStatus.lastModifiedTimestamp,
                      &sd->powerPolicyStatus, &pol, sizeof pol) == 0)
            tgpmW = (long)pol.info.tgpmW;
        if (rusd_copy(&sd->perfCurrentPstate.lastModifiedTimestamp,
                      &sd->perfCurrentPstate, &pst, sizeof pst) == 0)
            pstate = pst.currentPstate;
        if (rusd_copy(&sd->clkPublicDomainInfos.lastModifiedTimestamp,
                      &sd->clkPublicDomainInfos, &clks, sizeof clks) == 0) {
            gpcKHz = clks.info[RUSD_CLK_PUBLIC_DOMAIN_GRAPHICS].actualClkKHz;
            memKHz = clks.info[RUSD_CLK_PUBLIC_DOMAIN_MEMORY].actualClkKHz;
            smKHz  = clks.info[RUSD_CLK_PUBLIC_DOMAIN_SM].actualClkKHz;
        }

        int valid = tbuf[0] || fanMax >= 0 || instMW >= 0 || avgMW >= 0 ||
                    enforcedMW >= 0 || tgpmW >= 0 || pstate != 0xffffffffu ||
                    gpcKHz || memKHz || smKHz;
        nvx_emit(c, "V2.12b", "rusd-readout", 0, valid ? NVX_V_OK : NVX_V_SKIP,
                 "temp[%s] pstate %u gpc %u mem %u sm %u kHz inst %ld avg %ld mW "
                 "limit %ld tgp %ld mW fan %ld rpm",
                 tbuf, pstate, gpcKHz, memKHz, smKHz,
                 instMW, avgMW, enforcedMW, tgpmW, fanMax);
    }

    /* Keep the page alive for stage 3/4 sampling; connection teardown reclaims it. */
}

int nvx_stage2(nvx_ctx *c)
{
    if (!c->hSubdevice) {
        nvx_emit(c, "V2", "battery", 0, NVX_V_SKIP, "RM chain not open");
        return -1;
    }
    item_fb_info(c);
    item_fb_stats(c);
    item_pstate(c);
    item_powerstate_get(c);
    item_rated_tdp_get(c);
    item_edpp_get(c, NULL);
    item_gpumon(c);
    item_pmumon(c);
    item_bios_sku(c);
    item_bus(c);
    item_perf_levels(c);
    if (NVX_IDLE_ONLY) {
        nvx_emit(c, "V2.16", "i2c-read-buffer", 0, NVX_V_SKIP,
                 "idle-only build: I2C reachability probe disabled");
        nvx_emit(c, "V2.legacy", "legacy-probes", 0, NVX_V_SKIP,
                 "idle-only build: legacy command probes disabled");
    } else {
        item_i2c_probe(c);
        item_legacy_probes(c);
    }
    item_rusd(c);
    return 0;
}

/* ------------------------------------------------------------------ */
/* stage 3 — gated write tests                                         */
/* ------------------------------------------------------------------ */

static nvx_ctx *g_boost_ctx;

static void boost_clear(nvx_ctx *c)
{
    NV2080_CTRL_PERF_BOOST_PARAMS bp;
    memset(&bp, 0, sizeof bp);
    bp.flags = NV2080_CTRL_PERF_BOOST_FLAGS_CMD_CLEAR |
               (NV2080_CTRL_PERF_BOOST_FLAGS_CUDA_YES << 4);
    bp.duration = NV2080_CTRL_PERF_BOOST_DURATION_INFINITE;
    NvU32 st = SUBCTL(c, NV2080_CTRL_CMD_PERF_BOOST, &bp);
    printf("[..] boost clear -> 0x%x (%s)\n", st, nvx_status_name(st));
}

void nvx_restore_arm(nvx_ctx *c) { g_boost_ctx = c; }

void nvx_restore_run(void)
{
    if (g_boost_ctx && g_boost_ctx->hSubdevice) {
        printf("[..] restoring: clearing PERF_BOOST latched state\n");
        boost_clear(g_boost_ctx);
        g_boost_ctx = NULL;
    }
}

int nvx_stage3(nvx_ctx *c)
{
    if (NVX_IDLE_ONLY) {
        nvx_emit(c, "V3", "writes", 0, NVX_V_GATED,
                 "idle-only build: all performance writes disabled");
        return -1;
    }
    if (!c->hSubdevice) {
        nvx_emit(c, "V3", "writes", 0, NVX_V_SKIP, "RM chain not open");
        return -1;
    }

    /* --- V3.1 PERF_BOOST cycle (the NVK-proven write) ---
     * One-step boost first, then boost-to-max; every phase is cleared. */
    {
        nvx_emit(c, "V3.1", "perf-boost-cycle", 0, NVX_V_OK, "sampling baseline...");
        nvx_sample_once(c, "pre");

        NV2080_CTRL_PERF_BOOST_PARAMS bp;
        memset(&bp, 0, sizeof bp);
        bp.flags = NV2080_CTRL_PERF_BOOST_FLAGS_CMD_BOOST_1LEVEL |
                   (NV2080_CTRL_PERF_BOOST_FLAGS_CUDA_YES << 4);
        bp.duration = NV2080_CTRL_PERF_BOOST_DURATION_INFINITE;
        NvU32 st = SUBCTL(c, NV2080_CTRL_CMD_PERF_BOOST, &bp);
        nvx_emit(c, "V3.1a", "perf-boost-1level", st,
                 st == 0 ? NVX_V_OK : NVX_V_REJECTED,
                 st == 0 ? "BOOST_1LEVEL accepted" : "-");
        if (st == 0) { sleep(2); nvx_sample_once(c, "boost1"); boost_clear(c); sleep(1); }

        memset(&bp, 0, sizeof bp);
        bp.flags = NV2080_CTRL_PERF_BOOST_FLAGS_CMD_BOOST_TO_MAX |
                   (NV2080_CTRL_PERF_BOOST_FLAGS_CUDA_YES << 4);
        bp.duration = NV2080_CTRL_PERF_BOOST_DURATION_INFINITE;
        st = SUBCTL(c, NV2080_CTRL_CMD_PERF_BOOST, &bp);
        nvx_emit(c, "V3.1b", "perf-boost-to-max", st,
                 st == 0 ? NVX_V_OK : NVX_V_REJECTED,
                 st == 0 ? "BOOST_TO_MAX accepted by GSP — the §3.4 OS-privilege "
                           "question is answered YES" : "-");
        if (st == 0) {
            for (int i = 0; i < 5; i++) { sleep(1); nvx_sample_once(c, "boost"); }
            boost_clear(c);
            sleep(1);
            nvx_sample_once(c, "clear");
        }
    }

    /* --- V3.2 power-state roundtrip (write-back the value just read) --- */
    {
        NV2080_CTRL_PERF_GET_POWERSTATE_PARAMS g;
        memset(&g, 0, sizeof g);
        NvU32 st = SUBCTL(c, NV2080_CTRL_CMD_PERF_GET_POWERSTATE, &g);
        if (st != 0) {
            nvx_emit(c, "V3.2", "powerstate-roundtrip", st, NVX_V_REJECTED, "GET failed");
        } else {
            NV2080_CTRL_PERF_SET_POWERSTATE_PARAMS s;
            memset(&s, 0, sizeof s);
            s.powerStateInfo.powerState = g.powerStateInfo.powerState;
            st = SUBCTL(c, NV2080_CTRL_CMD_PERF_SET_POWERSTATE, &s);
            NvU32 st2 = SUBCTL(c, NV2080_CTRL_CMD_PERF_GET_POWERSTATE, &g);
            nvx_emit(c, "V3.2", "powerstate-roundtrip", st,
                     st == 0 ? NVX_V_OK : NVX_V_REJECTED,
                     st == 0 ? "SET(current=%u) accepted; verify GET 0x%x"
                             : "-",
                     s.powerStateInfo.powerState, st2 == 0 ? g.powerStateInfo.powerState : 0xffffffffu);
        }
    }

    /* --- V3.3 EDPp limit roundtrip (write-back current limit) --- */
    {
        NV2080_CTRL_CMD_INTERNAL_PMGR_PFM_REQ_HNDLR_GET_EDPP_LIMIT_INFO_PARAMS g;
        memset(&g, 0, sizeof g);
        NvU32 st = SUBCTL(c, NV2080_CTRL_CMD_INTERNAL_PMGR_PFM_REQ_HNDLR_GET_EDPP_LIMIT_INFO, &g);
        if (st != 0) {
            nvx_emit(c, "V3.3", "edpp-roundtrip", st, NVX_V_REJECTED, "GET failed");
        } else {
            NV2080_CTRL_CMD_INTERNAL_PMGR_PFM_REQ_HNDLR_UPDATE_EDPP_LIMIT_PARAMS u;
            memset(&u, 0, sizeof u);
            u.bEnable = NV_TRUE;
            u.clientLimit = g.limitCurr;
            st = SUBCTL(c, NV2080_CTRL_CMD_INTERNAL_PMGR_PFM_REQ_HNDLR_UPDATE_EDPP_LIMIT, &u);
            NvU32 st2 = 0;
            NV2080_CTRL_CMD_INTERNAL_PMGR_PFM_REQ_HNDLR_GET_EDPP_LIMIT_INFO_PARAMS g2;
            memset(&g2, 0, sizeof g2);
            st2 = SUBCTL(c, NV2080_CTRL_CMD_INTERNAL_PMGR_PFM_REQ_HNDLR_GET_EDPP_LIMIT_INFO, &g2);
            nvx_emit(c, "V3.3", "edpp-roundtrip", st,
                     st == 0 ? NVX_V_OK : NVX_V_REJECTED,
                     st == 0 ? "UPDATE(curr=%u) accepted; verify GET curr=%u"
                             : "-",
                             u.clientLimit, st2 == 0 ? g2.limitCurr : 0xffffffffu);
        }
    }

    nvx_restore_run();
    return 0;
}

/* ------------------------------------------------------------------ */
/* stage 4 — sampling loop                                             */
/* ------------------------------------------------------------------ */

void nvx_stage4(nvx_ctx *c, int seconds)
{
    if (!c->hSubdevice) return;
    printf("-- sampling %d s (1 Hz)\n", seconds);
    for (int i = 0; i < seconds; i++) {
        nvx_sample_once(c, "loop");
        sleep(1);
    }
}

/* ------------------------------------------------------------------ */
/* stage 5 — channel smoke: make the GPU execute a pushbuffer          */
/*                                                                     */
/* Port of NullMoth's own push_test (kexts/NVRM/push.inc): sysmem/VRAM */
/* buffers -> FERMI_VASPACE -> NV01_CONTEXT_DMA notifier -> GPFIFO     */
/* channel -> BIND/SCHEDULE -> semaphore release via USERD GP put +    */
/* TURING_USERMODE_A doorbell. PASS = the GPU executed work under      */
/* macOS through our private client.                                   */
/* ------------------------------------------------------------------ */

struct nvx_buf {
    NvHandle hPhys, hVirt;
    NvU64 gpuva;
    volatile uint8_t *cpu;
    NvU64 size;
};

static NvHandle g_smoke_next = 0xcafe0100;

static int smoke_alloc_buf(nvx_ctx *c, NvHandle hVaSpace, int vidmem,
                           NvU64 size, const char *what, struct nvx_buf *b,
                           NvU32 *st)
{
    memset(b, 0, sizeof *b);
    b->size = size;
    NV_MEMORY_ALLOCATION_PARAMS mp;
    memset(&mp, 0, sizeof mp);
    mp.owner = c->hClient;
    mp.type = NVOS32_TYPE_IMAGE;
    mp.size = size;
    mp.attr = DRF_DEF(OS32, _ATTR, _PAGE_SIZE, _4KB) |
              (vidmem ? (DRF_DEF(OS32, _ATTR, _LOCATION, _VIDMEM) |
                         DRF_DEF(OS32, _ATTR, _COHERENCY, _UNCACHED))
                      : (DRF_DEF(OS32, _ATTR, _LOCATION, _PCI) |
                         DRF_DEF(OS32, _ATTR, _COHERENCY, _CACHED)));
    if (vidmem) mp.flags |= NVOS32_ALLOC_FLAGS_PERSISTENT_VIDMEM;
    b->hPhys = nvx_rm_alloc(c->ctl_fd, c->hClient, c->hDevice, g_smoke_next++,
                            vidmem ? NVX_CLASS_MEMORY_LOCAL_USER
                                   : NVX_CLASS_MEMORY_SYSTEM,
                            &mp, sizeof mp, st);
    if (!b->hPhys) return -1;

    b->cpu = nvx_map_memory(c->ctl_fd, c->dev_fd, c->hClient, c->hSubdevice,
                            b->hPhys, size, NULL);
    if (!b->cpu) return -1;

    NV_MEMORY_ALLOCATION_PARAMS vp;
    memset(&vp, 0, sizeof vp);
    vp.owner = c->hClient;
    vp.type = NVOS32_TYPE_IMAGE;
    vp.flags = NVOS32_ALLOC_FLAGS_VIRTUAL;
    vp.size = size;
    vp.hVASpace = hVaSpace;
    b->hVirt = nvx_rm_alloc(c->ctl_fd, c->hClient, c->hDevice, g_smoke_next++,
                            NVX_CLASS_MEMORY_VIRTUAL_NV50, &vp, sizeof vp, st);
    if (!b->hVirt) return -1;
    b->gpuva = vp.offset;   /* RM fills in the allocated VA */

    NVOS46_PARAMETERS d;
    memset(&d, 0, sizeof d);
    d.hClient = c->hClient;
    d.hDevice = c->hDevice;
    d.hDma = b->hVirt;
    d.hMemory = b->hPhys;
    d.offset = 0;
    d.length = size;
    d.flags = DRF_DEF(OS46, _FLAGS, _PAGE_KIND, _VIRTUAL) |
              (vidmem ? DRF_DEF(OS46, _FLAGS, _CACHE_SNOOP, _DISABLE)
                      : DRF_DEF(OS46, _FLAGS, _CACHE_SNOOP, _ENABLE));
    d.dmaOffset = 0;
    if (nvx_esc(c->ctl_fd, NV_ESC_RM_MAP_MEMORY_DMA, &d, sizeof d) != 0 || d.status != 0)
        return -1;
    return 0;
}

static void smoke_free_buf(nvx_ctx *c, struct nvx_buf *b)
{
    if (b->hVirt) nvx_rm_free(c->ctl_fd, c->hClient, c->hDevice, b->hVirt);
    if (b->hPhys) nvx_rm_free(c->ctl_fd, c->hClient, c->hDevice, b->hPhys);
    memset(b, 0, sizeof *b);
}

int nvx_stage5(nvx_ctx *c)
{
    if (NVX_IDLE_ONLY) {
        nvx_emit(c, "V5", "channel-smoke", 0, NVX_V_GATED,
                 "idle-only build: GPU channel submission disabled");
        return -1;
    }
    if (!c->hSubdevice || c->dev_fd < 0) {
        nvx_emit(c, "V5", "channel-smoke", 0, NVX_V_SKIP,
                 "needs RM chain + device connection");
        return -1;
    }

    /* pick the newest GPFIFO channel class from the classlist (0x??6f) */
    static NV0080_CTRL_GPU_GET_CLASSLIST_V2_PARAMS cls;
    memset(&cls, 0, sizeof cls);
    NvU32 st = nvx_rm_control(c->ctl_fd, c->hClient, c->hDevice,
                              NV0080_CTRL_CMD_GPU_GET_CLASSLIST_V2, &cls, sizeof cls);
    NvU32 chanClass = 0;
    if (st == 0)
        for (NvU32 i = 0; i < cls.numClasses && i < NV0080_CTRL_GPU_CLASSLIST_MAX_SIZE; i++)
            if ((cls.classList[i] & 0xff) == 0x6f && cls.classList[i] > chanClass)
                chanClass = cls.classList[i];
    if (!chanClass) {
        nvx_emit(c, "V5.0", "pick-channel-class", st, NVX_V_REJECTED,
                 "no ??6f GPFIFO class in classlist");
        return -1;
    }
    nvx_emit(c, "V5.0", "pick-channel-class", 0, NVX_V_OK,
             "channel class 0x%x", chanClass);

    /* usermode doorbell page (subdevice-parented, 4 KB) */
    NvHandle hUm = nvx_rm_alloc(c->ctl_fd, c->hClient, c->hSubdevice,
                                g_smoke_next++, NVX_CLASS_USERMODE_TURING_A,
                                NULL, 0, &st);
    if (!hUm) {
        nvx_emit(c, "V5.1", "alloc-usermode", st, NVX_V_REJECTED,
                 "TURING_USERMODE_A rejected");
        return -1;
    }
    volatile uint8_t *umap = nvx_map_memory(c->ctl_fd, c->dev_fd, c->hClient,
                                            c->hSubdevice, hUm, 0x1000, NULL);
    if (!umap) {
        nvx_emit(c, "V5.1", "alloc-usermode", 0, NVX_V_REJECTED,
                 "TURING_USERMODE_A 0x%08x alloc ok, map failed", hUm);
        return -1;
    }
    nvx_emit(c, "V5.1", "alloc-usermode", 0, NVX_V_OK,
             "doorbell page mapped at %p", (const void *)umap);

    /* address space */
    NV_VASPACE_ALLOCATION_PARAMETERS vap;
    memset(&vap, 0, sizeof vap);
    vap.flags = NV_VASPACE_ALLOCATION_FLAGS_RETRY_PTE_ALLOC_IN_SYS;
    NvHandle hVa = nvx_rm_alloc(c->ctl_fd, c->hClient, c->hDevice,
                                g_smoke_next++, NVX_CLASS_VASPACE_FERMI_A,
                                &vap, sizeof vap, &st);
    if (!hVa) {
        nvx_emit(c, "V5.2", "alloc-vaspace", st, NVX_V_REJECTED,
                 "FERMI_VASPACE_A rejected");
        return -1;
    }
    nvx_emit(c, "V5.2", "alloc-vaspace", 0, NVX_V_OK, "handle 0x%08x", hVa);

    /* buffers (push.inc sizes) */
    struct nvx_buf notifier, userd, gpfifo, cmdbuf, sem;
    int fail = 0;
    fail |= smoke_alloc_buf(c, hVa, 0, 0x1000,  "notifier", &notifier, &st) != 0;
    fail |= smoke_alloc_buf(c, hVa, 1, 0x80000, "userd",    &userd,    &st) != 0;
    fail |= smoke_alloc_buf(c, hVa, 0, 0x40000, "gpfifo",   &gpfifo,   &st) != 0;
    fail |= smoke_alloc_buf(c, hVa, 0, 0x10000, "cmdbuf",   &cmdbuf,   &st) != 0;
    fail |= smoke_alloc_buf(c, hVa, 0, 0x1000,  "sem",      &sem,      &st) != 0;
    if (fail) {
        nvx_emit(c, "V5.3", "alloc-buffers", st, NVX_V_REJECTED,
                 "one or more buffers failed (see kext log)");
        return -1;
    }
    nvx_emit(c, "V5.3", "alloc-buffers", 0, NVX_V_OK,
             "notifier/userd(vidmem)/gpfifo/cmdbuf/sem mapped, sem VA 0x%llx",
             (unsigned long long)sem.gpuva);
    memset((void *)sem.cpu, 0, 0x1000);
    memset((void *)notifier.cpu, 0, 0x1000);

    /* notifier context DMA */
    NV_CONTEXT_DMA_ALLOCATION_PARAMS cd;
    memset(&cd, 0, sizeof cd);
    cd.flags = DRF_DEF(OS03, _FLAGS, _MAPPING, _KERNEL) |
               DRF_DEF(OS03, _FLAGS, _HASH_TABLE, _DISABLE);
    cd.hMemory = notifier.hPhys;
    cd.offset = 0;
    cd.limit = notifier.size - 1;
    NvHandle hCtxDma = nvx_rm_alloc(c->ctl_fd, c->hClient, c->hDevice,
                                    g_smoke_next++, NVX_CLASS_CONTEXT_DMA,
                                    &cd, sizeof cd, &st);
    if (!hCtxDma) {
        nvx_emit(c, "V5.4", "alloc-ctxdma", st, NVX_V_REJECTED,
                 "NV01_CONTEXT_DMA rejected");
        return -1;
    }

    /* channel */
    NV_CHANNEL_ALLOC_PARAMS cp;
    memset(&cp, 0, sizeof cp);
    cp.hObjectError = hCtxDma;
    cp.gpFifoOffset = gpfifo.gpuva;
    cp.gpFifoEntries = 0x8000;
    cp.flags = 0;
    cp.hVASpace = hVa;
    cp.hUserdMemory[0] = userd.hPhys;
    cp.userdOffset[0] = 0;
    cp.engineType = NVX_ENGINE_TYPE_GRAPHICS;
    NvHandle hChan = nvx_rm_alloc(c->ctl_fd, c->hClient, c->hDevice,
                                  g_smoke_next++, chanClass, &cp, sizeof cp, &st);
    if (!hChan) {
        nvx_emit(c, "V5.5", "alloc-channel", st, NVX_V_REJECTED,
                 "channel class 0x%x rejected", chanClass);
        return -1;
    }
    nvx_emit(c, "V5.5", "alloc-channel", 0, NVX_V_OK,
             "channel 0x%08x (class 0x%x)", hChan, chanClass);

    NVA06F_CTRL_BIND_PARAMS bp;
    memset(&bp, 0, sizeof bp);
    bp.engineType = NVX_ENGINE_TYPE_GRAPHICS;
    st = nvx_rm_control(c->ctl_fd, c->hClient, hChan, NVA06F_CTRL_CMD_BIND,
                        &bp, sizeof bp);
    nvx_emit(c, "V5.6", "channel-bind", st,
             st == 0 ? NVX_V_OK : NVX_V_REJECTED, "GRAPHICS engine");

    NVA06F_CTRL_GPFIFO_SCHEDULE_PARAMS sp;
    memset(&sp, 0, sizeof sp);
    sp.bEnable = NV_TRUE;
    st = nvx_rm_control(c->ctl_fd, c->hClient, hChan, NVA06F_CTRL_CMD_GPFIFO_SCHEDULE,
                        &sp, sizeof sp);
    nvx_emit(c, "V5.7", "gpfifo-schedule", st,
             st == 0 ? NVX_V_OK : NVX_V_REJECTED, "-");
    if (st != 0) return -1;

    NVC36F_CTRL_GPFIFO_SET_WORK_SUBMIT_TOKEN_NOTIF_INDEX_PARAMS ni;
    memset(&ni, 0, sizeof ni);
    ni.index = NV_CHANNELGPFIFO_NOTIFICATION_TYPE_WORK_SUBMIT_TOKEN;
    (void)nvx_rm_control(c->ctl_fd, c->hClient, hChan,
                         NVC36F_CTRL_CMD_GPFIFO_SET_WORK_SUBMIT_TOKEN_NOTIF_INDEX,
                         &ni, sizeof ni);
    NVC36F_CTRL_CMD_GPFIFO_GET_WORK_SUBMIT_TOKEN_PARAMS tk;
    memset(&tk, 0, sizeof tk);
    st = nvx_rm_control(c->ctl_fd, c->hClient, hChan,
                        NVC36F_CTRL_CMD_GPFIFO_GET_WORK_SUBMIT_TOKEN, &tk, sizeof tk);
    if (st != 0) {
        nvx_emit(c, "V5.8", "work-submit-token", st, NVX_V_REJECTED, "-");
        return -1;
    }
    nvx_emit(c, "V5.8", "work-submit-token", 0, NVX_V_OK,
             "token 0x%08x", tk.workSubmitToken);

    /* pushbuffer: SEMAPHORE method 0x5c releases value 1 at sem.gpuva */
    volatile uint32_t *pb = (volatile uint32_t *)cmdbuf.cpu;
    pb[0] = 0x20000000u | (5u << 16) | (0u << 13) | (0x5cu >> 2);
    pb[1] = (uint32_t)sem.gpuva & ~3u;
    pb[2] = (uint32_t)(sem.gpuva >> 32) & 0xffu;
    pb[3] = 1u;
    pb[4] = 0u;
    pb[5] = 1u | (1u << 20) | (1u << 24);
    const uint32_t ndw = 6;
    volatile uint32_t *gp = (volatile uint32_t *)gpfifo.cpu;
    gp[0] = (uint32_t)cmdbuf.gpuva & ~3u;
    gp[1] = ((uint32_t)(cmdbuf.gpuva >> 32) & 0xffu) | (ndw << 10);
    __sync_synchronize();
    volatile uint32_t *ud = (volatile uint32_t *)userd.cpu;
    ud[NVX_USERD_GPPUT / 4] = 1;
    __sync_synchronize();
    *(volatile uint32_t *)(umap + NVC361_NOTIFY_CHANNEL_PENDING) = tk.workSubmitToken;
    printf("  doorbell rung — waiting for the GPU semaphore...\n");

    volatile uint64_t *semv = (volatile uint64_t *)sem.cpu;
    int ok = 0;
    for (int i = 0; i < 5000; i++) {
        if (*semv == 1) { ok = 1; break; }
        usleep(1000);
    }
    nvx_emit(c, "V5.9", "gpu-semaphore", 0,
             ok ? NVX_V_OK : NVX_V_REJECTED,
             ok ? "GPU executed the pushbuffer under macOS (semaphore==1, <5s) "
                  "— engine submission path fully alive"
                : "timeout: USERD GPGet=%u GPPut=%u",
             ok ? 0 : ud[NVX_USERD_GPGET / 4], ok ? 0 : ud[NVX_USERD_GPPUT / 4]);

    /* teardown of channel objects (buffers/mappings die with the connection) */
    nvx_rm_free(c->ctl_fd, c->hClient, c->hDevice, hChan);
    nvx_rm_free(c->ctl_fd, c->hClient, c->hDevice, hCtxDma);
    nvx_rm_free(c->ctl_fd, c->hClient, c->hDevice, hVa);
    nvx_rm_free(c->ctl_fd, c->hClient, c->hSubdevice, hUm);
    smoke_free_buf(c, &notifier);
    smoke_free_buf(c, &userd);
    smoke_free_buf(c, &gpfifo);
    smoke_free_buf(c, &cmdbuf);
    smoke_free_buf(c, &sem);
    return ok ? 0 : -1;
}
