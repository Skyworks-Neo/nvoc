# Security Policy

## EN

NVOC controls GPU clocks, power limits, fan behavior, voltage-related settings, and service endpoints. Treat changes in this repository as hardware-affecting software.

### Reporting Issues

Please report security-sensitive issues privately to the maintainers before publishing details. Include:

- The affected component path.
- Operating system, driver version, GPU model, and whether NVAPI, NVML, CUDA, or OpenCL is involved.
- Reproduction steps and expected impact.
- Whether the issue requires administrator or sudo privileges.

### Scope

Security reports may include unsafe service behavior, privilege boundary mistakes, command injection, unsafe file handling, or GPU-state writes that can be triggered unexpectedly.

Normal overclocking instability, failed stress tests, and GPU crashes caused by intentionally applying aggressive settings should be filed as regular bugs unless they expose a separate security boundary issue.

## 中文

NVOC 会控制 GPU 时钟、电源限制、风扇行为、电压相关设置以及服务端点。请将本仓库的变更视为会影响硬件的软件变更。

### 报告安全问题

在公开细节之前，请私下向维护者报告安全相关问题。请包含：

- 受影响的组件路径。
- 操作系统、驱动版本、GPU 型号，以及是否涉及 NVAPI、NVML、CUDA 或 OpenCL。
- 复现步骤与预期影响。
- 是否需要管理员或 sudo 权限。

### 范围

安全报告可包含：不安全的服务行为、权限边界错误、命令注入、不安全的文件处理、以及可被意外触发的 GPU 状态写入。

由于故意应用激进设置导致的常规超频不稳定、压力测试失败和 GPU 崩溃，应作为常规缺陷提交，除非它们暴露了独立的安全边界问题。
