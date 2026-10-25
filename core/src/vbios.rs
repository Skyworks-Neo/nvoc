//! vBIOS ROM 解析族（read-only）：容器/链、各代际表解码、架构门。
//!
//! 分层：[`image_chain`]（PCIR/NPDS 镜像链 + BIT 目录 + GSP 逻辑视图指针）
//! → 各代际表解码（[`legacy`] Fermi~Pascal BIT 'P' 链与 VP 表、
//! [`tables_v3`] Turing/Ampere/Ada 3x 表、[`gb20x`] GB20x object directory
//! v3），共享原语在 [`vfe`]（电压方程组）与 [`families`]（PCI-ID 架构门）；
//! [`dcb`] 是独立的显示连接器表，[`dcb_ext`] 解析 DCB v3/v4 头指针子表族
//! （I2C/GPIO/EXTDEV/CONN/MUX，envytools nvbios 移植），[`power_p2`] 解析
//! BIT 'P' v2 的功率/传感/基准时钟子表，[`bitsubs`] 解析其余 BIT token 的
//! 指针级表（'M'/'I'/'C'/'d'/'2'/'T'/'U'/'V'）。全部 fail-closed：形状不
//! 认识就报错或降级，不猜。

pub mod bitsubs;
pub mod dcb;
pub mod dcb_ext;
pub mod families;
pub mod gb20x;
pub mod image_chain;
pub mod legacy;
pub mod power_p2;
pub mod tables_v3;
pub mod vfe;
