from __future__ import annotations

import random
import time
from contextlib import nullcontext
from dataclasses import dataclass
from typing import List, Optional, Tuple

import torch

from .device import (
    detect_capability,
    empty_device_cache,
    maybe_set_tf32,
    synchronize_device,
)
from .models import KernelParamOverride, KernelType, PrecisionSpec, StressRunConfig


MAX_ATOMIC_ELEMENTS = 4_194_304


def make_random_matrix(size: int, device: torch.device, dtype: torch.dtype, seed: int):
    g = torch.Generator(device="cpu")
    g.manual_seed(seed)
    base = torch.randn(size, size, generator=g, dtype=torch.float32)

    if hasattr(torch, "float8_e4m3fn") and dtype == torch.float8_e4m3fn:
        return base.to(device=device).to(dtype=dtype)

    return base.to(device=device, dtype=dtype)


def make_random_vector(
    length: int, device: torch.device, dtype: torch.dtype, seed: int
):
    g = torch.Generator(device="cpu")
    g.manual_seed(seed)
    base = torch.randn(length, generator=g, dtype=torch.float32)

    if hasattr(torch, "float8_e4m3fn") and dtype == torch.float8_e4m3fn:
        return base.to(device=device).to(dtype=dtype)

    return base.to(device=device, dtype=dtype)


def atomic_element_count(size: int) -> int:
    return min(int(size) * int(size), MAX_ATOMIC_ELEMENTS)


def choose_kernel_type(
    mixture: List[Tuple[KernelType, float]], rng: random.Random
) -> KernelType:
    if not mixture:
        return KernelType.GEMM
    total_weight = sum(max(weight, 0.0) for _, weight in mixture)
    if total_weight <= 0.0:
        return mixture[0][0]
    pick = rng.random() * total_weight
    for kind, weight in mixture:
        weight = max(weight, 0.0)
        if weight > 0.0 and pick < weight:
            return kind
        pick -= weight
    return next(kind for kind, weight in reversed(mixture) if weight > 0.0)


def choose_precision_from_mixture(
    mixture: List[Tuple[PrecisionSpec, float]], rng: random.Random
) -> Optional[PrecisionSpec]:
    if not mixture:
        return None
    total_weight = sum(max(weight, 0.0) for _, weight in mixture)
    if total_weight <= 0.0:
        return mixture[0][0]
    pick = rng.random() * total_weight
    for spec, weight in mixture:
        weight = max(weight, 0.0)
        if weight > 0.0 and pick < weight:
            return spec
        pick -= weight
    return next(spec for spec, weight in reversed(mixture) if weight > 0.0)


def estimate_kernel_work_flops(kind: KernelType, size: int, burst_iters: int) -> int:
    n = int(size)
    iters = int(burst_iters)
    if kind == KernelType.GEMM:
        return 2 * n * n * n * iters
    if kind in (KernelType.TRANSPOSE, KernelType.ELEMENTWISE):
        return 2 * n * n * iters
    if kind == KernelType.ATOMIC:
        return atomic_element_count(n) * iters
    return n * n * iters


def get_streams(device: torch.device, stream_mode) -> List[torch.cuda.Stream]:
    if device.type != "cuda":
        return []
    return [torch.cuda.Stream() for _ in range(stream_mode.stream_count())]


@dataclass
class ResolvedKernelParams:
    precisions: Optional[List[PrecisionSpec]]
    precision_mixture: Optional[List[Tuple[PrecisionSpec, float]]]
    matrix_sizes: List[int]
    matrix_sizes_default: bool
    warmup_iters: int
    burst_iters: int
    transpose_prob: float
    minor_mixture_rate: float


def resolve_kernel_params(
    kind: KernelType, config: StressRunConfig
) -> ResolvedKernelParams:
    override_item = next(
        (item for item in config.kernel_param_overrides if item.kind == kind), None
    )
    matrix_sizes_default = override_item is None or override_item.matrix_sizes is None
    matrix_sizes = (
        override_item.matrix_sizes
        if override_item and override_item.matrix_sizes is not None
        else list(config.matrix_sizes)
    )
    return ResolvedKernelParams(
        precisions=override_item.precisions if override_item else None,
        precision_mixture=override_item.precision_mixture if override_item else None,
        matrix_sizes=matrix_sizes,
        matrix_sizes_default=matrix_sizes_default,
        warmup_iters=(
            override_item.warmup_iters
            if override_item and override_item.warmup_iters is not None
            else config.warmup_iters
        ),
        burst_iters=(
            override_item.burst_iters
            if override_item and override_item.burst_iters is not None
            else config.burst_iters
        ),
        transpose_prob=(
            override_item.transpose_prob
            if override_item and override_item.transpose_prob is not None
            else config.transpose_prob
        ),
        minor_mixture_rate=(
            override_item.minor_mixture_rate
            if override_item and override_item.minor_mixture_rate is not None
            else config.minor_mixture_rate
        ),
    )


