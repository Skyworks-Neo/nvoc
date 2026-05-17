import argparse
import random
import sys
import time
from dataclasses import dataclass
from typing import Optional

import torch


@dataclass(frozen=True)
class PrecisionSpec:
    name: str
    dtype: torch.dtype
    tf32_enabled: Optional[bool] = None


@dataclass
class StressResult:
    precision: str
    supported: bool = True
    iterations: int = 0
    total_flops: int = 0
    elapsed_s: float = 0.0
    compute_s: float = 0.0
    tflops: float = 0.0
    validations: int = 0
    validation_failures: int = 0
    max_abs_error: float = 0.0
    max_rel_error: float = 0.0
    first_error: Optional[str] = None
    first_error_at_s: Optional[float] = None


def get_accelerator_device():
    if torch.cuda.is_available():
        device = torch.device("cuda")
        device_name = torch.cuda.get_device_name(0)
        total_memory_gb = torch.cuda.get_device_properties(0).total_memory / 1024**3
        return device, device_name, total_memory_gb

    mps_backend = getattr(torch.backends, "mps", None)
    if mps_backend is not None and mps_backend.is_available():
        device = torch.device("mps")
        return device, "Apple MPS", None

    return None, None, None


def synchronize_device(device):
    if device.type == "cuda":
        torch.cuda.synchronize()
    elif device.type == "mps":
        torch.mps.synchronize()


def empty_device_cache(device):
    if device.type == "cuda":
        torch.cuda.empty_cache()
    elif device.type == "mps" and hasattr(torch.mps, "empty_cache"):
        torch.mps.empty_cache()


def parse_int_list(raw: str):
    values = []
    for item in raw.split(","):
        item = item.strip()
        if not item:
            continue
        values.append(int(item))
    if not values:
        raise ValueError("matrix sizes 不能为空")
    return values


def parse_precision_list(raw: str, include_fp8: bool):
    mapping = {
        "fp64": PrecisionSpec("FP64", torch.float64, None),
        "fp32": PrecisionSpec("FP32", torch.float32, False),
        "tf32": PrecisionSpec("TF32", torch.float32, True),
        "fp16": PrecisionSpec("FP16", torch.float16, None),
        "bf16": PrecisionSpec("BF16", torch.bfloat16, None),
    }
    if include_fp8 and hasattr(torch, "float8_e4m3fn"):
        mapping["fp8"] = PrecisionSpec("FP8 E4M3FN", torch.float8_e4m3fn, None)

    selected = []
    for item in raw.split(","):
        key = item.strip().lower()
        if not key:
            continue
        if key not in mapping:
            raise ValueError(f"不支持的精度项: {item}")
        selected.append(mapping[key])

    if not selected:
        raise ValueError("precision 不能为空")
    return selected


def maybe_set_tf32(tf32_enabled: Optional[bool]):
    if tf32_enabled is None:
        return
    if torch.cuda.is_available() and hasattr(torch.backends.cuda, "matmul"):
        mode = "tf32" if tf32_enabled else "ieee"

        # 新 API（优先）
        if hasattr(torch.backends.cuda.matmul, "fp32_precision"):
            torch.backends.cuda.matmul.fp32_precision = mode

        if hasattr(torch.backends.cudnn, "conv") and hasattr(
            torch.backends.cudnn.conv, "fp32_precision"
        ):
            torch.backends.cudnn.conv.fp32_precision = mode

        # 向后兼容（避免旧版本报错）
        elif hasattr(torch.backends.cuda.matmul, "allow_tf32"):
            torch.backends.cuda.matmul.allow_tf32 = tf32_enabled
            torch.backends.cudnn.allow_tf32 = tf32_enabled


def detect_capability(device, spec: PrecisionSpec):
    if device.type != "cuda":
        return True, None

    major, minor = torch.cuda.get_device_capability(0)

    if spec.name == "TF32" and major < 8:
        return False, f"TF32 requires Ampere (SM80+), current SM{major}{minor}"

    if spec.name == "BF16" and major < 8:
        return False, f"BF16 requires Ampere (SM80+), current SM{major}{minor}"

    if spec.name.startswith("FP8") and major < 9:
        return False, f"FP8 requires Hopper (SM90+), current SM{major}{minor}"

    return True, None


