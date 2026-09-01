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

from nvoc_tui.models import CurveData
from nvoc_tui.parsing import (
    build_vf_curves,
    curve_meta,
    compute_vf_plot_bounds,
    find_curve_point_for_voltage,
    load_vf_curve,
    load_vf_curve_deltas,
    normalize_domain_offsets,
    normalize_query_output,
    parse_get_output,
    parse_gpu_list,
    parse_info_output,
    parse_json_output,
    parse_status_output,
    synthesize_effective,
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
          "Memory": 405000,
          "Video": 1327000
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
    assert parsed["video_clock_mhz"] == 1327.0
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
    voltages, frequencies, defaults = vf_curve_points_to_series(
        [
            {
                "voltage_uv": 800000,
                "frequency_khz": 1800000,
                "default_frequency_khz": 1750000,
            },
            {"voltage_uv": 825000, "frequency_khz": 1840000},
        ]
    )

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


def test_curve_meta_fallback_for_unknown_ids() -> None:
    # Known ids resolve through CURVE_META unchanged.
    assert curve_meta("gpc")["label"] == "GPC"
    assert curve_meta("msd")["domain_bit"] == 21
    # unknownN ids synthesize display-only meta: no domain bit (no live
    # crosshair), neutral prior class, label UNK<n>.
    meta = curve_meta("unknown1")
    assert meta["label"] == "UNK1"
    assert meta["domain_bit"] is None
    assert meta["class"] == "graphics"


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
                "domain": "msd",
                "bank": 2,
                "start_index": 6,
                "end_index": 7,
            },
            # pstate_bins are never curves; the unnamed "sysclk" domain
            # displays as unknown1 (50-series fourth-curve support).
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
    assert set(curves) == {"gpc", "xbar", "msd", "unknown1"}
    # Canonical display order: GPC → XBAR → MSD → unknownN, even though the
    # GPC segment is resolved last (the selector/plot iterate this dict).
    assert list(curves) == ["gpc", "xbar", "msd", "unknown1"]
    assert curves["gpc"].source == "private"
    assert curves["gpc"].bank == 0
    assert (curves["gpc"].seg_start, curves["gpc"].seg_end) == (0, 1)
    assert curves["xbar"].bank == 1
    assert (curves["xbar"].seg_start, curves["xbar"].seg_end) == (2, 3)
    assert curves["msd"].voltages == [600.0, 650.0]
    # Unnamed domain → unknown1, plotted like any curve (one point here).
    assert curves["unknown1"].voltages == [500.0]
    assert curves["unknown1"].frequencies == [800.0]
    for curve in curves.values():
        assert curve.write_mode == "private"


def _private_gpc_clk_data(voltages_uv, currents, defaults):
    return {
        "segments": [
            {
                "kind": "vf_curve",
                "domain": "gpc",
                "bank": 0,
                "start_index": 0,
                "end_index": len(voltages_uv) - 1,
            }
        ],
        "points": [
            {
                "bank": 0,
                "index": i,
                "voltage_uV": v,
                "freq_current_mhz": c,
                "freq_default_mhz": d,
            }
            for i, (v, c, d) in enumerate(zip(voltages_uv, currents, defaults))
        ],
    }


def test_build_vf_curves_hybrid_public_currents_private_defaults() -> None:
    # Unshifted public grid matches the private voltage axis: the curve
    # adopts public CURRENT frequencies (OC state) while the private
    # segment stays the DEFAULT-axis authority.
    clk_data = _private_gpc_clk_data(
        [800000, 825000], currents=[1000.0, 1100.0], defaults=[1000.0, 1100.0]
    )
    gpc_points = [
        {
            "index": 0,
            "voltage_uv": 800000,
            "frequency_khz": 1050000,
            "default_frequency_khz": 999999,  # public default IGNORED
            "point_type": "prog",
        },
        {
            "index": 1,
            "voltage_uv": 825000,
            "frequency_khz": 1150000,
            "point_type": "prog",
        },
    ]

    curves = build_vf_curves(gpc_points, None, clk_data)

    assert curves["gpc"].source == "hybrid"
    assert curves["gpc"].frequencies == [1050.0, 1150.0]
    assert curves["gpc"].defaults == [1000.0, 1100.0]
    assert curves["gpc"].has_fixed is False


