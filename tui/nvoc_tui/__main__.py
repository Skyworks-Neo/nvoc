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
else:
    from nvoc_tui.app import NVOCApp

_boot_marker("imports done (textual+plotext+pynvoc)")


def main() -> int:
    app = NVOCApp()
    _boot_marker("NVOCApp constructed")
    app.run()
    _boot_marker("run() returned")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
