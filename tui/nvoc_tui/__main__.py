# Copyright (C) 2026 Ajax Dong
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
#     https://www.apache.org/licenses/LICENSE-2.0
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.

import argparse
import os
import time

# Cap BLAS/OpenMP thread pools before numpy loads through textual-plotext. The
# TUI does no matrix math, so per-core worker pools only reserve excess memory.
for _var in ("OPENBLAS_NUM_THREADS", "OMP_NUM_THREADS", "MKL_NUM_THREADS"):
    os.environ.setdefault(_var, "1")

_T0 = time.perf_counter()


def _boot_marker(label: str) -> None:
    """Startup phase decomposition into the support log (millisecond wall
    stamps + relative elapsed), so slow-disk vs slow-CPU shares of a slow
    start are separable without attaching a debugger to the terminal."""
    try:
        base = os.environ.get("LOCALAPPDATA") or os.path.expanduser("~")
        log_dir = os.path.join(base, "nvoc-tui")
        os.makedirs(log_dir, exist_ok=True)
        now = time.time()
        stamp = f"{time.strftime('%H:%M:%S')}.{int(now * 1000) % 1000:03d}"
        with open(os.path.join(log_dir, "boot.log"), "a", encoding="utf-8") as file:
            file.write(
                f"{stamp} [boot +{time.perf_counter() - _T0:7.3f}s] {label}" + chr(10)
            )
    except OSError:
        pass


_boot_marker("main entered (textual/plotext/pynvoc import pending)")

if __package__:
    from .app import NVOCApp
    from .native import apply_native_paths
else:
    from nvoc_tui.app import NVOCApp
    from nvoc_tui.native import apply_native_paths

_boot_marker("imports done (textual+plotext+pynvoc)")


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="nvoc-tui",
        description="NVOC TUI — NVIDIA GPU monitoring / overclocking terminal UI.",
    )
    parser.add_argument(
        "--nvapi-path",
        default=None,
        metavar="DIR_OR_FILE",
        help=(
            "NVAPI library override (env NVOC_NVAPI_PATH). Windows: the "
            "directory holding nvapi64.dll, or the DLL path itself. Linux: the "
            "directory holding libnvidia-api.so.1 (SONAME joined automatically), "
            "or the .so path directly (must carry that SONAME, else a warning "
            "prints and the system copy loads). Must precede the first GPU call."
        ),
    )
    parser.add_argument(
        "--nvml-path",
        default=None,
        metavar="FILE_OR_DIR",
        help=(
            "NVML library override (env NVOC_NVML_PATH). Path to nvml.dll / "
            "libnvidia-ml.so.1; Linux also accepts a directory (SONAME joined "
            "automatically). A wrong path fails NVML init loudly instead of "
            "silently falling back. Must precede the first GPU call."
        ),
    )
    return parser


def main() -> int:
    args = _build_parser().parse_args()
    apply_native_paths(args.nvapi_path, args.nvml_path, override=True)
    app = NVOCApp()
    _boot_marker("NVOCApp constructed")
    app.run()
    _boot_marker("run() returned")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
