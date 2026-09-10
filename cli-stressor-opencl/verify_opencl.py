"""Ride-on-load verification for the OpenCL stressor.

Mirrors the CUDA implementation (cli-stressor-cuda-rs verify_kernels.rs):
the checker rides the GEMM pipeline and inspects the buffers the stress ops
hammer, expected values are recomputed from the element index (no golden
buffer), per-check state is reduced device-side into a compact report, and
an injection self-test gate proves the detection pipeline before the run.

Pure-logic parts (hash, config, classification) are importable without
numpy/OpenCL for CI; the numpy dtype and pyopencl calls are only exercised
at runtime.

SEMANTICS NOTE: the OpenCL gemm kernel here is self-written textbook
row-major (C[i][j] = sum_k op_ta(A)[i][k] * op_tb(B)[k][j]) — unlike the
CUDA backend, there is no column-major pass-through, so the sampled
cross-check uses the same indices as the gemm kernel verbatim.
"""

import random
from dataclasses import dataclass, field
from enum import Enum

import numpy as np  # CI: a stub module is acceptable; only dtype helpers touch it

_MASK64 = (1 << 64) - 1
_MASK32 = (1 << 32) - 1


def vhash32(x: int) -> int:
    """splitmix64 finalizer, high 32 bits. Mirrors device vhash32()."""
    x &= _MASK64
    x ^= x >> 30
    x = (x * 0xBF58476D1CE4E5B9) & _MASK64
    x ^= x >> 27
    x = (x * 0x94D049BB133111EB) & _MASK64
    x ^= x >> 31
    return (x >> 32) & _MASK32


def vexpected_host(i: int, seed: int) -> int:
    """Expected pattern word for element i under seed. Mirror of vexpected()."""
    return vhash32((seed ^ (0x9E3779B97F4A7C15 * (i + 1))) & _MASK64)


VERIFY_REPORT_MAGIC = 0xADBA0000
SELF_TEST_IDX = 0xADBA
SELF_TEST_BIT = 22
SELF_TEST_SEED = 0xADBA
LOCAL_SIZE = 256
STATS_GRID_CAP = 4096
SAMPLE_SCRATCH_WORDS = 1 << 20  # self-test scratch: 4 MiB > the 0xADBA index
SAMPLE_CAP = 65536


def verify_report_dtype():
    """numpy dtype matching the device VerifyReport layout (176 bytes).

    OpenCL C lays structs out with natural alignment, so the u64 `checked`
    lands at offset 160 (after 28+128 bytes of u32s, padded). Explicit
    offsets keep the host/device contract exact regardless of numpy defaults.
    """
    import numpy as np

    return np.dtype(
        {
            "names": [
                "magic",
                "total_errors",
                "idx_min",
                "idx_max",
                "first_exp",
                "first_act",
                "first_lock",
                "bit_hist",
                "checked",
                "done",
                "_pad",
            ],
            "formats": [np.uint32] * 7
            + [(np.uint32, (32,)), np.uint64, np.uint32, np.uint32],
            "offsets": [0, 4, 8, 12, 16, 20, 24, 28, 160, 168, 172],
            "itemsize": 176,
        }
    )


def verify_report_init():
    import numpy as np

    dt = verify_report_dtype()
    row = np.zeros(1, dtype=dt)
    row["magic"] = VERIFY_REPORT_MAGIC
    row["idx_min"] = 0xFFFFFFFF
    row["first_lock"] = 0xFFFFFFFF
    return row


# ---------------------------------------------------------------------------
# OpenCL C sources
# ---------------------------------------------------------------------------

