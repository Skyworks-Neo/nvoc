/*
 * probe-macos-rm — entry point.
 *
 * The default build permits idle queries only. The full profile additionally
 * contains memory smoke, gated performance writes, and GPU channel submission.
 */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <signal.h>
#include <unistd.h>
#include <errno.h>
#include <limits.h>

#include "nvtypes.h"
#include "nvx_suite.h"
#include "nvx_xnu.h"

static nvx_ctx g_ctx;

static int ensure_transport(void)
{
    return g_ctx.ctl_fd >= 0 ? 0 : nvx_stage0(&g_ctx);
}

static int ensure_chain(NvU32 gpuId, int explicit_gpu)
{
    if (ensure_transport() != 0) return -1;
    if (g_ctx.hSubdevice) return 0;
    return nvx_stage1(&g_ctx, explicit_gpu ? gpuId : g_ctx.gpuId);
}

static int parse_number(const char *raw, unsigned long long max,
                        unsigned long long *out)
{
    if (!raw[0] || raw[0] < '0' || raw[0] > '9') return -1;
    char *end;
    errno = 0;
    unsigned long long n = strtoull(raw, &end, 0);
    if (errno || *end || n > max) return -1;
    *out = n;
    return 0;
}

static void on_signal(int sig)
{
    (void)sig;
    nvx_restore_run();
    _exit(130);
}

static void usage(void)
{
    printf(
        "probe-macos-rm — NullMoth NVRM kext RM-surface validation suite\n"
        "\n"
        "usage: probe-macos-rm [options]\n"
        "  --stages <set>   stage subset, e.g. 012 (default 012); 3=writes, 4=loop, 5=channel smoke\n"
        "  --write          allow stage-3 write tests (boost / powerstate / EDPp)\n"
        "  --sample <sec>   run the 1 Hz telemetry loop for <sec> seconds\n"
        "  --jsonl <file>   also write every result/sample as JSON lines\n"
        "  --gpu <id>       attach explicit gpuId (default: first from CARD_INFO)\n"
        "  --quiet          suppress text detail, keep JSONL\n"
        "  --help\n");
    if (NVX_IDLE_ONLY)
        printf("\nprofile: idle-only; --write and stages 3/5 are disabled; "
               "memory smoke, I2C and legacy probes are skipped.\n");
}

int main(int argc, char **argv)
{
    const char *stages = "012";
    const char *jsonl_path = NULL;
    int want_write = 0, sample_sec = 0, quiet = 0, explicit_gpu = 0;
    NvU32 gpuId = 0;

    for (int i = 1; i < argc; i++) {
        if (!strcmp(argv[i], "--stages") && i + 1 < argc) stages = argv[++i];
        else if (!strcmp(argv[i], "--write")) want_write = 1;
        else if (!strcmp(argv[i], "--sample") && i + 1 < argc) {
            unsigned long long value;
            if (parse_number(argv[++i], INT_MAX, &value) != 0 || value == 0) {
                fprintf(stderr, "invalid --sample duration\n"); return 2;
            }
            sample_sec = (int)value;
        }
        else if (!strcmp(argv[i], "--jsonl") && i + 1 < argc) jsonl_path = argv[++i];
        else if (!strcmp(argv[i], "--gpu") && i + 1 < argc) {
            unsigned long long value;
            if (parse_number(argv[++i], UINT32_MAX, &value) != 0) {
                fprintf(stderr, "invalid --gpu ID\n"); return 2;
            }
            gpuId = (NvU32)value;
            explicit_gpu = 1;
        }
        else if (!strcmp(argv[i], "--quiet")) quiet = 1;
        else if (!strcmp(argv[i], "--help")) { usage(); return 0; }
        else { usage(); return 2; }
    }

    if (!*stages || strspn(stages, "012345") != strlen(stages)) {
        fprintf(stderr, "invalid --stages set\n"); return 2;
    }
    if (NVX_IDLE_ONLY && (want_write || strpbrk(stages, "35"))) {
        fprintf(stderr, "idle-only build refuses --write and stages 3/5 "
                        "before opening the driver\n");
        return 2;
    }

    memset(&g_ctx, 0, sizeof g_ctx);
    g_ctx.ctl_fd = g_ctx.dev_fd = -1;
    g_ctx.quiet = quiet;
    if (jsonl_path) {
        g_ctx.jsonl = fopen(jsonl_path, "a");
        if (!g_ctx.jsonl) { perror("jsonl"); return 2; }
    }

    signal(SIGINT, on_signal);
    signal(SIGTERM, on_signal);
    atexit(nvx_restore_run);

    printf("== probe-macos-rm ==\n");

    int rc = 0;
    int gated3 = 0;
    int ran2 = 0;
    for (const char *p = stages; *p; p++) {
        switch (*p) {
        case '0':
            if (ensure_transport() != 0) { rc = 1; goto out; }
            break;
        case '1':
            if (ensure_chain(gpuId, explicit_gpu) != 0) { rc = 1; goto out; }
            break;
        case '2':
            if (ensure_chain(gpuId, explicit_gpu) != 0) { rc = 1; goto out; }
            if (!ran2 && nvx_stage2(&g_ctx) != 0) { rc = 1; goto out; }
            ran2 = 1;
            break;
        case '3':
            if (!want_write) {
                nvx_emit(&g_ctx, "V3", "writes", 0, NVX_V_GATED,
                         "write tests need --write (they touch perf state; "
                         "restore-on-exit is best-effort)");
                gated3 = 1;
                break;
            }
            if (ensure_chain(gpuId, explicit_gpu) != 0) { rc = 1; goto out; }
            nvx_restore_arm(&g_ctx);
            if (nvx_stage3(&g_ctx) != 0) { rc = 1; goto out; }
            break;
        case '4':
            if (sample_sec <= 0) sample_sec = 10;
            if (ensure_chain(gpuId, explicit_gpu) != 0) { rc = 1; goto out; }
            if (!ran2 && nvx_stage2(&g_ctx) != 0) { rc = 1; goto out; }
            ran2 = 1;
            nvx_stage4(&g_ctx, sample_sec);
            break;
        case '5':
            if (ensure_chain(gpuId, explicit_gpu) != 0) { rc = 1; goto out; }
            if (nvx_stage5(&g_ctx) != 0) rc = 1;
            break;
        default:
            fprintf(stderr, "unknown stage '%c'\n", *p);
            return 2;
        }
    }

    if (sample_sec > 0 && !strchr(stages, '4')) {
        if (ensure_chain(gpuId, explicit_gpu) != 0) { rc = 1; goto out; }
        if (!ran2 && nvx_stage2(&g_ctx) != 0) { rc = 1; goto out; }
        nvx_stage4(&g_ctx, sample_sec);
    }

out:
    nvx_restore_run();
    /* teardown: keep the RUSD mapping alive until the process dies; the kext
     * reclaims it with the connection. Close connections last. */
    if (g_ctx.dev_fd >= 0) nvx_close(g_ctx.dev_fd);
    if (g_ctx.ctl_fd >= 0) nvx_close(g_ctx.ctl_fd);

    if (rc == 0 && gated3) rc = 3;
    printf("== done (rc %d) ==\n", rc);
    if (g_ctx.jsonl) fclose(g_ctx.jsonl);
    return rc;
}
