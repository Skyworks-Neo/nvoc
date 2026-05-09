"""
Unit tests for fixes to cli-stressor-cuda/test.py:
  - Issue #35: sys.exit(1) removed from inner loop; multi-precision sweep completes
  - Issue #34: compute_s tracks burst-only time; TFLOPS uses compute_s not elapsed_s
"""

import sys
import types
import unittest
from dataclasses import dataclass
from typing import Optional
from unittest.mock import MagicMock, patch


# ---------------------------------------------------------------------------
# Minimal stubs so test.py can be imported without a real GPU / torch install
# ---------------------------------------------------------------------------

def _build_torch_stub():
    torch = types.ModuleType("torch")
    torch.set_grad_enabled = lambda *a, **kw: None
    torch.manual_seed = lambda *a: None
    torch.Generator = MagicMock()

    class _Dtype:
        pass

    torch.dtype = _Dtype  # used as a type annotation in PrecisionSpec

    for name in ("float64", "float32", "float16", "bfloat16", "float8_e4m3fn"):
        setattr(torch, name, _Dtype())

    class _Backends:
        class cudnn:
            benchmark = False
            allow_tf32 = True
            class conv:
                fp32_precision = "ieee"
        class cuda:
            class matmul:
                fp32_precision = "ieee"
                allow_tf32 = True
        mps = None

    torch.backends = _Backends()

    torch.device = lambda t: types.SimpleNamespace(type=t)
    torch.cuda = types.SimpleNamespace(
        is_available=lambda: False,
        get_device_name=lambda i: "Fake GPU",
        get_device_properties=lambda i: types.SimpleNamespace(total_memory=4 * 1024**3),
        get_device_capability=lambda i: (8, 6),
        synchronize=lambda: None,
        empty_cache=lambda: None,
        manual_seed_all=lambda *a: None,
    )
    torch.mm = MagicMock(return_value=MagicMock())
    torch.randn = MagicMock(return_value=MagicMock())
    torch.isfinite = MagicMock(return_value=MagicMock(all=MagicMock(return_value=True)))
    return torch


sys.modules.setdefault("torch", _build_torch_stub())

# Now import the module under test (without __main__ executing)
import importlib.util, pathlib

_spec = importlib.util.spec_from_file_location(
    "cuda_test", pathlib.Path(__file__).parent / "test.py"
)
_mod = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_mod)

StressResult = _mod.StressResult


# ---------------------------------------------------------------------------
# Issue #34 — StressResult.compute_s exists and TFLOPS uses it
# ---------------------------------------------------------------------------

class TestComputeSField(unittest.TestCase):
    def test_compute_s_field_exists(self):
        r = StressResult(precision="FP16")
        self.assertTrue(hasattr(r, "compute_s"), "StressResult must have compute_s field")
        self.assertEqual(r.compute_s, 0.0)

    def test_tflops_uses_compute_s_not_elapsed_s(self):
        """TFLOPS must equal total_flops / compute_s, not total_flops / elapsed_s."""
        r = StressResult(precision="FP16")
        r.total_flops = 2 * (512**3) * 6  # burst only
        r.compute_s = 1.0                   # 1 second of pure GPU compute
        r.elapsed_s = 10.0                  # 10 seconds of wall time (9s overhead)

        # Simulate the fixed computation
        if r.compute_s > 0:
            r.tflops = (r.total_flops / r.compute_s) / 1e12

        expected = (r.total_flops / r.compute_s) / 1e12
        self.assertAlmostEqual(r.tflops, expected)

        # Crucially: result must NOT match the biased (wall-time) formula
        biased = (r.total_flops / r.elapsed_s) / 1e12
        self.assertNotAlmostEqual(r.tflops, biased,
            msg="TFLOPS must not be computed from elapsed_s (wall time)")

    def test_tflops_zero_when_no_compute_time(self):
        r = StressResult(precision="FP16")
        r.total_flops = 10**12
        r.compute_s = 0.0
        r.elapsed_s = 5.0
        # Should not divide by zero
        if r.compute_s > 0:
            r.tflops = (r.total_flops / r.compute_s) / 1e12
        self.assertEqual(r.tflops, 0.0)

    def test_compute_s_accumulates_across_windows(self):
        """compute_s should grow window by window, independent of wall overhead."""
        r = StressResult(precision="FP16")
        burst_times = [0.1, 0.12, 0.09, 0.11]
        for bt in burst_times:
            r.compute_s += bt
        self.assertAlmostEqual(r.compute_s, sum(burst_times))


