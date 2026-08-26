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
from pathlib import Path

from nvoc_tui.parsing import (
    build_vf_curves,
    compute_vf_plot_bounds,
    find_curve_point_for_voltage,
    load_vf_curve,
    load_vf_curve_deltas,
    normalize_query_output,
    parse_get_output,
    parse_gpu_list,
    parse_info_output,
    parse_json_output,
    parse_status_output,
    vf_curve_points_to_series,
    public_vfp_unsupported,
    reverse_lookup_voltage,
)


def test_parse_gpu_list_with_uuid() -> None:
    output = """
    Detected 1 GPUs via NVML
    GPU 0: NVIDIA GeForce RTX 3060 UUID=GPU-1234-5678
    GPU 0: ID:0x0800 bus:12345678 - 1234 - 5678 - 01
    """

    gpus = parse_gpu_list(output)

    assert len(gpus) == 1
    assert gpus[0].index == 0
    assert gpus[0].name == "NVIDIA GeForce RTX 3060"
    assert gpus[0].uuid == "GPU-1234-5678"


def test_parse_info_output() -> None:
    output = """
    Architecture........: Ada
    VFP (Graphics)......: -500 MHz ~ 500 MHz
    VFP (Memory)........: -500 MHz ~ 1500 MHz
    Power Limit.........: 58% ~ 124% (100% default) | 100W min / 211W current / 212W max
    Thermal Limit.......: 65C ~ 90C (83C default)
    """

    parsed = parse_info_output(output)

    assert parsed["arch"] == "Ada"
    assert parsed["core_clock_min"] == -500
    assert parsed["mem_clock_max"] == 1500
    assert parsed["power_limit_default"] == 100
    assert parsed["power_limit_nvml_current_w"] == 211
    assert parsed["thermal_limit_default"] == 83


def test_parse_status_output() -> None:
    output = """
    Graphics Clock......: 1897 MHz
    Memory Clock........: 7500 MHz
    Core Voltage........: 918 mV (locked)
    Sensor..............: 47C (Internal / Core)
    Power Draw..........: 132 W
    """

    parsed = parse_status_output(output)

    assert parsed["gpu_clock_mhz"] == 1897.0
    assert parsed["mem_clock_mhz"] == 7500.0
    assert parsed["voltage_mv"] == 918.0
    assert parsed["vfp_locked"] is True
    assert "voltage_locked" not in parsed
    assert parsed["temperature_c"] == 47.0
    assert parsed["power_w"] == 132.0


def test_parse_status_output_with_vfp_lock() -> None:
    output = """
    Graphics Clock......: 1897 MHz
    Core Voltage........: 918 mV
    VFP Lock............: GPU Core Upperbound:875 mV
    """

    parsed = parse_status_output(output)

    assert parsed["gpu_clock_mhz"] == 1897.0
    assert parsed["voltage_mv"] == 918.0
    assert parsed["vfp_locked"] is True
    assert parsed["vfp_lock_mv"] == 875.0


def test_parse_status_output_with_vfp_lock_none() -> None:
    output = """
    Graphics Clock......: 1897 MHz
    VFP Lock............: None
    """

    parsed = parse_status_output(output)

    assert parsed["vfp_locked"] is False


def test_parse_get_output() -> None:
    output = """
    Supported P-States:
      P0:
        Core Clock Range   : 210 MHz - 2500 MHz
    Core Clock Offset (P0) : 150 MHz
    Mem Clock Offset (P0)  : 500 MHz
    Power Limit        : 211.00 W (Min: 100.00 W - Max: 212.00 W)
    """

    parsed = parse_get_output(output)

    assert parsed["supported_pstates"] == ["P0"]
    assert parsed["core_clock_current"] == 150
    assert parsed["mem_clock_current"] == 500
    assert parsed["power_limit_nvml_current_w"] == 211


def test_parse_json_output() -> None:
    output = '[{"gpu_clock_mhz": 2000}]'
    parsed = parse_json_output(output)
    assert parsed[0]["gpu_clock_mhz"] == 2000


def test_parse_json_output_with_prefixed_warnings() -> None:
    output = 'Warning: backend init failed\n[{"gpu_clock_mhz": 2000}]'
    parsed = parse_json_output(output)
    assert parsed[0]["gpu_clock_mhz"] == 2000


def test_normalize_status_json_output() -> None:
    output = """
    [
      {
        "clocks": {
          "Graphics": 300000,
          "Memory": 405000
        },
        "voltage": 650000,
        "power": {
          "TotalGpuPower": 1,
          "NormalizedTotalPower": 3
        },
        "sensors": [
          [
            {
              "target": "Gpu",
              "channel_type": 0
            },
            37
          ],
          [
            {
              "target": "Gpu",
              "channel_num": 2,
              "channel_type": 255
            },
            48.25
          ]
        ]
      }
    ]
    """

    parsed = normalize_query_output("status", output)

    assert parsed["gpu_clock_mhz"] == 300.0
    assert parsed["mem_clock_mhz"] == 405.0
    assert parsed["voltage_mv"] == 650.0
    assert parsed["temperature_c"] == 37.0
    # Typed core temp mirrors temperature_c (channel_type 0); the unclassified
    # channel (type 255) must NOT leak into the typed trio.
    assert parsed["temp_core"] == 37.0
    assert "temp_hotspot" not in parsed
    assert "temp_memory" not in parsed
    assert parsed["power_w"] == 1.0


