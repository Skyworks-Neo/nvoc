//! PCI-ID 架构门：NVIDIA open-gpu-kernel-modules 615.71.09 产品表的
//! 显式设备清单（mechanical port of vix-editor `nvidia_vbios/families.py`，
//! 条目按设备 ID 排序）。清单只是**架构闸门**，不代表任何表/编辑能力：
//! 未知 ID fail closed；BIOS 版本前缀（90./92./94./95./98.）作相干性旁证。
//! GA100 使用不同 GSP 固件、接收容量未审计（vix loader 明确排除）。

pub const SOURCE: &str =
    "https://raw.githubusercontent.com/NVIDIA/open-gpu-kernel-modules/615.71.09/README.md";

/// 架构族。`Excluded` = Hopper/数据中心 Blackwell 等明确排除项。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Turing,
    Ampere,
    Ada,
    BlackwellGb20x,
    Excluded,
}

impl Family {
    /// BIOS 版本前缀相干性旁证（vix families.py `expected` 映射）。
    pub fn bios_version_prefix(self) -> Option<&'static str> {
        match self {
            Family::Turing => Some("90."),
            Family::Ampere => Some("94."),
            Family::Ada => Some("95."),
            Family::BlackwellGb20x => Some("98."),
            Family::Excluded => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Family::Turing => "turing",
            Family::Ampere => "ampere",
            Family::Ada => "ada",
            Family::BlackwellGb20x => "blackwell-gb20x",
            Family::Excluded => "excluded",
        }
    }

    /// loader/装载口径的 in-scope 族（vix SCOPE；Excluded 恒 false）。
    pub fn in_scope(self) -> bool {
        !matches!(self, Family::Excluded)
    }
}

/// 显式 PCI 设备清单（按 ID 升序）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceEntry {
    pub id: u16,
    pub family: Family,
    pub product: &'static str,
}

