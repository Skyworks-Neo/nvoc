# Security Policy

NVOC controls GPU clocks, power limits, fan behavior, voltage-related settings, and service endpoints. Treat changes in this repository as hardware-affecting software.

## Reporting Issues

Please report security-sensitive issues privately to the maintainers before publishing details. Include:

- The affected component path.
- Operating system, driver version, GPU model, and whether NVAPI, NVML, CUDA, or OpenCL is involved.
- Reproduction steps and expected impact.
- Whether the issue requires administrator or sudo privileges.

## Scope

Security reports may include unsafe service behavior, privilege boundary mistakes, command injection, unsafe file handling, or GPU-state writes that can be triggered unexpectedly.

Normal overclocking instability, failed stress tests, and GPU crashes caused by intentionally applying aggressive settings should be filed as regular bugs unless they expose a separate security boundary issue.