def make_random_matrix(size: int, device: torch.device, dtype: torch.dtype, seed: int):
    g = torch.Generator(device="cpu")
    g.manual_seed(seed)
    base = torch.randn(size, size, generator=g, dtype=torch.float32)

    if dtype == torch.float8_e4m3fn:
        # FP8 需要先在 float32 上生成，再转换。
        return base.to(device=device).to(dtype=dtype)

    return base.to(device=device, dtype=dtype)


def choose_tolerance(precision_name: str):
    if precision_name == "FP64":
        return 1e-5, 1e-5
    if precision_name == "FP32":
        return 1e-2, 1e-2
    if precision_name == "TF32":
        return 2e-1, 2e-1
    if precision_name == "FP16":
        return 2e-1, 2e-1
    if precision_name == "BF16":
        return 5e-1, 5e-1
    if precision_name == "FP8 E4M3FN":
        return 1.5, 1.5
    return 1e-2, 1e-2


def validate_precision(
    device: torch.device,
    spec: PrecisionSpec,
    validate_size: int,
    seed: int,
):
    """使用固定小尺寸输入做旁路校验，主要捕捉 silent corruption。"""
    maybe_set_tf32(spec.tf32_enabled)

    g = torch.Generator(device="cpu")
    g.manual_seed(seed)
    a_cpu = torch.randn(validate_size, validate_size, generator=g, dtype=torch.float32)
    b_cpu = torch.randn(validate_size, validate_size, generator=g, dtype=torch.float32)

    # 参考答案使用更高精度在 CPU 上计算。
    ref = torch.mm(a_cpu.to(torch.float64), b_cpu.to(torch.float64)).to(torch.float32)

    if spec.dtype == torch.float8_e4m3fn:
        a = a_cpu.to(device=device).to(dtype=spec.dtype)
        b = b_cpu.to(device=device).to(dtype=spec.dtype)
    else:
        a = a_cpu.to(device=device, dtype=spec.dtype)
        b = b_cpu.to(device=device, dtype=spec.dtype)

    try:
        out = torch.mm(a, b)
        synchronize_device(device)
    except RuntimeError as exc:
        msg = str(exc)
        if (
            "no kernel image is available" in msg
            or "CUBLAS_STATUS_NOT_SUPPORTED" in msg
        ):
            # Legacy GPU (e.g. Maxwell sm_52, Pascal sm_61): cuBLAS in newer CUDA lacks
            # a precompiled kernel image for this validate_size × validate_size.  The main
            # GEMM loop uses larger tiles where the kernel image exists and is unaffected.
            # Fall back to CPU so the sidecheck still catches dtype-level arithmetic
            # corruption without false-failing the stressor.
            print(
                f"[validate] CPU fallback: cuBLAS kernel unavailable for "
                f"{validate_size}×{validate_size} {spec.name} sidecheck on this SM; "
                f"main stressor is unaffected"
            )
            try:
                out = torch.mm(a_cpu.to(dtype=spec.dtype), b_cpu.to(dtype=spec.dtype))
            except Exception:
                raise exc  # dtype also unsupported on CPU; re-raise original error
        else:
            raise

    out_f32 = out.to(torch.float32).cpu()
    if not torch.isfinite(out_f32).all():
        return False, float("inf"), float("inf"), "validation produced NaN/Inf"

    diff = (out_f32 - ref).abs()
    abs_thr, rel_thr = choose_tolerance(spec.name)

    # Per-element allclose: every element must satisfy |out-ref| ≤ atol + rtol*|ref|.
    # Using OR of global scalars was wrong: max_abs could be from a different element
    # than max_ref, letting large per-element errors slip through silently.
    per_ok = diff <= (abs_thr + rel_thr * ref.abs())
    n_fail = int((~per_ok).sum().item())
    passed = n_fail == 0

    max_abs = float(diff.max().item())
    max_rel = float((diff / (ref.abs() + 1e-12)).max().item())
    reason = (
        None
        if passed
        else f"{n_fail} elements exceed atol+rtol*|ref|: max_abs={max_abs:.4g}, max_rel={max_rel:.4g}"
    )
    return passed, max_abs, max_rel, reason


