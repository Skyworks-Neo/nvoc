"""EXT-slot domain-current overlay extraction (Turing/Ampere packing).

Each 488B private record carries optional EXT slots (+0x74+0x10*k pairs,
gated by the record's +0x2C/+0x40 markers) packing the curve domains
WITHOUT their own main record block, ascending:

* Turing (only GPC as main records): 4 slots = XBAR/SYS/MSD/HOST.
* Ampere (XBAR promoted to its own #127..253 block): the gpc block is
  base-only; the XBAR block fills 3 slots = SYS/MSD/HOST.
* Ada (MSD promoted too): the XBAR block fills 2 slots = SYS/HOST
  (live A/B: the 35-distinct 225..1335 slot is HOST, not MSD).

The overlay is display-only — never selectable, never a write target.
"""

from __future__ import annotations

import matplotlib

matplotlib.use("Agg")  # headless: no Tk display required

from src.tabs.vfcurve.tab import VFCurveTab, _extract_ext_curves  # noqa: E402


def _seg(domain: str, start: int, end: int, bank: int = 0) -> dict:
    return {
        "kind": "vf_curve",
        "domain": domain,
        "bank": bank,
        "start_index": start,
        "end_index": end,
    }


def _pt(i: int, slots: list[tuple[float, float]], bank: int = 0) -> dict:
    """One record with per-slot (freq MHz, volt µV) pairs in slots 0..3."""
    fs = [s[0] for s in slots] + [0] * (4 - len(slots))
    vs = [s[1] for s in slots] + [0] * (4 - len(slots))
    return {
        "bank": bank,
        "index": i,
        "voltage_uV": 450000,
        "freq_current_mhz": 405,
        "freq_default_mhz": 405,
        "domain_freq_mhz": fs,
        "domain_volt_uV": vs,
    }


def test_turing_gpc_block_four_slots_attributed() -> None:
    pts = [
        _pt(
            i,
            [
                (100 + i, 450000),  # ext0 XBAR
                (300 + i, 600000),  # ext1 SYS
                (500 + i, 800000),  # ext2 MSD
                (700 + i, 1000000),  # ext3 HOST
            ],
        )
        for i in range(12)
    ]
    out = _extract_ext_curves({"segments": [_seg("gpc", 0, 11)], "points": pts})

    assert [(e["owner"], e["slot"], e["label"]) for e in out] == [
        ("gpc", 0, "XBAR"),
        ("gpc", 1, "SYS"),
        ("gpc", 2, "MSD"),
        ("gpc", 3, "HOST"),
    ]
    xbar = out[0]
    assert xbar["volts"][0] == 450.0 and xbar["freqs"][0] == 100.0
    assert xbar["volts"][-1] == 450.0 and xbar["freqs"][-1] == 111.0


def test_ampere_xbar_block_three_slots_gpc_base_only() -> None:
    # Ampere: gpc records #0..126 are base-only (all-zero slot arrays);
    # the xbar block #127..253 fills SYS/MSD/HOST — no XBAR slot.
    gpc_pts = [_pt(i, []) for i in range(12)]
    xbar_pts = [
        _pt(
            127 + i,
            [
                (2000 + i, 600000),  # ext0 SYS
                (1500 + i, 800000),  # ext1 MSD
                (900 + i, 1000000),  # ext2 HOST
            ],
        )
        for i in range(12)
    ]
    out = _extract_ext_curves({
        "segments": [_seg("gpc", 0, 126), _seg("xbar", 127, 253)],
        "points": gpc_pts + xbar_pts,
    })

    assert [(e["owner"], e["slot"], e["label"]) for e in out] == [
        ("xbar", 0, "SYS"),
        ("xbar", 1, "MSD"),
        ("xbar", 2, "HOST"),
    ]


def test_base_only_records_yield_no_overlays() -> None:
    # Pascal (580 driver, 488B table): no markers, no slots at all.
    pts = [_pt(i, []) for i in range(12)]
    assert _extract_ext_curves({"segments": [_seg("gpc", 0, 11)], "points": pts}) == []


def test_unit_garbage_never_plots() -> None:
    # µV-as-raw or wrong-unit slot data must fail the plausibility gate
    # rather than draw a nonsense curve.
    pts = [_pt(i, [(100 + i, 450000000)]) for i in range(12)]  # 450000 mV?!
    assert _extract_ext_curves({"segments": [_seg("gpc", 0, 11)], "points": pts}) == []


def test_fewer_than_4_points_dropped() -> None:
    pts = [_pt(i, [(100 + i, 450000)]) for i in range(3)]
    assert _extract_ext_curves({"segments": [_seg("gpc", 0, 2)], "points": pts}) == []


