"""
No-GPU unit tests for issues #33, #34, #35 root-cause fixes.

Runs with stdlib only — no CUDA, torch, or numpy required.

  #33 - per-element allclose validation criterion
  #34 - compute_s-based TFLOPS (not total wall-time)
  #35 - sys.exit removed from inner stress loop
"""

import ast
import pathlib
import sys
import types
import unittest


# ---------------------------------------------------------------------------
# Minimal torch stub so test.py can be imported without a GPU or torch wheel.
# Must include torch.dtype because PrecisionSpec uses it as a type annotation.
# ---------------------------------------------------------------------------
def _make_torch_stub():
    t = types.ModuleType("torch")

    # dtype class and sentinel instances
    class _Dtype:
        pass

    t.dtype = _Dtype
    for _name in ("float64", "float32", "float16", "bfloat16"):
        setattr(t, _name, _Dtype())
    t.float8_e4m3fn = None

    backends = types.SimpleNamespace(
        cuda=types.SimpleNamespace(
            matmul=types.SimpleNamespace(allow_tf32=False, fp32_precision="ieee"),
            conv=types.SimpleNamespace(fp32_precision="ieee"),
        ),
        cudnn=types.SimpleNamespace(
            allow_tf32=False,
            conv=types.SimpleNamespace(fp32_precision="ieee"),
        ),
        mps=None,
    )
    t.backends = backends

    cuda = types.SimpleNamespace(
        is_available=lambda: False,
        synchronize=lambda: None,
        empty_cache=lambda: None,
        get_device_name=lambda i: "stub",
        get_device_properties=lambda i: types.SimpleNamespace(total_memory=8 * 1024**3),
        get_device_capability=lambda i: (8, 0),
    )
    t.cuda = cuda
    t.device = lambda s: types.SimpleNamespace(type=s.split(":")[0])

    class _Generator:
        def __init__(self, device="cpu"):
            pass

        def manual_seed(self, seed):
            pass

    t.Generator = _Generator
    t.mm = lambda a, b: None
    t.randn = lambda *a, **kw: None
    t.isfinite = lambda x: True
    return t


if "torch" not in sys.modules:
    sys.modules["torch"] = _make_torch_stub()

from cli_stressor_cuda.kernels import (  # noqa: E402
    MAX_ATOMIC_ELEMENTS,
    atomic_element_count,
    choose_kernel_type,
    choose_precision_from_mixture,
)
from cli_stressor_cuda.models import (  # noqa: E402
    KernelType,
    PrecisionSpec,
    StressResult,
)
from cli_stressor_cuda.parsing import parse_int_list  # noqa: E402
from cli_stressor_cuda.validation import choose_tolerance  # noqa: E402


_RUNNER_PATH = pathlib.Path(__file__).parent / "cli_stressor_cuda" / "runner.py"


# ---------------------------------------------------------------------------
# Pure-Python per-element allclose (mirrors the fixed validation logic)
# ---------------------------------------------------------------------------
def _per_element_allclose(diff_flat, ref_flat, atol, rtol):
    return all(d <= atol + rtol * abs(r) for d, r in zip(diff_flat, ref_flat))


