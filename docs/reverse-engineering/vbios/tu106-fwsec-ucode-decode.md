# TU106 VBIOS FWSEC 微码链解码与 parser 补齐

- 日期：2026-10-25
- 样本：`1.rom`（TU106 RTX 2070, 90.06.18.00.EE, 320KiB, PCI 10de:1f07）；对照 `reverse/gpuz/`（GP102/GP104×4/TU106×2）
- 信息源：NVIDIA open-gpu-kernel-modules `src/nvidia/src/kernel/gpu/gsp/kernel_gsp_fwsec.c`（官方枚举与描述符布局，在线核对）；`reverse/gpu-unlock/kernel/gpu_unlock/vbios_610.c`（BIT/ucode 表走链与 V3 布局）；hexpat（V1 布局与 PRE_OS/LS_UDE 命名，既有）
- 分支：`vbios-parser-enhanced`，nvoc 提交 `8353da3`；nvapi-rs 本地提交 `bd2cff8`（gitlink 修复，见文末）
- 关联：[gpu-unlock-ecosystem-survey.md](../gpu-unlock-ecosystem-survey.md)

## 1. 链路

```
BIT 头 (0xB8FF + "BIT\0" + 校验和)      ← 1.rom @0x1b0，size=12/stride=6/tokens=17
  └─ token 0x70 ('p') v2 len=4 @0x3b5   ← 指针 = u32 @0x3b5 = 0xeb24（相对扩展镜像基址）
       └─ ucode 表 @ adjust + 0xeb24 = 0x20124
            头: ver=1 hdr=6 stride=6 entries=16
            条目 6B: {u8 app, u8 target, u32 desc_off（相对 adjust）}
                 └─ 描述符（app 0x45/0x85/0x89 = FWSEC 族，vDesc 头 + 字段）
                      └─ 镜像 = desc + desc_size：[IMEM | DMEM]（V2 布局）
```

**关键 relocate**：`adjust = ext − base`。1.rom 中 base=首镜像长 0xea00（legacy x86）、ext=VN 扩展镜像 @0x20000 → **adjust=0x11600**；BIT token 的 ucode 表指针与描述符偏移都要加它。gpu-unlock `vbios_610.c` 的 `expansion()` 用 PCIR+0x14 的 0/0xE0 标记找 base/ext；我们的 parser 用等价的 EFI 间隙重定位（hexpat `adjust_skip_efi`）。

## 2. 应用/目标 ID

官方命名（kernel_gsp_fwsec.c，2026-10 在线核对）+ hexpat 既有：

| app ID | 名称 | 备注 |
|---|---|---|
| 0x01 | PRE_OS | PMU 引导装载，V1 描述符无 vDesc 头 |
| 0x04 | PRIMARY_DEVINIT_ENGINE | Pascal devinit |
| 0x05 | FIRMWARE_SEC_LIC | 安全许可证 ucode（Pascal） |
| 0x08 | LS_UDE | PMU |
| 0x45 | **FWSEC_DBG** | debug FWSEC，驱动按 `bUseDebugFwsec`（`kgspIsDebugModeEnabled`）与 PROD 二选一 |
| 0x85 | **FWSEC_PROD** | 生产 FWSEC；GA10x+ 为 V3 描述符（44B+384B/条 RSA3K 签名区） |

target：0x01=PMU、0x02=DPU、0x03=FECS（hexpat）；TU106 上 FWSEC 双轨 target=**0x07**、0x89 target=**0x05**——官方源码未定义这两个值，parser 如实留 `?`。

## 3. 描述符三版布局

vDesc 头（u32）：bit0=版本头存在、bit2=加密位、[15:8]=版本、[31:16]=描述符全长。字段偏移（含 4B 头）：

