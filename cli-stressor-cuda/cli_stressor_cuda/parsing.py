from __future__ import annotations

import math
from typing import Dict, List, Tuple

import torch

from .models import KernelParamOverride, KernelType, PrecisionSpec, StreamMode


def parse_int_list(raw: str) -> List[int]:
    values: List[int] = []
    for item in raw.split(","):
        trimmed = item.strip()
        if not trimmed:
            continue
        values.append(int(trimmed))
    if not values:
        raise ValueError("matrix sizes cannot be empty")
    return values


def build_precision_mapping(include_fp8: bool) -> Dict[str, PrecisionSpec]:
    mapping = {
        "fp64": PrecisionSpec("FP64", torch.float64, None),
        "fp32": PrecisionSpec("FP32", torch.float32, False),
        "tf32": PrecisionSpec("TF32", torch.float32, True),
        "fp16": PrecisionSpec("FP16", torch.float16, None),
        "bf16": PrecisionSpec("BF16", torch.bfloat16, None),
    }
    if include_fp8 and hasattr(torch, "float8_e4m3fn"):
        mapping["fp8"] = PrecisionSpec("FP8 E4M3FN", torch.float8_e4m3fn, None)
    return mapping


def parse_precision_name(raw: str, mapping: Dict[str, PrecisionSpec]) -> PrecisionSpec:
    key = raw.strip().lower()
    if not key:
        raise ValueError("precision list cannot be empty")
    if key not in mapping:
        raise ValueError(f"unsupported precision: {raw}")
    return mapping[key]


def parse_precision_list_with_mapping(
    raw: str, mapping: Dict[str, PrecisionSpec]
) -> List[PrecisionSpec]:
    selected: List[PrecisionSpec] = []
    for item in raw.split(","):
        key = item.strip().lower()
        if not key:
            continue
        selected.append(parse_precision_name(key, mapping))
    if not selected:
        raise ValueError("precision list cannot be empty")
    return selected


def parse_precision_list(raw: str, include_fp8: bool) -> List[PrecisionSpec]:
    mapping = build_precision_mapping(include_fp8)
    return parse_precision_list_with_mapping(raw, mapping)


def parse_kernel_type(raw: str) -> KernelType:
    key = raw.strip().lower()
    if key == "gemm":
        return KernelType.GEMM
    if key in ("memcpy", "copy", "clone"):
        return KernelType.MEMCPY
    if key in ("memset", "fill"):
        return KernelType.MEMSET
    if key == "transpose":
        return KernelType.TRANSPOSE
    if key in ("elementwise", "elem", "add"):
        return KernelType.ELEMENTWISE
    if key in ("reduction", "reduce", "sum"):
        return KernelType.REDUCTION
    if key == "atomic":
        return KernelType.ATOMIC
    raise ValueError(f"unsupported kernel type: {raw}")


def parse_kernel_type_list(raw: str) -> List[KernelType]:
    selected: List[KernelType] = []
    for item in raw.split(","):
        key = item.strip()
        if not key:
            continue
        kind = parse_kernel_type(key)
        if kind not in selected:
            selected.append(kind)
    if not selected:
        raise ValueError("kernel type list cannot be empty")
    return selected


def parse_stream_mode(raw: str) -> StreamMode:
    key = raw.strip().lower()
    if key in ("single", "1"):
        return StreamMode.SINGLE
    if key in ("dual", "2"):
        return StreamMode.DUAL
    if key in ("triple", "3"):
        return StreamMode.TRIPLE
    raise ValueError(f"unsupported stream mode: {raw}, expected single|dual|triple")


