import argparse
import math
import random
import sys
import time
import warnings
from dataclasses import dataclass, field
from typing import Any, Optional

import numpy as np

try:
    import pyopencl as cl
except ImportError as exc:
    cl = None
    PYOPENCL_IMPORT_ERROR = exc
else:
    PYOPENCL_IMPORT_ERROR = None

    if hasattr(cl, "CompilerWarning"):
        warnings.filterwarnings("ignore", category=cl.CompilerWarning)


PREFERRED_TILE_SIZE = 16
OPENCL_KERNEL_TEMPLATE = """
{extension_preamble}
#pragma OPENCL FP_CONTRACT ON
#define TILE_SIZE {tile_size}
typedef {scalar_type} scalar_t;
typedef {accum_type} accum_t;

__kernel void gemm(
    __global const scalar_t *a,
    __global const scalar_t *b,
    __global scalar_t *c,
    const int n,
    const int transpose_a,
    const int transpose_b
) {{
    const int col = get_global_id(0);
    const int row = get_global_id(1);
    const int local_col = get_local_id(0);
    const int local_row = get_local_id(1);

    __local scalar_t a_tile[TILE_SIZE][TILE_SIZE];
    __local scalar_t b_tile[TILE_SIZE][TILE_SIZE];
    accum_t sum = (accum_t)0;

    for (int tile = 0; tile < n; tile += TILE_SIZE) {{
        const int a_col = tile + local_col;
        const int b_row = tile + local_row;

        if (row < n && a_col < n) {{
            const int a_index = transpose_a ? (a_col * n + row) : (row * n + a_col);
            a_tile[local_row][local_col] = a[a_index];
        }} else {{
            a_tile[local_row][local_col] = (scalar_t)0;
        }}

        if (b_row < n && col < n) {{
            const int b_index = transpose_b ? (col * n + b_row) : (b_row * n + col);
            b_tile[local_row][local_col] = b[b_index];
        }} else {{
            b_tile[local_row][local_col] = (scalar_t)0;
        }}

        barrier(CLK_LOCAL_MEM_FENCE);

        #pragma unroll
        for (int k = 0; k < TILE_SIZE; ++k) {{
            sum += (accum_t)a_tile[local_row][k] * (accum_t)b_tile[k][local_col];
        }}

        barrier(CLK_LOCAL_MEM_FENCE);
    }}

    if (row < n && col < n) {{
        c[row * n + col] = (scalar_t)sum;
    }}
}}
""".strip()


@dataclass(frozen=True)
class PrecisionSpec:
    key: str
    name: str
    storage_dtype: Any
    scalar_type: str
    accum_type: str


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
class KernelBundle:
    program: Any
    kernel: Any


@dataclass
class DeviceBuffers:
    a: Any
    b: Any
    c: Any


@dataclass
class OpenCLRuntime:
    context: Any
    queue: Any
    platform: Any
    device: Any
    tile_size: int
    kernel_cache: dict[str, KernelBundle] = field(default_factory=dict)


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


def parse_precision_list(raw: str):
    mapping = {
        "fp16": PrecisionSpec("fp16", "FP16", np.float16, "half", "float"),
        "fp32": PrecisionSpec("fp32", "FP32", np.float32, "float", "float"),
        "fp64": PrecisionSpec("fp64", "FP64", np.float64, "double", "double"),
    }

    selected = []
    for item in raw.split(","):
        key = item.strip().lower()
        if not key:
            continue
        if key not in mapping:
            raise ValueError(
                f"不支持的精度项: {item}；OpenCL 版本仅支持 fp16、fp32、fp64（按设备能力跳过）"
            )
        selected.append(mapping[key])

    if not selected:
        raise ValueError("precision 不能为空")
    return selected


def get_extension_names(device) -> set[str]:
    raw = getattr(device, "extensions", "") or ""
    return {item.strip() for item in raw.split() if item.strip()}


def supports_fp64(device) -> bool:
    if {"cl_khr_fp64", "cl_amd_fp64"} & get_extension_names(device):
        return True
    return bool(getattr(device, "double_fp_config", 0))


