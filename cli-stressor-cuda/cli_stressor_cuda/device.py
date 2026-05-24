from __future__ import annotations

from typing import Optional, Tuple

import torch

from .models import PrecisionSpec


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


def synchronize_device(device: torch.device) -> None:
    if device.type == "cuda":
        torch.cuda.synchronize()
    elif device.type == "mps":
        torch.mps.synchronize()


def empty_device_cache(device: torch.device) -> None:
    if device.type == "cuda":
        torch.cuda.empty_cache()
    elif device.type == "mps" and hasattr(torch.mps, "empty_cache"):
        torch.mps.empty_cache()


def maybe_set_tf32(tf32_enabled: Optional[bool]) -> None:
    if tf32_enabled is None:
        return
    if torch.cuda.is_available() and hasattr(torch.backends.cuda, "matmul"):
        mode = "tf32" if tf32_enabled else "ieee"

        if hasattr(torch.backends.cuda.matmul, "fp32_precision"):
            torch.backends.cuda.matmul.fp32_precision = mode

        if hasattr(torch.backends.cudnn, "conv") and hasattr(
            torch.backends.cudnn.conv, "fp32_precision"
        ):
            torch.backends.cudnn.conv.fp32_precision = mode

        elif hasattr(torch.backends.cuda.matmul, "allow_tf32"):
            torch.backends.cuda.matmul.allow_tf32 = tf32_enabled
            torch.backends.cudnn.allow_tf32 = tf32_enabled


def detect_capability(
    device: torch.device, spec: PrecisionSpec
) -> Tuple[bool, Optional[str]]:
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