pub const DEVICES: &[DeviceEntry] = &[
    DeviceEntry {
        id: 0x1E02,
        family: Family::Turing,
        product: "NVIDIA TITAN RTX",
    },
    DeviceEntry {
        id: 0x1E04,
        family: Family::Turing,
        product: "NVIDIA GeForce RTX 2080 Ti",
    },
    DeviceEntry {
        id: 0x1E07,
        family: Family::Turing,
        product: "NVIDIA GeForce RTX 2080 Ti",
    },
    DeviceEntry {
        id: 0x1E09,
        family: Family::Turing,
        product: "NVIDIA CMP 50HX",
    },
    DeviceEntry {
        id: 0x1E30,
        family: Family::Turing,
        product: "Quadro RTX 6000",
    },
    DeviceEntry {
        id: 0x1E36,
        family: Family::Turing,
        product: "Quadro RTX 6000",
    },
    DeviceEntry {
        id: 0x1E78,
        family: Family::Turing,
        product: "Quadro RTX 8000",
    },
    DeviceEntry {
        id: 0x1E81,
        family: Family::Turing,
        product: "NVIDIA GeForce RTX 2080 SUPER",
    },
    DeviceEntry {
        id: 0x1E82,
        family: Family::Turing,
        product: "NVIDIA GeForce RTX 2080",
    },
    DeviceEntry {
        id: 0x1E84,
        family: Family::Turing,
        product: "NVIDIA GeForce RTX 2070 SUPER",
    },
    DeviceEntry {
        id: 0x1E87,
        family: Family::Turing,
        product: "NVIDIA GeForce RTX 2080",
    },
    DeviceEntry {
        id: 0x1E89,
        family: Family::Turing,
        product: "NVIDIA GeForce RTX 2060",
    },
    DeviceEntry {
        id: 0x1E90,
        family: Family::Turing,
        product: "NVIDIA GeForce RTX 2080",
    },
    DeviceEntry {
        id: 0x1E91,
        family: Family::Turing,
        product: "NVIDIA GeForce RTX 2070 Super",
    },
    DeviceEntry {
        id: 0x1E93,
        family: Family::Turing,
        product: "NVIDIA GeForce RTX 2080 Super",
    },
    DeviceEntry {
        id: 0x1EB0,
        family: Family::Turing,
        product: "Quadro RTX 5000",
    },
    DeviceEntry {
        id: 0x1EB1,
        family: Family::Turing,
        product: "Quadro RTX 4000",
    },
    DeviceEntry {
        id: 0x1EB5,
        family: Family::Turing,
        product: "Quadro RTX 5000",
    },
    DeviceEntry {
        id: 0x1EB6,
        family: Family::Turing,
        product: "Quadro RTX 4000",
    },
    DeviceEntry {
        id: 0x1EB8,
        family: Family::Turing,
        product: "Tesla T4",
    },
    DeviceEntry {
        id: 0x1EC2,
        family: Family::Turing,
        product: "NVIDIA GeForce RTX 2070 SUPER",
    },
    DeviceEntry {
        id: 0x1EC7,
        family: Family::Turing,
        product: "NVIDIA GeForce RTX 2070 SUPER",
    },
    DeviceEntry {
        id: 0x1ED0,
        family: Family::Turing,
        product: "NVIDIA GeForce RTX 2080",
    },
    DeviceEntry {
        id: 0x1ED1,
        family: Family::Turing,
        product: "NVIDIA GeForce RTX 2070 Super",
    },
    DeviceEntry {
        id: 0x1ED3,
        family: Family::Turing,
        product: "NVIDIA GeForce RTX 2080 Super",
    },
    DeviceEntry {
        id: 0x1EF5,
        family: Family::Turing,
        product: "Quadro RTX 5000",
    },
    DeviceEntry {
        id: 0x1F02,
        family: Family::Turing,
        product: "NVIDIA GeForce RTX 2070",
    },
    DeviceEntry {
        id: 0x1F03,
        family: Family::Turing,
        product: "NVIDIA GeForce RTX 2060",
    },
    DeviceEntry {
        id: 0x1F06,
        family: Family::Turing,
        product: "NVIDIA GeForce RTX 2060 SUPER",
    },
    DeviceEntry {
        id: 0x1F07,
        family: Family::Turing,
        product: "NVIDIA GeForce RTX 2070",
    },
    DeviceEntry {
        id: 0x1F08,
        family: Family::Turing,
        product: "NVIDIA GeForce RTX 2060",
    },
    DeviceEntry {
        id: 0x1F0A,
        family: Family::Turing,
        product: "NVIDIA GeForce GTX 1650",
    },
    DeviceEntry {
        id: 0x1F0B,
        family: Family::Turing,
        product: "NVIDIA CMP 40HX",
    },
    DeviceEntry {
        id: 0x1F10,
        family: Family::Turing,
        product: "NVIDIA GeForce RTX 2070",
    },
    DeviceEntry {
        id: 0x1F11,
        family: Family::Turing,
        product: "NVIDIA GeForce RTX 2060",
    },
    DeviceEntry {
        id: 0x1F12,
        family: Family::Turing,
        product: "NVIDIA GeForce RTX 2060",
    },
    DeviceEntry {
        id: 0x1F14,
        family: Family::Turing,
        product: "NVIDIA GeForce RTX 2070",
    },
    DeviceEntry {
        id: 0x1F15,
        family: Family::Turing,
        product: "NVIDIA GeForce RTX 2060",
    },
    DeviceEntry {
        id: 0x1F36,
        family: Family::Turing,
        product: "Quadro RTX 3000",
    },
    DeviceEntry {
        id: 0x1F42,
        family: Family::Turing,
        product: "NVIDIA GeForce RTX 2060 SUPER",
    },
    DeviceEntry {
        id: 0x1F47,
        family: Family::Turing,
        product: "NVIDIA GeForce RTX 2060 SUPER",
    },
    DeviceEntry {
        id: 0x1F50,
        family: Family::Turing,
        product: "NVIDIA GeForce RTX 2070",
    },
    DeviceEntry {
        id: 0x1F51,
        family: Family::Turing,
        product: "NVIDIA GeForce RTX 2060",
    },
    DeviceEntry {
        id: 0x1F54,
        family: Family::Turing,
        product: "NVIDIA GeForce RTX 2070",
    },
    DeviceEntry {
        id: 0x1F55,
        family: Family::Turing,
        product: "NVIDIA GeForce RTX 2060",
    },
    DeviceEntry {
        id: 0x1F76,
        family: Family::Turing,
        product: "Quadro RTX 3000",
    },
    DeviceEntry {
        id: 0x1F82,
        family: Family::Turing,
        product: "NVIDIA GeForce GTX 1650",
    },
    DeviceEntry {
        id: 0x1F83,
        family: Family::Turing,
        product: "NVIDIA GeForce GTX 1630",
    },
    DeviceEntry {
        id: 0x1F91,
        family: Family::Turing,
        product: "NVIDIA GeForce GTX 1650",
    },
    DeviceEntry {
        id: 0x1F95,
        family: Family::Turing,
        product: "NVIDIA GeForce GTX 1650 Ti",
    },
    DeviceEntry {
        id: 0x1F96,
        family: Family::Turing,
        product: "NVIDIA GeForce GTX 1650",
    },
    DeviceEntry {
        id: 0x1F97,
        family: Family::Turing,
        product: "NVIDIA GeForce MX450",
    },
    DeviceEntry {
        id: 0x1F98,
        family: Family::Turing,
        product: "NVIDIA GeForce MX450",
    },
    DeviceEntry {
        id: 0x1F99,
        family: Family::Turing,
        product: "NVIDIA GeForce GTX 1650",
    },
    DeviceEntry {
        id: 0x1F9C,
        family: Family::Turing,
        product: "NVIDIA GeForce MX450",
    },
    DeviceEntry {
        id: 0x1F9D,
        family: Family::Turing,
        product: "NVIDIA GeForce GTX 1650",
    },
    DeviceEntry {
        id: 0x1F9F,
        family: Family::Turing,
        product: "NVIDIA GeForce MX550",
    },
    DeviceEntry {
        id: 0x1FA0,
        family: Family::Turing,
        product: "NVIDIA GeForce MX550",
    },
    DeviceEntry {
        id: 0x1FB0,
        family: Family::Turing,
        product: "NVIDIA T1000",
    },
    DeviceEntry {
        id: 0x1FB1,
        family: Family::Turing,
        product: "NVIDIA T600",
    },
    DeviceEntry {
        id: 0x1FB2,
        family: Family::Turing,
        product: "NVIDIA T400",
    },
    DeviceEntry {
        id: 0x1FB6,
        family: Family::Turing,
        product: "NVIDIA T600 Laptop GPU",
    },
    DeviceEntry {
        id: 0x1FB7,
        family: Family::Turing,
        product: "NVIDIA T550 Laptop GPU",
    },
    DeviceEntry {
        id: 0x1FB8,
        family: Family::Turing,
        product: "Quadro T2000",
    },
    DeviceEntry {
        id: 0x1FB9,
        family: Family::Turing,
        product: "Quadro T1000",
    },
    DeviceEntry {
        id: 0x1FBA,
        family: Family::Turing,
        product: "NVIDIA T600 Laptop GPU",
    },
    DeviceEntry {
        id: 0x1FBB,
        family: Family::Turing,
        product: "NVIDIA T500",
    },
    DeviceEntry {
        id: 0x1FBC,
        family: Family::Turing,
        product: "NVIDIA T1200 Laptop GPU",
    },
    DeviceEntry {
        id: 0x1FDD,
        family: Family::Turing,
        product: "NVIDIA GeForce GTX 1650",
    },
    DeviceEntry {
        id: 0x1FF0,
        family: Family::Turing,
        product: "NVIDIA T1000 8GB",
    },
    DeviceEntry {
        id: 0x1FF2,
        family: Family::Turing,
        product: "NVIDIA T400 4GB",
    },
    DeviceEntry {
        id: 0x1FF9,
        family: Family::Turing,
        product: "Quadro T1000",
    },
    DeviceEntry {
        id: 0x20B0,
        family: Family::Ampere,
        product: "NVIDIA A100-SXM4-40GB",
    },
    DeviceEntry {
        id: 0x20B2,
        family: Family::Ampere,
        product: "NVIDIA A100-SXM4-80GB",
    },
    DeviceEntry {
        id: 0x20B3,
        family: Family::Ampere,
        product: "NVIDIA A100-SXM-64GB",
    },
    DeviceEntry {
        id: 0x20B5,
        family: Family::Ampere,
        product: "NVIDIA A100 80GB PCIe",
    },
    DeviceEntry {
        id: 0x20B7,
        family: Family::Ampere,
        product: "NVIDIA A30",
    },
    DeviceEntry {
        id: 0x20BD,
        family: Family::Ampere,
        product: "NVIDIA A800-SXM4-40GB",
    },
    DeviceEntry {
        id: 0x20F1,
        family: Family::Ampere,
        product: "NVIDIA A100-PCIE-40GB",
    },
    DeviceEntry {
        id: 0x20F3,
        family: Family::Ampere,
        product: "NVIDIA A800-SXM4-80GB",
    },
    DeviceEntry {
        id: 0x20F5,
        family: Family::Ampere,
        product: "NVIDIA A800 80GB PCIe",
    },
    DeviceEntry {
        id: 0x20F6,
        family: Family::Ampere,
        product: "NVIDIA A800 40GB Active",
    },
    DeviceEntry {
        id: 0x2182,
        family: Family::Turing,
        product: "NVIDIA GeForce GTX 1660 Ti",
    },
    DeviceEntry {
        id: 0x2184,
        family: Family::Turing,
        product: "NVIDIA GeForce GTX 1660",
    },
    DeviceEntry {
        id: 0x2187,
        family: Family::Turing,
        product: "NVIDIA GeForce GTX 1650 SUPER",
    },
    DeviceEntry {
        id: 0x2188,
        family: Family::Turing,
        product: "NVIDIA GeForce GTX 1650",
    },
    DeviceEntry {
        id: 0x2189,
        family: Family::Turing,
        product: "NVIDIA CMP 30HX",
    },
    DeviceEntry {
        id: 0x2191,
        family: Family::Turing,
        product: "NVIDIA GeForce GTX 1660 Ti",
    },
    DeviceEntry {
        id: 0x2192,
        family: Family::Turing,
        product: "NVIDIA GeForce GTX 1650 Ti",
    },
    DeviceEntry {
        id: 0x21C4,
        family: Family::Turing,
        product: "NVIDIA GeForce GTX 1660 SUPER",
    },
    DeviceEntry {
        id: 0x21D1,
        family: Family::Turing,
        product: "NVIDIA GeForce GTX 1660 Ti",
    },
    DeviceEntry {
        id: 0x2203,
        family: Family::Ampere,
        product: "NVIDIA GeForce RTX 3090 Ti",
    },
    DeviceEntry {
        id: 0x2204,
        family: Family::Ampere,
        product: "NVIDIA GeForce RTX 3090",
    },
    DeviceEntry {
        id: 0x2206,
        family: Family::Ampere,
        product: "NVIDIA GeForce RTX 3080",
    },
    DeviceEntry {
        id: 0x2207,
        family: Family::Ampere,
        product: "NVIDIA GeForce RTX 3070 Ti",
    },
    DeviceEntry {
        id: 0x2208,
        family: Family::Ampere,
        product: "NVIDIA GeForce RTX 3080 Ti",
    },
    DeviceEntry {
        id: 0x220A,
        family: Family::Ampere,
        product: "NVIDIA GeForce RTX 3080",
    },
    DeviceEntry {
        id: 0x220D,
        family: Family::Ampere,
        product: "NVIDIA CMP 90HX",
    },
    DeviceEntry {
        id: 0x2216,
        family: Family::Ampere,
        product: "NVIDIA GeForce RTX 3080",
    },
    DeviceEntry {
        id: 0x2230,
        family: Family::Ampere,
        product: "NVIDIA RTX A6000",
    },
    DeviceEntry {
        id: 0x2231,
        family: Family::Ampere,
        product: "NVIDIA RTX A5000",
    },
    DeviceEntry {
        id: 0x2232,
        family: Family::Ampere,
        product: "NVIDIA RTX A4500",
    },
    DeviceEntry {
        id: 0x2233,
        family: Family::Ampere,
        product: "NVIDIA RTX A5500",
    },
    DeviceEntry {
        id: 0x2235,
        family: Family::Ampere,
        product: "NVIDIA A40",
    },
    DeviceEntry {
        id: 0x2236,
        family: Family::Ampere,
        product: "NVIDIA A10",
    },
    DeviceEntry {
        id: 0x2237,
        family: Family::Ampere,
        product: "NVIDIA A10G",
    },
    DeviceEntry {
        id: 0x2238,
        family: Family::Ampere,
        product: "NVIDIA A10M",
    },
    DeviceEntry {
        id: 0x230E,
        family: Family::Excluded,
        product: "NVIDIA H20 NVL16",
    },
    DeviceEntry {
        id: 0x2321,
        family: Family::Excluded,
        product: "NVIDIA H100 NVL",
    },
    DeviceEntry {
        id: 0x2322,
        family: Family::Excluded,
        product: "NVIDIA H800 PCIe",
    },
    DeviceEntry {
        id: 0x2324,
        family: Family::Excluded,
        product: "NVIDIA H800",
    },
    DeviceEntry {
        id: 0x2329,
        family: Family::Excluded,
        product: "NVIDIA H20",
    },
    DeviceEntry {
        id: 0x232C,
        family: Family::Excluded,
        product: "NVIDIA H20-3e",
    },
    DeviceEntry {
        id: 0x2330,
        family: Family::Excluded,
        product: "NVIDIA H100 80GB HBM3",
    },
    DeviceEntry {
        id: 0x2331,
        family: Family::Excluded,
        product: "NVIDIA H100 PCIe",
    },
    DeviceEntry {
        id: 0x2335,
        family: Family::Excluded,
        product: "NVIDIA H200",
    },
    DeviceEntry {
        id: 0x2339,
        family: Family::Excluded,
        product: "NVIDIA H100",
    },
    DeviceEntry {
        id: 0x233A,
        family: Family::Excluded,
        product: "NVIDIA H800 NVL",
    },
    DeviceEntry {
        id: 0x233B,
        family: Family::Excluded,
        product: "NVIDIA H200 NVL",
    },
    DeviceEntry {
        id: 0x2342,
        family: Family::Excluded,
        product: "NVIDIA GH200 120GB",
    },
    DeviceEntry {
        id: 0x2348,
        family: Family::Excluded,
        product: "NVIDIA GH200 144G HBM3e",
    },
    DeviceEntry {
        id: 0x2414,
        family: Family::Ampere,
        product: "NVIDIA GeForce RTX 3060 Ti",
    },
    DeviceEntry {
        id: 0x2420,
        family: Family::Ampere,
        product: "NVIDIA GeForce RTX 3080 Ti Laptop GPU",
    },
    DeviceEntry {
        id: 0x2438,
        family: Family::Ampere,
        product: "NVIDIA RTX A5500 Laptop GPU",
    },
    DeviceEntry {
        id: 0x2460,
        family: Family::Ampere,
        product: "NVIDIA GeForce RTX 3080 Ti Laptop GPU",
    },
    DeviceEntry {
        id: 0x2482,
        family: Family::Ampere,
        product: "NVIDIA GeForce RTX 3070 Ti",
    },
    DeviceEntry {
        id: 0x2484,
        family: Family::Ampere,
        product: "NVIDIA GeForce RTX 3070",
    },
    DeviceEntry {
        id: 0x2486,
        family: Family::Ampere,
        product: "NVIDIA GeForce RTX 3060 Ti",
    },
    DeviceEntry {
        id: 0x2487,
        family: Family::Ampere,
        product: "NVIDIA GeForce RTX 3060",
    },
    DeviceEntry {
        id: 0x2488,
        family: Family::Ampere,
        product: "NVIDIA GeForce RTX 3070",
    },
    DeviceEntry {
        id: 0x2489,
        family: Family::Ampere,
        product: "NVIDIA GeForce RTX 3060 Ti",
    },
    DeviceEntry {
        id: 0x248A,
        family: Family::Ampere,
        product: "NVIDIA CMP 70HX",
    },
    DeviceEntry {
        id: 0x249C,
        family: Family::Ampere,
        product: "NVIDIA GeForce RTX 3080 Laptop GPU",
    },
    DeviceEntry {
        id: 0x249D,
        family: Family::Ampere,
        product: "NVIDIA GeForce RTX 3070 Laptop GPU",
    },
    DeviceEntry {
        id: 0x24A0,
        family: Family::Ampere,
        product: "NVIDIA GeForce RTX 3070 Ti Laptop GPU",
    },
    DeviceEntry {
        id: 0x24B0,
        family: Family::Ampere,
        product: "NVIDIA RTX A4000",
    },
    DeviceEntry {
        id: 0x24B1,
        family: Family::Ampere,
        product: "NVIDIA RTX A4000H",
    },
    DeviceEntry {
        id: 0x24B6,
        family: Family::Ampere,
        product: "NVIDIA RTX A5000 Laptop GPU",
    },
    DeviceEntry {
        id: 0x24B7,
        family: Family::Ampere,
        product: "NVIDIA RTX A4000 Laptop GPU",
    },
    DeviceEntry {
        id: 0x24B8,
        family: Family::Ampere,
        product: "NVIDIA RTX A3000 Laptop GPU",
    },
    DeviceEntry {
        id: 0x24B9,
        family: Family::Ampere,
        product: "NVIDIA RTX A3000 12GB Laptop GPU",
    },
    DeviceEntry {
        id: 0x24BA,
        family: Family::Ampere,
        product: "NVIDIA RTX A4500 Laptop GPU",
    },
    DeviceEntry {
        id: 0x24BB,
        family: Family::Ampere,
        product: "NVIDIA RTX A3000 12GB Laptop GPU",
    },
    DeviceEntry {
        id: 0x24C7,
        family: Family::Ampere,
        product: "NVIDIA GeForce RTX 3060",
    },
    DeviceEntry {
        id: 0x24C9,
        family: Family::Ampere,
        product: "NVIDIA GeForce RTX 3060 Ti",
    },
    DeviceEntry {
        id: 0x24DC,
        family: Family::Ampere,
        product: "NVIDIA GeForce RTX 3080 Laptop GPU",
    },
    DeviceEntry {
        id: 0x24DD,
        family: Family::Ampere,
        product: "NVIDIA GeForce RTX 3070 Laptop GPU",
    },
    DeviceEntry {
        id: 0x24E0,
        family: Family::Ampere,
        product: "NVIDIA GeForce RTX 3070 Ti Laptop GPU",
    },
    DeviceEntry {
        id: 0x24FA,
        family: Family::Ampere,
        product: "NVIDIA RTX A4500 Embedded GPU",
    },
    DeviceEntry {
        id: 0x2501,
        family: Family::Ampere,
        product: "NVIDIA GeForce RTX 3060",
    },
    DeviceEntry {
        id: 0x2503,
        family: Family::Ampere,
        product: "NVIDIA GeForce RTX 3060",
    },
    DeviceEntry {
        id: 0x2504,
        family: Family::Ampere,
        product: "NVIDIA GeForce RTX 3060",
    },
    DeviceEntry {
        id: 0x2507,
        family: Family::Ampere,
        product: "NVIDIA GeForce RTX 3050",
    },
    DeviceEntry {
        id: 0x2508,
        family: Family::Ampere,
        product: "NVIDIA GeForce RTX 3050 OEM",
    },
    DeviceEntry {
        id: 0x2520,
        family: Family::Ampere,
        product: "NVIDIA GeForce RTX 3060 Laptop GPU",
    },
    DeviceEntry {
        id: 0x2521,
        family: Family::Ampere,
        product: "NVIDIA GeForce RTX 3060 Laptop GPU",
    },
    DeviceEntry {
        id: 0x2523,
        family: Family::Ampere,
        product: "NVIDIA GeForce RTX 3050 Ti Laptop GPU",
    },
    DeviceEntry {
        id: 0x2531,
        family: Family::Ampere,
        product: "NVIDIA RTX A2000",
    },
    DeviceEntry {
        id: 0x2544,
        family: Family::Ampere,
        product: "NVIDIA GeForce RTX 3060",
    },
    DeviceEntry {
        id: 0x2560,
        family: Family::Ampere,
        product: "NVIDIA GeForce RTX 3060 Laptop GPU",
    },
    DeviceEntry {
        id: 0x2563,
        family: Family::Ampere,
        product: "NVIDIA GeForce RTX 3050 Ti Laptop GPU",
    },
    DeviceEntry {
        id: 0x2571,
        family: Family::Ampere,
        product: "NVIDIA RTX A2000 12GB",
    },
    DeviceEntry {
        id: 0x2582,
        family: Family::Ampere,
        product: "NVIDIA GeForce RTX 3050",
    },
    DeviceEntry {
        id: 0x2584,
        family: Family::Ampere,
        product: "NVIDIA GeForce RTX 3050",
    },
    DeviceEntry {
        id: 0x25A0,
        family: Family::Ampere,
        product: "NVIDIA GeForce RTX 3050 Ti Laptop GPU",
    },
    DeviceEntry {
        id: 0x25A2,
        family: Family::Ampere,
        product: "NVIDIA GeForce RTX 3050 Laptop GPU",
    },
    DeviceEntry {
        id: 0x25A5,
        family: Family::Ampere,
        product: "NVIDIA GeForce RTX 3050 Laptop GPU",
    },
    DeviceEntry {
        id: 0x25A6,
        family: Family::Ampere,
        product: "NVIDIA GeForce MX570",
    },
    DeviceEntry {
        id: 0x25A7,
        family: Family::Ampere,
        product: "NVIDIA GeForce RTX 2050",
    },
    DeviceEntry {
        id: 0x25A9,
        family: Family::Ampere,
        product: "NVIDIA GeForce RTX 2050",
    },
    DeviceEntry {
        id: 0x25AA,
        family: Family::Ampere,
        product: "NVIDIA GeForce MX570 A",
    },
    DeviceEntry {
        id: 0x25AB,
        family: Family::Ampere,
        product: "NVIDIA GeForce RTX 3050 4GB Laptop GPU",
    },
    DeviceEntry {
        id: 0x25AC,
        family: Family::Ampere,
        product: "NVIDIA GeForce RTX 3050 6GB Laptop GPU",
    },
    DeviceEntry {
        id: 0x25AD,
        family: Family::Ampere,
        product: "NVIDIA GeForce RTX 2050",
    },
    DeviceEntry {
        id: 0x25B0,
        family: Family::Ampere,
        product: "NVIDIA RTX A1000",
    },
    DeviceEntry {
        id: 0x25B2,
        family: Family::Ampere,
        product: "NVIDIA RTX A400",
    },
    DeviceEntry {
        id: 0x25B6,
        family: Family::Ampere,
        product: "NVIDIA A16",
    },
    DeviceEntry {
        id: 0x25B8,
        family: Family::Ampere,
        product: "NVIDIA RTX A2000 Laptop GPU",
    },
    DeviceEntry {
        id: 0x25B9,
        family: Family::Ampere,
        product: "NVIDIA RTX A1000 Laptop GPU",
    },
    DeviceEntry {
        id: 0x25BA,
        family: Family::Ampere,
        product: "NVIDIA RTX A2000 8GB Laptop GPU",
    },
    DeviceEntry {
        id: 0x25BB,
        family: Family::Ampere,
        product: "NVIDIA RTX A500 Laptop GPU",
    },
    DeviceEntry {
        id: 0x25BC,
        family: Family::Ampere,
        product: "NVIDIA RTX A1000 6GB Laptop GPU",
    },
    DeviceEntry {
        id: 0x25BD,
        family: Family::Ampere,
        product: "NVIDIA RTX A500 Laptop GPU",
    },
    DeviceEntry {
        id: 0x25E0,
        family: Family::Ampere,
        product: "NVIDIA GeForce RTX 3050 Ti Laptop GPU",
    },
    DeviceEntry {
        id: 0x25E2,
        family: Family::Ampere,
        product: "NVIDIA GeForce RTX 3050 Laptop GPU",
    },
    DeviceEntry {
        id: 0x25E5,
        family: Family::Ampere,
        product: "NVIDIA GeForce RTX 3050 Laptop GPU",
    },
    DeviceEntry {
        id: 0x25EC,
        family: Family::Ampere,
        product: "NVIDIA GeForce RTX 3050 6GB Laptop GPU",
    },
    DeviceEntry {
        id: 0x25ED,
        family: Family::Ampere,
        product: "NVIDIA GeForce RTX 2050",
    },
    DeviceEntry {
        id: 0x25F9,
        family: Family::Ampere,
        product: "NVIDIA RTX A1000 Embedded GPU",
    },
    DeviceEntry {
        id: 0x25FA,
        family: Family::Ampere,
        product: "NVIDIA RTX A2000 Embedded GPU",
    },
    DeviceEntry {
        id: 0x25FB,
        family: Family::Ampere,
        product: "NVIDIA RTX A500 Embedded GPU",
    },
    DeviceEntry {
        id: 0x2684,
        family: Family::Ada,
        product: "NVIDIA GeForce RTX 4090",
    },
    DeviceEntry {
        id: 0x2685,
        family: Family::Ada,
        product: "NVIDIA GeForce RTX 4090 D",
    },
    DeviceEntry {
        id: 0x2689,
        family: Family::Ada,
        product: "NVIDIA GeForce RTX 4070 Ti SUPER",
    },
    DeviceEntry {
        id: 0x26B1,
        family: Family::Ada,
        product: "NVIDIA RTX 6000 Ada Generation",
    },
    DeviceEntry {
        id: 0x26B2,
        family: Family::Ada,
        product: "NVIDIA RTX 5000 Ada Generation",
    },
    DeviceEntry {
        id: 0x26B3,
        family: Family::Ada,
        product: "NVIDIA RTX 5880 Ada Generation",
    },
    DeviceEntry {
        id: 0x26B5,
        family: Family::Ada,
        product: "NVIDIA L40",
    },
    DeviceEntry {
        id: 0x26B9,
        family: Family::Ada,
        product: "NVIDIA L40S",
    },
    DeviceEntry {
        id: 0x26BA,
        family: Family::Ada,
        product: "NVIDIA L20",
    },
    DeviceEntry {
        id: 0x2702,
        family: Family::Ada,
        product: "NVIDIA GeForce RTX 4080 SUPER",
    },
    DeviceEntry {
        id: 0x2704,
        family: Family::Ada,
        product: "NVIDIA GeForce RTX 4080",
    },
    DeviceEntry {
        id: 0x2705,
        family: Family::Ada,
        product: "NVIDIA GeForce RTX 4070 Ti SUPER",
    },
    DeviceEntry {
        id: 0x2709,
        family: Family::Ada,
        product: "NVIDIA GeForce RTX 4070",
    },
    DeviceEntry {
        id: 0x2717,
        family: Family::Ada,
        product: "NVIDIA GeForce RTX 4090 Laptop GPU",
    },
    DeviceEntry {
        id: 0x2730,
        family: Family::Ada,
        product: "NVIDIA RTX 5000 Ada Generation Laptop GPU",
    },
    DeviceEntry {
        id: 0x2757,
        family: Family::Ada,
        product: "NVIDIA GeForce RTX 4090 Laptop GPU",
    },
    DeviceEntry {
        id: 0x2770,
        family: Family::Ada,
        product: "NVIDIA RTX 5000 Ada Generation Embedded GPU",
    },
    DeviceEntry {
        id: 0x2782,
        family: Family::Ada,
        product: "NVIDIA GeForce RTX 4070 Ti",
    },
    DeviceEntry {
        id: 0x2783,
        family: Family::Ada,
        product: "NVIDIA GeForce RTX 4070 SUPER",
    },
    DeviceEntry {
        id: 0x2786,
        family: Family::Ada,
        product: "NVIDIA GeForce RTX 4070",
    },
    DeviceEntry {
        id: 0x2788,
        family: Family::Ada,
        product: "NVIDIA GeForce RTX 4060 Ti",
    },
    DeviceEntry {
        id: 0x27A0,
        family: Family::Ada,
        product: "NVIDIA GeForce RTX 4080 Laptop GPU",
    },
    DeviceEntry {
        id: 0x27B0,
        family: Family::Ada,
        product: "NVIDIA RTX 4000 SFF Ada Generation",
    },
    DeviceEntry {
        id: 0x27B1,
        family: Family::Ada,
        product: "NVIDIA RTX 4500 Ada Generation",
    },
    DeviceEntry {
        id: 0x27B2,
        family: Family::Ada,
        product: "NVIDIA RTX 4000 Ada Generation",
    },
    DeviceEntry {
        id: 0x27B6,
        family: Family::Ada,
        product: "NVIDIA L2",
    },
    DeviceEntry {
        id: 0x27B8,
        family: Family::Ada,
        product: "NVIDIA L4",
    },
    DeviceEntry {
        id: 0x27BA,
        family: Family::Ada,
        product: "NVIDIA RTX 4000 Ada Generation Laptop GPU",
    },
    DeviceEntry {
        id: 0x27BB,
        family: Family::Ada,
        product: "NVIDIA RTX 3500 Ada Generation Laptop GPU",
    },
    DeviceEntry {
        id: 0x27E0,
        family: Family::Ada,
        product: "NVIDIA GeForce RTX 4080 Laptop GPU",
    },
    DeviceEntry {
        id: 0x27FB,
        family: Family::Ada,
        product: "NVIDIA RTX 3500 Ada Generation Embedded GPU",
    },
    DeviceEntry {
        id: 0x2803,
        family: Family::Ada,
        product: "NVIDIA GeForce RTX 4060 Ti",
    },
    DeviceEntry {
        id: 0x2805,
        family: Family::Ada,
        product: "NVIDIA GeForce RTX 4060 Ti",
    },
    DeviceEntry {
        id: 0x2808,
        family: Family::Ada,
        product: "NVIDIA GeForce RTX 4060",
    },
    DeviceEntry {
        id: 0x2820,
        family: Family::Ada,
        product: "NVIDIA GeForce RTX 4070 Laptop GPU",
    },
    DeviceEntry {
        id: 0x2822,
        family: Family::Ada,
        product: "NVIDIA GeForce RTX 3050 A Laptop GPU",
    },
    DeviceEntry {
        id: 0x2838,
        family: Family::Ada,
        product: "NVIDIA RTX 3000 Ada Generation Laptop GPU",
    },
    DeviceEntry {
        id: 0x2860,
        family: Family::Ada,
        product: "NVIDIA GeForce RTX 4070 Laptop GPU",
    },
    DeviceEntry {
        id: 0x2882,
        family: Family::Ada,
        product: "NVIDIA GeForce RTX 4060",
    },
    DeviceEntry {
        id: 0x28A0,
        family: Family::Ada,
        product: "NVIDIA GeForce RTX 4060 Laptop GPU",
    },
    DeviceEntry {
        id: 0x28A1,
        family: Family::Ada,
        product: "NVIDIA GeForce RTX 4050 Laptop GPU",
    },
    DeviceEntry {
        id: 0x28A3,
        family: Family::Ada,
        product: "NVIDIA GeForce RTX 3050 A Laptop GPU",
    },
    DeviceEntry {
        id: 0x28B0,
        family: Family::Ada,
        product: "NVIDIA RTX 2000 Ada Generation",
    },
    DeviceEntry {
        id: 0x28B8,
        family: Family::Ada,
        product: "NVIDIA RTX 2000 Ada Generation Laptop GPU",
    },
    DeviceEntry {
        id: 0x28B9,
        family: Family::Ada,
        product: "NVIDIA RTX 1000 Ada Generation Laptop GPU",
    },
    DeviceEntry {
        id: 0x28BA,
        family: Family::Ada,
        product: "NVIDIA RTX 500 Ada Generation Laptop GPU",
    },
    DeviceEntry {
        id: 0x28BB,
        family: Family::Ada,
        product: "NVIDIA RTX 500 Ada Generation Laptop GPU",
    },
    DeviceEntry {
        id: 0x28E0,
        family: Family::Ada,
        product: "NVIDIA GeForce RTX 4060 Laptop GPU",
    },
    DeviceEntry {
        id: 0x28E1,
        family: Family::Ada,
        product: "NVIDIA GeForce RTX 4050 Laptop GPU",
    },
    DeviceEntry {
        id: 0x28E3,
        family: Family::Ada,
        product: "NVIDIA GeForce RTX 3050 A Laptop GPU",
    },
    DeviceEntry {
        id: 0x28F8,
        family: Family::Ada,
        product: "NVIDIA RTX 2000 Ada Generation Embedded GPU",
    },
    DeviceEntry {
        id: 0x2901,
        family: Family::Excluded,
        product: "NVIDIA B200",
    },
    DeviceEntry {
        id: 0x2909,
        family: Family::Excluded,
        product: "NVIDIA B200",
    },
    DeviceEntry {
        id: 0x2941,
        family: Family::Excluded,
        product: "NVIDIA GB200",
    },
    DeviceEntry {
        id: 0x29BB,
        family: Family::Excluded,
        product: "NVIDIA DRIVE P2021",
    },
    DeviceEntry {
        id: 0x2B85,
        family: Family::BlackwellGb20x,
        product: "NVIDIA GeForce RTX 5090",
    },
    DeviceEntry {
        id: 0x2B87,
        family: Family::BlackwellGb20x,
        product: "NVIDIA GeForce RTX 5090 D",
    },
    DeviceEntry {
        id: 0x2B8C,
        family: Family::BlackwellGb20x,
        product: "NVIDIA GeForce RTX 5090 D v2",
    },
    DeviceEntry {
        id: 0x2BB1,
        family: Family::BlackwellGb20x,
        product: "NVIDIA RTX PRO 6000 Blackwell Workstation Edition",
    },
    DeviceEntry {
        id: 0x2BB3,
        family: Family::BlackwellGb20x,
        product: "NVIDIA RTX PRO 5000 Blackwell",
    },
    DeviceEntry {
        id: 0x2BB4,
        family: Family::BlackwellGb20x,
        product: "NVIDIA RTX PRO 6000 Blackwell Max-Q Workstation Edition",
    },
    DeviceEntry {
        id: 0x2BB5,
        family: Family::BlackwellGb20x,
        product: "NVIDIA RTX PRO 6000 Blackwell Server Edition",
    },
    DeviceEntry {
        id: 0x2BB9,
        family: Family::BlackwellGb20x,
        product: "NVIDIA RTX 6000D",
    },
    DeviceEntry {
        id: 0x2C02,
        family: Family::BlackwellGb20x,
        product: "NVIDIA GeForce RTX 5080",
    },
    DeviceEntry {
        id: 0x2C05,
        family: Family::BlackwellGb20x,
        product: "NVIDIA GeForce RTX 5070 Ti",
    },
    DeviceEntry {
        id: 0x2C09,
        family: Family::BlackwellGb20x,
        product: "NVIDIA GeForce RTX 5070",
    },
    DeviceEntry {
        id: 0x2C18,
        family: Family::BlackwellGb20x,
        product: "NVIDIA GeForce RTX 5090 Laptop GPU",
    },
    DeviceEntry {
        id: 0x2C19,
        family: Family::BlackwellGb20x,
        product: "NVIDIA GeForce RTX 5080 Laptop GPU",
    },
    DeviceEntry {
        id: 0x2C31,
        family: Family::BlackwellGb20x,
        product: "NVIDIA RTX PRO 4500 Blackwell",
    },
    DeviceEntry {
        id: 0x2C33,
        family: Family::BlackwellGb20x,
        product: "NVIDIA RTX PRO 4000 Blackwell SFF Edition",
    },
    DeviceEntry {
        id: 0x2C34,
        family: Family::BlackwellGb20x,
        product: "NVIDIA RTX PRO 4000 Blackwell",
    },
    DeviceEntry {
        id: 0x2C38,
        family: Family::BlackwellGb20x,
        product: "NVIDIA RTX PRO 5000 Blackwell Generation Laptop GPU",
    },
    DeviceEntry {
        id: 0x2C39,
        family: Family::BlackwellGb20x,
        product: "NVIDIA RTX PRO 4000 Blackwell Generation Laptop GPU",
    },
    DeviceEntry {
        id: 0x2C3A,
        family: Family::BlackwellGb20x,
        product: "NVIDIA RTX PRO 4500 Blackwell Server Edition",
    },
    DeviceEntry {
        id: 0x2C58,
        family: Family::BlackwellGb20x,
        product: "NVIDIA GeForce RTX 5090 Laptop GPU",
    },
    DeviceEntry {
        id: 0x2C59,
        family: Family::BlackwellGb20x,
        product: "NVIDIA GeForce RTX 5080 Laptop GPU",
    },
    DeviceEntry {
        id: 0x2C77,
        family: Family::BlackwellGb20x,
        product: "NVIDIA RTX PRO 5000 Blackwell Embedded GPU",
    },
    DeviceEntry {
        id: 0x2C79,
        family: Family::BlackwellGb20x,
        product: "NVIDIA RTX PRO 4000 Blackwell Embedded GPU",
    },
    DeviceEntry {
        id: 0x2D04,
        family: Family::BlackwellGb20x,
        product: "NVIDIA GeForce RTX 5060 Ti",
    },
    DeviceEntry {
        id: 0x2D05,
        family: Family::BlackwellGb20x,
        product: "NVIDIA GeForce RTX 5060",
    },
    DeviceEntry {
        id: 0x2D18,
        family: Family::BlackwellGb20x,
        product: "NVIDIA GeForce RTX 5070 Laptop GPU",
    },
    DeviceEntry {
        id: 0x2D19,
        family: Family::BlackwellGb20x,
        product: "NVIDIA GeForce RTX 5060 Laptop GPU",
    },
    DeviceEntry {
        id: 0x2D30,
        family: Family::BlackwellGb20x,
        product: "NVIDIA RTX PRO 2000 Blackwell",
    },
    DeviceEntry {
        id: 0x2D39,
        family: Family::BlackwellGb20x,
        product: "NVIDIA RTX PRO 2000 Blackwell Generation Laptop GPU",
    },
    DeviceEntry {
        id: 0x2D58,
        family: Family::BlackwellGb20x,
        product: "NVIDIA GeForce RTX 5070 Laptop GPU",
    },
    DeviceEntry {
        id: 0x2D59,
        family: Family::BlackwellGb20x,
        product: "NVIDIA GeForce RTX 5060 Laptop GPU",
    },
    DeviceEntry {
        id: 0x2D79,
        family: Family::BlackwellGb20x,
        product: "NVIDIA RTX PRO 2000 Blackwell Embedded GPU",
    },
    DeviceEntry {
        id: 0x2D83,
        family: Family::BlackwellGb20x,
        product: "NVIDIA GeForce RTX 5050",
    },
    DeviceEntry {
        id: 0x2D98,
        family: Family::BlackwellGb20x,
        product: "NVIDIA GeForce RTX 5050 Laptop GPU",
    },
    DeviceEntry {
        id: 0x2DB8,
        family: Family::BlackwellGb20x,
        product: "NVIDIA RTX PRO 1000 Blackwell Generation Laptop GPU",
    },
    DeviceEntry {
        id: 0x2DB9,
        family: Family::BlackwellGb20x,
        product: "NVIDIA RTX PRO 500 Blackwell Generation Laptop GPU",
    },
    DeviceEntry {
        id: 0x2DD8,
        family: Family::BlackwellGb20x,
        product: "NVIDIA GeForce RTX 5050 Laptop GPU",
    },
    DeviceEntry {
        id: 0x2DF9,
        family: Family::BlackwellGb20x,
        product: "NVIDIA RTX PRO 500 Blackwell Embedded GPU",
    },
    DeviceEntry {
        id: 0x2F04,
        family: Family::BlackwellGb20x,
        product: "NVIDIA GeForce RTX 5070",
    },
    DeviceEntry {
        id: 0x2F06,
        family: Family::BlackwellGb20x,
        product: "NVIDIA GeForce RTX 5060",
    },
    DeviceEntry {
        id: 0x2F18,
        family: Family::BlackwellGb20x,
        product: "NVIDIA GeForce RTX 5070 Ti Laptop GPU",
    },
    DeviceEntry {
        id: 0x2F38,
        family: Family::BlackwellGb20x,
        product: "NVIDIA RTX PRO 3000 Blackwell Generation Laptop GPU",
    },
    DeviceEntry {
        id: 0x2F58,
        family: Family::BlackwellGb20x,
        product: "NVIDIA GeForce RTX 5070 Ti Laptop GPU",
    },
    DeviceEntry {
        id: 0x3182,
        family: Family::Excluded,
        product: "NVIDIA B300 SXM6 AC",
    },
    DeviceEntry {
        id: 0x31C2,
        family: Family::Excluded,
        product: "NVIDIA GB300",
    },
    DeviceEntry {
        id: 0x31C3,
        family: Family::Excluded,
        product: "NVIDIA GB300",
    },
];

