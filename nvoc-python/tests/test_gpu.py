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


@pytest.fixture()
def pynvoc():
    try:
        return importlib.import_module("pynvoc")
    except ImportError:
        pytest.skip("pynvoc native module not available")


def _first_gpu_id(pynvoc):
    """Return a GPU ID hex string if a GPU is found, else None."""
    try:
        gpus = pynvoc.discover_gpus("both")
    except RuntimeError:
        return None
    if not gpus:
        return None
    return gpus[0]["gpu_id_hex"]


@pytest.fixture()
def gpu(pynvoc):
    gpu_id = _first_gpu_id(pynvoc)
    if gpu_id is None:
        pytest.skip("No NVIDIA GPU available")
    return gpu_id


class TestDiscoverGpus:
    def test_returns_list(self, pynvoc, gpu):
        result = pynvoc.discover_gpus("both")
        assert isinstance(result, list)

    def test_gpu_entry_keys(self, pynvoc, gpu):
        gpus = pynvoc.discover_gpus("both")
        expected_keys = {
            "index",
            "gpu_id",
            "gpu_id_hex",
            "backend_nvapi",
            "backend_nvml",
        }
        assert expected_keys.issubset(gpus[0].keys())

    def test_nvml_backend_filter(self, pynvoc, gpu):
        # On a machine with NVAPI but no NVML binding, discover_gpus("nvml")
        # raises RuntimeError (NVML init failed) rather than returning a list.
        # Skip there rather than erroring — the NVML-backed path is exercised on
        # NVML-equipped CI, the normal environment.
        try:
            gpus = pynvoc.discover_gpus("nvml")
        except RuntimeError:
            pytest.skip("NVML unavailable")
        assert isinstance(gpus, list)

    def test_nvapi_backend_filter(self, pynvoc, gpu):
        gpus = pynvoc.discover_gpus("nvapi")
        assert isinstance(gpus, list)


class TestQueryInfo:
    def test_returns_dict(self, pynvoc, gpu):
        result = pynvoc.query_info(gpu, "both")
        assert isinstance(result, dict)
        assert "gpu_id" in result
        assert "name" in result

    def test_nvml_backend(self, pynvoc, gpu):
        # query_info(gpu, "nvml") hard-errors when NVML is unavailable (explicit
        # NVML request). Skip on no-NVML machines; the NVML path runs on CI.
        try:
            result = pynvoc.query_info(gpu, "nvml")
        except RuntimeError:
            pytest.skip("NVML unavailable")
        assert isinstance(result, dict)

    def test_nvapi_backend(self, pynvoc, gpu):
        result = pynvoc.query_info(gpu, "nvapi")
        assert isinstance(result, dict)


class TestQueryStatus:
    def test_returns_dict(self, pynvoc, gpu):
        result = pynvoc.query_status(gpu, "both")
        assert isinstance(result, dict)


class TestQuerySettings:
    def test_returns_dict(self, pynvoc, gpu):
        result = pynvoc.query_settings(gpu, "both")
        assert isinstance(result, dict)


class TestQueryDomainVfpPoints:
    def test_returns_list(self, pynvoc, gpu):
        result = pynvoc.query_public_vftable(gpu, "graphics")
        assert isinstance(result, list)
        if result:
            expected_keys = {
                "index",
                "voltage_uv",
                "frequency_khz",
                "delta_khz",
                "default_frequency_khz",
            }
            assert expected_keys.issubset(result[0].keys())
