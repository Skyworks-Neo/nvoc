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
        "pcie_link_gen": 4,
        "pcie_max_link_gen": 4,
        "pcie_tx_mibps": 1234.5,
        "pcie_rx_mibps": 7.8,
        "perf": {"unknown": 0, "limits": 32},
    }

    text = "\n".join(_format_metric_lines(status, "Ada"))

    assert "GPU: 1800 MHz" in text
    assert "MEM: 7500 MHz" in text
    assert "VOLT: 950 mV" in text
    assert "TEMP: CORE 62 C" in text
    assert "PWR: 132 W" in text
    assert "PSTATE: P0" in text
    assert "LOAD: GPU 100% | MC 0% | VEN 12% | BUS 2%" in text
    assert "VRAM: 2.0 / 8.0 GB" in text
    assert "FAN: 1234 RPM @ 45%" in text
    assert "PCIE: Gen4/4 x16" in text
    # PCIe generation prepended as "Gen<cur>/<max>".
    # Bidirectional bandwidth appended after lane count, nvitop-style (↑Tx ↓Rx).
    # 1234.5 MiB/s -> 1.2 GiB/s; 7.8 -> 7.8 MiB/s.
    assert "↑1.2 GiB/s" in text
    assert "↓7.8 MiB/s" in text
    # limits = 32 = UNKNOWN_32 bit -> decoded reason name, not raw hex.
    assert "PERF LIMIT: Unknown32" in text
    assert "ARCH: Ada" in text


def test_format_metric_lines_pwr_with_power_limit() -> None:
    """PWR line appends the enforced power limit (live TGP cap) as `draw W / limit W`
    when the backend reports it — mirroring nvidia-smi's `1W / 30W` form."""
    status_with_limit = {"power_w": 1.653, "power_limit_w": 30}
    text = "\n".join(_format_metric_lines(status_with_limit, "Ada"))
    assert "PWR: 1.653 W / 30 W" in text

    # Without a limit the line keeps the plain `draw W` form.
    status_no_limit = {"power_w": 132}
    text = "\n".join(_format_metric_lines(status_no_limit, "Ada"))
    assert "PWR: 132 W" in text
    assert "/" not in text


def test_format_metric_lines_pcie_bandwidth_absent() -> None:
    """No bandwidth keys -> PCIE line keeps just the lane count."""
    status = {"pcie_lanes": 8}
    text = "\n".join(_format_metric_lines(status, "Ada"))
    assert "PCIE: x8" in text
    assert "↑" not in text
    assert "↓" not in text


def test_format_metric_lines_pcie_bandwidth_only_lanes_missing() -> None:
    """Bandwidth present but no lane count -> shows bandwidth alone."""
    status = {"pcie_tx_mibps": 0.5, "pcie_rx_mibps": 0.3}
    text = "\n".join(_format_metric_lines(status, "Ada"))
    assert "↑0.5 MiB/s" in text
    assert "↓0.3 MiB/s" in text


def test_format_metric_lines_fabric_clocks() -> None:
    """FCLK line surfaces internal fabric clocks (Xbar/crossbar, Sys, Msd,
    Hub, ...) from the GetAllClocks V2 all_clocks_mhz breakdown. Msd renders:
    it is the uncore-band domain the ClkDomains bit-5 offset record drives
    (it was hidden back when it read as memory-subsystem noise)."""
    status = {
        "all_clocks_mhz": {
            "Gpc": 2100.0,
            "Xbar": 1800.0,  # the "crossbar clock" GPU-Z shows
            "Sys": 900.0,
            "Msd": 2460.0,
            "Hub": 600.0,
            "M": 7500.0,  # memory — not in the fabric list
            "Hotclk": 0.0,  # zero -> omitted
        }
    }
    text = "\n".join(_format_metric_lines(status, "Ada"))
    assert "FCLK: GPC 2100 | XBAR 1800 | SYS 900 | MSD 2460 | HUB 600 MHz" in text
    # Memory and zero clocks must NOT appear on the FCLK line.
    assert "M 7500" not in text
    assert "HOTCLK 0" not in text


def test_format_metric_lines_fabric_clocks_v2_suffixed_names() -> None:
    """Server Pascal (P100) reports the V2-suffixed Pascal-cluster domain
    names — Gpc2/Xbar2/Sys2/Hub2/Ltc2 plus Pwr/Utils — which must all render
    (regression: only bare names matched and FCLK showed HOST alone)."""
    status = {
        "all_clocks_mhz": {
            "Gpc2": 1256.923,
            "Xbar2": 1235.302,
            "Sys2": 1130.459,
            "Hub2": 1296.0,
            "Ltc2": 1209.988,
            "Host": 571.428,
            "Pwr": 540.0,
            "Utils": 108.0,
            "M": 715.5,  # memory — stays off FCLK
        }
    }
    text = "\n".join(_format_metric_lines(status, "Pascal"))
    assert (
        "FCLK: GPC 1257 | XBAR 1235 | SYS 1130 | HUB 1296 | HOST 571"
        " | LTC 1210 | PWR 540 | UTILS 108 MHz" in text
    )
    assert "M 716" not in text
    # No "2" suffix leaks into a rendered label.
    assert "GPC2" not in text


def test_format_metric_lines_fabric_clocks_prefers_first_of_duplicates() -> None:
    """A payload carrying both the bare and V2-suffixed spelling of one
    domain must not print the cluster twice."""
    status = {"all_clocks_mhz": {"Gpc": 2100.0, "Gpc2": 2099.0}}
    text = "\n".join(_format_metric_lines(status, "Ada"))
    assert text.count("GPC ") == 1
    assert "GPC 2100" in text


