//! 内核态驱动读写通道(kmd):经已签名内核驱动做物理内存原语,再在其上
//! 复刻 xOCD 2.0.0 `NvidiaKernelReader` 的 nvlddmkm 地址空间走查。
//!
//! 定位(与用户约定):内核态读写**不进** nvapi-rs 子模块——nvapi-rs 只管
//! NVAPI/NVML 用户态接口面,kmd 通道是 nvoc-core 的实验扩展,单独成目录,
//! 当前只读(写路径留给后续能力位实验)。
//!
//! 分层:
//! - [`pagewalk`]:平台中立的算法层(模块定位、低内存扫描、页表翻译、活体
//!   映像校验),物理读经 [`pagewalk::PhysicalMemory`] 抽象,单测用内存模拟
//!   页表,Linux CI 可跑;
//! - [`layout_probe`]:静态跨代布局推导(纯镜像字节 → RM 电源对象链偏移,
//!   fail-closed;三代校准:576.02/610.74/616.92);
//! - [`pmxdrv`](cfg(windows)):**主传输层** —— Intel PMxDrv(`pmxdrv.sys`,
//!   xOCD 内嵌同款,Intel CSE/ME 工具链出品、签名有效)的 map/unmap 直连,
//!   接口全量逆向见模块文档;本机实机复现走查用它;
//! - [`power`](cfg(windows)):功率墙原子流(set-power-command --kmd)。
//!
//! 历史裁决(2026-10-05/06,代码已清退,结论保留):PawnIO 白手套调用层零
//! 签名工作已证(官方 Echo 模块往返通),但官方 23 枚签名模块**没有任意
//! 物理内存读模块**,自研模块等上游收编签名=不可控等待,经用户裁决放弃;
//! 传输层与模块工件(pawnio.rs/PhysMem.*/Echo.bin)已删除,完整结论与
//! 实测记录在 `core/kmd/README.md` 与
//! `docs/reverse-engineering/nvapi/xocd-oc-tool-audit.md`。
//!
//! 风险口径:pmxdrv 的映射窗口是 PAGE_READWRITE(传输层天然可写)。写路径
//! 于 2026-10-06 受控开放,仅限 [`power`] 的功率墙原子流(身份门+读回+回滚+
//! finally 服务注销,绝对上限 500 W);其余通道仍只读。pmxdrv 不加探测直接
//! 解引用用户指针,窗口内访问不可访问物理页的行为与 xOCD 同源,探针只触碰
//! RAM 范围(低内存/页表帧/定位目标帧)。

pub mod layout_probe;
pub mod pagewalk;

/// 功率墙原子流(set-power-command --kmd 的执行核心;写路径受控开放)。
#[cfg(windows)]
pub mod power;

#[cfg(windows)]
pub mod pmxdrv;