def parse_kernel_mixture(
    raw: str, kernel_types: List[KernelType]
) -> List[Tuple[KernelType, float]]:
    if not kernel_types:
        raise ValueError("kernel types cannot be empty")
    if not raw.strip():
        return [(kind, 1.0) for kind in kernel_types]

    entries: List[Tuple[KernelType, float]] = []
    for item in raw.split(","):
        trimmed = item.strip()
        if not trimmed:
            continue
        if ":" not in trimmed:
            raise ValueError(
                f"invalid kernel mixture item: {trimmed}, expected type:weight"
            )
        name, weight_raw = trimmed.split(":", 1)
        kind = parse_kernel_type(name)
        if kind not in kernel_types:
            raise ValueError(
                f"kernel type {kind.value} is not included in --kernel-types"
            )
        try:
            weight = float(weight_raw.strip())
        except ValueError as exc:
            raise ValueError(f"invalid mixture weight: {weight_raw.strip()}") from exc
        if not math.isfinite(weight) or weight < 0.0:
            raise ValueError(f"mixture weight must be finite and >= 0: {weight}")
        entries.append((kind, weight))

    if not entries:
        raise ValueError("kernel mixture cannot be empty")

    for kind in kernel_types:
        if not any(entry[0] == kind for entry in entries):
            entries.append((kind, 0.0))
    return entries


def parse_precision_mixture(
    raw: str, mapping: Dict[str, PrecisionSpec]
) -> List[Tuple[PrecisionSpec, float]]:
    if not raw.strip():
        return []
    entries: List[Tuple[PrecisionSpec, float]] = []
    for item in raw.split(","):
        trimmed = item.strip()
        if not trimmed:
            continue
        if ":" not in trimmed:
            raise ValueError(
                f"invalid precision mixture item: {trimmed}, expected precision:weight"
            )
        name, weight_raw = trimmed.split(":", 1)
        spec = parse_precision_name(name, mapping)
        try:
            weight = float(weight_raw.strip())
        except ValueError as exc:
            raise ValueError(
                f"invalid precision mixture weight: {weight_raw.strip()}"
            ) from exc
        if not math.isfinite(weight) or weight < 0.0:
            raise ValueError(
                f"precision mixture weight must be finite and >= 0: {weight}"
            )
        entries.append((spec, weight))
    if not entries:
        raise ValueError("precision mixture cannot be empty")
    return entries


def parse_kernel_param_overrides(
    raw: str, mapping: Dict[str, PrecisionSpec]
) -> List[KernelParamOverride]:
    if not raw.strip():
        return []
    overrides: List[KernelParamOverride] = []
    for entry in raw.split(";"):
        trimmed = entry.strip()
        if not trimmed:
            continue
        if ":" not in trimmed:
            raise ValueError(f"invalid kernel params entry: {trimmed}")
        kind_raw, params_raw = trimmed.split(":", 1)
        kind = parse_kernel_type(kind_raw)
        item = KernelParamOverride(kind=kind)
        for kv in params_raw.split(","):
            kv = kv.strip()
            if not kv:
                continue
            if "=" not in kv:
                raise ValueError(f"invalid key=value in kernel params: {kv}")
            k, v = kv.split("=", 1)
            key = k.strip().lower()
            value = v.strip()
            if key in ("precisions", "precision"):
                normalized = value.replace("|", ",")
                item.precisions = parse_precision_list_with_mapping(normalized, mapping)
            elif key in ("precision_mixture", "precision_mix"):
                normalized = value.replace("|", ",")
                item.precision_mixture = parse_precision_mixture(normalized, mapping)
            elif key in ("matrix_sizes", "sizes"):
                normalized = value.replace("|", ",")
                item.matrix_sizes = parse_int_list(normalized)
            elif key in ("warmup_iters", "warmup"):
                item.warmup_iters = int(value)
            elif key in ("burst_iters", "burst"):
                item.burst_iters = int(value)
            elif key in ("transpose_prob", "transpose"):
                item.transpose_prob = float(value)
            elif key in ("minor_mixture_rate", "minor"):
                item.minor_mixture_rate = float(value)
            else:
                raise ValueError(f"unsupported kernel param key: {k}")
        overrides.append(item)
    return overrides
