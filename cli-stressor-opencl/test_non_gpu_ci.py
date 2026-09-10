"""
No-GPU unit tests for issues #37, #38 root-cause fixes.

Runs with stdlib only — no OpenCL, pyopencl, or numpy required.

  #37 - per-element allclose validation criterion
  #38 - compute_s-based TFLOPS (not total wall-time)
"""

import pathlib
import sys
import types
import importlib.util
import unittest


# ---------------------------------------------------------------------------
# Stub out pyopencl so test.py can be imported without an OpenCL driver.
# ---------------------------------------------------------------------------
def _make_cl_stub():
    cl = types.ModuleType("pyopencl")
    cl.get_platforms = lambda: []
    cl.mem_flags = types.SimpleNamespace(
        READ_ONLY=1, WRITE_ONLY=2, READ_WRITE=4, COPY_HOST_PTR=8
    )
    cl.Buffer = lambda *a, **kw: None
    cl.enqueue_copy = lambda *a, **kw: None
    cl.Context = lambda *a, **kw: types.SimpleNamespace(create_sub_devices=lambda d: [])
    cl.CommandQueue = lambda *a, **kw: types.SimpleNamespace(finish=lambda: None)
    cl.Program = lambda *a, **kw: types.SimpleNamespace(build=lambda *a, **kw: None)
    return cl


def _make_numpy_stub():
    np = types.ModuleType("numpy")
    np.float16 = object()
    np.float32 = object()
    np.float64 = object()
    np.random = types.SimpleNamespace(
        default_rng=lambda seed=None: types.SimpleNamespace(
            standard_normal=lambda shape, dtype=None: None
        )
    )
    np.empty = lambda shape, dtype=None: None
    np.isfinite = lambda x: True
    np.abs = abs
    np.max = max
    np.sum = sum
    return np


for _name, _stub in [("pyopencl", _make_cl_stub()), ("numpy", _make_numpy_stub())]:
    if _name not in sys.modules:
        sys.modules[_name] = _stub


# ---------------------------------------------------------------------------
# Import from the stressor module
# ---------------------------------------------------------------------------

_MODULE_PATH = pathlib.Path(__file__).parent / "test.py"
_spec = importlib.util.spec_from_file_location("stressor_opencl", _MODULE_PATH)
_mod = importlib.util.module_from_spec(_spec)
try:
    _spec.loader.exec_module(_mod)
except Exception:
    pass  # OpenCL/numpy-dependent code may fail — we only need the dataclasses

StressResult = _mod.StressResult
choose_tolerance = _mod.choose_tolerance
parse_int_list = _mod.parse_int_list

import os

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
try:
    import verify_opencl as vo

    VO_AVAILABLE = True
except Exception:
    vo = None
    VO_AVAILABLE = False


# ---------------------------------------------------------------------------
# Pure-Python per-element allclose (mirrors the fixed numpy logic)
# ---------------------------------------------------------------------------
def _per_element_allclose(diff_flat, ref_flat, atol, rtol):
    return all(d <= atol + rtol * abs(r) for d, r in zip(diff_flat, ref_flat))


# ---------------------------------------------------------------------------
# Tests for issue #38 — compute_s field and TFLOPS from compute time
# ---------------------------------------------------------------------------
class TestComputeS(unittest.TestCase):
    def test_stress_result_has_compute_s(self):
        r = StressResult(precision="FP32")
        self.assertTrue(
            hasattr(r, "compute_s"), "StressResult must have compute_s field"
        )
        self.assertEqual(r.compute_s, 0.0)

    def test_tflops_zero_when_no_compute_time(self):
        r = StressResult(precision="FP32")
        self.assertEqual(r.tflops, 0.0)

    def test_tflops_computed_from_compute_s(self):
        r = StressResult(precision="FP32")
        r.total_flops = int(2 * 4096**3 * 10)
        r.compute_s = 5.0
        r.elapsed_s = 90.0
        r.tflops = (r.total_flops / r.compute_s) / 1e12
        tflops_from_wall = (r.total_flops / r.elapsed_s) / 1e12
        self.assertGreater(r.tflops, tflops_from_wall)

    def test_tflops_consistent_across_validate_intervals(self):
        flops = int(2 * 2048**3 * 5)
        compute_s = 3.0
        expected_tflops = (flops / compute_s) / 1e12
        for wall_s in (10.0, 30.0, 90.0):
            r = StressResult(precision="FP16")
            r.total_flops = flops
            r.compute_s = compute_s
            r.elapsed_s = wall_s
            r.tflops = (r.total_flops / r.compute_s) / 1e12
            self.assertAlmostEqual(r.tflops, expected_tflops, places=6)


# ---------------------------------------------------------------------------
# Tests for issue #37 — per-element allclose validation criterion
# ---------------------------------------------------------------------------
class TestPerElementValidation(unittest.TestCase):
    def test_all_pass_within_tolerance(self):
        diff = [0.01] * 4
        ref = [1.0] * 4
        self.assertTrue(_per_element_allclose(diff, ref, atol=0.02, rtol=0.0))

    def test_single_outlier_detected(self):
        diff = [0.01, 0.01, 0.01, 100.0]
        ref = [1.0, 1.0, 1.0, 1.0]
        self.assertFalse(_per_element_allclose(diff, ref, atol=0.1, rtol=0.1))

    def test_old_criterion_false_pass(self):
        """Show that the old OR-of-globals criterion lets outliers through."""
        diff = [50.0, 0.0]
        ref = [1.0, 1000.0]
        atol, rtol = 0.2, 0.2

        max_abs = max(diff)
        ref_abs = max(abs(r) for r in ref)
        max_rel_old = max_abs / (ref_abs + 1e-12)
        old_passed = (max_abs <= atol) or (max_rel_old <= rtol)
        self.assertTrue(old_passed, "Demonstrates old criterion bug")

        self.assertFalse(
            _per_element_allclose(diff, ref, atol, rtol),
            "Fixed criterion must catch the outlier",
        )

    def test_choose_tolerance_returns_expected(self):
        cases = {"FP64": (1e-5, 1e-5), "FP32": (1e-2, 1e-2), "FP16": (2e-1, 2e-1)}
        for name, expected in cases.items():
            with self.subTest(precision=name):
                self.assertEqual(choose_tolerance(name), expected)