def test_build_vf_curves_shifted_public_grid_falls_back_to_private() -> None:
    # Negative slot1 shift: the public voltage grid moved, so it no longer
    # matches the private default axis — public currents are discarded.
    clk_data = _private_gpc_clk_data(
        [800000, 825000], currents=[1000.0, 1100.0], defaults=[1000.0, 1100.0]
    )
    gpc_points = [
        {
            "index": 0,
            "voltage_uv": 780000,  # shifted by -20mV
            "frequency_khz": 1050000,
            "point_type": "prog",
        },
        {
            "index": 1,
            "voltage_uv": 805000,
            "frequency_khz": 1150000,
            "point_type": "prog",
        },
    ]

    curves = build_vf_curves(gpc_points, None, clk_data)

    assert curves["gpc"].source == "private"
    assert curves["gpc"].frequencies == [1000.0, 1100.0]
    assert curves["gpc"].defaults == [1000.0, 1100.0]
    assert curves["gpc"].has_fixed is True


def test_build_vf_curves_broken_public_frequencies_rejected() -> None:
    # Positive slot1 breakage: the public fill path bails and zeroes the
    # data words. Even if the voltage grid happened to match, an
    # all-zero CURRENT frequency column must not become a hybrid curve.
    clk_data = _private_gpc_clk_data(
        [800000, 825000], currents=[1000.0, 1100.0], defaults=[1000.0, 1100.0]
    )
    gpc_points = [
        {
            "index": 0,
            "voltage_uv": 800000,
            "frequency_khz": 0,
            "point_type": "prog",
        },
        {
            "index": 1,
            "voltage_uv": 825000,
            "frequency_khz": 0,
            "point_type": "prog",
        },
    ]

    curves = build_vf_curves(gpc_points, None, clk_data)

    assert curves["gpc"].source == "private"
    assert curves["gpc"].frequencies == [1000.0, 1100.0]
    assert curves["gpc"].defaults == [1000.0, 1100.0]


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


def _domain_info(entries):
    return {"entries": entries}


def test_normalize_domain_offsets_gates_and_write_bit_mapping() -> None:
    # value_modifiable=False entries are dropped (value fields are not
    # driver data), measure bits (msd 21 / mem 4) never map — only the
    # WRITE bits do (msd 5 / mem 2), and values_kHz[0]=kHz / [1]=µV.
    raw = _domain_info(
        [
            {"bit": 0, "value_modifiable": True, "values_kHz": [25000, 100000]},
            {
                "bit": 5,
                "value_modifiable": True,
                "values_kHz": [0, 50000],
            },  # msd WRITE bit
            {
                "bit": 21,
                "value_modifiable": True,
                "values_kHz": [0, 999999],
            },  # msd MEASURE bit — must be dropped
            {
                "bit": 2,
                "value_modifiable": True,
                "values_kHz": [0, 0],
            },  # mem WRITE bit, zero offsets still mapped
            {
                "bit": 4,
                "value_modifiable": True,
                "values_kHz": [0, 123456],
            },  # mem MEASURE bit — dropped
            {
                "bit": 1,
                "value_modifiable": False,
                "values_kHz": [0, 777777],
            },  # unmodifiable — dropped
            {"bit": None, "value_modifiable": True, "values_kHz": [1, 2]},
            {"bit": 3, "value_modifiable": True, "values_kHz": [1]},  # short list
        ]
    )

    offsets = normalize_domain_offsets(raw)

    assert offsets == {
        "gpc": {"slot0_khz": 25000, "slot1_uv": 100000},
        "msd": {"slot0_khz": 0, "slot1_uv": 50000},
        "mem": {"slot0_khz": 0, "slot1_uv": 0},
    }
    assert normalize_domain_offsets(None) == {}
    assert normalize_domain_offsets({"entries": []}) == {}
    assert normalize_domain_offsets({"supported": False}) == {}


