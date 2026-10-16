/*
 * probe-macos-rm — staged validation suite.
 *
 * Stage 0: IOKit transport (service presence, open, card info, register fd)
 * Stage 1: RM object chain (root client / attach / device / subdevice / classlist)
 * Stage 2: read-only telemetry battery (OGKM in-tree FINN controls + RUSD page
 *          + campaign-sourced legacy GET probes)
 * Stage 3: gated write tests (PERF_BOOST cycle, power-state roundtrip,
 *          EDPp limit roundtrip) — requires --write
 * Stage 4: telemetry sampling loop (feeds JSONL for before/after diffing)
 *
 * Every item emits a nvx_res_t record; main() renders text and JSONL.
 */
#ifndef NVX_SUITE_H
#define NVX_SUITE_H

#include "nvtypes.h"
#include <stdio.h>

#ifndef NVX_IDLE_ONLY
#define NVX_IDLE_ONLY 1
#endif

enum {
    NVX_V_OK = 0,      /* NV_OK, values plausible            */
    NVX_V_REJECTED,    /* call completed, RM returned error  */
    NVX_V_TRANSPORT,   /* IOKit/esc layer failure            */
    NVX_V_SKIP,        /* prerequisite missing               */
    NVX_V_GATED,       /* needs --write                      */
    NVX_V_HARD         /* chain-breaking failure             */
};

typedef struct {
    char id[16];
    char name[48];
    NvU32 status;
    int   verdict;
    char  detail[288];
} nvx_res_t;

typedef struct nvx_ctx {
    int     ctl_fd, dev_fd;
    NvHandle hClient, hDevice, hSubdevice;
    NvU32   gpuId;
    void   *rusd;        /* mapped NV00DE_SHARED_DATA page, or NULL */
    NvU64   rusd_len;
    /* output */
    FILE   *jsonl;
    int     quiet;
} nvx_ctx;

void nvx_emit(nvx_ctx *c, const char *id, const char *name,
              NvU32 status, int verdict, const char *fmt, ...)
              __attribute__((format(printf, 6, 7)));

/* One telemetry snapshot (text line + jsonl "sample" record). */
void nvx_sample_once(nvx_ctx *c, const char *tag);

int nvx_stage0(nvx_ctx *c);                    /* transport                */
int nvx_stage1(nvx_ctx *c, NvU32 gpuId);       /* RM chain                 */
int nvx_stage2(nvx_ctx *c);                    /* read-only battery        */
int nvx_stage3(nvx_ctx *c);                    /* gated writes             */
void nvx_stage4(nvx_ctx *c, int seconds);      /* sampling loop            */
int nvx_stage5(nvx_ctx *c);                    /* channel smoke (GPU exec) */

/* Restore path: clear a latched boost if stage3 armed one (atexit/SIGINT). */
void nvx_restore_arm(nvx_ctx *c);
void nvx_restore_run(void);

#endif /* NVX_SUITE_H */
