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
//! - [`pmxdrv`](cfg(windows)):**主传输层** —— Intel PMxDrv(`pmxdrv.sys`,
//!   xOCD 内嵌同款,Intel CSE/ME 工具链出品、签名有效)的 map/unmap 直连,
//!   接口全量逆向见模块文档;本机实机复现走查用它;
//! - [`pawnio`](cfg(windows)):PawnIO 设备直连的传输层(实验存档)。2026-10-05
//!   实裁决:PawnIO 官方签名模块集(23 枚)**没有任意物理内存读模块**——
//!   调用层(官方 Echo/Nvidia 模块)零签名工作且本机已跑通,但 nvlddmkm
//!   走查所需的原语只有自研模块能提供,而自研模块等上游签名收编=不可控
//!   等待,经用户裁决放弃;PhysMem.p 源码与 blob 保留在 `core/kmd/`,
//!   驱动 `physical_read_*` 原语在位(上游就绪),若未来 PawnIO.Modules
//!   收编即可无缝换回。
//!
//! 风险口径:pmxdrv 的映射窗口是 PAGE_READWRITE(传输层天然可写),本车道
//! 只从窗口拷出字节;pmxdrv 不加探测直接解引用用户指针,窗口内访问不可访问
//! 物理页的行为与 xOCD 同源,探针只触碰 RAM 范围(低内存/页表帧)。

pub mod pagewalk;

#[cfg(windows)]
pub mod pmxdrv;

#[cfg(windows)]
pub mod pawnio;
