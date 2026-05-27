from __future__ import annotations

import os
import random
import time
from typing import List

import torch

from .device import (
    detect_capability,
    empty_device_cache,
    maybe_set_tf32,
    synchronize_device,
)
from .kernels import (
    choose_kernel_type,
    choose_precision_from_mixture,
    estimate_kernel_work_flops,
    filter_supported_kernel_precisions,
    get_streams,
    make_random_matrix,
    resolve_kernel_params,
    run_kernel_path,
)
from .models import PrecisionSpec, StressResult, StressRunConfig
from .validation import validate_precision

_KERNEL_COLORS = {
    "GEMM": "\033[1;91m",
    "MEMCPY": "\033[1;92m",
    "MEMSET": "\033[1;93m",
    "TRANSPOSE": "\033[1;95m",
    "ELEMENTWISE": "\033[1;96m",
    "REDUCTION": "\033[1;94m",
    "ATOMIC": "\033[1;91m",
}
_RESET = "\033[0m"


def _stylize_line(line: str) -> str:
    if os.environ.get("NO_COLOR"):
        return line
    for name, code in _KERNEL_COLORS.items():
        line = line.replace(name, f"{code}{name}{_RESET}")
    return line


def run_stress_mixed(
    device: torch.device,
    precisions: List[PrecisionSpec],
    config: StressRunConfig,
) -> List[StressResult]:
    results = [StressResult(precision=spec.name, supported=True) for spec in precisions]
    index_by_name = {spec.name: idx for idx, spec in enumerate(precisions)}

    supported: List[PrecisionSpec] = []
    for spec in precisions:
        ok, reason = detect_capability(device, spec)
        if not ok:
            idx = index_by_name[spec.name]
            results[idx].supported = False
            results[idx].first_error = f"SKIP: {reason}"
            continue
        try:
            maybe_set_tf32(spec.tf32_enabled)
            probe_a = make_random_matrix(8, device, spec.dtype, 101)
            probe_b = make_random_matrix(8, device, spec.dtype, 102)
            _ = torch.mm(probe_a, probe_b)
            synchronize_device(device)
            del probe_a, probe_b
        except Exception as exc:
            idx = index_by_name[spec.name]
            results[idx].supported = False
            results[idx].first_error = f"probe failed: {exc}"
            continue
        supported.append(spec)

    if not supported:
        return results

    rng = random.Random(config.base_seed)
    start = time.monotonic()
    next_validate = max(0.0, config.validate_interval_s)
    validation_seed = config.base_seed ^ 0x5F3759DF
    effective_overrides = filter_supported_kernel_precisions(
        device, config.kernel_param_overrides
    )
    effective_config = StressRunConfig(
        matrix_sizes=config.matrix_sizes,
        fp64_matrix_sizes=config.fp64_matrix_sizes,
        duration_s=config.duration_s,
        warmup_iters=config.warmup_iters,
        burst_iters=config.burst_iters,
        validate_interval_s=config.validate_interval_s,
        validate_size=config.validate_size,
        transpose_prob=config.transpose_prob,
        base_seed=config.base_seed,
        minor_mixture_rate=config.minor_mixture_rate,
        kernel_mixture=config.kernel_mixture,
        stream_mode=config.stream_mode,
        kernel_param_overrides=effective_overrides,
    )
    streams = get_streams(device, effective_config.stream_mode)

    while time.monotonic() - start < config.duration_s:
        kernel_kind = choose_kernel_type(config.kernel_mixture, rng)
        params = resolve_kernel_params(kernel_kind, effective_config)
        if params.precision_mixture:
            op_spec = choose_precision_from_mixture(params.precision_mixture, rng)
            op_spec = op_spec or supported[0]
        elif params.precisions:
            op_spec = rng.choice(params.precisions)
        else:
            op_spec = rng.choice(supported)

        size_pool = (
            effective_config.fp64_matrix_sizes
            if op_spec.name == "FP64" and params.matrix_sizes_default
            else params.matrix_sizes
        )
        if rng.random() > params.minor_mixture_rate:
            size = rng.choice(size_pool)
        else:
            size = rng.choice([127, 256, 511, 512, 1023])

        op_seed = rng.getrandbits(64)
        try:
            op_elapsed = run_kernel_path(
                device=device,
                spec=op_spec,
                kind=kernel_kind,
                size=size,
                warmup_iters=params.warmup_iters,
                burst_iters=params.burst_iters,
                transpose_prob=params.transpose_prob,
                seed=op_seed,
                streams=streams,
            )
        except Exception as exc:
            idx = index_by_name[op_spec.name]
            results[idx].first_error = f"runtime error: {exc}"
            results[idx].first_error_at_s = time.monotonic() - start
            break

        flops = estimate_kernel_work_flops(kernel_kind, size, params.burst_iters)
        inst_tflops = (flops / op_elapsed / 1e12) if op_elapsed > 0 else 0.0
        elapsed_total = time.monotonic() - start

        print(
            _stylize_line(
                f"[MIX] t={elapsed_total:6.1f}s/{config.duration_s:.0f}s | "
                f"{kernel_kind.value:10} | p={op_spec.name:11} | "
                f"size={size:5d} | inst={inst_tflops:7.2f} TFLOPS(eqv)"
            )
        )

        idx = index_by_name[op_spec.name]
        result = results[idx]
        result.iterations += params.burst_iters
        result.total_flops += flops
        result.compute_s += op_elapsed
        if result.compute_s > 0:
            result.tflops = (result.total_flops / result.compute_s) / 1e12

        empty_device_cache(device)

        if elapsed_total >= next_validate:
            passed, max_abs, max_rel, reason = validate_precision(
                device=device,
                spec=op_spec,
                validate_size=effective_config.validate_size,
                seed=validation_seed,
            )
            status = "OK" if passed else "FAIL"
            print(
                f"[{op_spec.name}] validate | abs={max_abs:.3e} | rel={max_rel:.3e} | {status}"
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
            next_validate = elapsed_total + max(0.0, config.validate_interval_s)
            validation_seed += 1

        if op_elapsed < 0.01:
            time.sleep(0)

    total_elapsed = time.monotonic() - start
    for result in results:
        result.elapsed_s = total_elapsed
        if result.compute_s > 0:
            result.tflops = (result.total_flops / result.compute_s) / 1e12
    return results


def print_summary(
    device_name: str, total_memory_gb, results: List[StressResult]
) -> bool:
    print("\n" + "=" * 72)
    print("Phase 1 core stability summary")
    print(f"Device: {device_name}")
    if total_memory_gb is not None:
        print(f"Video Memory: {total_memory_gb:.1f} GB")

    overall_ok = True
    any_supported = False
    for r in results:
        if not r.supported:
            status = "SKIP"
        else:
            any_supported = True
            status = (
                "OK"
                if (r.first_error is None and r.validation_failures == 0)
                else "FAIL"
            )
        if status == "FAIL":
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
    print("Result:")
    if overall_ok:
        if any_supported:
            print(
                "- No obvious computation errors or validation failures were observed in the current test window."
            )
        else:
            print("- All requested precision modes were unsupported on this device.")
    else:
        print("- At least one precision mode reported an error or validation failure.")
    print("=" * 72)
    return overall_ok
