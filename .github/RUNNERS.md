# CI Runner Setup

NVOC CI runs in two tiers:

1. **Hosted CI** (`.github/workflows/ci.yml`) — GitHub-hosted `windows-latest`
   and `ubuntu-latest`. Builds, lints, unit tests, no GPU required. Runs on
   every PR and push.
2. **GPU CI** (`.github/workflows/gpu-ci.yml`) — self-hosted runners with real
   NVIDIA GPUs. Runs only when a PR carries the `ci:gpu` label, or when a
   maintainer dispatches it manually. Required before merging anything that
   changes NVAPI / NVML / stressor code paths.

## Self-hosted runner inventory

We need three runners (or one host with three runner registrations under
different labels — the GPU is shared but jobs are serialized per host via
GitHub's concurrency).

| Hostname (suggested) | OS | Required labels | Purpose |
|---|---|---|---|
| `nvoc-gpu-win` | Windows 11 / Server 2022 | `self-hosted, windows, gpu-nvidia, cuda, opencl` | NVAPI, srv, CUDA + OpenCL stressors |
| `nvoc-gpu-linux` | Ubuntu 24.04 LTS | `self-hosted, linux, gpu-nvidia, opencl` | NVML + OpenCL stressor on Linux |
| (optional) `nvoc-gpu-bench` | Windows 11 | `self-hosted, windows, gpu-nvidia, write-allowed` | **Reserved** for human-supervised write/overclock validation. Not auto-triggered. |

### Hardware

- NVIDIA GPU within the support matrix in `auto-optimizer/README-en.md`
  (RTX 50/40/30/20, GTX 16/10/9, Volta, mobile). One generation per runner is
  fine; rotate hardware for broader coverage.
- ≥ 16 GB RAM, ≥ 100 GB SSD (CUDA toolkit + PyTorch wheels are large).
- Wired network. Avoid sleep / hibernation in power settings.
- TDR (Timeout Detection and Recovery) configured per
  `auto-optimizer/GpuTdrRecovery.reg` on Windows. Without it, a hang during
  read-only probing can wedge the runner until reboot.
- Out-of-band reset path (IPMI, Wake-on-LAN + scheduled reboot, smart PDU,
  or a watchdog service). The whole point of this fleet is to recover from
  driver crashes without a human.

### Windows runner setup

1. Install latest stable NVIDIA Studio or Game Ready driver. Reboot.
2. Install build tooling:
   - Visual Studio 2022 Build Tools with the **Desktop development with C++**
     workload (MSVC, Windows 10/11 SDK).
   - `rustup` → stable toolchain.
   - `uv` (https://docs.astral.sh/uv/).
   - Git for Windows.
   - CUDA Toolkit matching the PyTorch wheel pinned in
     `cli-stressor-cuda/pyproject.toml` (currently `cu129`).
   - OpenCL ICD loader (ships with the NVIDIA driver).
3. Apply `auto-optimizer/GpuTdrRecovery.reg` and reboot.
4. Create a low-privilege local user `gha-runner`. Grant it
   `SeServiceLogonRight` if you intend to test `srv/` as a service.
5. Install the GitHub Actions runner under that user (Repo → Settings →
   Actions → Runners → New self-hosted runner). Register with labels:
   `self-hosted,windows,gpu-nvidia,cuda,opencl`.
6. Install the runner as a Windows service so it survives reboots.
7. Set `ACTIONS_RUNNER_HOOK_JOB_STARTED` / `_JOB_COMPLETED` to a script that
   resets clocks and fan curves to default (`nvoc-auto-optimizer --reset`)
   between jobs. This is the safety net if a job leaves the GPU in a bad
   state.

### Linux runner setup

1. Install the proprietary NVIDIA driver (`ubuntu-drivers autoinstall` or the
   `.run` installer). Reboot. Verify with `nvidia-smi`.
2. Install OpenCL ICD: `sudo apt install -y nvidia-opencl-icd-* ocl-icd-opencl-dev clinfo`.
   Verify with `clinfo | grep "Device Name"`.
3. Install `uv`, `git`, `build-essential`, `pkg-config`.
4. Create user `gha-runner`. Add to `video` and `render` groups.
5. Register the runner with labels:
   `self-hosted,linux,gpu-nvidia,opencl`.
6. Install as a systemd service (`./svc.sh install gha-runner`).
7. (Optional) `systemd` watchdog: a unit that runs `nvidia-smi` every minute
   and reboots if it hangs.

### Job-isolation hooks (both OSes)

Place these in the runner's `.env` or as runner hooks:

```
ACTIONS_RUNNER_HOOK_JOB_STARTED=/opt/runner-hooks/pre.sh
ACTIONS_RUNNER_HOOK_JOB_COMPLETED=/opt/runner-hooks/post.sh
```

`pre.sh` should:
- log `nvidia-smi` baseline,
- ensure no leftover stressor processes,
- reset clocks if a previous job aborted.

`post.sh` should:
- kill any child stressor processes,
- reset clocks/fans to default,
- collect `nvidia-smi -q` and dmesg / Event Viewer extracts as artifacts.

### Security

- Self-hosted runners on **public** repos are dangerous: any forked PR can run
  arbitrary code on the runner. NVOC's repo is currently private; if it goes
  public, switch the GPU CI trigger to require a maintainer to add the
  `ci:gpu` label (already enforced in `gpu-ci.yml`) **and** restrict workflow
  approval to maintainers in *Settings → Actions → General → Fork pull
  request workflows*.
- Do not store production credentials on these hosts. The runner needs
  nothing beyond GitHub's registration token.
- Network-isolate the bench: it should not be able to reach internal infra.

## Branch protection

Configure on `main` (Settings → Branches → Rules):

- Require pull request reviews.
- Require status checks: `ci summary` (the `ci-summary` job in `ci.yml`).
- Optional but recommended: require `auto-optimizer-gpu` from `gpu-ci.yml`
  for PRs that touch `auto-optimizer/**`, `srv/**`, or `cli-stressor-*/**`
  (use a CODEOWNERS-driven label or a small bot to apply `ci:gpu`).

## Operational runbook

- **Runner offline**: check the service (`Get-Service actions.runner.*` /
  `systemctl status actions.runner.*`). Restart, then check `nvidia-smi`.
  If the GPU is unresponsive, hit the OOB reset.
- **Wedged after a CI run**: post-job hook should have caught it. If not,
  reboot and add the failing test to the `--skip` list while you debug.
- **CUDA / driver upgrade**: drain the runner (Settings → Runners →
  pause), upgrade, reboot, run `cli-stressor-cuda` manually, un-pause.
- **Adding a new GPU generation**: register a new runner with an extra label
  (e.g. `gpu-rtx50`) and add a job in `gpu-ci.yml` that targets that label.
