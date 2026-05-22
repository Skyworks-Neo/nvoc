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


# --- Backend validation (parse_backends / parse_backend) ---
# These parse the backend string BEFORE touching GPU hardware,
# so they raise ValueError even without a GPU present.


class TestBackendSetValidation:
    """Test invalid backend-set strings (used by discover/query functions).

    parse_backends() runs before target_inventory, so ValueError is raised
    regardless of GPU availability.
    """

    def test_invalid_backend_discover(self, pynvoc):
        with pytest.raises(ValueError, match="invalid backend set"):
            pynvoc.discover_gpus("cuda")

    def test_invalid_backend_query_info(self, pynvoc):
        with pytest.raises(ValueError, match="invalid backend set"):
            pynvoc.query_info("0", "badbackend")

    def test_invalid_backend_query_status(self, pynvoc):
        with pytest.raises(ValueError, match="invalid backend set"):
            pynvoc.query_status("0", "badbackend")

    def test_invalid_backend_query_settings(self, pynvoc):
        with pytest.raises(ValueError, match="invalid backend set"):
            pynvoc.query_settings("0", "badbackend")


class TestBackendValidation:
    """Test invalid backend strings (used by set/reset functions).

    parse_backend() runs before target_inventory, so ValueError is raised
    regardless of GPU availability.
    """

    def test_invalid_backend_set_clock_offset(self, pynvoc):
        with pytest.raises(ValueError, match="invalid backend"):
            pynvoc.set_clock_offset("0", "cuda", "core", 100, "P0")

    def test_invalid_backend_set_power_limit(self, pynvoc):
        with pytest.raises(ValueError, match="invalid backend"):
            pynvoc.set_power_limit("0", "cuda", 250)

    def test_invalid_backend_set_fan(self, pynvoc):
        with pytest.raises(ValueError, match="invalid backend"):
            pynvoc.set_fan("0", "cuda", "all", "continuous", 60)

    def test_invalid_backend_reset_core_clocks(self, pynvoc):
        with pytest.raises(ValueError, match="invalid backend"):
            pynvoc.reset_core_clocks("0", "cuda")

    def test_invalid_backend_reset_mem_clocks(self, pynvoc):
        with pytest.raises(ValueError, match="invalid backend"):
            pynvoc.reset_mem_clocks("0", "cuda")


# --- Domain validation ---
# parse_domain() runs before target_inventory in set_clock_offset,
# so ValueError is raised even without a GPU present.
# In other functions (set_locked_clocks, etc.), domain parsing happens
# AFTER target_inventory, so RuntimeError masks the ValueError.


class TestDomainValidation:
    """Test invalid clock domain strings where parsing precedes GPU access."""

    def test_invalid_domain_set_clock_offset(self, pynvoc):
        with pytest.raises(ValueError, match="invalid clock domain"):
            pynvoc.set_clock_offset("0", "nvml", "video", 100, "P0")


# --- Valid backend aliases ---
# These should not raise ValueError (may raise RuntimeError if no GPU).


class TestBackendAliases:
    """Verify valid backend strings are accepted by parse_backends()."""

    @pytest.mark.parametrize("backend", ["both", "all", "nvml", "nvapi"])
    def test_valid_backend_set(self, pynvoc, backend):
        try:
            pynvoc.discover_gpus(backend)
        except ValueError:
            pytest.fail(f"'{backend}' should be a valid backend-set string")
        except RuntimeError:
            pass  # No GPU hardware, but backend string was accepted

    @pytest.mark.parametrize(
        "backend", ["nvml", "nvapi", "nvml-cooler", "nvapi-cooler"]
    )
    def test_valid_backend(self, pynvoc, backend):
        try:
            pynvoc.set_fan("0", backend, "all", "continuous", 60)
        except ValueError:
            pytest.fail(f"'{backend}' should be a valid backend string")
        except RuntimeError:
            pass


# --- Domain aliases ---
# These should not raise ValueError for domain (may RuntimeError if no GPU).


class TestDomainAliases:
    """Verify clock domain aliases are accepted by parse_domain()."""

    @pytest.mark.parametrize("alias", ["core", "gpu", "graphics", "mem", "memory"])
    def test_domain_alias_accepted(self, pynvoc, alias):
        try:
            pynvoc.set_clock_offset("0", "nvml", alias, 100, "P0")
        except ValueError:
            pytest.fail(f"'{alias}' should be a valid domain alias")
        except RuntimeError:
            pass