VERIFY_PATTERN_SRC = """
typedef struct VerifyReport {
    unsigned int magic;
    unsigned int total_errors;
    unsigned int idx_min;      // init 0xFFFFFFFF
    unsigned int idx_max;
    unsigned int first_exp;
    unsigned int first_act;
    unsigned int first_lock;   // init 0xFFFFFFFF
    unsigned int bit_hist[32];
    unsigned long checked;
    unsigned int done;
} VerifyReport;

unsigned int vhash32(unsigned long x) {
    x ^= x >> 30; x *= 0xBF58476D1CE4E5B9UL;
    x ^= x >> 27; x *= 0x94D049BB133111EBUL;
    x ^= x >> 31;
    return (unsigned int)(x >> 32);
}
unsigned int vexpected(unsigned long i, unsigned long seed) {
    return vhash32(seed ^ (0x9E3779B97F4A7C15UL * (i + 1)));
}

__kernel void verify_pattern_fill(
    __global unsigned int* buf, const unsigned long n, const unsigned long seed)
{
    unsigned long idx = (unsigned long)get_global_id(0);
    if (idx >= n) return;
    buf[idx] = vexpected(idx, seed);
}

__kernel void verify_compare(
    __global const unsigned int* buf, const unsigned long n,
    const unsigned long seed, __global VerifyReport* report)
{
    unsigned long idx = (unsigned long)get_global_id(0);
    if (idx >= n) return;
    if (get_local_id(0) == 0) atomic_inc(&report->done);
    unsigned int act = buf[idx];
    unsigned int exp = vexpected(idx, seed);
    if (act != exp) {
        unsigned int x = act ^ exp;
        atomic_inc(&report->total_errors);
        atomic_min(&report->idx_min, (unsigned int)idx);
        atomic_max(&report->idx_max, (unsigned int)idx);
        unsigned int prev = atomic_cmpxchg(&report->first_lock, 0xFFFFFFFFu, 0u);
        if (prev == 0xFFFFFFFFu) {
            report->first_exp = exp;
            report->first_act = act;
        }
        unsigned int bit = popcount(x ^ (x - 1u)) - 1u;  // lowest set bit
        atomic_inc(&report->bit_hist[bit]);
    }
}

__kernel void verify_inject_error(
    __global unsigned int* buf, const unsigned int idx, const unsigned int mask)
{
    buf[idx] ^= mask;
}
"""

# Per-precision GEMM verification kernels (appended to a program built with
# the same extension preamble as the gemm program).
GEMM_VERIFY_SRC_TEMPLATE = """
__EXT_PREAMBLE__

// Block-reduced scan of the C buffer: non-finite / exact-zero counts and an
// |C| sum, per-group partials (no float atomics needed).
__kernel void gemm_stats_reduce(
    __global const __SCALAR_T__* c, const unsigned long n,
    __global unsigned int* nonfinite, __global unsigned int* zeros,
    __global float* sums)
{
    __local unsigned int s_nf;
    __local unsigned int s_z;
    __local float s_sums[__LOCAL_SIZE__];
    size_t lid = get_local_id(0);
    size_t gid = get_group_id(0);
    if (lid == 0) { s_nf = 0u; s_z = 0u; }
    s_sums[lid] = 0.0f;
    barrier(CLK_LOCAL_MEM_FENCE);
    unsigned long stride = (unsigned long)get_global_size(0);
    unsigned int nf = 0u, z = 0u;
    float s = 0.0f;
    for (unsigned long idx = (unsigned long)get_global_id(0); idx < n; idx += stride) {
        float v = (float)c[idx];
        if (!isfinite(v)) {
            nf++;
        } else {
            if (v == 0.0f) z++;
            s += (v >= 0.0f ? v : -v);
        }
    }
    if (nf) atomic_add(&s_nf, nf);
    if (z) atomic_add(&s_z, z);
    s_sums[lid] = s;
    barrier(CLK_LOCAL_MEM_FENCE);
    if (lid == 0) {
        float total = 0.0f;
        for (unsigned int k = 0; k < __LOCAL_SIZE__; ++k) total += s_sums[k];
        nonfinite[gid] = s_nf;
        zeros[gid] = s_z;
        sums[gid] = total;
    }
}

// One work-item per sampled output element: recompute the dot product from
// the same A/B the gemm kernel consumed, using the gemm kernel's exact index
// identity (textbook row-major, transpose flags honored), and compare.
// Accumulator: float for fp32/fp16, double for fp64 (matches device math).
__kernel void gemm_sample_check(
    __global const __SCALAR_T__* a, __global const __SCALAR_T__* b,
    __global const __SCALAR_T__* c,
    const int size, const int ta, const int tb,
    __global const unsigned int* samples, const unsigned int m,
    const float atol, const float rtol,
    __global unsigned int* out)  // [0]=mismatch [1]=nonfinite [2]=maxdiff_bits [3]=first_bad [4]=checked
{
    unsigned int k = (unsigned int)get_global_id(0);
    unsigned int mm = 0u, nf = 0u, ck = 0u;
    if (k < m) {
        ck = 1u;
        unsigned int i = samples[2u * k];
        unsigned int j = samples[2u * k + 1u];
        __ACCUM_T__ acc = (__ACCUM_T__)0;
        for (int kk = 0; kk < size; ++kk) {
            unsigned int ai = ta ? ((unsigned int)kk * (unsigned int)size + i)
                                 : (i * (unsigned int)size + (unsigned int)kk);
            unsigned int bi = tb ? (j * (unsigned int)size + (unsigned int)kk)
                                 : ((unsigned int)kk * (unsigned int)size + j);
            acc += (__ACCUM_T__)a[ai] * (__ACCUM_T__)b[bi];
        }
        float cv = (float)c[i * (unsigned int)size + j];
        float refv = (float)acc;
        int finite = isfinite(cv) && isfinite(refv);
        if (!finite) {
            nf = 1u;
        } else {
            float diff = refv - cv;
            if (diff < 0.0f) diff = -diff;
            float aref = refv >= 0.0f ? refv : -refv;
            if (diff > atol + rtol * aref) {
                mm = 1u;
                // Non-negative floats order like their uint bit pattern.
                atomic_max(&out[2], as_uint(diff));
                atomic_cmpxchg(&out[3], 0xFFFFFFFFu, k);
            }
        }
    }
    if (mm) atomic_inc(&out[0]);
    if (nf) atomic_inc(&out[1]);
    if (ck) atomic_inc(&out[4]);
    if (get_local_id(0) == 0) atomic_inc(&out[5]);  // groups done
}
"""