def test_normalize_status_json_output_with_vfp_lock() -> None:
    output = """
    [
      {
        "voltage": 650000,
        "vfp_locks": {
          "GPU": {
            "voltage": 850000
          }
        }
      }
    ]
    """

    parsed = normalize_query_output("status", output)

    assert parsed["voltage_mv"] == 650.0
    assert parsed["vfp_locked"] is True
    assert parsed["vfp_lock_mv"] == 850.0
    assert "voltage_locked" not in parsed


def test_normalize_info_json_output() -> None:
    output = """
    {
      "id": 0,
      "name": "GPU",
      "arch": "Ada",
      "gpu_type": "Desktop"
    }
    """

    parsed = normalize_query_output("info", output)

    assert parsed["arch"] == "Ada"
    assert parsed["gpu_type"] == "Desktop"


def test_load_vf_curve(tmp_path: Path) -> None:
    csv_path = tmp_path / "curve.csv"
    csv_path.write_text(
        "voltage_uv,frequency_khz,delta,default_frequency_khz\n"
        "800000,1800000,0,1750000\n"
        "825000,1840000,0,1775000\n"
        "850000,1900000,0,1800000\n",
        encoding="utf-8",
    )

    voltages, freqs, defaults = load_vf_curve(str(csv_path))

    assert voltages == [800.0, 825.0, 850.0]
    assert freqs == [1800.0, 1840.0, 1900.0]
    assert defaults == [1750.0, 1775.0, 1800.0]


def test_vf_curve_points_to_series_uses_frequency_as_missing_default() -> None:
    voltages, frequencies, defaults = vf_curve_points_to_series([
        {
            "voltage_uv": 800000,
            "frequency_khz": 1800000,
            "default_frequency_khz": 1750000,
        },
        {"voltage_uv": 825000, "frequency_khz": 1840000},
    ])

    assert voltages == [800.0, 825.0]
    assert frequencies == [1800.0, 1840.0]
    assert defaults == [1750.0, 1840.0]


def test_load_vf_curve_deltas_skips_invalid_rows(tmp_path: Path) -> None:
    csv_path = tmp_path / "curve.csv"
    csv_path.write_text(
        "voltage_uv,frequency_khz,delta,default_frequency_khz\n"
        "800000,1800000,25000,1750000\n"
        "825000,1840000,not-a-delta,1775000\n"
        "bad-voltage,1900000,10000,1800000\n"
        "850000,1900000,5000 # edited,1800000\n"
        "875000,1910000,-10000,1810000\n",
        encoding="utf-8",
    )

    deltas = load_vf_curve_deltas(
        str(csv_path),
        [
            {"index": 3, "voltage_uv": 800000},
            {"index": 4, "voltage_uv": 825000},
            {"index": 5, "voltage_uv": "invalid"},
            {"index": 6, "voltage_uv": 875000},
        ],
    )

    assert deltas == [(3, 25000), (6, -10000)]


def test_find_curve_point_for_voltage_returns_nearest_match() -> None:
    point = find_curve_point_for_voltage(
        [800.0, 825.0, 850.0],
        [1800.0, 1840.0, 1900.0],
        833.0,
    )

    assert point == (825.0, 1840.0)


def test_find_curve_point_for_voltage_handles_missing_or_invalid_data() -> None:
    assert find_curve_point_for_voltage([], [], 825.0) is None
    assert find_curve_point_for_voltage([800.0], [], 800.0) is None
    assert find_curve_point_for_voltage([800.0], [1800.0], None) is None


def test_compute_vf_plot_bounds_includes_live_and_working_points() -> None:
    bounds = compute_vf_plot_bounds(
        [800.0, 825.0, 850.0],
        [1800.0, 1840.0, 1900.0],
        [1750.0, 1775.0, 1800.0],
        live_point=(870.0, 2050.0),
        lock_point=(875.0, 2100.0),
        working_point=(850.0, 1900.0),
    )

    assert bounds is not None
    (x_min, x_max), (y_min, y_max) = bounds
    assert x_min < 800.0
    assert x_max > 875.0
    assert y_min == 0.0
    assert y_max > 2100.0


def test_public_vfp_unsupported_substring_match() -> None:
    assert public_vfp_unsupported(None) is False
    assert public_vfp_unsupported("pynvoc VFP query failed: boom") is False
    assert public_vfp_unsupported("Rusty VFP error: Not Supported") is True
    assert public_vfp_unsupported("driver said no implementation") is True