def fp64_extension_name(device) -> Optional[str]:
    extensions = get_extension_names(device)
    if "cl_khr_fp64" in extensions:
        return "cl_khr_fp64"
    if "cl_amd_fp64" in extensions:
        return "cl_amd_fp64"
    return None


def detect_capability(device, spec: PrecisionSpec):
    if spec.key == "fp16" and "cl_khr_fp16" not in get_extension_names(device):
        return False, "FP16 requires cl_khr_fp16"
    if spec.key == "fp64" and not supports_fp64(device):
        return False, "FP64 requires cl_khr_fp64/cl_amd_fp64"
    return True, None


def round_up(value: int, multiple: int) -> int:
    return ((value + multiple - 1) // multiple) * multiple


def choose_tile_size(device) -> int:
    max_group = int(getattr(device, "max_work_group_size", 1) or 1)
    max_items = tuple(int(item) for item in getattr(device, "max_work_item_sizes", (1, 1, 1)))
    local_mem_bytes = int(getattr(device, "local_mem_size", 0) or 0)
    limit_x = max_items[0] if len(max_items) >= 1 else 1
    limit_y = max_items[1] if len(max_items) >= 2 else limit_x

    for tile_size in (32, PREFERRED_TILE_SIZE, 8, 4, 2, 1):
        local_ok = local_mem_bytes <= 0 or (2 * tile_size * tile_size * 8) <= local_mem_bytes
        if tile_size <= limit_x and tile_size <= limit_y and tile_size * tile_size <= max_group and local_ok:
            return tile_size
    return 1


def platform_priority(platform) -> int:
    name = platform.name.strip().lower()
    if "portable computing language" in name or "pocl" in name:
        return 1
    return 0


def make_random_matrix(size: int, spec: PrecisionSpec, seed: int):
    rng = np.random.default_rng(seed)
    base = rng.standard_normal((size, size), dtype=np.float32)
    return np.asarray(base, dtype=spec.storage_dtype, order="C")


def choose_tolerance(precision_name: str):
    if precision_name == "FP64":
        return 1e-5, 1e-5
    if precision_name == "FP32":
        return 1e-2, 1e-2
    if precision_name == "FP16":
        return 2e-1, 2e-1
    return 1e-2, 1e-2


def build_workload_size_choices(matrix_sizes):
    base_sizes = sorted(set(int(size) for size in matrix_sizes if int(size) > 0))
    jitter_sizes = set(base_sizes) | {32, 64, 128}
    for size in base_sizes:
        if size > 1:
            jitter_sizes.add(size - 1)
        jitter_sizes.add(max(64, size // 2))
    return base_sizes, sorted(jitter_sizes)


def release_mem_object(obj: Any):
    if obj is None:
        return
    release = getattr(obj, "release", None)
    if release is not None:
        release()


def get_optional_device_info(device, attr_name: str):
    attr = getattr(cl.device_info, attr_name, None)
    if attr is None:
        return None
    try:
        return device.get_info(attr)
    except Exception:
        return None


def get_pci_bus_id(device) -> Optional[str]:
    domain = get_optional_device_info(device, "PCI_DOMAIN_ID_NV")
    bus = get_optional_device_info(device, "PCI_BUS_ID_NV")
    slot = get_optional_device_info(device, "PCI_SLOT_ID_NV")
    if domain is None or bus is None or slot is None:
        return None
    return f"{int(domain):08x}:{int(bus):02x}:{int(slot):02x}.0"


def get_device_handle_id(device) -> str:
    return f"0x{int(getattr(device, 'int_ptr', 0)):x}"


def get_opencl_platforms():
    if cl is None:
        raise RuntimeError(f"PyOpenCL 未安装: {PYOPENCL_IMPORT_ERROR}")

    try:
        platforms = cl.get_platforms()
    except Exception as exc:
        raise RuntimeError(f"无法枚举 OpenCL 平台: {exc}") from exc

    if not platforms:
        raise RuntimeError("未发现任何 OpenCL 平台")
    return platforms


def is_gpu_device(device) -> bool:
    return bool(int(getattr(device, "type", 0)) & int(cl.device_type.GPU))


def format_device_type(device) -> str:
    device_type = int(getattr(device, "type", 0))
    labels = []
    candidates = (
        ("DEFAULT", getattr(cl.device_type, "DEFAULT", 0)),
        ("CPU", getattr(cl.device_type, "CPU", 0)),
        ("GPU", getattr(cl.device_type, "GPU", 0)),
        ("ACCELERATOR", getattr(cl.device_type, "ACCELERATOR", 0)),
        ("CUSTOM", getattr(cl.device_type, "CUSTOM", 0)),
    )
    for label, flag in candidates:
        if flag and device_type & int(flag):
            labels.append(label)
    return "|".join(labels) if labels else str(device_type)


def choose_default_gpu(platforms):
    candidates = []
    for platform_idx, platform in enumerate(platforms):
        try:
            devices = platform.get_devices(device_type=cl.device_type.GPU)
        except Exception:
            continue
        for gpu_index, device in enumerate(devices):
            candidates.append(
                (
                    platform_priority(platform),
                    -int(getattr(device, "global_mem_size", 0)),
                    platform_idx,
                    gpu_index,
                    platform,
                    device,
                )
            )
    if not candidates:
        return None
    _, _, platform_idx, gpu_index, platform, device = sorted(candidates, key=lambda item: item[:4])[0]
    return platform_idx, gpu_index, platform, device


def print_available_devices(platforms):
    print("可用 OpenCL 平台和设备:")
    default_gpu = choose_default_gpu(platforms)

    for platform_idx, platform in enumerate(platforms):
        print(f"\n[platform {platform_idx}] {platform.name.strip()}")
        print(f"  vendor: {platform.vendor.strip()}")
        print(f"  version: {platform.version.strip()}")

        try:
            devices = platform.get_devices()
        except Exception as exc:
            print(f"  devices_error: {exc}")
            continue

        if not devices:
            print("  (no devices)")
            continue

        gpu_index_by_ptr = {}
        try:
            gpu_devices = platform.get_devices(device_type=cl.device_type.GPU)
        except Exception:
            gpu_devices = []
        for gpu_index, device in enumerate(gpu_devices):
            gpu_index_by_ptr[getattr(device, "int_ptr", None)] = gpu_index

        for device_idx, device in enumerate(devices):
            fp16_supported = "yes" if "cl_khr_fp16" in get_extension_names(device) else "no"
            fp64_supported = "yes" if supports_fp64(device) else "no"
            memory_gb = getattr(device, "global_mem_size", 0) / 1024**3
            pci_bus_id = get_pci_bus_id(device)
            print(f"  [device {device_idx}] {device.name.strip()}")
            print(
                "    "
                f"type={format_device_type(device)} | "
                f"mem={memory_gb:.1f} GB | "
                f"compute_units={getattr(device, 'max_compute_units', 'n/a')} | "
                f"fp16={fp16_supported} | "
                f"fp64={fp64_supported} | "
                f"tile={choose_tile_size(device)}"
            )
            identity_parts = [f"opencl_id={get_device_handle_id(device)}"]
            if pci_bus_id is not None:
                identity_parts.append(f"pci={pci_bus_id}")
            print("    " + " | ".join(identity_parts))

            gpu_index = gpu_index_by_ptr.get(getattr(device, "int_ptr", None))
            if gpu_index is not None:
                select_marker = ""
                if default_gpu is not None and default_gpu[0] == platform_idx and default_gpu[1] == gpu_index:
                    select_marker = " [default]"
                print(
                    "    "
                    f"benchmark-select=--platform-index {platform_idx} --device-index {gpu_index}{select_marker}"
                )
            else:
                print("    benchmark-select=not available (non-GPU device)")

    if default_gpu is None:
        print("\n默认自动选择: 未检测到可用于压力测试的 GPU 设备")
    else:
        platform_idx, gpu_index, platform, device = default_gpu
        print(
            "\n默认自动选择: "
            f"--platform-index {platform_idx} --device-index {gpu_index} "
            f"({platform.name.strip()} / {device.name.strip()})"
        )


def create_runtime(platform_index: Optional[int], device_index: Optional[int]):
    platforms = get_opencl_platforms()

    if platform_index is None:
        if device_index is not None:
            raise ValueError("单独指定 --device-index 无效；请同时指定 --platform-index")

        default_gpu = choose_default_gpu(platforms)
        if default_gpu is not None:
            _, _, platform, device = default_gpu
            context = cl.Context(devices=[device])
            queue = cl.CommandQueue(context)
            return OpenCLRuntime(
                context=context,
                queue=queue,
                platform=platform,
                device=device,
                tile_size=choose_tile_size(device),
            )
        raise RuntimeError("未检测到 OpenCL GPU 设备")

    if platform_index < 0 or platform_index >= len(platforms):
        raise ValueError(f"platform index 越界: {platform_index}")

    platform = platforms[platform_index]
    try:
        devices = platform.get_devices(device_type=cl.device_type.GPU)
    except Exception as exc:
        raise RuntimeError(f"无法枚举平台 {platform_index} 上的 GPU: {exc}") from exc

    if not devices:
        raise RuntimeError(f"平台 {platform_index} 上没有 GPU 设备")

    resolved_device_index = 0 if device_index is None else device_index
    if resolved_device_index < 0 or resolved_device_index >= len(devices):
        raise ValueError(f"device index 越界: {resolved_device_index}")

    device = devices[resolved_device_index]
    context = cl.Context(devices=[device])
    queue = cl.CommandQueue(context)
    return OpenCLRuntime(
        context=context,
        queue=queue,
        platform=platform,
        device=device,
        tile_size=choose_tile_size(device),
    )


def build_kernel_bundle(runtime: OpenCLRuntime, spec: PrecisionSpec):
    extension_preamble = ""
    if spec.key == "fp16":
        extension_preamble = "#pragma OPENCL EXTENSION cl_khr_fp16 : enable"
    elif spec.key == "fp64":
        extension = fp64_extension_name(runtime.device)
        if extension is not None:
            extension_preamble = f"#pragma OPENCL EXTENSION {extension} : enable"

    source = OPENCL_KERNEL_TEMPLATE.format(
        extension_preamble=extension_preamble,
        tile_size=runtime.tile_size,
        scalar_type=spec.scalar_type,
        accum_type=spec.accum_type,
    )
    program = cl.Program(runtime.context, source).build()
    kernel = program.gemm
    if hasattr(kernel, "set_scalar_arg_dtypes"):
        kernel.set_scalar_arg_dtypes([None, None, None, np.int32, np.int32, np.int32])
    return KernelBundle(program=program, kernel=kernel)


def get_kernel_bundle(runtime: OpenCLRuntime, spec: PrecisionSpec):
    bundle = runtime.kernel_cache.get(spec.key)
    if bundle is None:
        bundle = build_kernel_bundle(runtime, spec)
        runtime.kernel_cache[spec.key] = bundle
    return bundle


def create_device_buffers(runtime: OpenCLRuntime, a_host, b_host):
    flags = cl.mem_flags
    a_buf = cl.Buffer(runtime.context, flags.READ_ONLY | flags.COPY_HOST_PTR, hostbuf=a_host)
    b_buf = cl.Buffer(runtime.context, flags.READ_ONLY | flags.COPY_HOST_PTR, hostbuf=b_host)
    c_buf = cl.Buffer(runtime.context, flags.WRITE_ONLY, a_host.nbytes)
    return DeviceBuffers(a=a_buf, b=b_buf, c=c_buf)


def release_device_buffers(buffers: Optional[DeviceBuffers]):
    if buffers is None:
        return
    release_mem_object(buffers.a)
    release_mem_object(buffers.b)
    release_mem_object(buffers.c)


def enqueue_gemm(
    runtime: OpenCLRuntime,
    bundle: KernelBundle,
    buffers: DeviceBuffers,
    size: int,
    transpose_a: bool,
    transpose_b: bool,
):
    global_size = (round_up(size, runtime.tile_size), round_up(size, runtime.tile_size))
    local_size = (runtime.tile_size, runtime.tile_size)
    return bundle.kernel(
        runtime.queue,
        global_size,
        local_size,
        buffers.a,
        buffers.b,
        buffers.c,
        np.int32(size),
        np.int32(1 if transpose_a else 0),
        np.int32(1 if transpose_b else 0),
    )


def fetch_output(runtime: OpenCLRuntime, buffers: DeviceBuffers, size: int, spec: PrecisionSpec):
    out = np.empty((size, size), dtype=spec.storage_dtype)
    cl.enqueue_copy(runtime.queue, out, buffers.c)
    runtime.queue.finish()
    return out


def validate_precision(
    runtime: OpenCLRuntime,
    spec: PrecisionSpec,
    bundle: KernelBundle,
    validate_size: int,
    seed: int,
):
    rng = np.random.default_rng(seed)
    a_cpu = rng.standard_normal((validate_size, validate_size), dtype=np.float32)
    b_cpu = rng.standard_normal((validate_size, validate_size), dtype=np.float32)
    ref = (a_cpu.astype(np.float64) @ b_cpu.astype(np.float64)).astype(np.float32)

    a_host = np.asarray(a_cpu, dtype=spec.storage_dtype, order="C")
    b_host = np.asarray(b_cpu, dtype=spec.storage_dtype, order="C")

    buffers = None
    try:
        buffers = create_device_buffers(runtime, a_host, b_host)
        enqueue_gemm(runtime, bundle, buffers, validate_size, False, False)
        out = fetch_output(runtime, buffers, validate_size, spec)
    finally:
        release_device_buffers(buffers)

    out_f32 = out.astype(np.float32, copy=False)
    if not np.isfinite(out_f32).all():
        return False, float("inf"), float("inf"), "validation produced NaN/Inf"

    diff = np.abs(out_f32 - ref)
    abs_thr, rel_thr = choose_tolerance(spec.name)

    # Per-element bound: atol + rtol * |ref|  (same convention as numpy.allclose).
    # A global max-of-diff vs max-of-ref with `or` lets a single badly-corrupted element
    # pass whenever the global reference max is large enough to make max_rel look small.
    elementwise_bound = abs_thr + rel_thr * np.abs(ref)
    violated = diff > elementwise_bound
    n_violated = int(np.sum(violated))
    max_abs = float(np.max(diff))
    ref_abs_max = float(np.max(np.abs(ref)))
    max_rel = max_abs / (ref_abs_max + 1e-12)
    passed = n_violated == 0
    reason = (None if passed else
              f"{n_violated} elements exceed atol+rtol*|ref|; max_abs={max_abs:.4g}, "
              f"max_rel={max_rel:.4g}")
    return passed, max_abs, max_rel, reason


def run_stress_for_precision(
    runtime: OpenCLRuntime,
    spec: PrecisionSpec,
    matrix_sizes,
    duration_s: float,
    warmup_iters: int,
    burst_iters: int,
    validate_interval_s: float,
    validate_size: int,
    jitter_rate: float,
    transpose_prob: float,
    min_burst_ms: float,
    input_refresh_interval: int,
    base_seed: int,
):
    supported, reason = detect_capability(runtime.device, spec)
    result = StressResult(precision=spec.name, supported=supported)

    if not supported:
        result.first_error = f"SKIP: {reason}"
        return result

    try:
        bundle = get_kernel_bundle(runtime, spec)
    except Exception as exc:
        result.first_error = f"kernel build failed: {exc}"
        return result

    rng = random.Random(base_seed)
    start = time.monotonic()
    next_validate = start + max(0.0, validate_interval_s)
    validation_seed = base_seed ^ 0x5F3759DF
    preferred_sizes, jitter_sizes = build_workload_size_choices(matrix_sizes)
    refresh_every = max(1, int(input_refresh_interval))
    target_burst_s = max(0.0, float(min_burst_ms)) / 1000.0

    active_size = None
    active_buffers = None
    windows_since_refresh = 0

    try:
        probe_a = make_random_matrix(8, spec, base_seed + 1)
        probe_b = make_random_matrix(8, spec, base_seed + 2)
        probe_buffers = create_device_buffers(runtime, probe_a, probe_b)
        try:
            enqueue_gemm(runtime, bundle, probe_buffers, 8, False, False)
            runtime.queue.finish()
        finally:
            release_device_buffers(probe_buffers)
    except Exception as exc:
        result.first_error = f"probe failed: {exc}"
        return result

    while True:
        now = time.monotonic()
        if now - start >= duration_s:
            break

        if rng.random() < jitter_rate:
            size = rng.choice(jitter_sizes)
        else:
            size = rng.choice(preferred_sizes)

        transpose_a = rng.random() < transpose_prob
        transpose_b = rng.random() < transpose_prob

        try:
            if active_buffers is None or active_size != size:
                release_device_buffers(active_buffers)
                a_host = make_random_matrix(size, spec, rng.randrange(1 << 30))
                b_host = make_random_matrix(size, spec, rng.randrange(1 << 30))
                active_buffers = create_device_buffers(runtime, a_host, b_host)
                active_size = size
                windows_since_refresh = 0
            elif windows_since_refresh >= refresh_every:
                a_host = make_random_matrix(size, spec, rng.randrange(1 << 30))
                b_host = make_random_matrix(size, spec, rng.randrange(1 << 30))
                cl.enqueue_copy(runtime.queue, active_buffers.a, a_host)
                cl.enqueue_copy(runtime.queue, active_buffers.b, b_host)
                windows_since_refresh = 0

            for _ in range(warmup_iters):
                enqueue_gemm(runtime, bundle, active_buffers, size, transpose_a, transpose_b)
            runtime.queue.finish()

            op_start = time.monotonic()
            executed_iters = burst_iters
            for _ in range(burst_iters):
                enqueue_gemm(runtime, bundle, active_buffers, size, transpose_a, transpose_b)
            runtime.queue.finish()
            op_elapsed = time.monotonic() - op_start

            if target_burst_s > 0 and op_elapsed < target_burst_s:
                multiplier = max(1, int(math.ceil(target_burst_s / max(op_elapsed, 1e-9))))
                extra_iters = burst_iters * (multiplier - 1)
                if extra_iters > 0:
                    extra_start = time.monotonic()
                    for _ in range(extra_iters):
                        enqueue_gemm(runtime, bundle, active_buffers, size, transpose_a, transpose_b)
                    runtime.queue.finish()
                    op_elapsed += time.monotonic() - extra_start
                    executed_iters += extra_iters

            inst_tflops = (2 * (size**3) * executed_iters / op_elapsed) / 1e12
            elapsed_total = time.monotonic() - start

            print(
                f"[{spec.name}] "
                f"t={elapsed_total:6.1f}s/{duration_s:.1f}s | "
                f"size={size:5d} | "
                f"inst={inst_tflops:7.2f} TFLOPS | "
            )

            result.iterations += executed_iters
            result.total_flops += int(2 * (size**3) * executed_iters)
            result.compute_s += op_elapsed
            windows_since_refresh += 1

            if time.monotonic() >= next_validate:
                passed, max_abs, max_rel, reason = validate_precision(
                    runtime=runtime,
                    spec=spec,
                    bundle=bundle,
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

        if op_elapsed < 0.01:
            time.sleep(0)

    release_device_buffers(active_buffers)

    result.elapsed_s = time.monotonic() - start
    if result.compute_s > 0:
        result.tflops = (result.total_flops / result.compute_s) / 1e12
    return result


def result_status(result: StressResult) -> str:
    if not result.supported:
        return "SKIP"
    if result.first_error is None and result.validation_failures == 0:
        return "OK"
    return "FAIL"


def print_summary(runtime: OpenCLRuntime, results):
    print("\n" + "=" * 72)
    print("OpenCL core stability summary")
    print(f"平台: {runtime.platform.name.strip()}")
    print(f"设备: {runtime.device.name.strip()}")
    if hasattr(runtime.device, "global_mem_size"):
        print(f"显存: {runtime.device.global_mem_size / 1024**3:.1f} GB")

    overall_ok = True
    ran_any_precision = False
    for r in results:
        status = result_status(r)
        if status == "FAIL":
            overall_ok = False
        if r.supported:
            ran_any_precision = True
        wallclock_eff = (f"{100 * r.compute_s / r.elapsed_s:.1f}%"
                         if r.elapsed_s > 0 else "n/a")
        print(
            f"{r.precision:12} {status:4} | "
            f"iters={r.iterations:8d} | "
            f"wall={r.elapsed_s:7.1f}s | "
            f"compute={r.compute_s:6.1f}s | "
            f"{r.tflops:8.2f} TFLOPS | "
            f"eff={wallclock_eff:6} | "
            f"validations={r.validations:3d} | "
            f"val_fail={r.validation_failures:3d} | "
            f"max_abs={r.max_abs_error:.3g} | max_rel={r.max_rel_error:.3g}"
        )
        if r.first_error:
            print(f"{'':12}      first_error: {r.first_error}")
            if r.first_error_at_s is not None:
                print(f"{'':12}      at: {r.first_error_at_s:.1f}s")

    print("=" * 72)
    print("总体结论:")
    if not ran_any_precision:
        print("- 选定精度均不受当前设备支持，未执行压力测试。")
    elif overall_ok:
        print("- 在当前测试窗口内，没有出现明显计算错误或验证失败。")
    else:
        print("- 至少有一个受支持的精度模式出现错误或验证失败。")
    print("=" * 72)


def build_arg_parser():
    p = argparse.ArgumentParser(
        description="OpenCL GPU core-domain stressor: randomized GEMM with validation sidecar."
    )
    p.add_argument(
        "--platform-index",
        type=int,
        default=None,
        help="OpenCL 平台索引；不指定时自动选择首个可用 GPU 平台",
    )
    p.add_argument(
        "--device-index",
        type=int,
        default=None,
        help="平台内 GPU 索引；需与 --platform-index 配合使用，省略时默认 0",
    )
    p.add_argument(
        "--list-devices",
        action="store_true",
        help="列出所有可见 OpenCL 平台和设备并退出",
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
        default=parse_int_list("2049,4096,4097,8192,8193,16384"),
        help="矩阵尺寸列表，逗号分隔，例如 1024,2048,4096",
    )
    p.add_argument(
        "--fp64-matrix-sizes",
        type=parse_int_list,
        default=parse_int_list("2048,4096"),
        help="专门用于 FP64 模式的矩阵尺寸，避免双精度压力阶段过慢",
    )
    p.add_argument(
        "--precisions",
        type=str,
        default="fp16,fp32",
        help="精度列表，支持 fp16、fp32、fp64（按设备能力自动跳过）",
    )
    p.add_argument("--warmup-iters", type=int, default=3, help="每个工作负载窗口的预热轮数")
    p.add_argument("--burst-iters", type=int, default=6, help="每个工作负载窗口的正式压力轮数")
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
    p.add_argument(
        "--jitter-rate",
        type=float,
        default=0.3,
        help="选择 jitter 尺寸的概率，其余情况使用首选矩阵尺寸",
    )
    p.add_argument(
        "--min-burst-ms",
        type=float,
        default=40.0,
        help="每个工作负载窗口最少执行时长（毫秒），不足时自动追加 kernel 轮数",
    )
    p.add_argument(
        "--input-refresh-interval",
        type=int,
        default=1,
        help="连续复用同一组输入的窗口数；1 表示每个窗口都刷新输入",
    )
    p.add_argument("--seed", type=int, default=12345, help="随机种子")
    p.add_argument(
        "--disable-fp8",
        action="store_true",
        help=argparse.SUPPRESS,
    )
    return p


def main():
    args = build_arg_parser().parse_args()

    if args.device_index is not None and args.platform_index is None:
        print("参数错误: 单独指定 --device-index 无效；请同时指定 --platform-index")
        return 2

    try:
        precisions = parse_precision_list(args.precisions)
    except ValueError as exc:
        print(f"参数错误: {exc}")
        return 2

    if not 0.0 <= args.jitter_rate <= 1.0:
        print("参数错误: --jitter-rate 必须位于 0.0 到 1.0 之间")
        return 2

    if args.input_refresh_interval < 1:
        print("参数错误: --input-refresh-interval 必须 >= 1")
        return 2

    if args.min_burst_ms < 0:
        print("参数错误: --min-burst-ms 必须 >= 0")
        return 2

    if cl is None:
        print(f"未安装 PyOpenCL：{PYOPENCL_IMPORT_ERROR}")
        print("请先安装 OpenCL 驱动/ICD，然后执行 `uv sync` 或 `pip install pyopencl`。")
        return 3

    if args.list_devices:
        try:
            platforms = get_opencl_platforms()
        except RuntimeError as exc:
            print(f"OpenCL 初始化失败: {exc}")
            return 3
        print_available_devices(platforms)
        return 0

    try:
        runtime = create_runtime(args.platform_index, args.device_index)
    except ValueError as exc:
        print(f"参数错误: {exc}")
        return 2
    except RuntimeError as exc:
        print(f"OpenCL 初始化失败: {exc}")
        return 3

    print(f"OpenCL 平台: {runtime.platform.name.strip()}")
    print(f"测试设备: {runtime.device.name.strip()}")
    print(f"厂商: {runtime.device.vendor.strip()}")
    print(f"驱动/版本: {runtime.device.version.strip()}")
    print(f"显存大小: {runtime.device.global_mem_size / 1024**3:.1f} GB")
    print(f"OpenCL设备ID: {get_device_handle_id(runtime.device)}")
    pci_bus_id = get_pci_bus_id(runtime.device)
    if pci_bus_id is not None:
        print(f"PCI地址: {pci_bus_id}")
    print(f"Kernel tile: {runtime.tile_size}x{runtime.tile_size}")
    print(f"Python版本: {sys.version.split()[0]}")
    print(f"NumPy版本: {np.__version__}")
    print(f"PyOpenCL版本: {getattr(cl, 'VERSION_TEXT', getattr(cl, '__version__', 'unknown'))}")

    random.seed(args.seed)
    results = []
    for idx, spec in enumerate(precisions):
        current_sizes = args.fp64_matrix_sizes if spec.name == "FP64" else args.matrix_sizes

        print("\n" + "-" * 72)
        print(f"开始测试 {spec.name}")
        print(f"  矩阵尺寸: {current_sizes}")
        print(f"  持续时间: {args.duration:.1f} 秒")
        print(f"  预热轮数: {args.warmup_iters}")
        print(f"  压力轮数: {args.burst_iters}")
        print(f"  校验间隔: {args.validate_interval:.1f} 秒")
        print(f"  校验尺寸: {args.validate_size}")
        print(f"  Jitter rate: {args.jitter_rate:.2f}")

        res = run_stress_for_precision(
            runtime=runtime,
            spec=spec,
            matrix_sizes=current_sizes,
            duration_s=args.duration,
            warmup_iters=args.warmup_iters,
            burst_iters=args.burst_iters,
            validate_interval_s=args.validate_interval,
            validate_size=args.validate_size,
            jitter_rate=args.jitter_rate,
            transpose_prob=args.transpose_prob,
            min_burst_ms=args.min_burst_ms,
            input_refresh_interval=args.input_refresh_interval,
            base_seed=args.seed + idx * 1000,
        )
        results.append(res)

        if res.first_error:
            print(f"  结果: {res.first_error}")
        else:
            print("  结果: completed without detected error")
        print(f"  累计: {res.iterations} 次 matmul, {res.tflops:.2f} TFLOPS")

    print_summary(runtime, results)
    return 1 if any(result_status(result) == "FAIL" for result in results) else 0


if __name__ == "__main__":
    raise SystemExit(main())