def format_gemm_verify_source(spec_scalar_type: str, spec_accum_type: str, preamble: str):
    return (
        GEMM_VERIFY_SRC_TEMPLATE.replace("__EXT_PREAMBLE__", preamble)
        .replace("__SCALAR_T__", spec_scalar_type)
        .replace("__ACCUM_T__", spec_accum_type)
        .replace("__LOCAL_SIZE__", str(LOCAL_SIZE))
    )


# ---------------------------------------------------------------------------
# Config / classification (mirror of the CUDA verify module)
# ---------------------------------------------------------------------------


@dataclass
class VerifyConfig:
    enabled: bool = True
    self_test: bool = True
    gemm_every: int = 4
    gemm_samples: int = 512
    pattern_mib: int = 64

    def due(self, every: int, seed: int) -> bool:
        return self.enabled and every > 0 and seed % every == 0


@dataclass
class DetectorStats:
    ops_checked: int = 0
    elements_checked: int = 0
    total_errors: int = 0
    mismatches: int = 0
    nonfinite: int = 0
    first_error: str | None = None

    def merge(self, other: "DetectorStats") -> None:
        self.ops_checked += other.ops_checked
        self.elements_checked += other.elements_checked
        self.total_errors += other.total_errors
        self.mismatches += other.mismatches
        self.nonfinite += other.nonfinite
        if self.first_error is None:
            self.first_error = other.first_error


@dataclass
class SelfTestCheck:
    name: str
    passed: bool
    detail: str


@dataclass
class SelfTestReport:
    passed: bool
    checks: list = field(default_factory=list)


class VerdictClass(Enum):
    NONE = "none"
    DATA_ERROR = "data_error"
    MEMORY_ERROR = "memory_error"
    API_ERROR = "api_error"
    SELF_TEST_FAILED = "self_test_failed"


def classify(
    self_test_passed: bool,
    validation_failures: int,
    detector_errors: int,
    runtime_errors: int,
) -> VerdictClass:
    if not self_test_passed:
        return VerdictClass.SELF_TEST_FAILED
    if runtime_errors > 0:
        return VerdictClass.API_ERROR
    if detector_errors > 0:
        return VerdictClass.MEMORY_ERROR
    if validation_failures > 0:
        return VerdictClass.DATA_ERROR
    return VerdictClass.NONE


def sample_tolerance(precision_name: str, size: int):
    """Tolerance for the sampled cross-check, scaled for accumulation length."""
    atol, rtol = choose_tolerance_shared(precision_name)
    scale = max(1.0, (size / 1024.0) ** 0.5)
    return atol * scale, rtol


def choose_tolerance_shared(precision_name: str):
    # Kept in sync with test.py choose_tolerance (duplicated here so this
    # module stays importable without the stressor module).
    if precision_name == "FP64":
        return 1e-5, 1e-5
    if precision_name == "FP32":
        return 1e-2, 1e-2
    if precision_name == "FP16":
        return 2e-1, 2e-1
    return 1e-2, 1e-2


# ---------------------------------------------------------------------------
# Engine (runtime side — requires a live OpenCL context)
# ---------------------------------------------------------------------------


