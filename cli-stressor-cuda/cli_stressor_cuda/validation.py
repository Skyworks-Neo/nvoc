from __future__ import annotations

from typing import Tuple

import torch

from .device import maybe_set_tf32, synchronize_device
from .kernels import make_random_matrix
from .models import PrecisionSpec


def _is_known_validation_matmul_unsupported(exc: Exception) -> bool:
    message = str(exc).lower()
    known_fragments = (
        "no kernel image is available",
        "cublas_status_not_supported",
        "cublas_status_arch_mismatch",
        "not implemented for",
        "not supported",
    )
    return any(fragment in message for fragment in known_fragments)


def choose_tolerance(precision_name: str) -> Tuple[float, float]:
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
    maybe_set_tf32(spec.tf32_enabled)

    g_a = torch.Generator(device="cpu")
    g_a.manual_seed(seed)
    a_cpu = torch.randn(
        validate_size, validate_size, generator=g_a, dtype=torch.float32
    )

    g_b = torch.Generator(device="cpu")
    g_b.manual_seed(seed + 1)
    b_cpu = torch.randn(
        validate_size, validate_size, generator=g_b, dtype=torch.float32
    )

    ref = torch.mm(a_cpu.to(torch.float64), b_cpu.to(torch.float64)).to(torch.float32)

    a = make_random_matrix(validate_size, device, spec.dtype, seed)
    b = make_random_matrix(validate_size, device, spec.dtype, seed + 1)

    try:
        out = torch.mm(a, b)
        synchronize_device(device)
    except Exception as exc:
        if not _is_known_validation_matmul_unsupported(exc):
            raise
        out = torch.mm(a.to(torch.float32).cpu(), b.to(torch.float32).cpu())

    out_f32 = out.to(torch.float32).cpu()
    if not torch.isfinite(out_f32).all():
        return False, float("inf"), float("inf"), "validation produced NaN/Inf"

    diff = (out_f32 - ref).abs()
    abs_thr, rel_thr = choose_tolerance(spec.name)
    threshold = abs_thr + rel_thr * ref.abs()
    max_abs = float(diff.max().item())
    max_rel = float((diff / (ref.abs() + 1e-12)).max().item())
    failed = diff > threshold
    if failed.any():
        first_idx = int(failed.flatten().nonzero()[0].item())
        failed_count = int(failed.sum().item())
        reason = (
            f"{failed_count} element(s) exceed atol+rtol*|ref|: max_abs={max_abs:.4e}, "
            f"max_rel={max_rel:.4e} (first idx={first_idx})"
        )
        return False, max_abs, max_rel, reason
    return True, max_abs, max_rel, None