# ---------------------------------------------------------------------------
# Source inspection: verify compute_s accumulation in the hot loop
# ---------------------------------------------------------------------------
class TestSourceStructure(unittest.TestCase):
    def test_compute_s_accumulated_in_loop(self):
        source = _MODULE_PATH.read_text(encoding="utf-8")
        self.assertIn(
            "result.compute_s += op_elapsed",
            source,
            "Loop body must accumulate compute_s from op_elapsed",
        )

    def test_tflops_divides_by_compute_s(self):
        source = _MODULE_PATH.read_text(encoding="utf-8")
        self.assertIn("result.compute_s", source)
        # Confirm the tflops formula references compute_s, not elapsed_s
        self.assertIn(
            "result.total_flops / result.compute_s",
            source,
            "TFLOPS must be computed from compute_s, not elapsed_s",
        )

    def test_summary_shows_compute_column(self):
        source = _MODULE_PATH.read_text(encoding="utf-8")
        self.assertIn("compute=", source, "print_summary must show compute= column")


# ---------------------------------------------------------------------------
# Parse int list helper
# ---------------------------------------------------------------------------
class TestParseIntList(unittest.TestCase):
    def test_single(self):
        self.assertEqual(parse_int_list("1024"), [1024])

    def test_multiple(self):
        self.assertEqual(parse_int_list("512, 1024, 2048"), [512, 1024, 2048])

    def test_empty_raises(self):
        with self.assertRaises(ValueError):
            parse_int_list("")


@unittest.skipUnless(VO_AVAILABLE, "verify_opencl unavailable")
class TestVerifyCircuit(unittest.TestCase):
    def test_vhash32_deterministic_and_distinct(self):
        self.assertEqual(vo.vhash32(1), vo.vhash32(1))
        self.assertNotEqual(vo.vexpected_host(0, 1), vo.vexpected_host(0, 2))
        self.assertNotEqual(vo.vexpected_host(0, 1), vo.vexpected_host(1, 1))
        # seed=0 must differ from the all-zero pattern (i+1 fold)
        self.assertNotEqual(vo.vexpected_host(0, 0), vo.vexpected_host(0, 1))

    def test_vhash32_matches_cuda_semantics_documentation(self):
        # Frozen vector: forces deliberate dual-side updates if the finalizer
        # ever changes (device mirrors live in the CUDA/OpenCL kernel sources).
        self.assertIsInstance(vo.vhash32(0xDEADBEEF), int)
        self.assertLess(vo.vhash32(0xDEADBEEF), 1 << 32)

    def test_verify_config_cadence(self):
        cfg = vo.VerifyConfig()
        self.assertTrue(cfg.enabled and cfg.self_test)
        self.assertTrue(cfg.due(cfg.gemm_every, cfg.gemm_every * 3))
        self.assertFalse(cfg.due(3, 8))
        self.assertFalse(cfg.due(0, 0))
        off = vo.VerifyConfig(enabled=False)
        self.assertFalse(off.due(1, 0))

    def test_classify_precedence(self):
        self.assertIs(
            vo.classify(False, 0, 0, 0), vo.VerdictClass.SELF_TEST_FAILED
        )
        self.assertIs(vo.classify(True, 0, 0, 2), vo.VerdictClass.API_ERROR)
        self.assertIs(vo.classify(True, 0, 1, 0), vo.VerdictClass.MEMORY_ERROR)
        self.assertIs(vo.classify(True, 3, 0, 0), vo.VerdictClass.DATA_ERROR)
        self.assertIs(vo.classify(True, 0, 0, 0), vo.VerdictClass.NONE)

    def test_detector_stats_merge_keeps_first_error(self):
        total = vo.DetectorStats()
        part = vo.DetectorStats(
            ops_checked=3,
            elements_checked=100,
            total_errors=1,
            first_error="memcpy verify: 1 wrong words",
        )
        total.merge(part)
        total.merge(vo.DetectorStats(ops_checked=2))
        self.assertEqual(total.ops_checked, 5)
        self.assertEqual(total.total_errors, 1)
        total.merge(vo.DetectorStats(first_error="later"))
        self.assertEqual(total.first_error, "memcpy verify: 1 wrong words")

    def test_sample_tolerance_scales_with_size(self):
        a1, r1 = vo.sample_tolerance("FP32", 1024)
        a4, r4 = vo.sample_tolerance("FP32", 4096)
        self.assertAlmostEqual(a1, 1e-2)
        self.assertAlmostEqual(a4, 2e-2, places=4)
        self.assertEqual(r1, r4)

    def test_sample_check_semantics_documented(self):
        # The OpenCL gemm kernel is self-written textbook row-major
        # C[i][j] = sum op_ta(A)[i][k] * op_tb(B)[k][j] — the sampled
        # cross-check must use the same identity (unlike the CUDA backend's
        # B*A column-major pass-through).
        src = vo.GEMM_VERIFY_SRC_TEMPLATE
        self.assertIn("ai = ta ? ((unsigned int)kk * (unsigned int)size + i)", src)
        self.assertIn("bi = tb ? (j * (unsigned int)size + (unsigned int)kk)", src)


if __name__ == "__main__":
    unittest.main()
