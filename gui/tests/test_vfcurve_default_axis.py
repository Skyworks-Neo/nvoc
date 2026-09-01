"""Tests for the VF-curve default-axis resolution in ``VFCurveTab._build_curves``.

Server-card regression (V100/GV100, driver 538.78): the private vftable GPC
segment always reflects DEFAULT values regardless of any gpc slot1 voltage
offset, while the public vftable read is only trustworthy when its voltage
grid still matches the default grid:

* unshifted grid          → hybrid (public CURRENT freqs, private defaults)
* shifted grid (slot1<0)  → private only (public freqs discarded)
* broken read (slot1>0)   → private only (public fill path bails to zeros;
                            guarded even if the voltage grid happens to match)

The private GPC segment is the default-axis authority whenever its voltage
axis is populated; a Pascal-style all-zero axis keeps the public source.
"""

from __future__ import annotations

import matplotlib

matplotlib.use("Agg")  # headless: no Tk display required

from src.tabs.vfcurve.tab import VFCurveTab  # noqa: E402


def _make_tab() -> VFCurveTab:
    tab = VFCurveTab.__new__(VFCurveTab)
    tab._curves = {}
    tab._curve_visible = {}
    tab._active_curve = "gpc"
    return tab


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


def test_hybrid_public_currents_with_private_defaults() -> None:
    tab = _make_tab()
    clk_data = _private_gpc_clk_data(
        [800000, 825000], [1000.0, 1100.0], [1000.0, 1100.0]
    )
    gpc_points = [
        {
            "index": 0,
            "voltage_uv": 800000,
            "frequency_khz": 1050000,
            "default_frequency_khz": 999999,  # public default must be IGNORED
            "point_type": "prog",
        },
        {
            "index": 1,
            "voltage_uv": 825000,
            "frequency_khz": 1150000,
            "point_type": "prog",
        },
    ]

    assert tab._build_curves("GPU0", gpc_points, None, False, clk_data) is True

    gpc = tab._curves["gpc"]
    assert gpc.source == "hybrid"
    assert gpc.frequencies == [1050.0, 1150.0]
    assert gpc.defaults == [1000.0, 1100.0]
    assert gpc.has_fixed is False


