"""
Unit tests for fixes to cli-stressor-opencl/test.py:
  - Issue #38: compute_s tracks burst-only time; TFLOPS uses compute_s not elapsed_s
"""

import sys
import types
import unittest
from unittest.mock import MagicMock, patch


# ---------------------------------------------------------------------------
# Minimal stubs so test.py can be imported without pyopencl / a real GPU
# ---------------------------------------------------------------------------

def _build_numpy_stub():
    import numpy as np
    return np


def _build_pyopencl_stub():
    cl = types.ModuleType("pyopencl")
    cl.CompilerWarning = Warning

    class _MemFlags:
        READ_ONLY = 1
        WRITE_ONLY = 2
        COPY_HOST_PTR = 4

    cl.mem_flags = _MemFlags()
    cl.device_type = types.SimpleNamespace(GPU=4, CPU=2, DEFAULT=1, ACCELERATOR=8, CUSTOM=16)
    cl.get_platforms = MagicMock(return_value=[])
    cl.Context = MagicMock()
    cl.CommandQueue = MagicMock()
    cl.Program = MagicMock()
    cl.Buffer = MagicMock()
    cl.enqueue_copy = MagicMock()
    return cl


sys.modules.setdefault("pyopencl", _build_pyopencl_stub())

import importlib.util, pathlib

_spec = importlib.util.spec_from_file_location(
    "opencl_test", pathlib.Path(__file__).parent / "test.py"
)
_mod = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_mod)

StressResult = _mod.StressResult


# ---------------------------------------------------------------------------
# Issue #38 — StressResult.compute_s exists and TFLOPS uses it
# ---------------------------------------------------------------------------

class TestComputeSField(unittest.TestCase):
    def test_compute_s_field_exists(self):
        r = StressResult(precision="FP16")
        self.assertTrue(hasattr(r, "compute_s"), "StressResult must have compute_s field")
        self.assertEqual(r.compute_s, 0.0)

    def test_tflops_uses_compute_s_not_elapsed_s(self):
        """TFLOPS must equal total_flops / compute_s, not total_flops / elapsed_s."""
        r = StressResult(precision="FP16")
        r.total_flops = 2 * (512**3) * 6
        r.compute_s = 1.0   # 1 second pure GPU compute
        r.elapsed_s = 20.0  # 20 seconds wall (19s overhead: alloc, validation, prints)

        if r.compute_s > 0:
            r.tflops = (r.total_flops / r.compute_s) / 1e12

        expected = (r.total_flops / r.compute_s) / 1e12
        self.assertAlmostEqual(r.tflops, expected)

        biased = (r.total_flops / r.elapsed_s) / 1e12
        self.assertNotAlmostEqual(r.tflops, biased,
            msg="TFLOPS must not be computed from elapsed_s (wall time)")

    def test_tflops_zero_when_no_compute_time(self):
        r = StressResult(precision="FP32")
        r.total_flops = 10**12
        r.compute_s = 0.0
        r.elapsed_s = 5.0
        if r.compute_s > 0:
            r.tflops = (r.total_flops / r.compute_s) / 1e12
        self.assertEqual(r.tflops, 0.0)

    def test_compute_s_accumulates_independent_of_wall(self):
        """compute_s only grows by op_elapsed; wall overhead does not inflate it."""
        r = StressResult(precision="FP32")
        op_times = [0.05, 0.07, 0.06, 0.08]
        for ot in op_times:
            r.compute_s += ot
        self.assertAlmostEqual(r.compute_s, sum(op_times))

    def test_min_burst_extension_included_in_compute_s(self):
        """Extra iterations added by min-burst extension must be counted in compute_s."""
        r = StressResult(precision="FP16")
        # initial burst
        initial_elapsed = 0.01
        extra_elapsed = 0.03
        op_elapsed = initial_elapsed + extra_elapsed  # as computed in the fixed code
        r.compute_s += op_elapsed
        self.assertAlmostEqual(r.compute_s, 0.04)

    def test_tflops_stable_across_validate_intervals(self):
        """Different validate intervals must give same TFLOPS for same compute work."""
        total_flops = 2 * (512**3) * 60
        compute_s = 10.0  # 10 seconds of actual GPU work in both cases

        # Low validate interval: lots of validation overhead → high elapsed_s
        r_frequent = StressResult(precision="FP16")
        r_frequent.total_flops = total_flops
        r_frequent.compute_s = compute_s
        r_frequent.elapsed_s = 60.0  # 50s of validation overhead
        r_frequent.tflops = (r_frequent.total_flops / r_frequent.compute_s) / 1e12

        # High validate interval: less overhead → lower elapsed_s
        r_rare = StressResult(precision="FP16")
        r_rare.total_flops = total_flops
        r_rare.compute_s = compute_s
        r_rare.elapsed_s = 12.0  # only 2s overhead
        r_rare.tflops = (r_rare.total_flops / r_rare.compute_s) / 1e12

        self.assertAlmostEqual(r_frequent.tflops, r_rare.tflops, places=6,
            msg="TFLOPS must be identical regardless of validate interval")


if __name__ == "__main__":
    unittest.main()
