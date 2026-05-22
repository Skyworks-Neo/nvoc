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
    "check_voltage_frequency",
    "discover_gpus",
    "probe_voltage_limits",
    "query_clock_offset",
    "query_info",
    "query_legacy_p0_core_max_voltage_delta",
    "query_status",
    "query_settings",
    "query_supported_applications_clocks",
    "query_tdp_temp_limits",
    "query_vfp_point_voltage",
    "reset_all",
    "reset_applications_clocks",
    "reset_cooler_levels",
    "reset_core_clocks",
    "reset_fan_speed",
    "reset_locked_clocks",
    "reset_mem_clocks",
    "reset_nvapi_power_limits",
    "reset_nvapi_sensor_limits",
    "reset_pstate_base_voltages",
    "reset_pstate_clock_offsets",
    "reset_vfp_deltas",
    "reset_vfp_frequency_lock",
    "reset_vfp_lock",
    "set_applications_clocks",
    "set_clock_offset",
    "set_cooler_levels",
    "set_domain_vfp_deltas",
    "set_fan",
    "set_legacy_clocks",
    "set_legacy_voltage_delta",
    "set_locked_clocks",
    "set_nvapi_power_limits",
    "set_nvapi_pstate_lock",
    "set_nvapi_sensor_limits",
    "set_nvml_pstate_lock",
    "set_power_limit",
    "set_pstate_base_voltage",
    "set_pstate_clock_offset",
    "set_thermal_limit",
    "set_vfp_frequency_lock",
    "set_vfp_point_delta",
    "set_vfp_range_delta",
    "set_vfp_voltage_lock",
    "set_voltage_boost",
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
    assert pynvoc.__all__ == EXPECTED_EXPORTS
    for name in EXPECTED_EXPORTS:
        assert name in pynvoc.__all__, f"{name} missing from __all__"
        assert hasattr(pynvoc, name), f"{name} missing from module"


def test_all_names_callable(pynvoc):
    for name in EXPECTED_EXPORTS:
        obj = getattr(pynvoc, name)
        assert callable(obj), f"{name} should be callable"


def test_native_module_exists(pynvoc):
    assert hasattr(pynvoc, "_native"), "pynvoc should expose _native submodule"


def test_all_native_functions_are_top_level_exports(pynvoc):
    native_names = {name for name in dir(pynvoc._native) if not name.startswith("_")}
    assert native_names == set(pynvoc.__all__)
