from __future__ import annotations

import json

from src.tabs.vfcurve.sections.stressor_args import (
    build_stressor_args,
    discover_stressor_exe,
    flag_for,
    format_exit_status,
    parse_help_flags,
    split_extra_args,
    summarize_line,
)

# Help excerpts shaped like real clap output. "main" is the main-lineage
# build on disk today; "enhanced" carries the renamed Vulkan family and the
# verify/SDC family from stressor-enhanced-refactor.
MAIN_HELP = """\
Options:
      --config <CONFIG>
          Optional TOML config file.
      --duration <DURATION>
          [default: 90]
      --enable-vulkan-stress
          Enable the Vulkan graphics stressor thread (optional)
      --vulkan-only
          Run Vulkan-only stress (skip CUDA workload)
      --vulkan-image-width <VULKAN_IMAGE_WIDTH>
          Vulkan image width for 3D render stress [default: 8192]
      --vulkan-image-msaa <VULKAN_IMAGE_MSAA>
          [default: 1]
      --gpu-index <INDEX>
          CUDA GPU index in PCI-bus-sorted order (0-based, default: 0)
"""
ENHANCED_HELP = """\
Options:
      --duration <DURATION>
          [default: 90]
      --vulkan, --vulkan-heavy
          Primary Vulkan render load
      --vulkan-width <VULKAN_WIDTH>
          Render target width [default: 8192]
      --vulkan-msaa <VULKAN_MSAA>
          [default: 1]
      --vulkan-heavy-offscreen
          Offscreen render mode
      --vulkan-rotate <VULKAN_ROTATE>
          Animate the torus rotation (disable for the static-mesh A/B
          baseline) [default: true]
      --vulkan-particles <VULKAN_PARTICLES>
          Compute->graphics particle pool size (0 = off) [default: 262144]
      --verify-continue-on-error
          Keep running after a detector fault, accumulating SDC statistics
      --verify-resident-interval <VERIFY_RESIDENT_INTERVAL>
          Seconds between resident-block pattern checks (0 = off) [default: 2]
      --no-verify
          Disable the verification sidecar
      --skip-self-test
          Skip the startup self test
      --json-out <JSON_OUT>
          Write the machine-readable verdict JSON to this path
      --cuda-path <DIR>
          Directory holding the CUDA runtime libraries, loaded before any
          CUDA call
      --vulkan-image-width <VULKAN_IMAGE_WIDTH>
          Legacy Vulkan image width (alias: --legacy-vulkan-image-width)
"""


def test_parse_help_flags_extracts_long_flags() -> None:
    flags = parse_help_flags(MAIN_HELP)

    assert "--duration" in flags
    assert "--enable-vulkan-stress" in flags
    assert "--gpu-index" in flags
    # A bare "--" separator must not become a flag entry.
    assert "--" not in flags


def test_parse_help_flags_strips_alias_star_suffix() -> None:
    flags = parse_help_flags("aliases: --vulkan-heavy-* keep old names")

    assert "--vulkan-heavy" in flags


def test_build_args_main_lineage_prefers_legacy_vulkan_names() -> None:
    caps = parse_help_flags(MAIN_HELP)
    state = {
        "duration": "30",
        "vulkan": True,
        "vulkan_width": "4096",
        "vulkan_msaa": "4",
        "verify_continue_on_error": True,
    }

    args = build_stressor_args(state, caps)

    assert args == [
        "--duration",
        "30",
        "--enable-vulkan-stress",
        "--vulkan-image-width",
        "4096",
        "--vulkan-image-msaa",
        "4",
    ]


def test_build_args_enhanced_lineage_prefers_new_names_and_verify() -> None:
    caps = parse_help_flags(ENHANCED_HELP)
    state = {
        "duration": "30",
        "vulkan": True,
        "vulkan_width": "4096",
        "vulkan_offscreen": True,
        "verify_continue_on_error": True,
        "verify_resident_interval": "1.5",
        "json_out": r"C:\ws\verdict.json",
    }

    args = build_stressor_args(state, caps)

    assert "--vulkan" in args
    assert "--vulkan-width" in args
    assert "--vulkan-heavy-offscreen" in args
    assert "--verify-continue-on-error" in args
    assert ["--verify-resident-interval", "1.5"] == args[
        args.index("--verify-resident-interval") :
    ][:2]
    assert ["--json-out", r"C:\ws\verdict.json"] == args[args.index("--json-out") :][:2]
    # The old spellings must not leak in alongside the new ones.
    assert "--enable-vulkan-stress" not in args
    assert "--vulkan-image-width" not in args


def test_build_args_forwards_value_taking_checkbox_and_particle_pool() -> None:
    caps = parse_help_flags(ENHANCED_HELP)

    rot_on = build_stressor_args({"vulkan": True, "vulkan_rotate": True}, caps)
    rot_off = build_stressor_args({"vulkan": True, "vulkan_rotate": False}, caps)
    particles = build_stressor_args(
        {"vulkan": True, "vulkan_particles": "262144"}, caps
    )
    blank = build_stressor_args({"vulkan": True, "vulkan_particles": ""}, caps)

    assert ["--vulkan-rotate", "true"] == rot_on[rot_on.index("--vulkan-rotate") :][:2]
    assert ["--vulkan-rotate", "false"] == rot_off[rot_off.index("--vulkan-rotate") :][
        :2
    ]
    assert ["--vulkan-particles", "262144"] == particles[
        particles.index("--vulkan-particles") :
    ][:2]
    assert "--vulkan-particles" not in blank