/// vix EDITOR_ONLY（pci.ids 来源，仅离线编辑口径；装载视为未知）。
pub const EDITOR_ONLY: &[DeviceEntry] = &[DeviceEntry {
    id: 0x2501,
    family: Family::Ampere,
    product: "NVIDIA GeForce RTX 3060",
}];

/// GA100 设备 ID（不同 GSP 固件；接收容量未审计）。
pub const GA100_DEVICE_IDS: &[u16] = &[
    0x20B0, 0x20B2, 0x20B3, 0x20B5, 0x20B7, 0x20BD, 0x20F1, 0x20F3, 0x20F5, 0x20F6,
];

/// 查主清单（含 excluded 条目）。
pub fn device_info(id: u16) -> Option<&'static DeviceEntry> {
    DEVICES
        .binary_search_by_key(&id, |e| e.id)
        .ok()
        .map(|i| &DEVICES[i])
}

/// 架构门判定结果（vix `architecture()` 的 Rust 形态）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Architecture {
    pub family: Option<Family>,
    pub product: String,
    /// 非 excluded 且在主清单内。
    pub in_scope: bool,
    /// 全部 BIOS 版本串与族前缀相干（需要同时给出版本串）。
    pub bios_family_consistent: bool,
    /// 0x2501 一类 pci.ids-only 条目。
    pub editor_only: bool,
}