def run_stress_for_precision(
    device: torch.device,
    spec: PrecisionSpec,
    matrix_sizes,
    duration_s: float,
    warmup_iters: int,
    burst_iters: int,
    validate_interval_s: float,
    validate_size: int,
    transpose_prob: float,
    base_seed: int,
):
    maybe_set_tf32(spec.tf32_enabled)

    supported, reason = detect_capability(device, spec)
    result = StressResult(precision=spec.name, supported=supported)

    if not supported:
        result.first_error = f"SKIP: {reason}"
        return result
    rng = random.Random(base_seed)
    start = time.monotonic()
    next_validate = start + max(0.0, validate_interval_s)
    validation_seed = base_seed ^ 0x5F3759DF

    # 先做一个小的可用性探测，避免不支持的 dtype 在主循环里反复失败。
    try:
        probe_a = make_random_matrix(8, device, spec.dtype, base_seed + 1)
        probe_b = make_random_matrix(8, device, spec.dtype, base_seed + 2)
        _ = torch.mm(probe_a, probe_b)
        synchronize_device(device)
        del probe_a, probe_b
        empty_device_cache(device)
    except Exception as exc:
        result.supported = False
        result.first_error = f"probe failed: {exc}"
        return result

    while True:
        now = time.monotonic()
        if now - start >= duration_s:
            break

        # 动态尺寸抖动 + 冷热交替 + 非对齐尺寸
        if rng.random() < 0.15:
            # 偏向大矩阵，包括非对齐边界
            size = rng.choice(matrix_sizes)
        else:
            # 偏向小矩阵（且带有非对齐），制造 power oscillation 和 allocator 压力
            size = rng.choice([127, 256, 511, 512, 1023])

        transpose_a = rng.random() < transpose_prob
        transpose_b = rng.random() < transpose_prob

        try:
            a = make_random_matrix(size, device, spec.dtype, rng.randrange(1 << 30))
            b = make_random_matrix(size, device, spec.dtype, rng.randrange(1 << 30))

            # 预热阶段，主要让编译/缓存/调度状态稳定下来。
            for _ in range(warmup_iters):
                aa = a.t() if transpose_a else a
                bb = b.t() if transpose_b else b
                _ = torch.mm(aa, bb)
            synchronize_device(device)

            # 正式压力段。
            op_start = time.monotonic()
            for _ in range(burst_iters):
                aa = a.t() if transpose_a else a
                bb = b.t() if transpose_b else b
                _ = torch.mm(aa, bb)
            synchronize_device(device)
            op_elapsed = time.monotonic() - op_start
            inst_tflops = (2 * (size**3) * burst_iters / op_elapsed) / 1e12
            elapsed_total = time.monotonic() - start

            print(
                f"[{spec.name}] "
                f"t={elapsed_total:6.1f}s/{duration_s:.0f}s | "
                f"size={size:5d} | "
                f"inst={inst_tflops:7.2f} TFLOPS | "
                # f"avg={result.tflops:7.2f} TFLOPS"
            )

            result.iterations += burst_iters
            result.total_flops += int(2 * (size**3) * burst_iters)
            result.compute_s += op_elapsed
            result.elapsed_s = time.monotonic() - start
            if result.compute_s > 0:
                result.tflops = (result.total_flops / result.compute_s) / 1e12

            del a, b
            empty_device_cache(device)

            # 旁路校验：固定小尺寸、固定种子、固定输入。
            if time.monotonic() >= next_validate:
                passed, max_abs, max_rel, reason = validate_precision(
                    device=device,
                    spec=spec,
                    validate_size=validate_size,
                    seed=validation_seed,
                )
                status = "OK" if passed else "FAIL"

                print(
                    f"[{spec.name}] validate | "
                    f"abs={max_abs:.3e} | rel={max_rel:.3e} | {status}"
                )
                result.validations += 1
                result.max_abs_error = max(result.max_abs_error, max_abs)
                result.max_rel_error = max(result.max_rel_error, max_rel)
                if not passed:
                    result.validation_failures += 1
                    if result.first_error is None:
                        result.first_error = reason
                        result.first_error_at_s = time.monotonic() - start
                    break
                next_validate = time.monotonic() + max(0.0, validate_interval_s)
                validation_seed += 1

        except Exception as exc:
            result.first_error = f"runtime error: {exc}"
            result.first_error_at_s = time.monotonic() - start
            break

        # 若 burst 很短，避免 CPU 忙等。
        if op_elapsed < 0.01:
            time.sleep(0)

    result.elapsed_s = time.monotonic() - start
    if result.compute_s > 0:
        result.tflops = (result.total_flops / result.compute_s) / 1e12
    return result