def test_value_taking_vulkan_options_are_gated_off_the_main_lineage() -> None:
    caps = parse_help_flags(MAIN_HELP)

    assert flag_for("vulkan_rotate", caps) is None
    assert flag_for("vulkan_particles", caps) is None


def test_cuda_path_is_forwarded_when_supported() -> None:
    caps = parse_help_flags(ENHANCED_HELP)

    args = build_stressor_args({"cuda_path": r"D:\cuda\runtime"}, caps)

    assert ["--cuda-path", r"D:\cuda\runtime"] == args[args.index("--cuda-path") :][:2]


def test_cuda_path_is_gated_off_older_builds() -> None:
    caps = parse_help_flags(MAIN_HELP)

    assert flag_for("cuda_path", caps) is None
    assert build_stressor_args({"cuda_path": r"D:\cuda\runtime"}, caps) == []


def test_build_args_unknown_capabilities_do_not_gate() -> None:
    state = {"verify_continue_on_error": True, "vulkan": True}

    args = build_stressor_args(state, None)

    assert args == ["--verify-continue-on-error", "--vulkan"]


def test_build_args_omits_empty_values_and_false_booleans() -> None:
    state = {
        "duration": "",
        "no_verify": False,
        "gpu_generate": False,
        "profile": "",
    }

    assert build_stressor_args(state, None) == []


def test_build_args_appends_extra_args_last() -> None:
    state = {"duration": "10", "extra_args": "--seed 7 --kernel-mixture gemm:1.0"}

    args = build_stressor_args(state, None)

    assert args == ["--duration", "10", "--seed", "7", "--kernel-mixture", "gemm:1.0"]


def test_flag_for_maps_keys_to_build_spelling() -> None:
    caps = parse_help_flags(MAIN_HELP)

    assert flag_for("vulkan", caps) == "--enable-vulkan-stress"
    assert flag_for("vulkan_width", caps) == "--vulkan-image-width"
    # Verify family missing on the main lineage build.
    assert flag_for("no_verify", caps) is None
    assert flag_for("vulkan_iters", caps) is None


def test_split_extra_args_keeps_backslash_paths_and_quotes() -> None:
    text = r'--json-out "C:\ws dir\verdict.json" --seed 7'

    assert split_extra_args(text) == [
        "--json-out",
        r"C:\ws dir\verdict.json",
        "--seed",
        "7",
    ]


def test_split_extra_args_empty() -> None:
    assert split_extra_args("") == []
    assert split_extra_args("   ") == []


def test_discover_prefers_existing_configured_path(tmp_path) -> None:
    exe = tmp_path / "cli-stressor-cuda-rs.exe"
    exe.write_text("")

    assert discover_stressor_exe(str(exe), "") == str(exe)


def test_discover_falls_back_to_optimizer_directory(tmp_path) -> None:
    opt_dir = tmp_path / "release"
    opt_dir.mkdir()
    optimizer = opt_dir / "nvoc-auto-optimizer.exe"
    optimizer.write_text("")
    stressor = opt_dir / "cli-stressor-cuda-rs.exe"
    stressor.write_text("")

    assert discover_stressor_exe("", str(optimizer)) == str(stressor)


def test_discover_uses_path_last(tmp_path, monkeypatch) -> None:
    exe = tmp_path / "cli-stressor-cuda-rs"
    exe.write_text("")

    def fake_which(name: str):
        return str(exe) if name == "cli-stressor-cuda-rs" else None

    monkeypatch.setattr(
        "src.tabs.vfcurve.sections.stressor_args.shutil.which", fake_which
    )

    assert discover_stressor_exe("", "") == str(exe)


def test_discover_returns_empty_when_nothing_found(monkeypatch, tmp_path) -> None:
    monkeypatch.setattr(
        "src.tabs.vfcurve.sections.stressor_args.shutil.which", lambda _name: None
    )

    assert discover_stressor_exe(str(tmp_path / "missing.exe"), "") == ""


def test_summarize_line_parses_verdict_payload() -> None:
    payload = {
        "stressor": "cli-stressor-cuda-rs",
        "result": "fail",
        "classification": "sdc",
        "device": "NVIDIA GeForce RTX 4060 Laptop GPU",
        "confidence_pct": 87,
    }
    line = f"(ANSI) VERDICT_JSON: {json.dumps(payload)}\n"

    summary = summarize_line(line)

    assert summary is not None
    assert "verdict: fail" in summary
    assert "classification=sdc" in summary
    assert "confidence=87%" in summary
    assert "device=NVIDIA GeForce RTX 4060 Laptop GPU" in summary


def test_summarize_line_ignores_regular_lines() -> None:
    assert summarize_line("Validation loop 3 OK (0 errors)") is None


def test_summarize_line_handles_malformed_payload() -> None:
    assert summarize_line("VERDICT_JSON: {not json") == "verdict: {not json"


def test_format_exit_status() -> None:
    assert format_exit_status(0) == "finished (exit 0)"
    assert format_exit_status(-1) == "cancelled"
    assert format_exit_status(1) == "failed (exit 1)"
    assert format_exit_status(2) == "error (exit 2)"
