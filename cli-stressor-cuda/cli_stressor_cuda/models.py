from __future__ import annotations

from dataclasses import dataclass
from enum import Enum
from typing import List, Optional, Tuple

import torch


@dataclass(frozen=True)
class PrecisionSpec:
    name: str
    dtype: torch.dtype
    tf32_enabled: Optional[bool] = None


class KernelType(Enum):
    GEMM = "GEMM"
    MEMCPY = "MEMCPY"
    MEMSET = "MEMSET"
    TRANSPOSE = "TRANSPOSE"
    ELEMENTWISE = "ELEMENTWISE"
    REDUCTION = "REDUCTION"
    ATOMIC = "ATOMIC"


class StreamMode(Enum):
    SINGLE = "single"
    DUAL = "dual"
    TRIPLE = "triple"

    def stream_count(self) -> int:
        if self == StreamMode.DUAL:
            return 2
        if self == StreamMode.TRIPLE:
            return 3
        return 1


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


@dataclass
class KernelParamOverride:
    kind: KernelType
    precisions: Optional[List[PrecisionSpec]] = None
    precision_mixture: Optional[List[Tuple[PrecisionSpec, float]]] = None
    matrix_sizes: Optional[List[int]] = None
    warmup_iters: Optional[int] = None
    burst_iters: Optional[int] = None
    transpose_prob: Optional[float] = None
    minor_mixture_rate: Optional[float] = None


@dataclass
class StressRunConfig:
    matrix_sizes: List[int]
    fp64_matrix_sizes: List[int]
    duration_s: float
    warmup_iters: int
    burst_iters: int
    validate_interval_s: float
    validate_size: int
    transpose_prob: float
    base_seed: int
    minor_mixture_rate: float
    kernel_mixture: List[Tuple[KernelType, float]]
    stream_mode: StreamMode
    kernel_param_overrides: List[KernelParamOverride]