def test_shifted_public_grid_falls_back_to_private() -> None:
    tab = _make_tab()
    clk_data = _private_gpc_clk_data(
        [800000, 825000], [1000.0, 1100.0], [1000.0, 1100.0]
    )
    gpc_points = [
        {
            "index": 0,
            "voltage_uv": 780000,  # grid shifted by a negative slot1 offset
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

    assert tab._build_curves("GPU0", gpc_points, None, False, clk_data) is True

    gpc = tab._curves["gpc"]
    assert gpc.source == "private"
    assert gpc.frequencies == [1000.0, 1100.0]
    assert gpc.defaults == [1000.0, 1100.0]
    assert gpc.has_fixed is True


def test_broken_public_frequencies_rejected_even_on_matching_grid() -> None:
    tab = _make_tab()
    clk_data = _private_gpc_clk_data(
        [800000, 825000], [1000.0, 1100.0], [1000.0, 1100.0]
    )
    # Positive slot1 breakage: voltage grid survived but the fill path
    # zeroed the data words — must not become an all-zero hybrid curve.
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

    assert tab._build_curves("GPU0", gpc_points, None, False, clk_data) is True

    gpc = tab._curves["gpc"]
    assert gpc.source == "private"
    assert gpc.frequencies == [1000.0, 1100.0]
    assert gpc.defaults == [1000.0, 1100.0]


def test_all_zero_private_voltage_axis_keeps_public_source() -> None:
    # Pascal server cards: the private GPC voltage axis is all-zero — the
    # public read stays the GPC source untouched.
    tab = _make_tab()
    clk_data = _private_gpc_clk_data(
        [0, 0], [0.0, 0.0], [0.0, 0.0]
    )
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

    assert tab._build_curves("GPU0", gpc_points, None, False, clk_data) is True

    gpc = tab._curves["gpc"]
    assert gpc.source == "public"
    assert gpc.frequencies == [1050.0, 1150.0]
    assert gpc.defaults == [1000.0, 1100.0]


# ── Tier 1: effective-series synthesis (broken-positive-slot1 fallback) ──


def _domain_info(entries):
    return {"entries": entries}


_GPC_OFFSETS = _domain_info(
    [{"bit": 0, "value_modifiable": True, "values_kHz": [0, 100000]}]
)


def test_effective_synthesized_only_on_broken_public_read() -> None:
    # Positive-slot1 broken state: the public fill path bailed (all-zero
    # frequency column) — the ONLY state that triggers the synthesis
    # fallback. The effective series is the private base right-shifted by
    # the gpc slot1 readback; the base series itself stays unshifted.
    tab = _make_tab()
    clk_data = _private_gpc_clk_data(
        [800000, 825000], [1000.0, 1100.0], [1000.0, 1100.0]
    )
    broken_public = [
        {"index": 0, "voltage_uv": 800000, "frequency_khz": 0, "point_type": "prog"},
        {"index": 1, "voltage_uv": 825000, "frequency_khz": 0, "point_type": "prog"},
    ]

    assert (
        tab._build_curves("GPU0", broken_public, None, False, clk_data, _GPC_OFFSETS)
        is True
    )

    eff = tab._curves["gpc"].effective
    assert eff is not None
    assert eff.applicable is True
    assert eff.voltages == [900.0, 925.0]
    assert eff.freqs == [1000.0, 1100.0]
    # Display-only: never written back into the base axes.
    assert tab._curves["gpc"].voltages == [800.0, 825.0]
    assert tab._curves["gpc"].frequencies == [1000.0, 1100.0]


def test_effective_not_synthesized_on_healthy_or_shifted_public() -> None:
    # Healthy public (hybrid) and shifted-but-valid public (negative slot1)
    # must NOT synthesize even with offsets present — the fallback is
    # gated on DETECTED breakage, not on offset != 0.
    tab = _make_tab()
    clk_data = _private_gpc_clk_data(
        [800000, 825000], [1000.0, 1100.0], [1000.0, 1100.0]
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
            "voltage_uv": 780000,  # grid shifted, data intact
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

    assert (
        tab._build_curves("GPU0", healthy_public, None, False, clk_data, _GPC_OFFSETS)
        is True
    )
    assert tab._curves["gpc"].source == "hybrid"
    assert tab._curves["gpc"].effective is None

    assert (
        tab._build_curves("GPU0", shifted_public, None, False, clk_data, _GPC_OFFSETS)
        is True
    )
    assert tab._curves["gpc"].source == "private"
    assert tab._curves["gpc"].effective is None


def test_effective_not_synthesized_when_public_absent() -> None:
    # Public family absent ("not supported") is NOT breakage — no synthesis
    # even with offsets present.
    tab = _make_tab()
    clk_data = _private_gpc_clk_data(
        [800000, 825000], [1000.0, 1100.0], [1000.0, 1100.0]
    )

    assert (
        tab._build_curves("GPU0", None, "not supported", True, clk_data, _GPC_OFFSETS)
        is True
    )
    assert tab._curves["gpc"].source == "private"
    assert tab._curves["gpc"].effective is None


def test_normalize_domain_offsets_write_bit_not_measure_bit() -> None:
    # The MEASURE bits (msd 21 / mem 4) must never map — only the WRITE bits
    # (msd 5 / mem 2); unmodifiable entries are dropped entirely.
    from src.tabs.vfcurve.tab import _normalize_domain_offsets

    raw = _domain_info(
        [
            {"bit": 0, "value_modifiable": True, "values_kHz": [25000, 100000]},
            {"bit": 5, "value_modifiable": True, "values_kHz": [0, 50000]},
            {"bit": 21, "value_modifiable": True, "values_kHz": [0, 999999]},
            {"bit": 1, "value_modifiable": False, "values_kHz": [0, 777777]},
        ]
    )

    assert _normalize_domain_offsets(raw) == {
        "gpc": {"slot0_khz": 25000, "slot1_uv": 100000},
        "msd": {"slot0_khz": 0, "slot1_uv": 50000},
    }
    assert _normalize_domain_offsets(None) == {}
    assert _normalize_domain_offsets({"entries": []}) == {}
