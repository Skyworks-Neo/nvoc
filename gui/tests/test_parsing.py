from __future__ import annotations

from pathlib import Path

from src.parsing import (
    analyze_vfp_offsets,
    get_vfp_offset_state_from_csv,
    load_vfp_deltas,
    native_query_payload,
    normalize_native_vfp_lock_bounds,
    parse_dashboard_status,
    parse_gpu_list_output,
    parse_info_limits,
    parse_legacy_overvolt_bounds,
    parse_status_current_values,
    parse_supported_pstates,
    parse_vfp_lock_bounds,
    write_vfp_points,
)


def test_parse_gpu_list_output_keeps_first_name_and_uuid() -> None:
    output = """
    GPU 0: NVIDIA GeForce RTX 3060 UUID=GPU-1234-5678
    GPU 0: ID:0x0800 bus:12345678
    """

    short, long, names, uuid_map = parse_gpu_list_output(output)

    assert short == {0: "GPU 0: NVIDIA GeForce RTX 3060"}
    assert long == {0: "GPU 0: NVIDIA GeForce RTX 3060  [GPU-1234-5678]"}
    assert names == {0: "NVIDIA GeForce RTX 3060"}
    assert uuid_map == {0: "GPU-1234-5678"}


def test_parse_info_limits_text_output() -> None:
    output = """
    Architecture........: Ada
    VFP (Graphics)......: -500 MHz ~ 500 MHz
    VFP (Memory)........: -500 MHz ~ 1500 MHz
    Power Limit.........: 58% ~ 124% (100% default) | 100W min / 211W current / 212W max
    Thermal Limit.......: 65C ~ 90C (83C default)
    Overvolt P0.........: 0 mV (range: -1018.461 mV - 256.019 mV)
    """

    parsed = parse_info_limits(output)

    assert parsed["gpu_architecture"] == "Ada"
    assert parsed["core_clock_min"] == -500
    assert parsed["mem_clock_max"] == 1500
    assert parsed["power_watt_current"] == 211
    assert parsed["thermal_limit_default"] == 83
    assert parsed["legacy_overvolt_pstate"] == "P0"


def test_native_query_payload_with_prefixed_text() -> None:
    payload = native_query_payload('> native status\n{"gpu_clock_mhz": 2000}')

    assert payload == {"gpu_clock_mhz": 2000}


def test_parse_dashboard_status_text_output() -> None:
    output = """
    Graphics Clock......: 1897 MHz
    Memory Clock........: 7500 MHz
    Core Voltage........: 918 mV (locked)
    Sensor..............: 47C (Internal / Core)
    Power Draw..........: 132 W
    """

    parsed = parse_dashboard_status(output)

    assert parsed["gpu_clock_mhz"] == 1897.0
    assert parsed["mem_clock_mhz"] == 7500.0
    assert parsed["voltage_mv"] == 918.0
    assert parsed["vfp_locked"] is True
    assert parsed["temperature_c"] == 47.0
    assert parsed["power_w"] == 132.0


def test_parse_status_and_get_helpers() -> None:
    status = """
    VFP Lock............: Voltage:875 mV
    OC (Graphics).......: +150 MHz
    OC (Memory).........: +500 MHz
    Power Limit.........: 105%
    Thermal Limit.......: 83C
    Voltage Boost.......: 10%
    """
    locked_mv, current = parse_status_current_values(status)

    assert locked_mv == 875.0
    assert current == {
        "core_clock_current": 150,
        "mem_clock_current": 500,
        "power_limit_current": 105,
        "thermal_limit_current": 83,
        "voltage_boost_current": 10,
    }

    get_output = """
    Supported P-States:
      P0:
      P2:
    Supported Applications Clocks:
    VFP Lock GPU Core Upperbound: 2100 MHz
    VFP Lock GPU Core Lowerbound: 1800 MHz
    Overvolt P0: 0 mV (range: -100 mV - 250 mV)
    """

    assert parse_supported_pstates(get_output) == ["P0", "P2"]
    assert parse_vfp_lock_bounds(get_output) == {
        "vfp_lock_gpu_core_upperbound_mhz": 2100,
        "vfp_lock_gpu_core_lowerbound_mhz": 1800,
    }
    assert parse_legacy_overvolt_bounds(get_output)["legacy_overvolt_max_mv"] == 250


def test_normalize_native_vfp_lock_bounds() -> None:
    payload = {
        "vfp_locks": {
            "Gpu": "2100 MHz",
            "GpuLowerbound": "1800000 kHz",
            "Memory": "9501 MHz",
            "MemoryUnknown": "9000 MHz",
            "Voltage": "875 mV",
        }
    }

    assert normalize_native_vfp_lock_bounds(payload) == {
        "vfp_lock_gpu_core_upperbound_mhz": 2100,
        "vfp_lock_gpu_core_lowerbound_mhz": 1800,
        "vfp_lock_memory_upperbound_mhz": 9501,
        "vfp_lock_memory_lowerbound_mhz": 9000,
    }


def test_vfp_csv_helpers(tmp_path: Path) -> None:
    csv_path = tmp_path / "curve.csv"
    points = [
        {
            "index": 3,
            "voltage_uv": 800000,
            "frequency_khz": 1800000,
            "delta_khz": 25000,
            "default_frequency_khz": 1775000,
        }
    ]

    write_vfp_points(str(csv_path), points)

    assert get_vfp_offset_state_from_csv(str(csv_path)) == (True, 25)
    assert analyze_vfp_offsets([1800.0], [1775.0]) == (True, 25)
    assert load_vfp_deltas(str(csv_path), points) == [(3, 25000)]