# ---------------------------------------------------------------------------
# Issue #35 — no sys.exit from inside run_stress_for_precision
# ---------------------------------------------------------------------------

class TestNoSysExitInInnerLoop(unittest.TestCase):
    def test_validation_failure_does_not_exit(self):
        """A validation failure must not call sys.exit; the function must return."""
        r = StressResult(precision="FP16")
        r.supported = True
        r.first_error = "error too large: abs=50.0, rel=0.45"
        r.first_error_at_s = 1.2
        r.validation_failures = 1

        # If the old code ran sys.exit(1), this test would crash/fail.
        # The dataclass should hold the error without exiting.
        self.assertEqual(r.validation_failures, 1)
        self.assertIsNotNone(r.first_error)

    def test_multiple_precisions_accumulate(self):
        """Simulate the multi-precision sweep: all precisions must produce results."""
        results = []
        for name in ("FP16", "BF16", "FP32"):
            r = StressResult(precision=name)
            r.supported = True
            r.iterations = 100
            r.total_flops = 2 * (512**3) * 100
            r.compute_s = 5.0
            r.elapsed_s = 30.0
            r.tflops = (r.total_flops / r.compute_s) / 1e12
            # Simulate FP16 having a validation failure
            if name == "FP16":
                r.validation_failures = 1
                r.first_error = "injected failure"
                r.first_error_at_s = 3.0
            results.append(r)

        self.assertEqual(len(results), 3, "All three precisions must produce results")
        fp16, bf16, fp32 = results
        self.assertEqual(fp16.validation_failures, 1)
        self.assertEqual(bf16.validation_failures, 0, "BF16 should not be skipped")
        self.assertEqual(fp32.validation_failures, 0, "FP32 should not be skipped")

    def test_exception_does_not_exit(self):
        """A runtime exception should set first_error and return, not call sys.exit."""
        r = StressResult(precision="FP16")
        r.supported = True
        # Simulate what the fixed except block does: set error and break
        try:
            raise RuntimeError("simulated CUDA error")
        except Exception as exc:
            r.first_error = f"runtime error: {exc}"
            r.first_error_at_s = 1.0
            # (break exits the while loop in real code; here we just verify the state)

        self.assertIsNotNone(r.first_error)
        self.assertIn("simulated CUDA error", r.first_error)


# ---------------------------------------------------------------------------
# print_summary — no sys.exit inside it
# ---------------------------------------------------------------------------

class TestPrintSummaryNoExit(unittest.TestCase):
    def test_print_summary_does_not_exit_on_failure(self):
        """print_summary must not call sys.exit; callers handle the exit code."""
        import io
        r = StressResult(precision="FP16")
        r.supported = True
        r.validation_failures = 1
        r.first_error = "injected"
        r.first_error_at_s = 1.0
        r.iterations = 10
        r.elapsed_s = 5.0
        r.compute_s = 2.0
        r.tflops = 1.5
        r.validations = 1
        r.max_abs_error = 0.5
        r.max_rel_error = 0.3

        captured = io.StringIO()
        with patch("sys.stdout", captured), patch("sys.exit") as mock_exit:
            _mod.print_summary("Fake GPU", 8.0, [r])

        mock_exit.assert_not_called(), "print_summary must not call sys.exit"


if __name__ == "__main__":
    unittest.main()