/// PCI ID + ROM 内观测到的全部 `9x.xx.xx.xx.xx.xx` 版本串 → 架构判定。
/// 未知/排除 ID 返回 `family: None/Excluded` 且 fail closed。
pub fn architecture(device: u16, bios_versions: &[&str]) -> Architecture {
    let known = device_info(device);
    let editor = EDITOR_ONLY.iter().find(|e| e.id == device);
    let entry = known.or(editor);
    let family = entry.map(|e| e.family);
    // GA100 特判优先：92. 前缀（DGX A100/A800 固件发行注记）；其余按族。
    let expected = if GA100_DEVICE_IDS.contains(&device) {
        Some("92.")
    } else {
        family.and_then(Family::bios_version_prefix)
    };
    let coherent = match (expected, bios_versions.is_empty()) {
        (Some(p), false) => bios_versions.iter().all(|v| v.starts_with(p)),
        _ => false,
    };
    Architecture {
        family,
        product: entry
            .map(|e| e.product.to_string())
            .unwrap_or_else(|| "unknown PCI device".into()),
        in_scope: family.is_some_and(|f| f.in_scope()),
        bios_family_consistent: coherent,
        editor_only: editor.is_some(),
    }
}

/// 从 ROM 字节里抓 BIOS 版本串（vix metadata 的 `9[0-9]\.xx.xx.xx.xx` 正则
/// 等价，14 字节匹配）。
pub fn scan_bios_versions(data: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 14 <= data.len() {
        let seg = |a: usize| -> Option<String> {
            let mut s = String::new();
            for k in 0..2 {
                let c = data.get(a + k)?;
                if !c.is_ascii_hexdigit() {
                    return None;
                }
                s.push(*c as char);
            }
            Some(s)
        };
        if data[i] == b'9'
            && data[i + 1].is_ascii_digit()
            && data[i + 2] == b'.'
            && seg(i + 3).is_some()
            && data[i + 5] == b'.'
            && seg(i + 6).is_some()
            && data[i + 8] == b'.'
            && seg(i + 9).is_some()
            && data[i + 11] == b'.'
            && seg(i + 12).is_some()
        {
            let s: String = data[i..i + 14].iter().map(|&c| c as char).collect();
            if !out.contains(&s) {
                out.push(s);
            }
            i += 14;
            continue;
        }
        i += 1;
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn devices_are_sorted_and_unique() {
        assert!(DEVICES.windows(2).all(|w| w[0].id < w[1].id));
    }

    #[test]
    fn known_and_unknown_ids() {
        assert_eq!(device_info(0x2684).unwrap().family, Family::Ada);
        assert_eq!(device_info(0x2B85).unwrap().family, Family::BlackwellGb20x);
        assert!(device_info(0x2329).unwrap().family == Family::Excluded); // H20
        assert!(device_info(0x1234).is_none());
    }

    #[test]
    fn ga100_version_gate() {
        let a = architecture(0x20B0, &["92.02.18.00.01"]);
        assert!(a.in_scope);
        assert!(a.bios_family_consistent);
        // 非 GA100 的 ampere 期待 94. 前缀。
        assert!(!architecture(0x2204, &["92.02"]).bios_family_consistent);
    }

    #[test]
    fn version_scan() {
        let mut rom = vec![0xFFu8; 64];
        rom[10..24].copy_from_slice(b"95.02.18.40.35");
        let v = scan_bios_versions(&rom);
        assert_eq!(v, vec!["95.02.18.40.35".to_string()]);
    }
}