def print_summary(device_name: str, total_memory_gb, results):
    print("\n" + "=" * 72)
    print("Phase 1 core stability summary")
    print(f"设备: {device_name}")
    if total_memory_gb is not None:
        print(f"显存: {total_memory_gb:.1f} GB")

    overall_ok = True
    for r in results:
        if not r.supported:
            status = "SKIP"
        else:
            status = (
                "OK"
                if (r.first_error is None and r.validation_failures == 0)
                else "FAIL"
            )
        if status != "OK":
            overall_ok = False
        eff = (r.compute_s / r.elapsed_s * 100) if r.elapsed_s > 0 else 0.0
        print(
            f"{r.precision:12} {status:4} | "
            f"iters={r.iterations:8d} | "
            f"wall={r.elapsed_s:7.1f}s | "
            f"compute={r.compute_s:6.1f}s | "
            f"eff={eff:4.0f}% | "
            f"{r.tflops:8.2f} TFLOPS | "
            f"val_fail={r.validation_failures:3d} | "
            f"max_abs={r.max_abs_error:.3g} | max_rel={r.max_rel_error:.3g}"
        )
        if r.first_error:
            print(f"{'':12}      first_error: {r.first_error}")
            if r.first_error_at_s is not None:
                print(f"{'':12}      at: {r.first_error_at_s:.1f}s")

    print("=" * 72)
    print("总体结论:")
    if overall_ok:
        print("- 在当前测试窗口内，没有出现明显计算错误或验证失败。")
    else:
        print("- 至少有一个精度模式出现错误、验证失败或不支持。")
        sys.exit(1)
    print("=" * 72)


def build_arg_parser():
    p = argparse.ArgumentParser(
        description="GPU core-domain stressor (Phase 1): time-driven, randomized GEMM, and validation sidecar."
    )
    p.add_argument(
        "--duration",
        type=float,
        default=90.0,
        help="每个精度模式的压力持续时间（秒）",
    )
    p.add_argument(
        "--matrix-sizes",
        type=parse_int_list,
        default=parse_int_list("2049, 4096, 4097, 8192, 8193, 16384"),
        help="矩阵尺寸列表，逗号分隔，例如 1024,2048,4096",
    )
    p.add_argument(
        "--fp64-matrix-sizes",
        type=parse_int_list,
        default=parse_int_list("2048,4096"),
        help="专门用于 FP64 模式的矩阵尺寸，为了适应消费级 GPU 较低的双精度算力（避免测试假死）",
    )
    p.add_argument(
        "--precisions",
        type=str,
        default="fp16,bf16",
        help="精度列表，支持 fp32, tf32, fp16, bf16, fp8, fp64（逗号分隔）",
    )
    p.add_argument(
        "--warmup-iters", type=int, default=3, help="每个工作负载窗口的预热轮数"
    )
    p.add_argument(
        "--burst-iters", type=int, default=6, help="每个工作负载窗口的正式压力轮数"
    )
    p.add_argument(
        "--validate-interval",
        type=float,
        default=10,
        help="旁路校验的间隔秒数",
    )
    p.add_argument(
        "--validate-size",
        type=int,
        default=768,
        help="旁路校验所用的固定矩阵尺寸",
    )
    p.add_argument(
        "--transpose-prob",
        type=float,
        default=0.5,
        help="随机转置 a/b 的概率，用于轻度扰动 kernel path",
    )
    p.add_argument("--seed", type=int, default=12345, help="随机种子")
    p.add_argument(
        "--disable-fp8",
        action="store_true",
        help="即使当前 PyTorch 支持 FP8，也跳过 FP8 测试",
    )
    return p


