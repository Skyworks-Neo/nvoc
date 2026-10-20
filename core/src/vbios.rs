//! vBIOS ROM 解析族（read-only）：容器/链、各代际表解码、架构门。
//!
//! 分层：[`image_chain`]（PCIR/NPDS 镜像链 + BIT 目录 + GSP 逻辑视图指针）
//! → 各代际表解码（[`legacy`] Fermi~Pascal BIT 'P' 链与 VP 表、
//! [`tables_v3`] Turing/Ampere/Ada 3x 表、[`gb20x`] GB20x object directory
//! v3），共享原语在 [`vfe`]（电压方程组）与 [`families`]（PCI-ID 架构门）；
//! [`dcb`] 是独立的显示连接器表。全部 fail-closed：形状不认识就报错或
//! 降级，不猜。

pub mod dcb;
pub mod families;
pub mod gb20x;
pub mod image_chain;
pub mod legacy;
pub mod tables_v3;
pub mod vfe;