class VerifyEngine:
    """Buffers + pattern kernels + per-precision GEMM verify entry points."""

    def __init__(self, runtime, cfg: VerifyConfig, base_seed: int):
        import pyopencl as cl

        self.cfg = cfg
        self.drained = DetectorStats()
        self.failure: str | None = None
        self.self_test: SelfTestReport | None = None

        flags = cl.mem_flags
        self.report_buf = cl.Buffer(runtime.context, flags.READ_WRITE, size=176)
        self.stats_nonfinite = cl.Buffer(
            runtime.context, flags.READ_WRITE, size=4 * STATS_GRID_CAP
        )
        self.stats_zeros = cl.Buffer(
            runtime.context, flags.READ_WRITE, size=4 * STATS_GRID_CAP
        )
        self.stats_sums = cl.Buffer(
            runtime.context, flags.READ_WRITE, size=4 * STATS_GRID_CAP
        )
        self.sample_stats = cl.Buffer(runtime.context, flags.READ_WRITE, size=4 * 8)
        self.samples_buf = cl.Buffer(
            runtime.context, flags.READ_WRITE, size=4 * 2 * SAMPLE_CAP
        )
        self.scratch = cl.Buffer(
            runtime.context, flags.READ_WRITE, size=4 * SAMPLE_SCRATCH_WORDS
        )
        self.scratch_words = SAMPLE_SCRATCH_WORDS

        self.pattern_buf = None
        self.pattern_words = 0
        if cfg.pattern_mib > 0:
            self.pattern_words = cfg.pattern_mib * (1 << 20) // 4
            self.pattern_buf = cl.Buffer(
                runtime.context, flags.READ_WRITE, size=4 * self.pattern_words
            )

        self.pattern_program = cl.Program(
            runtime.context, VERIFY_PATTERN_SRC
        ).build()
        self._fill_kernel = self.pattern_program.verify_pattern_fill
        self._compare_kernel = self.pattern_program.verify_compare
        self._inject_kernel = self.pattern_program.verify_inject_error
        for kernel, dtypes in (
            (self._fill_kernel, [None, np.uint64, np.uint64]),
            (self._compare_kernel, [None, np.uint64, np.uint64, None]),
            (self._inject_kernel, [None, np.uint32, np.uint32]),
        ):
            if hasattr(kernel, "set_scalar_arg_dtypes"):
                kernel.set_scalar_arg_dtypes(dtypes)

        self._np = np
        self._cl = cl
        # Fill the resident pattern block once (static victim; the GEMM
        # working set next to it provides the neighbor pressure).
        if self.pattern_buf is not None:
            self.fill(
                runtime.queue, self.pattern_buf, base_seed & _MASK32
            )
            runtime.queue.finish()

    # -- low-level primitives ------------------------------------------------

    def _reset_report(self, queue):
        self._cl.enqueue_copy(queue, self.report_buf, verify_report_init())

    def _launch_fill(self, queue, buf, seed) -> int:
        n = buf.size // 4
        blocks = (n + LOCAL_SIZE - 1) // LOCAL_SIZE
        self._fill_kernel(
            queue, (blocks * LOCAL_SIZE,), (LOCAL_SIZE,), buf, np.uint64(n), np.uint64(seed)
        )
        return blocks

    def _launch_compare(self, queue, buf, seed) -> int:
        n = buf.size // 4
        blocks = (n + LOCAL_SIZE - 1) // LOCAL_SIZE
        self._compare_kernel(
            queue, (blocks * LOCAL_SIZE,), (LOCAL_SIZE,), buf, np.uint64(n), np.uint64(seed), self.report_buf
        )
        return blocks

    def _read_report(self, queue):
        host = self._np.zeros(1, dtype=verify_report_dtype())
        self._cl.enqueue_copy(queue, host, self.report_buf)
        queue.finish()
        return host[0]

    def _set_failure(self, msg: str) -> None:
        if self.failure is None:
            self.drained.first_error = msg
            self.failure = msg

    def take_failure(self) -> str | None:
        return self.failure

    def set_failure(self, msg: str) -> None:
        self._set_failure(msg)

    def drain(self) -> DetectorStats:
        self.failure = None
        drained, self.drained = self.drained, DetectorStats()
        return drained

    # -- pattern checks -------------------------------------------------------

    def fill(self, queue, buf, seed) -> None:
        self._launch_fill(queue, buf, seed)

    def pattern_check(self, queue, seed: int, label: str) -> None:
        if self.pattern_buf is None:
            return
        self._reset_report(queue)
        self._launch_fill(queue, self.pattern_buf, seed)
        blocks = self._launch_compare(queue, self.pattern_buf, seed)
        queue.finish()
        report = self._read_report(queue)
        self.absorb_pattern(
            int(report["total_errors"]),
            int(report["done"]),
            blocks,
            int(report["idx_min"]),
            int(report["idx_max"]),
            int(report["first_exp"]),
            int(report["first_act"]),
            report["bit_hist"],
            self.pattern_words,
            label,
        )

    def absorb_pattern(
        self,
        total_errors: int,
        done: int,
        expected_done: int,
        idx_min: int,
        idx_max: int,
        first_exp: int,
        first_act: int,
        bit_hist,
        n_words: int,
        label: str,
    ) -> None:
        self.drained.ops_checked += 1
        self.drained.elements_checked += n_words
        if done != expected_done:
            self._set_failure(
                f"{label}: report completion counter mismatch (done={done}, expected={expected_done})"
            )
            return
        if total_errors > 0:
            bits = ",".join(
                f"bit{b}={c}" for b, c in enumerate(bit_hist) if c > 0
            )
            self._set_failure(
                f"{label}: {total_errors} wrong words (idx {idx_min}..{idx_max}, "
                f"exp=0x{first_exp:08X} act=0x{first_act:08X}, bits: {bits or 'none'})"
            )

    # -- GEMM checks ------------------------------------------------------------

    def gemm_check(
        self,
        queue,
        program,
        spec_name: str,
        buffers,
        size: int,
        transpose_a: bool,
        transpose_b: bool,
        window_seed: int,
        atol: float,
        rtol: float,
    ) -> None:
        """Stats scan + sampled cross-check on the live workload buffers."""
        import numpy as np

        n = size * size
        stats_reduce = program.gemm_stats_reduce
        sample_check = program.gemm_sample_check
        if hasattr(stats_reduce, "set_scalar_arg_dtypes"):
            stats_reduce.set_scalar_arg_dtypes(
                [None, np.uint64, None, None, None]
            )
        if hasattr(sample_check, "set_scalar_arg_dtypes"):
            sample_check.set_scalar_arg_dtypes(
                [None, None, None, np.int32, np.int32, np.int32, None, np.uint32, np.float32, np.float32, None]
            )

        # Stats pass (grid-stride, per-group partials).
        grid_blocks = min((n + LOCAL_SIZE - 1) // LOCAL_SIZE, STATS_GRID_CAP)
        global_size = (grid_blocks * LOCAL_SIZE,)
        local_size = (LOCAL_SIZE,)
        stats_reduce(
            queue,
            global_size,
            local_size,
            buffers.c,
            np.uint64(n),
            self.stats_nonfinite,
            self.stats_zeros,
            self.stats_sums,
        )
        queue.finish()

        nf = self._np.zeros(STATS_GRID_CAP, dtype=self._np.uint32)
        zeros = self._np.zeros(STATS_GRID_CAP, dtype=self._np.uint32)
        sums = self._np.zeros(STATS_GRID_CAP, dtype=self._np.float32)
        self._cl.enqueue_copy(queue, nf, self.stats_nonfinite)
        self._cl.enqueue_copy(queue, zeros, self.stats_zeros)
        self._cl.enqueue_copy(queue, sums, self.stats_sums)
        queue.finish()
        total_nonfinite = int(nf.sum())
        total_zero = int(zeros.sum())
        abs_sum = float(sums.sum())

        self.drained.ops_checked += 1
        self.drained.elements_checked += n
        self.drained.nonfinite += total_nonfinite
        if total_nonfinite > 0:
            self._set_failure(
                f"gemm output scan: {total_nonfinite}/{n} non-finite values (abs_sum={abs_sum:.4e}, zero={total_zero})"
            )

        # Sampled cross-check pass.
        m = min(self.cfg.gemm_samples, SAMPLE_CAP, n)
        if m <= 0:
            return
        rng = random.Random(window_seed ^ 0x5EED5EED)
        samples = self._np.array(
            [rng.randrange(size) for _ in range(2 * m)], dtype=self._np.uint32
        )
        init = self._np.zeros(8, dtype=self._np.uint32)
        init[3] = 0xFFFFFFFF  # first_bad
        self._cl.enqueue_copy(queue, self.sample_stats, init)
        self._cl.enqueue_copy(queue, self.samples_buf, samples)
        sample_check(
            queue,
            (m,),
            (1,),
            buffers.a,
            buffers.b,
            buffers.c,
            self._np.int32(size),
            self._np.int32(1 if transpose_a else 0),
            self._np.int32(1 if transpose_b else 0),
            self.samples_buf,
            self._np.uint32(m),
            self._np.float32(atol),
            self._np.float32(rtol),
            self.sample_stats,
        )
        queue.finish()
        out = self._np.zeros(8, dtype=self._np.uint32)
        self._cl.enqueue_copy(queue, out, self.sample_stats)
        queue.finish()

        mismatch = int(out[0])
        nonfinite = int(out[1])
        max_diff_bits = int(out[2])
        first_bad = int(out[3])
        checked = int(out[4])

        max_diff = abs(struct_pack(max_diff_bits))
        self.drained.elements_checked += checked
        self.drained.mismatches += mismatch
        self.drained.nonfinite += nonfinite
        if nonfinite > 0:
            self._set_failure(
                f"gemm sample check: {nonfinite} sampled outputs non-finite"
            )
        if mismatch > 0:
            self._set_failure(
                f"gemm sample check: {mismatch}/{checked} sampled outputs exceed tolerance "
                f"(first_bad={first_bad if first_bad != 0xFFFFFFFF else 0}, "
                f"max_abs_diff={max_diff:.4e}, atol={atol:.2e}, rtol={rtol:.2e})"
            )

    # -- self-test gate ---------------------------------------------------------

    def run_self_test(self, queue) -> SelfTestReport:
        checks = []
        seed = SELF_TEST_SEED
        scratch_words = self.scratch_words

        def gate(fill_first: bool, inject: bool) -> tuple:
            self._reset_report(queue)
            if fill_first:
                self._launch_fill(queue, self.scratch, seed)
            queue.finish()
            if inject:
                one = ((1,), (1,))
                self._inject_kernel(
                    queue,
                    one[0],
                    one[1],
                    self.scratch,
                    self._np.uint32(SELF_TEST_IDX),
                    self._np.uint32(1 << SELF_TEST_BIT),
                )
            blocks = self._launch_compare(queue, self.scratch, seed)
            queue.finish()
            report = self._read_report(queue)
            return report, blocks

        # Gate 1: clean pattern reports zero errors.
        try:
            report, blocks = gate(fill_first=True, inject=False)
            checks.append(
                SelfTestCheck(
                    name="pattern_clean",
                    passed=bool(
                        report["total_errors"] == 0 and report["done"] == blocks
                    ),
                    detail=f"total_errors={int(report['total_errors'])}, done={int(report['done'])} (expected {blocks})",
                )
            )
        except Exception as exc:
            checks.append(
                SelfTestCheck(name="pattern_clean", passed=False, detail=f"kernel error: {exc}")
            )

        # Gate 2: single injected error at 0xADBA / bit 22 is caught exactly.
        try:
            report, blocks = gate(fill_first=True, inject=True)
            want_idx = SELF_TEST_IDX
            want_bit = SELF_TEST_BIT
            bit_hist_clean = all(
                int(c) == (1 if b == want_bit else 0)
                for b, c in enumerate(report["bit_hist"])
            )
            passed = (
                int(report["total_errors"]) == 1
                and int(report["idx_min"]) == want_idx
                and int(report["idx_max"]) == want_idx
                and (int(report["first_exp"]) ^ int(report["first_act"]))
                == (1 << want_bit)
                and bit_hist_clean
            )
            checks.append(
                SelfTestCheck(
                    name="injection_capture",
                    passed=bool(passed),
                    detail=(
                        f"total_errors={int(report['total_errors'])}, "
                        f"idx={int(report['idx_min'])}..{int(report['idx_max'])} (want {want_idx}..{want_idx}), "
                        f"xor=0x{int(report['first_exp']) ^ int(report['first_act']):08X} "
                        f"(want 0x{1 << want_bit:08X}), bit_hist[{want_bit}]={int(report['bit_hist'][want_bit])}"
                    ),
                )
            )
        except Exception as exc:
            checks.append(
                SelfTestCheck(name="injection_capture", passed=False, detail=f"kernel error: {exc}")
            )

        # Restore the scratch pattern for later reuse.
        self._launch_fill(queue, self.scratch, seed)
        queue.finish()

        report = SelfTestReport(
            passed=all(check.passed for check in checks), checks=checks
        )
        self.self_test = report
        return report


def struct_pack(bits: int) -> float:
    """Reinterpret a uint bit pattern as a non-negative float."""
    import struct as _struct

    return _struct.unpack("f", bits.to_bytes(4, "little", signed=False))[0]
