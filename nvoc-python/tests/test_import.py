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

import importlib

import pytest

EXPECTED_EXPORTS = [
    "discover_gpus",
    "query_info",
    "query_status",
    "query_settings",
    "set_clock_offset",
    "set_power_limit",
    "set_thermal_limit",
    "set_voltage_boost",
    "set_legacy_voltage_delta",
    "set_fan",
    "reset_core_clocks",
    "reset_mem_clocks",
    "reset_vfp_lock",
    "reset_all",
]


@pytest.fixture()
def pynvoc():
    """Import pynvoc, skipping the test if the native module isn't built."""
    try:
        return importlib.import_module("pynvoc")
    except ImportError:
        pytest.skip("pynvoc native module not available")


def test_all_exports_present(pynvoc):
    assert hasattr(pynvoc, "__all__"), "pynvoc should define __all__"
    for name in EXPECTED_EXPORTS:
        assert name in pynvoc.__all__, f"{name} missing from __all__"
        assert hasattr(pynvoc, name), f"{name} missing from module"


def test_all_names_callable(pynvoc):
    for name in EXPECTED_EXPORTS:
        obj = getattr(pynvoc, name)
        assert callable(obj), f"{name} should be callable"


def test_native_module_exists(pynvoc):
    assert hasattr(pynvoc, "_native"), "pynvoc should expose _native submodule"
