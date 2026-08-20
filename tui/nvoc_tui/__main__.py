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

# Cap BLAS/OpenMP thread pools BEFORE numpy loads (pulled in via
# textual-plotext): the TUI does no matrix math, and the default per-core
# OpenBLAS arenas commit ~650MB of pagefile for nothing (measured: full
# import stack 673MB -> ~50MB).
for _var in ("OPENBLAS_NUM_THREADS", "OMP_NUM_THREADS", "MKL_NUM_THREADS"):
    os.environ.setdefault(_var, "1")

if __package__:
    from .app import NVOCApp
else:
    from nvoc_tui.app import NVOCApp


def main() -> int:
    app = NVOCApp()
    app.run()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
