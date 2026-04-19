from pathlib import Path

from nvoc_tui.parsing import (
    compute_vf_plot_bounds,
    find_curve_point_for_voltage,
    load_vf_curve,
    normalize_query_output,
    parse_get_output,
    parse_gpu_list,
    parse_info_output,
    parse_json_output,
    parse_status_output,
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
    assert parsed["voltage_locked"] is True
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
              "controller": "GpuInternal",
              "target": "Gpu"
            },
            37
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
        working_point=(850.0, 1900.0),
    )

    assert bounds is not None
    (x_min, x_max), (y_min, y_max) = bounds
    assert x_min < 800.0
    assert x_max > 870.0
    assert y_min == 0.0
    assert y_max > 2050.0