def test_build_vf_curves_public_only() -> None:
    gpc_points = [
        {
            "index": 0,
            "voltage_uv": 800000,
            "frequency_khz": 1800000,
            "default_frequency_khz": 1785000,
            "point_type": "prog",
        }
    ]

    curves = build_vf_curves(gpc_points, None, None)

    assert set(curves) == {"gpc"}
    assert curves["gpc"].source == "public"
    assert curves["gpc"].write_mode == "public"
    assert curves["gpc"].has_fixed is False
    assert curves["gpc"].voltages == [800.0]
    assert curves["gpc"].frequencies == [1800.0]
    assert curves["gpc"].defaults == [1785.0]


def test_build_vf_curves_fixed_point_forces_private_write() -> None:
    gpc_points = [
        {
            "index": 0,
            "voltage_uv": 800000,
            "frequency_khz": 1800000,
            "default_frequency_khz": 1785000,
            "point_type": "fixed",
        }
    ]

    curves = build_vf_curves(gpc_points, None, None)

    assert curves["gpc"].has_fixed is True
    assert curves["gpc"].write_mode == "private"


def test_build_vf_curves_private_segments_and_skips() -> None:
    clk_data = {
        "segments": [
            {
                "kind": "vf_curve",
                "domain": "gpc",
                "bank": 0,
                "start_index": 0,
                "end_index": 1,
            },
            {
                "kind": "vf_curve",
                "domain": "xbar",
                "bank": 1,
                "start_index": 2,
                "end_index": 3,
            },
            {
                "kind": "vf_curve",
                "domain": "host",
                "bank": 2,
                "start_index": 6,
                "end_index": 7,
            },
            # pstate_bins and unknown domains are never curves.
            {
                "kind": "pstate_bins",
                "domain": "gpc",
                "bank": 3,
                "start_index": 0,
                "end_index": 9,
            },
            {
                "kind": "vf_curve",
                "domain": "sysclk",
                "bank": 4,
                "start_index": 0,
                "end_index": 2,
            },
        ],
        "points": [
            {
                "bank": 0,
                "index": 0,
                "voltage_uV": 700000,
                "freq_current_mhz": 1000.0,
                "freq_default_mhz": 1000.0,
            },
            {
                "bank": 0,
                "index": 1,
                "voltage_uV": 750000,
                "freq_current_mhz": 1100.0,
                "freq_default_mhz": 1100.0,
            },
            {
                "bank": 1,
                "index": 2,
                "voltage_uV": 700000,
                "freq_current_mhz": 1200.0,
                "freq_default_mhz": 1200.0,
            },
            {
                "bank": 1,
                "index": 3,
                "voltage_uV": 750000,
                "freq_current_mhz": 1300.0,
                "freq_default_mhz": 1300.0,
            },
            {
                "bank": 2,
                "index": 6,
                "voltage_uV": 600000,
                "freq_current_mhz": 900.0,
                "freq_default_mhz": 900.0,
            },
            {
                "bank": 2,
                "index": 7,
                "voltage_uV": 650000,
                "freq_current_mhz": 950.0,
                "freq_default_mhz": 950.0,
            },
            {
                "bank": 4,
                "index": 0,
                "voltage_uV": 500000,
                "freq_current_mhz": 800.0,
                "freq_default_mhz": 800.0,
            },
        ],
    }

    curves = build_vf_curves(None, "driver said Not Supported", clk_data)

    # Public read unsupported → private GPC segment is the GPC source.
    assert set(curves) == {"gpc", "xbar", "host"}
    assert curves["gpc"].source == "private"
    assert curves["gpc"].bank == 0
    assert (curves["gpc"].seg_start, curves["gpc"].seg_end) == (0, 1)
    assert curves["xbar"].bank == 1
    assert (curves["xbar"].seg_start, curves["xbar"].seg_end) == (2, 3)
    assert curves["host"].voltages == [600.0, 650.0]
    for curve in curves.values():
        assert curve.write_mode == "private"


def test_build_vf_curves_none_when_no_source() -> None:
    assert build_vf_curves(None, "boom", None) is None
    assert build_vf_curves([], None, {"segments": [], "points": []}) is None


def test_reverse_lookup_voltage_interpolates_and_clamps() -> None:
    volts = [700.0, 750.0, 800.0]
    freqs = [1200.0, 1300.0, 1400.0]

    assert reverse_lookup_voltage(volts, freqs, 1350.0) == 775.0
    assert reverse_lookup_voltage(volts, freqs, 1100.0) == 700.0
    assert reverse_lookup_voltage(volts, freqs, 1500.0) == 800.0
    # Descending order still resolves.
    assert (
        reverse_lookup_voltage(list(reversed(volts)), list(reversed(freqs)), 1250.0)
        == 725.0
    )
    assert reverse_lookup_voltage([], [], 1000.0) is None
    assert reverse_lookup_voltage([700.0], [1200.0], 900.0) == 700.0
