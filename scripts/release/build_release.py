#!/usr/bin/env python3
"""Stage NVOC release artifacts for one OS/architecture matrix cell."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import struct
import subprocess
import sys
import tarfile
from pathlib import Path


PE_MACHINES = {
    "amd64": 0x8664,
    "arm64": 0xAA64,
}

ELF_MACHINES = {
    "amd64": 0x3E,
    "arm64": 0xB7,
}


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as fh:
        for chunk in iter(lambda: fh.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def run_text(cmd: list[str], cwd: Path) -> str:
    try:
        return subprocess.check_output(
            cmd, cwd=cwd, text=True, stderr=subprocess.DEVNULL
        ).strip()
    except (OSError, subprocess.CalledProcessError):
        return "unknown"


def copy_file(src: Path, dst: Path) -> None:
    if not src.is_file():
        raise FileNotFoundError(f"release input does not exist: {src}")
    dst.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(src, dst)
    if os.name != "nt":
        dst.chmod(dst.stat().st_mode | 0o755)


def verify_executable_arch(path: Path, platform: str, arch: str) -> None:
    with path.open("rb") as fh:
        header = fh.read(0x1000)

    if platform == "linux":
        if not header.startswith(b"\x7fELF"):
            raise ValueError(f"{path} is not an ELF executable")
        if header[4] != 2:
            raise ValueError(f"{path} is not an ELF64 executable")
        machine = struct.unpack_from("<H", header, 18)[0]
        expected = ELF_MACHINES[arch]
        if machine != expected:
            raise ValueError(
                f"{path} has ELF machine 0x{machine:x}, expected 0x{expected:x}"
            )
        return

    if platform == "windows":
        if not header.startswith(b"MZ"):
            raise ValueError(f"{path} is not a PE executable")
        pe_offset = struct.unpack_from("<I", header, 0x3C)[0]
        if pe_offset + 6 > len(header):
            raise ValueError(f"{path} PE header is outside the inspected range")
        if header[pe_offset : pe_offset + 4] != b"PE\0\0":
            raise ValueError(f"{path} is missing the PE signature")
        machine = struct.unpack_from("<H", header, pe_offset + 4)[0]
        expected = PE_MACHINES[arch]
        if machine != expected:
            raise ValueError(
                f"{path} has PE machine 0x{machine:x}, expected 0x{expected:x}"
            )
        return

    raise ValueError(f"unsupported platform: {platform}")


def add_if_exists(entries: list[tuple[Path, str]], src: Path, arcname: str) -> None:
    if src.exists():
        entries.append((src, arcname))


def collect_auxiliary_files(root: Path, platform: str) -> list[tuple[Path, str]]:
    entries: list[tuple[Path, str]] = []
    common_files = [
        "LICENSE",
        "NOTICE",
        "README.md",
        "cli/README.md",
        "auto-optimizer/README.md",
        "auto-optimizer/README-en.md",
        "cli-stressor-cuda-rs/README.md",
    ]
    for rel in common_files:
        add_if_exists(entries, root / rel, rel)

    test_dir = root / "auto-optimizer" / "test"
    if test_dir.is_dir():
        for path in sorted(test_dir.rglob("*")):
            if path.is_file():
                rel = path.relative_to(root).as_posix()
                add_if_exists(entries, path, rel)

    if platform == "linux":
        systemd_dir = root / "auto-optimizer" / "systemd"
        if systemd_dir.is_dir():
            for path in sorted(systemd_dir.rglob("*")):
                if path.is_file():
                    rel = path.relative_to(root).as_posix()
                    add_if_exists(entries, path, rel)

    if platform == "windows":
        add_if_exists(
            entries,
            root / "auto-optimizer" / "GpuTdrRecovery.reg",
            "auto-optimizer/GpuTdrRecovery.reg",
        )

    return entries


def make_tarball(path: Path, entries: list[tuple[Path, str]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with tarfile.open(path, "w:gz", format=tarfile.PAX_FORMAT) as tar:
        for src, arcname in entries:
            tar.add(src, arcname=arcname, recursive=src.is_dir())


def write_sha256s(paths: list[Path], out: Path) -> None:
    lines = [
        f"{sha256(path)}  {path.name}" for path in sorted(paths, key=lambda p: p.name)
    ]
    out.write_text("\n".join(lines) + "\n", encoding="utf-8")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--platform", choices=["linux", "windows"], required=True)
    parser.add_argument("--arch", choices=["amd64", "arm64"], required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--workspace", type=Path, default=Path.cwd())
    parser.add_argument("--out-dir", type=Path, default=Path("dist/release"))
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    root = args.workspace.resolve()
    out_root = (root / args.out_dir).resolve()
    cell = f"{args.platform}-{args.arch}"
    cell_dir = out_root / cell
    target_release = root / "target" / "release"
    suffix = ".exe" if args.platform == "windows" else ""

    shutil.rmtree(cell_dir, ignore_errors=True)
    cell_dir.mkdir(parents=True, exist_ok=True)

    inputs = {
        "nvoc-cli": target_release / f"nvoc-cli{suffix}",
        "nvoc-auto-optimizer": target_release / f"nvoc-auto-optimizer{suffix}",
        "cli-stressor-cuda-rs": target_release / f"cli-stressor-cuda-rs{suffix}",
        "nvoc-tui": root / "tui" / "dist" / f"nvoc-tui{suffix}",
        "nvoc-gui": root / "gui" / "dist" / f"NVOC-GUI{suffix}",
    }

    single_outputs: list[Path] = []
    for name in ("nvoc-cli", "nvoc-tui", "nvoc-gui"):
        dst = cell_dir / f"{name}-{args.version}-{cell}{suffix}"
        copy_file(inputs[name], dst)
        verify_executable_arch(dst, args.platform, args.arch)
        single_outputs.append(dst)

    tools_root = cell_dir / f"nvoc-tools-{args.version}-{cell}"
    bin_dir = tools_root / "bin"
    tools_outputs = {
        "nvoc-auto-optimizer": bin_dir / f"nvoc-auto-optimizer{suffix}",
        "cli-stressor-cuda-rs": bin_dir / f"cli-stressor-cuda-rs{suffix}",
    }
    for name, dst in tools_outputs.items():
        copy_file(inputs[name], dst)
        verify_executable_arch(dst, args.platform, args.arch)

    metadata = {
        "version": args.version,
        "platform": args.platform,
        "arch": args.arch,
        "target": cell,
        "git_sha": os.environ.get("GITHUB_SHA")
        or run_text(["git", "rev-parse", "HEAD"], root),
        "rustc": run_text(["rustc", "--version"], root),
        "cargo": run_text(["cargo", "--version"], root),
        "python": sys.version.split()[0],
        "feature_flags": {
            "cli-stressor-cuda-rs": ["cuda", "vulkan"],
        },
        "linux_compatibility_baseline": (
            "Debian 12 / Ubuntu 22.04 and newer" if args.platform == "linux" else None
        ),
        "runtime_dependencies": [
            "NVIDIA driver libraries",
            "CUDA runtime libraries for cli-stressor-cuda-rs",
        ],
        "files": {},
    }

    staged_files = [path for path in tools_root.rglob("*") if path.is_file()]
    for path in sorted(staged_files):
        metadata["files"][path.relative_to(tools_root).as_posix()] = {
            "sha256": sha256(path),
            "size": path.stat().st_size,
        }

    manifest_path = tools_root / "manifest.json"
    manifest_path.write_text(
        json.dumps(metadata, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )

    tarball = cell_dir / f"nvoc-tools-{args.version}-{cell}.tar.gz"
    tar_entries: list[tuple[Path, str]] = [(tools_root, tools_root.name)]
    for src, arcname in collect_auxiliary_files(root, args.platform):
        tar_entries.append((src, f"{tools_root.name}/{arcname}"))
    make_tarball(tarball, tar_entries)

    all_outputs = single_outputs + [tarball]
    write_sha256s(all_outputs, cell_dir / f"SHA256SUMS-{cell}.txt")
    print(
        json.dumps(
            {"cell": cell, "artifacts": [str(path) for path in all_outputs]}, indent=2
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