def test_format_metric_lines_fabric_clocks_absent() -> None:
    """No all_clocks_mhz -> FCLK line shows dashes."""
    status = {}
    text = "\n".join(_format_metric_lines(status, "Ada"))
    assert "FCLK: ---" in text


def test_format_metric_lines_perf_decodes_multiple_reasons() -> None:
    # limits = 18 = THERMAL_LIMIT(2) | NO_LOAD_LIMIT(16), decoded in bit order.
    text = "\n".join(
        _format_metric_lines({"perf": {"unknown": 0, "limits": 18}}, "---")
    )
    assert "PERF LIMIT: Temperature, No Load" in text


def test_format_metric_lines_perf_zero_is_none() -> None:
    # No active limit reason -> "none" (not "0x0").
    text = "\n".join(_format_metric_lines({"perf": {"unknown": 0, "limits": 0}}, "---"))
    assert "PERF LIMIT: none" in text


def test_format_metric_lines_thermal_trio() -> None:
    status = {
        "temperature_c": 55,
        "temp_hotspot": 63.5,
        "temp_memory": 58,
    }

    text = "\n".join(_format_metric_lines(status, "---"))

    # All three present — core always shown, hotspot/memory appended in order.
    assert "TEMP: CORE 55 C | HOTSPOT 64 C | MEM 58 C" in text


def test_format_metric_lines_effective_clocks() -> None:
    status = {
        "gpu_clock_mhz": 1800,
        "mem_clock_mhz": 7500,
        "eff_gpu_clock_mhz": 1897.5,
        "eff_mem_clock_mhz": 7500,
    }

    text = "\n".join(_format_metric_lines(status, "---"))

    assert "ECLK: GPU 1898 | MEM 7500 MHz" in text


def test_format_metric_lines_effective_clocks_absent() -> None:
    text = "\n".join(_format_metric_lines({"gpu_clock_mhz": 1800}, "---"))

    assert "ECLK: ---" in text


def test_format_metric_lines_core_only_when_no_extra_temps() -> None:
    text = "\n".join(_format_metric_lines({"temperature_c": 47}, "---"))

    assert "TEMP: CORE 47 C" in text
    assert "HOT" not in text
    assert "MEM 47 C" not in text


def test_format_metric_lines_pairs_live_with_policy_thresholds() -> None:
    # target_temp_c (policy 2 = target-temperature wall) pairs with the core
    # reading; max_temp_c (policy 1 = max operating temp) pairs with hot spot.
    status = {
        "temperature_c": 46,
        "target_temp_c": 87,
        "temp_hotspot": 53,
        "max_temp_c": 105,
    }

    text = "\n".join(_format_metric_lines(status, "---"))

    assert "TEMP: CORE 46 / 87 C | HOTSPOT 53 / 105 C" in text


def test_format_metric_lines_thresholds_optional() -> None:
    # Only the target wall present, no max temp: core pairs, hot spot stays bare.
    status = {"temperature_c": 46, "target_temp_c": 87, "temp_hotspot": 53}

    text = "\n".join(_format_metric_lines(status, "---"))

    assert "TEMP: CORE 46 / 87 C | HOTSPOT 53 C" in text


def test_format_metric_lines_multi_rail_voltage() -> None:
    """Multi-rail part: VOLT goes per-rail (GPC | MEM/MSVDD) using the live
    rail currents the dashboard poll attaches; fractional mV kept, whole mV
    rendered bare."""
    status = {
        "voltage_mv": 950,
        "rail_volts_mv": [("GPC", 1050.0), ("MEM", 681.25)],
    }
    text = "\n".join(_format_metric_lines(status, "Pascal"))
    assert "VOLT: GPC 1050 mV | MEM 681.25 mV" in text

    # Fabric rail (50-series MSVDD) uses its own label.
    status["rail_volts_mv"] = [("GPC", 1000.0), ("MSVDD", 655.5)]
    text = "\n".join(_format_metric_lines(status, "Blackwell"))
    assert "VOLT: GPC 1000 mV | MSVDD 655.5 mV" in text


def test_format_metric_lines_single_rail_uses_real_rail_current() -> None:
    """Single-rail part: VOLT shows the real rail current_uV (volt-rails
    status), not the coarse NVAPI ``voltage_mv`` field — they differ by a
    few mV on a 4060 Laptop (1020 rail vs 1010 voltage_mv). Plain form,
    no label (only one rail to name)."""
    # Real rail 1020 mV, voltage_mv would say 1010 — the rail value wins.
    status = {"voltage_mv": 1010, "rail_volts_mv": [("GPC", 1020.0)]}
    text = "\n".join(_format_metric_lines(status, "Ada"))
    assert "VOLT: 1020 mV" in text
    assert "1010" not in text  # voltage_mv must NOT leak in
    assert "GPC" not in text  # no label on single-rail form

    # Second rail present but reading zero (idle) — dropped, stays single-rail.
    status["rail_volts_mv"] = [("GPC", 950.0), ("MSVDD", 0.0)]
    text = "\n".join(_format_metric_lines(status, "Ada"))
    assert "VOLT: 950 mV" in text


def test_format_metric_lines_voltage_falls_back_without_rails() -> None:
    """No rail data at all (volt-rails family unsupported on this part):
    fall back to the NVAPI ``voltage_mv`` field."""
    status = {"voltage_mv": 950}
    text = "\n".join(_format_metric_lines(status, "Ada"))
    assert "VOLT: 950 mV" in text


def test_format_metric_lines_missing_fields_render_dashes() -> None:
    text = "\n".join(_format_metric_lines({}, "---"))

    assert "LOAD: GPU --- | MC --- | VEN --- | BUS ---" in text
    assert "VRAM: ---" in text
    assert "FAN: ---" in text
    assert "PCIE: ---" in text
    assert "PSTATE: ---" in text
    assert "PERF LIMIT: ---" in text


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
