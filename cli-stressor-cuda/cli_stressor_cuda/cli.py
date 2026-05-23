from __future__ import annotations

import sys

import torch

from .config import (
    apply_file_config_to_args,
    load_kernel_overrides_from_config,
    load_toml_config,
    merge_kernel_overrides,
)
from .device import get_accelerator_device
from .models import StressRunConfig
from .parsing import (
    build_precision_mapping,
    parse_int_list,
    parse_kernel_mixture,
    parse_kernel_param_overrides,
    parse_kernel_type_list,
    parse_precision_list,
    parse_stream_mode,
)
from .runner import print_summary, run_stress_mixed
from .models import KernelType


def build_arg_parser():
    import argparse

    p = argparse.ArgumentParser(
        description="GPU core-domain stressor (Phase 1): mixed kernel-path stress + validation sidecar."
    )
    p.add_argument(
        "--config",
        type=str,
        default=None,
        help=(
            "Optional TOML config file. Supports [kernel_params.<kernel>] for per-kernel "
            "precisions/precision_mixture/matrix_sizes/warmup_iters/burst_iters/transpose_prob/minor_mixture_rate"
        ),
    )
    p.add_argument(
        "--duration",
        type=float,
        default=90.0,
        help="Stress duration per precision mode, in seconds",
    )
    p.add_argument(
        "--matrix-sizes",
        type=parse_int_list,
        default=parse_int_list("2049, 4096, 4097, 8192, 8193, 16384"),
        help="Comma-separated matrix sizes, e.g. 1024,2048,4096",
    )
    p.add_argument(
        "--fp64-matrix-sizes",
        type=parse_int_list,
        default=parse_int_list("2048,4096"),
        help="FP64 matrix sizes for consumer GPUs with lower FP64 throughput",
    )
    p.add_argument(
        "--precisions",
        type=str,
        default="fp16,bf16",
        help="Precision list: fp32, tf32, fp16, bf16, fp8, fp64 (comma-separated)",
    )
    p.add_argument(
        "--warmup-iters",
        type=int,
        default=3,
        help="Warmup iterations per workload window",
    )
    p.add_argument(
        "--burst-iters",
        type=int,
        default=6,
        help="Stress iterations per workload window",
    )
    p.add_argument(
        "--validate-interval",
        type=float,
        default=10,
        help="Validation interval in seconds",
    )
    p.add_argument(
        "--validate-size",
        type=int,
        default=1024,
        help="Validation matrix size",
    )
    p.add_argument(
        "--transpose-prob",
        type=float,
        default=0.5,
        help="Probability of transposing A/B to perturb kernel path",
    )
    p.add_argument(
        "--minor-mixture-rate",
        type=float,
        default=0.15,
        help="Small-size mixture rate",
    )
    p.add_argument("--seed", type=int, default=12345, help="Random seed")
    p.add_argument(
        "--kernel-types",
        type=str,
        default="gemm,memcpy,memset,transpose,elementwise,reduction,atomic",
        help=(
            "Enabled kernel paths (comma-separated): gemm,memcpy,memset,transpose,elementwise,reduction,atomic"
        ),
    )
    p.add_argument(
        "--kernel-mixture",
        type=str,
        default="",
        help=(
            "Kernel mixture weights as type:weight pairs, e.g. gemm:0.5,memcpy:0.3,reduction:0.2 "
            "(empty = equal weights)"
        ),
    )
    p.add_argument(
        "--kernel-params",
        type=str,
        default="",
        help=(
            "Per-kernel overrides, e.g. 'gemm:precisions=fp16|bf16,precision_mixture=fp16:0.7|bf16:0.3,"
            "matrix_sizes=2049|4096,warmup=4,burst=8;memcpy:matrix_sizes=8192|16384,burst=64'"
        ),
    )
    p.add_argument(
        "--stream-mode",
        type=str,
        default="single",
        help="Submission stream mode: single|dual|triple",
    )
    p.add_argument(
        "--disable-fp8",
        action="store_true",
        help="Skip FP8 even if the runtime supports it",
    )
    return p


def filter_atomic_for_sm(kernel_types, device):
    if device.type != "cuda":
        return kernel_types
    major, _ = torch.cuda.get_device_capability(0)
    if major >= 8:
        return kernel_types
    if KernelType.ATOMIC in kernel_types:
        print(f"Atomic path disabled: current GPU is below SM80 (detected SM{major})")
        return [kind for kind in kernel_types if kind != KernelType.ATOMIC]
    return kernel_types


