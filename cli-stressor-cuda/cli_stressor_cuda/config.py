from __future__ import annotations

import math
from typing import Dict, List

try:
    import tomllib
except ImportError:  # pragma: no cover
    tomllib = None

from .models import KernelParamOverride, KernelType, PrecisionSpec
from .parsing import (
    parse_kernel_type,
    parse_precision_list_with_mapping,
    parse_precision_name,
)


def load_toml_config(path: str) -> Dict[str, object]:
    if tomllib is None:
        raise ValueError("TOML config requires Python 3.11+ (tomllib)")
    with open(path, "rb") as handle:
        return tomllib.load(handle)


def cli_has_flag(argv: List[str], flag: str) -> bool:
    prefix = f"{flag}="
    return any(arg == flag or arg.startswith(prefix) for arg in argv)


def apply_file_config_to_args(args, argv: List[str], config: Dict[str, object]) -> None:
    if not cli_has_flag(argv, "--duration") and "duration" in config:
        args.duration = float(config["duration"])
    if not cli_has_flag(argv, "--matrix-sizes") and "matrix_sizes" in config:
        args.matrix_sizes = list(config["matrix_sizes"])
    if not cli_has_flag(argv, "--fp64-matrix-sizes") and "fp64_matrix_sizes" in config:
        args.fp64_matrix_sizes = list(config["fp64_matrix_sizes"])
    if not cli_has_flag(argv, "--precisions") and "precisions" in config:
        args.precisions = ",".join(config["precisions"])
    if not cli_has_flag(argv, "--warmup-iters") and "warmup_iters" in config:
        args.warmup_iters = int(config["warmup_iters"])
    if not cli_has_flag(argv, "--burst-iters") and "burst_iters" in config:
        args.burst_iters = int(config["burst_iters"])
    if not cli_has_flag(argv, "--validate-interval") and "validate_interval" in config:
        args.validate_interval = float(config["validate_interval"])
    if not cli_has_flag(argv, "--validate-size") and "validate_size" in config:
        args.validate_size = int(config["validate_size"])
    if not cli_has_flag(argv, "--transpose-prob") and "transpose_prob" in config:
        args.transpose_prob = float(config["transpose_prob"])
    if (
        not cli_has_flag(argv, "--minor-mixture-rate")
        and "minor_mixture_rate" in config
    ):
        args.minor_mixture_rate = float(config["minor_mixture_rate"])
    if not cli_has_flag(argv, "--seed") and "seed" in config:
        args.seed = int(config["seed"])
    if not cli_has_flag(argv, "--kernel-types") and "kernel_types" in config:
        args.kernel_types = ",".join(config["kernel_types"])
    if not cli_has_flag(argv, "--kernel-mixture") and "kernel_mixture" in config:
        kernel_mixture = config["kernel_mixture"]
        if isinstance(kernel_mixture, dict):
            args.kernel_mixture = ",".join(
                f"{name}:{weight}" for name, weight in kernel_mixture.items()
            )
        else:
            args.kernel_mixture = str(kernel_mixture)
    if not cli_has_flag(argv, "--stream-mode") and "stream_mode" in config:
        args.stream_mode = str(config["stream_mode"])
    if not cli_has_flag(argv, "--disable-fp8") and "disable_fp8" in config:
        args.disable_fp8 = bool(config["disable_fp8"])
    if not cli_has_flag(argv, "--kernel-params") and "kernel_params" in config:
        args.kernel_params = kernel_params_to_cli_string(config["kernel_params"])


