# Linux 侧 vbios runtime override 可行性评估

日期：2026-11-16 · 结论：**可行，且有已发布、已实测的参考实现**

## 1. 生态地图（本轮考察定案）

| 仓库 | 角色 | 平台 | 关键事实 |
|---|---|---|---|
| vix editor（reverse/vix-editor/） | **纯编辑器**（Python/GTK4，nvidia-vbios-editor 0.1.45） | Linux 第一发布目标 | **不含任何加载车道代码**（全仓无 SetupDi/winreg-vbios/romOverride；唯一 winreg = GUI 主题检测）。patch 输出为 loader payload 文件 |
| nvidia-linux-vbios-vix-loader（Panchovix，已克隆 reverse/nvidia-linux-vbios-vix-loader/） | **Linux 加载器** | Fedora 43 实测；RHEL/Debian/Arch 实验性 | patch open-gpu-kernel-modules 注入 GSP；per-boot 生效 |
| nvidia-windows-vbios-vix-loader（Panchovix） | Windows 加载器（= 我们已复刻的注册表 `vbios` REG_BINARY 车道） | Windows | Linux 无对应 devreg 车道 |
| LACT（reverse/LACT-master/） | 对照组 | Linux | NVIDIA 走 NVML + 私有 NVAPI（dlopen libnvidia-api.so.1，QI ID 表）+ RM ioctl（NVOS21/64/NVOS54，`/dev/nvidiactl`）；vbios 只有版本字符串（rom dump NVIDIA 未实现） |

vix 的 payload 表示法（编辑器→加载器的接口）：
`turing-e0-end-v1`（裁 type-0x70 NPDS 尾图 + last-image 标志 + checksum）、
`ada-relocated-v2`、`ampere-relocated-v1`（**GA10x 与 AD10x 共用 GSP 固件，
override 接收器上限 legacy image 起 512 KiB——丢弃 EFI 图**，源码注释
`rom/ampere_relocated.py:2-5`）。Turing 单 bank ~250-355 KB 天然适配。

## 2. Linux 加载机制全解（patches/615-vbios-override-per-gpu.patch，源码已核实）

GSP-RM 固件有**原生 vbios override 接收面**——patch 只是喂数：

- 改动面：`src/nvidia/src/kernel/gpu/gsp/kernel_gsp.c`
  （`kgspAllocBootArgsCommon` 末尾 `_kgspVbiosOverridePrepare`、销毁
  `_kgspVbiosOverrideFree`）、`registry.c`、`kernel-open/nvidia/nv.c`、
  `g_kernel_gsp_nvoc.h`（KernelGsp 增 `pVbiosOverrideDescriptor`/
  `vbiosOverrideSize`）；新增 `vbios_override_image.h`（纯解析器
  `kgspVbiosOverrideFindLegacy`：0x200 步进扫 0x55AA → PCIR 结构 →
  PCI ID 匹配 → image length + checksum==0 → **恰一 legacy image**，
  否则回退 stock）
- registry keys（payload C 头 `_payload.py:69-73` 实证）：
  `RmLinuxVbiosOverride`（mode，**必须 = 2**）、`RmLinuxVbiosOverrideSize`
  （0x200..**4 MiB**）、`RmLinuxVbiosOverrideImage`（经
  RegistryBinaryFilePerDevice 在 Linux probe 期把文件读进 registry
  binary——RM 锁内不 request_firmware）、`RmLinuxVbiosOverrideDisable`
- 替换点：`pGspArgs->vbiosOverrideArgs.pa/size`（sysmem 描述符物理地址）
  → GSP 固件 boot args 原生消费
- 生命周期：payload 路径 `/var/lib/nvidia-vbios/roms/gpuN.bin` + 配置
  `/etc/nvidia-vbios/lifecycle.json`（PCI bdf→文件）→ initramfs 打包 →
  专用启动项（原启动项保留 fallback）→ DNF/apt/pacman hook + systemd
  timer 在内核/驱动更新后自动重建
- 限制：GSP-only（专有内核模块不支持）；Secure Boot 关闭或 MOK 自签；
  R615+/620.70.06 open modules；Ada 笔记本镜像基本超限不可用；
  驱动大版本可能移除 override 接收器（每次驱动更新后 `nvidia-smi -q -d
  POWER` 复核）。作者实测列表含 **RTX 5060 Ti**、5090/5080/5070Ti/5070、
  RTX PRO 6000 Blackwell、RTX 2000 Ada；Turing 支持（无需 relocation）

## 3. 可行性判决

**可行**。三个层次的通道现状：

1. **镜像注入（per-boot）——现成**：上述 kernel patch 车道。Windows 三车道
   （vix 注册表整镜像 / mVolt romOverride 片段 / NVAPI 运行时 CONTROL）
   中前两条依赖 Windows devreg（Linux `NVreg_RegistryDwords` 只能传
   string dword，无二进制 blob 等价物），在 Linux 的对应物就是 GSP
   boot-args 注入 = 已被该 loader 实现。
2. **运行时设置面（mid-session）——部分现成**：LACT 证明 Linux 用户态可达
   NVML + 私有 NVAPI（libnvidia-api.so.1 的 QueryInterface）+ RM ioctl
   （NVOS54）。nvoc 已有 `get-rm-info`（/dev/nvidiactl NVOS54 汇总）。
   mVolt CONTROL 车道（0x8B3E7343/0xAFFC2279）背后的 RM 命令号可从
   nvapi64_impl / libnvidia-api 挖出后在 Linux 直发——**phase-2 研究方向**
   （OCP/TGP 运行时写不重刷、不重启）。
3. **内核内存 patch（Windows kmd lane 等价）——不推荐**：`/proc/kcore`
   扫描+patch 技术上可行（root 即可，无 kmod），但表在 RmInitAdapter 消费，
   patch 后需 re-init 才生效且 re-init 会重新从 HW 拷贝——时序不利，劣于
   boot-args 车道。

## 4. nvoc 集成路线

- **A（推荐起步）**：nvoc 的 `set-vbios-override`（power cap/OCP/电压墙
  双代 patch 生成器已实装）输出 .rom → vix `normalize --representation`
  （Turing 不需要 / Ada `ada-relocated-v2` / Ampere `ampere-relocated-v1`）
  → loader payload（`nvbios system prepare/install/enroll/reconcile`）。
  nvoc 侧新增文档 + 校验（payload SHA、bdf 对应）即可，不碰内核。
- **B（自研车道）**：以该 repo `patches/*.patch` 为底，把 vbios_override
  patch 移植进我们自己的 open-gpu-kernel-modules 构建流（xtask），省掉
  第三方生命周期工具的依赖。
- **C（phase-2，真 mid-session）**：CONTROL 车道 RM 命令号挖掘 →
  NVOS54 直发（跨代 runtime OCP/TGP 写，Windows/Linux 同源）。

## 5. 交付物

- 本评估文档
- `reverse/nvidia-linux-vbios-vix-loader/` 克隆（untracked，含 patches/
  原文与 `_payload.py` C 头，源码级参考）
- Windows 侧对照：mVolt OCP CONTROL 车道全解见
  [mvolt-ocp-control-lane.md](../nvapi/mvolt-ocp-control-lane.md)
