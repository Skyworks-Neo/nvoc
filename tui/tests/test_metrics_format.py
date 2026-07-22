from nvoc_tui.metrics_format import _format_metric_lines


def test_format_metric_lines_full() -> None:
    status = {
        "gpu_clock_mhz": 1800,
        "mem_clock_mhz": 7500,
        "voltage_mv": 950,
        "temperature_c": 62.4,
        "power_w": 132,
        "pstate": "P0",
        "utilization": {
            "Graphics": 100,
            "FrameBuffer": 0,
            "VideoEngine": 12,
            "BusInterface": 2,
        },
        "vram": {
            # 8 GiB total, 2 GiB used (KiB).
            "total_kib": 8_388_608,
            "used_kib": 2_097_152,
            "free_kib": 6_291_456,
            "shared_kib": 0,
        },
        "coolers": {
            "Cooler1": {"current_level": 45, "current_tach": 1234, "active": True},
        },
        "pcie_lanes": 16,
        "perf": {"unknown": 0, "limits": 64},
    }

    text = "\n".join(_format_metric_lines(status, "Ada"))

    assert "GPU: 1800 MHz" in text
    assert "MEM: 7500 MHz" in text
    assert "VOLT: 950 mV" in text
    assert "TEMP: 62 C" in text
    assert "PWR: 132 W" in text
    assert "PSTATE: P0" in text
    assert "LOAD: GPU 100% | MC 0% | VEN 12% | BUS 2%" in text
    assert "VRAM: 2.0 / 8.0 GB" in text
    assert "FAN: 1234 RPM @ 45%" in text
    assert "PCIE: x16" in text
    assert "PERF: 0x40" in text
    assert "ARCH: Ada" in text


def test_format_metric_lines_missing_fields_render_dashes() -> None:
    text = "\n".join(_format_metric_lines({}, "---"))

    assert "LOAD: GPU --- | MC --- | VEN --- | BUS ---" in text
    assert "VRAM: ---" in text
    assert "FAN: ---" in text
    assert "PCIE: ---" in text
    assert "PSTATE: ---" in text
    assert "PERF: ---" in text


def test_format_metric_lines_multi_cooler_labels() -> None:
    status = {
        "coolers": {
            "Cooler1": {"current_level": 30, "current_tach": 1000, "active": True},
            "Cooler2": {"current_level": 50, "current_tach": 2000, "active": True},
        }
    }

    text = "\n".join(_format_metric_lines(status, "---"))

    assert "FAN1: 1000 RPM @ 30%" in text
    assert "FAN2: 2000 RPM @ 50%" in text