def kernel_params_to_cli_string(kernel_params: Dict[str, object]) -> str:
    entries: List[str] = []
    for name, item in kernel_params.items():
        kvs: List[str] = []
        if "precisions" in item:
            kvs.append(f"precisions={'|'.join(item['precisions'])}")
        if "precision_mixture" in item:
            parts = [f"{p}:{w}" for p, w in item["precision_mixture"].items()]
            kvs.append(f"precision_mixture={'|'.join(parts)}")
        if "precision_weight" in item or "precision_weights" in item:
            weights = item.get("precision_weight", item.get("precision_weights", []))
            precisions = item.get("precisions")
            if not precisions:
                raise ValueError(
                    f"kernel_params.{name}.precision_weight requires precisions"
                )
            parts = [f"{p}:{w}" for p, w in zip(precisions, weights)]
            kvs.append(f"precision_mixture={'|'.join(parts)}")
        if "matrix_sizes" in item:
            kvs.append(f"matrix_sizes={'|'.join(str(x) for x in item['matrix_sizes'])}")
        if "warmup_iters" in item:
            kvs.append(f"warmup_iters={item['warmup_iters']}")
        if "burst_iters" in item:
            kvs.append(f"burst_iters={item['burst_iters']}")
        if "transpose_prob" in item:
            kvs.append(f"transpose_prob={item['transpose_prob']}")
        if "minor_mixture_rate" in item:
            kvs.append(f"minor_mixture_rate={item['minor_mixture_rate']}")
        entries.append(f"{name}:{','.join(kvs)}")
    return ";".join(entries)


def load_kernel_overrides_from_config(
    config: Dict[str, object], mapping: Dict[str, PrecisionSpec]
) -> List[KernelParamOverride]:
    kernel_params = config.get("kernel_params")
    if not kernel_params:
        return []
    overrides: List[KernelParamOverride] = []
    for name, item in kernel_params.items():
        kind = parse_kernel_type(name)
        if "matrix_sizes" in item and not item["matrix_sizes"]:
            raise ValueError(f"kernel_params.{name}.matrix_sizes cannot be empty")
        precisions = None
        if "precisions" in item and item["precisions"]:
            precisions = parse_precision_list_with_mapping(
                ",".join(item["precisions"]), mapping
            )
        precision_mixture = None
        if "precision_mixture" in item:
            precision_mixture = []
            for key, weight in item["precision_mixture"].items():
                spec = parse_precision_name(key, mapping)
                if not math.isfinite(weight) or weight < 0.0:
                    raise ValueError(
                        f"kernel_params.{name}.precision_mixture must be finite and >= 0: {weight}"
                    )
                precision_mixture.append((spec, float(weight)))
        elif "precision_weight" in item or "precision_weights" in item:
            weights = item.get("precision_weight", item.get("precision_weights", []))
            if not precisions:
                raise ValueError(
                    f"kernel_params.{name}.precision_weight requires precisions"
                )
            if len(weights) != len(precisions):
                raise ValueError(
                    f"kernel_params.{name}.precision_weight length ({len(weights)}) must match precisions ({len(precisions)})"
                )
            precision_mixture = []
            for spec, weight in zip(precisions, weights):
                if not math.isfinite(weight) or weight < 0.0:
                    raise ValueError(
                        f"kernel_params.{name}.precision_weight must be finite and >= 0: {weight}"
                    )
                precision_mixture.append((spec, float(weight)))
        overrides.append(
            KernelParamOverride(
                kind=kind,
                precisions=precisions,
                precision_mixture=precision_mixture,
                matrix_sizes=item.get("matrix_sizes"),
                warmup_iters=item.get("warmup_iters"),
                burst_iters=item.get("burst_iters"),
                transpose_prob=item.get("transpose_prob"),
                minor_mixture_rate=item.get("minor_mixture_rate"),
            )
        )
    return overrides


def merge_kernel_overrides(
    base: List[KernelParamOverride],
    cli: List[KernelParamOverride],
) -> List[KernelParamOverride]:
    merged: Dict[KernelType, KernelParamOverride] = {item.kind: item for item in base}
    for item in cli:
        if item.kind in merged:
            target = merged[item.kind]
            if item.precisions is not None:
                target.precisions = item.precisions
            if item.precision_mixture is not None:
                target.precision_mixture = item.precision_mixture
            if item.matrix_sizes is not None:
                target.matrix_sizes = item.matrix_sizes
            if item.warmup_iters is not None:
                target.warmup_iters = item.warmup_iters
            if item.burst_iters is not None:
                target.burst_iters = item.burst_iters
            if item.transpose_prob is not None:
                target.transpose_prob = item.transpose_prob
            if item.minor_mixture_rate is not None:
                target.minor_mixture_rate = item.minor_mixture_rate
        else:
            merged[item.kind] = item
    return list(merged.values())