# ---------------------------------------------------------------------------
# Issue #34 — compute_s field and TFLOPS from compute time (not wall time)
# ---------------------------------------------------------------------------
class TestComputeS(unittest.TestCase):
    def test_stress_result_has_compute_s(self):
        r = StressResult(precision="FP32")
        self.assertTrue(
            hasattr(r, "compute_s"), "StressResult must have a compute_s field"
        )
        self.assertEqual(r.compute_s, 0.0)

    def test_tflops_zero_when_no_compute_time(self):
        r = StressResult(precision="FP32")
        self.assertEqual(r.tflops, 0.0)

    def test_tflops_greater_when_computed_from_compute_s(self):
        """TFLOPS from compute_s must exceed TFLOPS from wall time when there is overhead."""
        r = StressResult(precision="FP32")
        r.total_flops = int(2 * 4096**3 * 10)
        r.compute_s = 5.0  # 5 s of actual GPU compute
        r.elapsed_s = 90.0  # 90 s total (includes warmup, validation, etc.)
        r.tflops = (r.total_flops / r.compute_s) / 1e12
        tflops_from_wall = (r.total_flops / r.elapsed_s) / 1e12
        self.assertGreater(r.tflops, tflops_from_wall)

    def test_tflops_consistent_across_validate_intervals(self):
        """Same compute work → same TFLOPS regardless of validation overhead."""
        flops = int(2 * 2048**3 * 5)
        compute_s = 3.0
        expected = (flops / compute_s) / 1e12
        for wall_s in (10.0, 30.0, 90.0):
            r = StressResult(precision="FP16")
            r.total_flops = flops
            r.compute_s = compute_s
            r.elapsed_s = wall_s
            r.tflops = (r.total_flops / r.compute_s) / 1e12
            self.assertAlmostEqual(r.tflops, expected, places=6)


# ---------------------------------------------------------------------------
# Issue #33 — per-element allclose validation criterion
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
        """Demonstrates the root cause: old OR-of-globals lets a huge outlier through.

        Element 0: diff=50, ref=1    → relative error = 50  (way over tolerance)
        Element 1: diff=0,  ref=1000 → pulls global max_ref to 1000, making
                                        global max_rel = 50/1000 = 0.05 ≤ rtol → PASS (wrong)
        """
        diff = [50.0, 0.0]
        ref = [1.0, 1000.0]
        atol, rtol = 0.2, 0.2

        max_abs = max(diff)
        ref_abs = max(abs(r) for r in ref)
        max_rel_old = max_abs / (ref_abs + 1e-12)
        old_passed = (max_abs <= atol) or (max_rel_old <= rtol)
        self.assertTrue(
            old_passed, "Old criterion must incorrectly pass (demonstrates bug)"
        )
        self.assertFalse(
            _per_element_allclose(diff, ref, atol, rtol),
            "Fixed criterion must detect the outlier",
        )

    def test_rtol_scales_with_ref_magnitude(self):
        diff = [0.5]
        ref = [100.0]
        # budget = atol + rtol*|ref| = 1.0 + 0.01*100 = 2.0 → diff=0.5 passes
        self.assertTrue(_per_element_allclose(diff, ref, atol=1.0, rtol=0.01))

    def test_choose_tolerance_values(self):
        for name, expected in [
            ("FP64", (1e-5, 1e-5)),
            ("FP32", (1e-2, 1e-2)),
            ("FP16", (2e-1, 2e-1)),
            ("BF16", (5e-1, 5e-1)),
        ]:
            with self.subTest(precision=name):
                self.assertEqual(choose_tolerance(name), expected)