def main() -> int:
    torch.set_grad_enabled(False)
    torch.backends.cudnn.benchmark = False

    args = build_arg_parser().parse_args()
    config = None
    if args.config:
        try:
            config = load_toml_config(args.config)
            apply_file_config_to_args(args, sys.argv[1:], config)
        except ValueError as exc:
            print(f"Invalid config file: {exc}")
            return 2

    device, device_name, total_memory_gb = get_accelerator_device()
    if device is None:
        print("No CUDA or MPS device detected")
        return 1
    if device.type == "cuda":
        major, minor = torch.cuda.get_device_capability(0)
        print(f"Testing Device: {device_name}")
        print(f"Compute Capability: SM{major}.{minor}")

    if total_memory_gb is not None:
        print(f"Video Memory: {total_memory_gb:.1f} GB")
    elif device.type == "mps":
        print("Using Apple MPS accelerator")

    print(f"Python: {sys.version.split()[0]}")
    print(f"PyTorch: {torch.__version__}")

    has_fp8 = hasattr(torch, "float8_e4m3fn")
    include_fp8 = has_fp8 and not args.disable_fp8
    try:
        precisions = parse_precision_list(args.precisions, include_fp8=include_fp8)
    except ValueError as exc:
        print(f"Invalid argument: {exc}")
        return 2

    filtered = []
    for spec in precisions:
        if spec.name.startswith("FP8") and not include_fp8:
            print("FP8 E4M3FN unsupported or disabled, skipping")
            continue
        filtered.append(spec)
    precisions = filtered
    if not precisions:
        print("No runnable precision modes available")
        return 0

    kernel_types_all = parse_kernel_type_list(args.kernel_types)
    kernel_types = filter_atomic_for_sm(kernel_types_all, device)
    if not kernel_types:
        print("No runnable kernel types after capability filtering")
        return 1

    if args.kernel_mixture.strip():
        kernel_mixture = parse_kernel_mixture(args.kernel_mixture, kernel_types_all)
    else:
        kernel_mixture = parse_kernel_mixture(args.kernel_mixture, kernel_types)
    if kernel_types != kernel_types_all:
        kernel_mixture = [entry for entry in kernel_mixture if entry[0] in kernel_types]
        if not kernel_mixture:
            kernel_mixture = [(kind, 1.0) for kind in kernel_types]

    try:
        stream_mode = parse_stream_mode(args.stream_mode)
    except ValueError as exc:
        print(f"Invalid stream mode argument: {exc}")
        return 2

    mapping_all = build_precision_mapping(has_fp8)
    config_overrides = (
        load_kernel_overrides_from_config(config, mapping_all) if config else []
    )
    cli_overrides = parse_kernel_param_overrides(args.kernel_params, mapping_all)
    kernel_param_overrides = merge_kernel_overrides(config_overrides, cli_overrides)

    print("\n" + "-" * 72)
    print("Starting mixed-kernel stress")
    print(f"  Precisions: {[spec.name for spec in precisions]}")
    print(f"  Duration: {args.duration:.1f} s")
    print(f"  Warmup iterations: {args.warmup_iters}")
    print(f"  Burst iterations: {args.burst_iters}")
    print(f"  Validation interval: {args.validate_interval:.1f} s")
    print(f"  Validation size: {args.validate_size}")
    print(f"  Minor mixture rate: {args.minor_mixture_rate:.2f}")
    print(f"  Kernel types: {[kind.value for kind in kernel_types]}")
    print(
        f"  Kernel mixture: {[(kind.value, weight) for kind, weight in kernel_mixture]}"
    )
    print(f"  Stream mode: {stream_mode.value} ({stream_mode.stream_count()} streams)")

    results = run_stress_mixed(
        device=device,
        precisions=precisions,
        config=StressRunConfig(
            matrix_sizes=args.matrix_sizes,
            fp64_matrix_sizes=args.fp64_matrix_sizes,
            duration_s=args.duration,
            warmup_iters=args.warmup_iters,
            burst_iters=args.burst_iters,
            validate_interval_s=args.validate_interval,
            validate_size=args.validate_size,
            transpose_prob=args.transpose_prob,
            base_seed=args.seed,
            minor_mixture_rate=args.minor_mixture_rate,
            kernel_mixture=kernel_mixture,
            stream_mode=stream_mode,
            kernel_param_overrides=kernel_param_overrides,
        ),
    )

    overall_ok = all(not (res.first_error and res.supported) for res in results)
    summary_ok = print_summary(device_name, total_memory_gb, results)
    if not (overall_ok and summary_ok):
        return 1
    return 0