| 偏移 | V1（40B） | V2（60B，Turing） | V3（44B，GA10x+） |
|---|---|---|---|
| 0x04 | StoredSize | StoredSize | StoredSize |
| 0x08 | UncompressedSize | UncompressedSize | **PKCDataOffset** |
| 0x0C | VirtualEntry | VirtualEntry | InterfaceOffset |
| 0x10 | IMEMPhysBase | **InterfaceOffset** | IMEMPhysBase |
| 0x14 | IMEMLoadSize | IMEMPhysBase | IMEMLoadSize |
| 0x18 | — | IMEMLoadSize | IMEMVirtBase |
| 0x1C | — | IMEMVirtBase | DMEMPhysBase |
| 0x20 | DMEMPhysBase | **IMEMSecBase** | DMEMLoadSize |
| 0x24 | DMEMLoadSize | **IMEMSecSize** | EngineIdMask(u16)/UcodeId/SigCount/SigVersions |
| 0x28+ | — | DMEMOffset/DMEMPhysBase/DMEMLoadSize/alt×2 | — |

V2 启动方式=BOOT_WITH_LOADER（代码/数据分开 memdesc），V3=BOOT_FROM_HS（单 memdesc+签名区跟随描述符）。**V2 与 V3 字段序不同**（PKC 插入后全部前移 4 字节），不可共用偏移。

## 4. TU106 实测（1.rom）

| 条目 | target | stored | 备注 |
|---|---|---|---|
| PRE_OS | PMU | 0x6774 | **明文 fuc5**（熵 6.63）——envydis 可直接分析；尾部 DMEM 含 devinit 脚本记号（bw/sw/2d2t） |
| LS_UDE | PMU | 0x2fe8 | |
| FWSEC_DBG | 0x07 | 0x9a10 | 与 PROD 几何逐位相同 |
| FWSEC_PROD | 0x07 | 0x9a10 | IMEM 0x9600 + DMEM 0x410；InterfaceOffset=0xdc（落 DMEM 内） |
| 0x89 | 0x05 | 0x2c34 | **官方源码亦无名的第三条 FWSEC 族条目**，IMEM 0x2900/DMEM 0x45c0 |
| 0x82 / 0x83 | PMU | 0x2680 / 0x1da0 | V1 描述符无版本头 |

FWSEC V2 几何自洽：`IMEMLoadSize 0x9600 = IMEMSecBase 0x300 + IMEMSecSize 0x9300`（安全代码区恰占满 IMEM），`stored 0x9a10 = IMEM 0x9600 + DMEM 0x410`。

加密态实测（idalib 三墙实验素材）：FWSEC IMEM 熵 **7.99**（AES 密文，密钥在熔丝）；GOP EFI 镜像（@0xea00, code_type=3）整段压缩无 PE 签名；仅 PRE_OS 明文。

## 5. parser 侧变更（8353da3）

- `falcon_app_name` 补 0x45/0x85（官方名）；`FalconUcodeDesc` 重构为三版布局解码（修掉旧代码把 V2 字段按 V1 偏移误读的 bug：如 TU106 上 `imem_phys_base` 实读到 InterfaceOffset 0xdc、`dmem_load_size` 实读到 IMEMSecSize 0x9300）。
- verbose 行新增几何：`FWSEC_PROD/? 0x9a10 imem 0x9600 dmem 0x410`；V3 条目附 `sig <n>@<verbits>`；JSON 全字段（desc_size/encrypted/imem_*/dmem_*/signature_*）。
- 回归测试 `falcon_turing_fwsec_when_present`（锚 `../1.rom`，文件缺席自动跳过，`*_when_present` 惯例）：命名+V2 几何+自洽断言全绿；Pascal（GP104/GP102/P100）旧测试无损——FIRMWARE_SEC_LIC 在 Pascal 上本就是 V2 描述符，同样受益于修复。

用法：`nvoc-cli get-vbios -i <rom> --verbose`，看 `falcon ucode:` 行。

## 6. nvapi-rs gitlink 事故（同分支）

`vbios-parser-enhanced` 的 nvapi-rs gitlink 记录 `8fc2fbe`——从未推送的丢失提交（本机全部本地引用+`origin` 全量 fetch 后均不存在）。缺的 API 面=`nvapi::hi::AdcDevices` 类型+`Gpu::adc_devices()`。本地提交 `bd2cff8` 补最小 shim（`hi/gpu.rs`：`pub type AdcDevices = clock::AdcDevicesInfo` + 透传方法），随 `8353da3` 更新 gitlink。**教训：分支记录的子模块提交若 fetch 不到，先查 reflog 找替代检出处，找不到就按消费侧期望重建 API 面而非改业务代码。**