def test_synthesize_effective_positive_shift() -> None:
    curve = CurveData("gpc")
    curve.voltages = [800.0, 825.0]
    curve.frequencies = [1000.0, 1100.0]
    curve.defaults = [1000.0, 1100.0]

    eff = synthesize_effective(curve, {"gpc": {"slot0_khz": 25000, "slot1_uv": 100000}})

    assert eff.applicable is True
    assert eff.offset_mv == 100.0
    assert eff.offset_mhz == 25.0
    assert eff.voltages == [900.0, 925.0]
    assert eff.freqs == [1025.0, 1125.0]


def test_synthesize_effective_no_op_guards() -> None:
    curve = CurveData("gpc")
    curve.voltages = [800.0, 825.0]
    curve.frequencies = [1000.0, 1100.0]

    # Zero offsets → not applicable.
    eff = synthesize_effective(curve, {"gpc": {"slot0_khz": 0, "slot1_uv": 0}})
    assert eff.applicable is False
    assert eff.voltages == []

    # No own-domain entry (other domains' offsets are not ours to apply).
    eff = synthesize_effective(curve, {"xbar": {"slot0_khz": 0, "slot1_uv": 50000}})
    assert eff.applicable is False

    # No offset data at all.
    eff = synthesize_effective(curve, {})
    assert eff.applicable is False

    # Pascal-style all-zero voltage axis → nothing to shift.
    pascal = CurveData("gpc")
    pascal.voltages = [0.0, 0.0]
    pascal.frequencies = [1000.0, 1100.0]
    eff = synthesize_effective(pascal, {"gpc": {"slot0_khz": 0, "slot1_uv": 100000}})
    assert eff.applicable is False

    # GCOFF-style null-filled frequency segment → nothing to lift.
    gcoff = CurveData("gpc")
    gcoff.voltages = [800.0, 825.0]
    gcoff.frequencies = [0.0, 0.0]
    eff = synthesize_effective(gcoff, {"gpc": {"slot0_khz": 25000, "slot1_uv": 100000}})
    assert eff.applicable is False


def test_build_vf_curves_attaches_effective_on_private_gpc() -> None:
    # Positive-slot1 broken state: public read zeroed, private defaults rule,
    # and the gpc slot1 readback synthesizes the effective (right-shifted)
    # series — the display cure for the broken-positive-offset state.
    clk_data = _private_gpc_clk_data(
        [800000, 825000], currents=[1000.0, 1100.0], defaults=[1000.0, 1100.0]
    )
    gpc_points = [
        {"index": 0, "voltage_uv": 800000, "frequency_khz": 0, "point_type": "prog"},
        {"index": 1, "voltage_uv": 825000, "frequency_khz": 0, "point_type": "prog"},
    ]
    domain_info = _domain_info(
        [{"bit": 0, "value_modifiable": True, "values_kHz": [0, 100000]}]
    )

    curves = build_vf_curves(gpc_points, None, clk_data, domain_info)

    eff = curves["gpc"].effective
    assert eff is not None
    assert eff.applicable is True
    assert eff.voltages == [900.0, 925.0]
    assert eff.freqs == [1000.0, 1100.0]
    # Display-only: the base series itself must stay unshifted.
    assert curves["gpc"].voltages == [800.0, 825.0]
    assert curves["gpc"].frequencies == [1000.0, 1100.0]