def filter_supported_kernel_precisions(
    device: torch.device, overrides: List[KernelParamOverride]
) -> List[KernelParamOverride]:
    filtered: List[KernelParamOverride] = []
    for item in overrides:
        cloned = KernelParamOverride(kind=item.kind)
        if item.precisions is not None:
            supported = []
            for spec in item.precisions:
                ok, _ = detect_capability(device, spec)
                if ok:
                    supported.append(spec)
                else:
                    print(
                        f"Kernel {item.kind.value} precision {spec.name} unsupported on this device, skipping it"
                    )
            cloned.precisions = supported if supported else None
        if item.precision_mixture is not None:
            supported_mix = []
            for spec, weight in item.precision_mixture:
                ok, _ = detect_capability(device, spec)
                if ok:
                    supported_mix.append((spec, weight))
                else:
                    print(
                        f"Kernel {item.kind.value} precision {spec.name} unsupported on this device, skipping it"
                    )
            cloned.precision_mixture = supported_mix if supported_mix else None
        cloned.matrix_sizes = item.matrix_sizes
        cloned.warmup_iters = item.warmup_iters
        cloned.burst_iters = item.burst_iters
        cloned.transpose_prob = item.transpose_prob
        cloned.minor_mixture_rate = item.minor_mixture_rate
        filtered.append(cloned)
    return filtered


def run_kernel_path(
    device: torch.device,
    spec: PrecisionSpec,
    kind: KernelType,
    size: int,
    warmup_iters: int,
    burst_iters: int,
    transpose_prob: float,
    seed: int,
    streams: List[torch.cuda.Stream],
) -> float:
    maybe_set_tf32(spec.tf32_enabled)
    op_rng = random.Random(seed)
    stream = None
    if streams:
        stream = streams[seed % len(streams)]
    context = torch.cuda.stream(stream) if stream is not None else nullcontext()

    with context:
        if kind == KernelType.GEMM:
            a = make_random_matrix(size, device, spec.dtype, op_rng.randrange(1 << 30))
            b = make_random_matrix(size, device, spec.dtype, op_rng.randrange(1 << 30))
            transpose_a = op_rng.random() < transpose_prob
            transpose_b = op_rng.random() < transpose_prob
            for _ in range(warmup_iters):
                aa = a.t() if transpose_a else a
                bb = b.t() if transpose_b else b
                _ = torch.mm(aa, bb)
            synchronize_device(device)
            op_start = time.monotonic()
            for _ in range(burst_iters):
                aa = a.t() if transpose_a else a
                bb = b.t() if transpose_b else b
                _ = torch.mm(aa, bb)
            synchronize_device(device)
            return time.monotonic() - op_start

        if kind == KernelType.MEMCPY:
            src = make_random_matrix(
                size, device, spec.dtype, op_rng.randrange(1 << 30)
            )
            for _ in range(warmup_iters):
                _ = src.clone()
            synchronize_device(device)
            op_start = time.monotonic()
            for _ in range(burst_iters):
                _ = src.clone()
            synchronize_device(device)
            return time.monotonic() - op_start

        if kind == KernelType.MEMSET:
            dst = make_random_matrix(
                size, device, spec.dtype, op_rng.randrange(1 << 30)
            )
            fill_value = float((seed % 97) - 48)
            for _ in range(warmup_iters):
                dst.fill_(fill_value)
            synchronize_device(device)
            op_start = time.monotonic()
            for _ in range(burst_iters):
                dst.fill_(fill_value)
            synchronize_device(device)
            return time.monotonic() - op_start

        if kind == KernelType.TRANSPOSE:
            a = make_random_matrix(size, device, spec.dtype, op_rng.randrange(1 << 30))
            for _ in range(warmup_iters):
                _ = a.t().contiguous()
            synchronize_device(device)
            op_start = time.monotonic()
            for _ in range(burst_iters):
                _ = a.t().contiguous()
            synchronize_device(device)
            return time.monotonic() - op_start

        if kind == KernelType.ELEMENTWISE:
            a = make_random_matrix(size, device, spec.dtype, op_rng.randrange(1 << 30))
            b = make_random_matrix(size, device, spec.dtype, op_rng.randrange(1 << 30))
            for _ in range(warmup_iters):
                _ = a + b
            synchronize_device(device)
            op_start = time.monotonic()
            for _ in range(burst_iters):
                _ = a + b
            synchronize_device(device)
            return time.monotonic() - op_start

        if kind == KernelType.REDUCTION:
            a = make_random_matrix(size, device, spec.dtype, op_rng.randrange(1 << 30))
            for _ in range(warmup_iters):
                _ = a.sum()
            synchronize_device(device)
            op_start = time.monotonic()
            for _ in range(burst_iters):
                _ = a.sum()
            synchronize_device(device)
            return time.monotonic() - op_start

        if kind == KernelType.ATOMIC:
            length = atomic_element_count(size)
            indices = torch.randint(
                0, length, (length,), device=device, dtype=torch.int64
            )
            values = make_random_vector(
                length, device, spec.dtype, op_rng.randrange(1 << 30)
            )
            dst = torch.zeros(length, device=device, dtype=spec.dtype)
            for _ in range(warmup_iters):
                dst.scatter_add_(0, indices, values)
            synchronize_device(device)
            op_start = time.monotonic()
            for _ in range(burst_iters):
                dst.scatter_add_(0, indices, values)
            synchronize_device(device)
            return time.monotonic() - op_start

    raise ValueError(f"unsupported kernel type: {kind}")


def empty_cache(device: torch.device) -> None:
    empty_device_cache(device)