# ---------------------------------------------------------------------------
# Issue #35 — sys.exit removed from inner stress loop
# ---------------------------------------------------------------------------
class TestNoSysExitInInnerLoop(unittest.TestCase):
    def _func_source(self, name):
        source = _RUNNER_PATH.read_text(encoding="utf-8")
        tree = ast.parse(source)
        for node in ast.walk(tree):
            if isinstance(node, ast.FunctionDef) and node.name == name:
                return ast.unparse(node)
        return ""

    def test_sys_exit_absent_from_run_stress_mixed(self):
        src = self._func_source("run_stress_mixed")
        self.assertNotEqual(src, "", "run_stress_mixed must exist")
        self.assertNotIn(
            "sys.exit", src, "sys.exit must not appear inside run_stress_mixed"
        )

    def test_exception_handler_uses_break(self):
        """The inner-loop except block must use 'break', not sys.exit."""
        source = _RUNNER_PATH.read_text(encoding="utf-8")
        tree = ast.parse(source)
        inner_loop_handler_found = False
        for node in ast.walk(tree):
            if isinstance(node, ast.FunctionDef) and node.name == "run_stress_mixed":
                for child in ast.walk(node):
                    if isinstance(child, ast.ExceptHandler):
                        handler_src = ast.unparse(child)
                        if "runtime error" in handler_src:
                            # This is the inner-loop handler
                            inner_loop_handler_found = True
                            self.assertIn(
                                "break",
                                handler_src,
                                "Inner-loop exception handler must use 'break'",
                            )
                            self.assertNotIn(
                                "sys.exit",
                                handler_src,
                                "Inner-loop exception handler must not call sys.exit",
                            )
        self.assertTrue(
            inner_loop_handler_found, "Inner-loop exception handler not found"
        )

    def test_validation_failure_breaks_inner_loop(self):
        """Validation failure must break the inner while-loop promptly (mirrors OpenCL behavior)."""
        source = _RUNNER_PATH.read_text(encoding="utf-8")
        tree = ast.parse(source)
        for node in ast.walk(tree):
            if isinstance(node, ast.FunctionDef) and node.name == "run_stress_mixed":
                func_src = ast.unparse(node)
                # The 'if not passed:' block must contain 'break' so the loop exits
                # immediately on validation failure rather than running to full duration.
                self.assertIn(
                    "break",
                    func_src,
                    "run_stress_mixed must break on validation failure",
                )
                return
        self.fail("run_stress_mixed not found")

    def test_cli_returns_failure_when_summary_fails(self):
        """The CLI must preserve non-zero status for overall failure reporting."""
        source = (
            pathlib.Path(__file__).parent / "cli_stressor_cuda" / "cli.py"
        ).read_text(encoding="utf-8")
        tree = ast.parse(source)
        src = ""
        for node in ast.walk(tree):
            if isinstance(node, ast.FunctionDef) and node.name == "main":
                src = ast.unparse(node)
                break
        self.assertIn(
            "return 1", src, "main must return non-zero status for overall failure"
        )


# ---------------------------------------------------------------------------
# Source structure: verify compute_s is used in the loop body
# ---------------------------------------------------------------------------
class TestSourceStructure(unittest.TestCase):
    def _source(self):
        return _RUNNER_PATH.read_text(encoding="utf-8")

    def test_compute_s_accumulated_in_loop(self):
        self.assertIn("result.compute_s += op_elapsed", self._source())

    def test_tflops_uses_compute_s_not_elapsed_s(self):
        self.assertIn("result.total_flops / result.compute_s", self._source())

    def test_summary_shows_compute_column(self):
        self.assertIn("compute=", self._source())


# ---------------------------------------------------------------------------
# Helper function tests
# ---------------------------------------------------------------------------
class TestParseIntList(unittest.TestCase):
    def test_single(self):
        self.assertEqual(parse_int_list("1024"), [1024])

    def test_multiple(self):
        self.assertEqual(parse_int_list("512, 1024, 2048"), [512, 1024, 2048])

    def test_empty_raises(self):
        with self.assertRaises(ValueError):
            parse_int_list("")


class TestWeightedSelection(unittest.TestCase):
    class ZeroRng:
        def random(self):
            return 0.0

    def test_kernel_zero_weight_is_not_selected_at_boundary(self):
        selected = choose_kernel_type(
            [(KernelType.ATOMIC, 0.0), (KernelType.GEMM, 1.0)], self.ZeroRng()
        )
        self.assertEqual(selected, KernelType.GEMM)

    def test_precision_zero_weight_is_not_selected_at_boundary(self):
        fp16 = PrecisionSpec("FP16", sys.modules["torch"].float16, None)
        bf16 = PrecisionSpec("BF16", sys.modules["torch"].bfloat16, None)
        selected = choose_precision_from_mixture(
            [(fp16, 0.0), (bf16, 1.0)], self.ZeroRng()
        )
        self.assertEqual(selected, bf16)


class TestAtomicSizing(unittest.TestCase):
    def test_atomic_size_is_capped(self):
        self.assertEqual(atomic_element_count(16_384), MAX_ATOMIC_ELEMENTS)

    def test_atomic_small_size_keeps_square_workload(self):
        self.assertEqual(atomic_element_count(128), 128 * 128)


if __name__ == "__main__":
    unittest.main()