def test_gcoff_zeroed_pairs_skipped_not_fatal() -> None:
    # Zeroed (f=0) pairs are skipped individually; the rest still plots.
    pts = []
    for i in range(12):
        slots = [(0.0, 450000)] if i % 2 == 0 else [(100 + i, 450000 + i * 1000)]
        pts.append(_pt(i, slots))
    out = _extract_ext_curves({"segments": [_seg("gpc", 0, 11)], "points": pts})
    assert len(out) == 1 and out[0]["label"] == "XBAR"
    assert len(out[0]["freqs"]) == 6


def test_roster_excludes_main_block_domains_not_owner() -> None:
    # Attribution is layout-derived: the roster is the pool minus every
    # domain with a main segment — independent of which block hosts the
    # slots. An msd-main layout packs [XBAR, SYS, HOST] into its ext
    # slots, so an msd block's ext0 is XBAR by the same rule.
    pts = [_pt(254 + i, [(100 + i, 450000)]) for i in range(12)]
    out = _extract_ext_curves({"segments": [_seg("msd", 254, 265)], "points": pts})
    assert [(e["owner"], e["slot"], e["label"]) for e in out] == [("msd", 0, "XBAR")]


def test_slots_beyond_dynamic_roster_dropped() -> None:
    # Ada-style table (gpc+xbar+msd mains): only SYS/HOST remain in the
    # ext pool, so a third populated slot has no attribution — it stays
    # in the CLI's domain_currents instead of being guessed at.
    pts = [
        _pt(127 + i, [(2000 + i, 600000), (900 + i, 1000000), (500 + i, 800000)])
        for i in range(12)
    ]
    out = _extract_ext_curves({
        "segments": [
            _seg("gpc", 0, 126),
            _seg("xbar", 127, 253),
            _seg("msd", 254, 265),
        ],
        "points": [_pt(i, []) for i in range(12)] + pts,
    })
    assert [(e["owner"], e["slot"], e["label"]) for e in out] == [
        ("xbar", 0, "SYS"),
        ("xbar", 1, "HOST"),
    ]


def test_build_curves_populates_and_guards_overlays() -> None:
    tab = VFCurveTab.__new__(VFCurveTab)
    tab._curves = {}
    tab._curve_visible = {}
    tab._active_curve = "gpc"

    pts = [_pt(i, [(100 + i, 450000), (300 + i, 600000)]) for i in range(12)]
    clk_data = {"segments": [_seg("gpc", 0, 11)], "points": pts}

    assert tab._build_curves("GPU0", None, None, False, clk_data) is True
    assert [(e["slot"], e["label"]) for e in tab._domain_curves] == [
        (0, "XBAR"),
        (1, "SYS"),
    ]
    # Visibility map follows the discovered labels (default visible).
    assert tab._domain_curve_visible == {"XBAR": True, "SYS": True}


def test_build_curves_ada_roster_excludes_main_block_domains() -> None:
    # Ada: MSD has its own main segment, so the xbar block's ext slots
    # are only SYS/HOST — live A/B: the 35-distinct 225..1335 slot is
    # HOST, not MSD. The earlier "hide HOST" toggle carries over.
    tab = VFCurveTab.__new__(VFCurveTab)
    tab._curves = {}
    tab._curve_visible = {}
    tab._active_curve = "gpc"
    tab._domain_curve_visible = {"HOST": False}  # previously toggled off

    gpc_pts = [_pt(i, []) for i in range(12)]
    xbar_pts = [
        _pt(127 + i, [(2000 + i, 600000), (900 + i, 1000000)]) for i in range(12)
    ]
    clk_data = {
        "segments": [
            _seg("gpc", 0, 126),
            _seg("xbar", 127, 253),
            _seg("msd", 254, 265),
        ],
        "points": gpc_pts + xbar_pts + [_pt(254 + i, []) for i in range(12)],
    }

    assert tab._build_curves("GPU0", None, None, False, clk_data) is True
    assert [e["label"] for e in tab._domain_curves] == ["SYS", "HOST"]
    assert tab._domain_curve_visible == {"SYS": True, "HOST": False}


def test_build_curves_clears_overlays_when_no_curve() -> None:
    tab = VFCurveTab.__new__(VFCurveTab)
    tab._curves = {}
    tab._curve_visible = {}
    tab._active_curve = "gpc"
    tab._domain_curves = [{"owner": "gpc", "slot": 0, "label": "XBAR"}]

    # No segments at all → no curves AND no overlays.
    assert tab._build_curves("GPU0", None, None, False, None) is False
    assert tab._domain_curves == []