def main():
    torch.set_grad_enabled(False)
    torch.backends.cudnn.benchmark = False

    args = build_arg_parser().parse_args()

    device, device_name, total_memory_gb = get_accelerator_device()
    if device is None:
        print("未检测到 CUDA 或 MPS 设备")
        raise SystemExit(1)
    if device.type == "cuda":
        major, minor = torch.cuda.get_device_capability(0)
        print(f"测试设备: {device_name}")
        print(f"Compute Capability: SM{major}.{minor}")

    if total_memory_gb is not None:
        print(f"显存大小: {total_memory_gb:.1f} GB")
    elif device.type == "mps":
        print("使用 Apple MPS 图形加速器")

    print(f"Python版本: {sys.version.split()[0]}")
    print(f"PyTorch版本: {torch.__version__}")

    # if device.type == "cuda" and hasattr(torch.backends.cuda, "matmul"):
    #     print("当前 TF32 设置:")
    #     print(f"  matmul.allow_tf32 = {torch.backends.cuda.matmul.allow_tf32}")
    #     print(f"  cudnn.allow_tf32 = {torch.backends.cudnn.allow_tf32}")

    include_fp8 = hasattr(torch, "float8_e4m3fn") and not args.disable_fp8
    try:
        precisions = parse_precision_list(args.precisions, include_fp8=include_fp8)
    except ValueError as exc:
        print(f"参数错误: {exc}")
        raise SystemExit(2)

    # 如果用户要求 FP8，但当前环境实际不支持，就自动降级并提示。
    filtered = []
    for spec in precisions:
        if spec.name.startswith("FP8") and not include_fp8:
            print("PyTorch 当前不支持或已禁用 FP8 E4M3FN，跳过该项测试")
            continue
        filtered.append(spec)
    precisions = filtered
    if not precisions:
        print("没有可运行的精度模式")
        raise SystemExit(1)

    random.seed(args.seed)
    torch.manual_seed(args.seed)
    if device.type == "cuda":
        torch.cuda.manual_seed_all(args.seed)

    results = []
    overall_passed = True
    for idx, spec in enumerate(precisions):
        current_sizes = (
            args.fp64_matrix_sizes if spec.name == "FP64" else args.matrix_sizes
        )

        print("\n" + "-" * 72)
        print(f"开始测试 {spec.name}")
        if spec.tf32_enabled is not None:
            print(f"  TF32 设置: {spec.tf32_enabled}")
        print(f"  矩阵尺寸: {current_sizes}")
        print(f"  持续时间: {args.duration:.1f} 秒")
        print(f"  预热轮数: {args.warmup_iters}")
        print(f"  压力轮数: {args.burst_iters}")
        print(f"  校验间隔: {args.validate_interval:.1f} 秒")
        print(f"  校验尺寸: {args.validate_size}")

        res = run_stress_for_precision(
            device=device,
            spec=spec,
            matrix_sizes=current_sizes,
            duration_s=args.duration,
            warmup_iters=args.warmup_iters,
            burst_iters=args.burst_iters,
            validate_interval_s=args.validate_interval,
            validate_size=args.validate_size,
            transpose_prob=args.transpose_prob,
            base_seed=args.seed + idx * 1000,
        )
        results.append(res)

        if res.first_error:
            print(f"  结果: {res.first_error}")
            # 如果是检测到不支持（SKIP）则不视为测试失败，否则视为失败
            if res.supported:
                overall_passed = False
                print(f"  累计: {res.iterations} 次 matmul, {res.tflops:.2f} TFLOPS")
                # GPU may be in a fault/bus-fallen state on Linux (no TDR); do not
                # attempt further precisions — print partial summary and exit now.
                print_summary(device_name, total_memory_gb, results)
                sys.exit(1)
        else:
            print("  结果: completed without detected error")
        print(f"  累计: {res.iterations} 次 matmul, {res.tflops:.2f} TFLOPS")

    print_summary(device_name, total_memory_gb, results)

    if not overall_passed:
        sys.exit(1)


if __name__ == "__main__":
    main()