def test_build_vf_curves_sentinel_survivor_still_detected_broken() -> None:
    # Live-V100 shape: the broken positive-slot1 read zeroes every data word
    # EXCEPT the #0 sentinel (450 mV / 0.4 MHz survives) — `any(freq>0)`
    # misses the breakage; the populated-fraction detector must catch it and
    # the sentinel must never become a hybrid curve.
    volts = [800000, 812500, 825000, 837500]
    clk_data = _private_gpc_clk_data(
        volts,
        currents=[1000.0, 1050.0, 1100.0, 1150.0],
        defaults=[1000.0, 1050.0, 1100.0, 1150.0],
    )
    gpc_points = [
        {
            "index": 0,
            "voltage_uv": 450000,  # the surviving sentinel
            "frequency_khz": 405,
            "point_type": "fixed",
        },
    ] + [
        {"index": i, "voltage_uv": 0, "frequency_khz": 0, "point_type": "fixed"}
        for i in range(1, 4)
    ]
    domain_info = _domain_info(
        [{"bit": 0, "value_modifiable": True, "values_kHz": [0, 100000]}]
    )

    curves = build_vf_curves(gpc_points, None, clk_data, domain_info)

    assert curves["gpc"].source == "private"
    eff = curves["gpc"].effective
    assert eff is not None
    assert eff.applicable is True
    assert eff.voltages == [900.0, 912.5, 925.0, 937.5]


def test_build_vf_curves_no_effective_on_public_source_or_no_offsets() -> None:
    # Pascal-style: private axis all-zero → public stays the GPC source, and
    # a public-source GPC never synthesizes (no trustworthy private base).
    clk_data = _private_gpc_clk_data([0, 0], [0.0, 0.0], [0.0, 0.0])
    gpc_points = [
        {
            "index": 0,
            "voltage_uv": 800000,
            "frequency_khz": 1050000,
            "default_frequency_khz": 1000000,
            "point_type": "prog",
        },
        {
            "index": 1,
            "voltage_uv": 825000,
            "frequency_khz": 1150000,
            "default_frequency_khz": 1100000,
            "point_type": "prog",
        },
    ]
    domain_info = _domain_info(
        [{"bit": 0, "value_modifiable": True, "values_kHz": [0, 100000]}]
    )

    curves = build_vf_curves(gpc_points, None, clk_data, domain_info)
    assert curves["gpc"].source == "public"
    assert curves["gpc"].effective is None

    # No domain_info at all → zero behavior change for a private GPC too.
    clk_data2 = _private_gpc_clk_data(
        [800000, 825000], [1000.0, 1100.0], [1000.0, 1100.0]
    )
    curves2 = build_vf_curves(None, "not supported", clk_data2, None)
    assert curves2["gpc"].effective is None

    # Public absent with offsets present is "not supported", NOT breakage —
    # the fallback must not fire.
    curves3 = build_vf_curves(None, "not supported", clk_data2, domain_info)
    assert curves3["gpc"].effective is None


def test_build_vf_curves_no_effective_on_healthy_or_shifted_public() -> None:
    # The synthesis fallback fires ONLY on the detected-broken public read.
    # A healthy unshifted public read (hybrid) already carries live currents,
    # and a shifted-but-valid read (negative slot1) displays fine through
    # the private axis — neither synthesizes even with offsets present.
    clk_data = _private_gpc_clk_data(
        [800000, 825000], currents=[1000.0, 1100.0], defaults=[1000.0, 1100.0]
    )
    domain_info = _domain_info(
        [{"bit": 0, "value_modifiable": True, "values_kHz": [0, 100000]}]
    )
    healthy_public = [
        {
            "index": 0,
            "voltage_uv": 800000,
            "frequency_khz": 1050000,
            "point_type": "prog",
        },
        {
            "index": 1,
            "voltage_uv": 825000,
            "frequency_khz": 1150000,
            "point_type": "prog",
        },
    ]
    shifted_public = [
        {
            "index": 0,
            "voltage_uv": 780000,  # negative-slot1 grid shift, data intact
            "frequency_khz": 1050000,
            "point_type": "prog",
        },
        {
            "index": 1,
            "voltage_uv": 805000,
            "frequency_khz": 1150000,
            "point_type": "prog",
        },
    ]

    curves = build_vf_curves(healthy_public, None, clk_data, domain_info)
    assert curves["gpc"].source == "hybrid"
    assert curves["gpc"].effective is None

    curves = build_vf_curves(shifted_public, None, clk_data, domain_info)
    assert curves["gpc"].source == "private"
    assert curves["gpc"].effective is None
