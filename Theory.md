# 理论基础：GPU 性能优化与 V-F 曲线调优

> 本章节阐述了 GPU 超频与 V-F 曲线自动优化的底层理论，帮助理解 NVOC 各工具的设计动机与工作原理。建议在阅读 [[Autoscan-Workflow]] 之前先了解本文内容。

[English](#english) | [中文](#chinese)

<a id="english"></a>

## English

> **Theoretical Foundations: GPU Performance Optimization & V-F Curve Tuning**
>
> This chapter explains the underlying theory of GPU overclocking and automated V-F curve optimization, providing the rationale behind NVOC's tool design and workflows. It is recommended to read this before [[Autoscan-Workflow]].

---

### 1. Origins of the Performance Optimization Problem

#### 1.1 Why Performance Optimization Matters

NVIDIA GPUs ship from the factory with considerable headroom for performance and power efficiency tuning. This headroom exists for several reasons:

- **Conservative Calibration**: Manufacturers baseline the factory V-F curve on worst-case silicon, the harshest thermal conditions, and the broadest range of workloads. This ensures every GPU remains stable under any scenario.
- **Inter-Chip Variation (Silicon Lottery)**: Due to manufacturing process variation, identical GPU models exhibit significant differences in their actual voltage-frequency tolerance. Some chips can reach target frequencies at below-nominal voltage, while others can sustain frequencies far above the default curve at higher voltage points.
- **Uniform DVFS Strategy**: Factory firmware uses a one-size-fits-all Dynamic Voltage and Frequency Scaling (DVFS) strategy that cannot account for individual chip characteristics or application-specific workload profiles.

NVOC's objective is to explore and actualize a **unified optimization logic between maximizing GPU performance and achieving optimal power efficiency** under specific constraints (power limits, thermal limits, voltage limits) through meticulous parameter tuning.

#### 1.2 GPU Application Domains and Optimization Needs

GPU application domains have expanded far beyond traditional graphics rendering:

| Domain | Typical Workloads | Optimization Focus |
|---|---|---|
| Traditional Graphics Rendering | Gaming, VFX, CAD/CAM | Higher FPS, lower frame latency |
| High-Performance Computing (HPC) | Weather forecasting, molecular dynamics, financial modeling | Maximum throughput |
| Artificial Intelligence (AI) | Deep learning training/inference, LLMs | Compute utilization, energy efficiency ratio |
| Cryptography & Security | Hashcat-based password recovery | Peak core frequency |
| Data Analytics | Large-scale ETL, graph processing | Memory bandwidth, I/O throughput |
| Cloud & Virtualization | Multi-tenant GPU sharing | Resource utilization, SLA compliance |

#### 1.3 The Necessity of GPU Performance Optimization

##### 1.3.1 Power and Thermal Bottlenecks

Top-tier GPUs can draw several hundred watts at peak load. This creates the **"power wall"** phenomenon: once the chip's power consumption reaches its cooling system's dissipation limit, simply raising frequency can no longer deliver performance gains. The chip instead throttles down to stay within thermal and power budgets. DVFS technology alleviates this by lowering voltage and frequency during idle or low-load periods, directly reducing both dynamic power and static leakage.

##### 1.3.2 Economic Cost Considerations

High-end GPUs (e.g., NVIDIA A100 80GB at approximately $15,000 per unit) represent substantial capital investment. For data centers with large-scale GPU deployments, long-term electricity consumption is an equally significant operational expense. Any improvement in energy efficiency ratio (EER) — the computational output per unit of energy — directly lowers Total Cost of Ownership (TCO).

##### 1.3.3 Resource Utilization Maximization

In cloud computing and HPC cluster environments, average GPU utilization often falls below 50% due to workload fluctuations, suboptimal job scheduling, or applications failing to fully exploit GPU parallelism. Fine-grained, adaptive optimization strategies help match GPU performance output to actual workload demands, improving overall utilization while meeting Service Level Agreements (SLAs).

##### 1.3.4 The Limits of Traditional DVFS

Traditional DVFS dynamically selects operating points from a single, fixed V-F curve based on load heuristics. However, this approach ignores the **optimizability of the V-F curve itself**. By directly optimizing the default V-F curve and then applying DVFS on top of the optimized curve, significantly greater performance and efficiency gains can be achieved. This is precisely the value proposition and research focus of NVOC.

#### 1.4 Literature Review and Engineering Practice

The field of GPU overclocking and automated performance tuning is relatively niche, with limited published formal research. Key prior work includes:

**AOA: Adaptive Overclocking Algorithm on CPU-GPU Heterogeneous Platforms** — This research proposed dynamically coordinating CPU and GPU frequency and power allocation to improve overall system performance while maintaining constant total power consumption. On older server platforms (Intel E5 2660 + Tesla K80), AOA achieved up to 6.1% performance improvement and ~4.4% energy savings. However, on newer consumer platforms (Intel i9 + RTX 2080Ti), gains were limited to 0.2%-0.6%, with no significant power optimization.

The key difference between AOA and NVOC: AOA operates on the assumption of constant total CPU+GPU power, redistributing power budgets at runtime. NVOC instead focuses on **directly optimizing the GPU's V-F curve itself**, achieving fundamental hardware-level improvements that persist regardless of the system's power allocation strategy.

---

### 2. Theoretical Foundations of Performance Optimization

#### 2.1 Signal Integrity and Propagation Delay

The physical upper limit of GPU operating frequency is determined by fundamental digital circuit characteristics:

##### 2.1.1 CMOS Inverter Model

The core building block of on-chip logic is the CMOS (Complementary Metal-Oxide-Semiconductor) inverter. When an input signal transitions (0→1 or 1→0), the output does not switch instantaneously. There exists a finite charge/discharge time determined by the driving transistor's current capability and the load capacitance of downstream gates.

##### 2.1.2 Frequency-Voltage Relationship

Higher operating voltage accelerates transistor switching speed by increasing the drive current, thereby reducing propagation delay and enabling higher clock frequencies. However, the relationship is non-linear: dynamic power consumption grows as `C × V² × f`, meaning voltage increases carry a squared penalty on power.

##### 2.1.3 Signal Integrity Constraints

As frequency increases, secondary effects become non-negligible:
- **Reflections**: Signal reflections at impedance discontinuities along interconnects
- **Crosstalk**: Capacitive coupling between adjacent signal lines
- **Attenuation**: Resistive losses in long-distance on-chip routing

The GPU's **critical path** — the longest combinational logic path between two sequential elements — determines the theoretical maximum frequency at each voltage point. In summary, **each voltage point has a physically-determined maximum usable frequency**, which forms the physical basis for V-F curve optimization.

#### 2.2 Chip Power and Thermal Characteristics

##### 2.2.1 Static Power (Leakage)

Static power consumption arises from transistor leakage currents that flow even when circuits are not actively switching:
- **Subthreshold leakage**: Current that flows when the transistor is nominally "off"
- **Gate leakage**: Tunneling current through the gate oxide
- Leakage increases **exponentially** with temperature
- In advanced process nodes (7nm, 5nm, 4nm, 3nm), leakage accounts for an increasingly large share of total power

##### 2.2.2 Dynamic Power (Switching)

The core formula: `P_dynamic = α × C × V² × f`

Where:
- `α` = Activity factor (fraction of circuits switching per cycle)
- `C` = Total load capacitance
- `V` = Operating voltage
- `f` = Switching frequency

Key insight: voltage has the dominant impact (quadratic). A small voltage reduction can yield a significant power savings — this is the basis of **undervolting** strategies.

##### 2.2.3 NVIDIA GPU Functional Module Power Characteristics

| Module | Power Characteristic | Frequency Sensitivity |
|---|---|---|
| CUDA Cores / Streaming Multiprocessors (SM) | Primary source of dynamic power | High |
| Memory Controller (MC) | Correlated with VRAM frequency and bandwidth | Medium |
| VRAM (GDDR / HBM) | Significant static + dynamic power | Medium |
| Raster Operation Units (ROP) | Driven by rendering load | Medium |
| Ray Tracing Cores (RT Core) | Dense computation, high power | High |
| Tensor Cores | Matrix multiplication intensive, high power | High |
| L1 / L2 Cache | Large area, high leakage at elevated temperature | Low-Medium |
| NVENC / NVDEC | Dedicated fixed-function encoders/decoders | Low (fixed) |
| PCIe / NVLink Interface | I/O-dependent | Low-Medium |

#### 2.3 Chip Performance Optimization Logic

The core contradiction in V-F curve optimization is the tension between frequency gains and power/thermal increases:

```
Performance ↑ → Frequency ↑ → Voltage demand ↑ → Power ↑↑ → Temperature ↑ → Leakage ↑ → Stability ↓
```

This creates two optimization paths:

1. **Performance-Oriented**: Within given thermal and power constraints, maximize the frequency offset at each voltage point
2. **Efficiency-Oriented (Undervolting)**: Without losing performance, find the lowest viable voltage for each frequency target, reducing power consumption via the V² relationship

#### 2.4 GPU Overclocking: Modes and Principles

##### 2.4.1 NVIDIA GPU Dynamic Voltage-Frequency Operating Mode

Starting with Pascal (GTX 10-series), NVIDIA introduced **GPU Boost 3.0**. The core mechanism:

- The chip contains a preset **V-F Curve Table** (VFP Table) defining the default core frequency for each of ~80 voltage points
- At runtime, the GPU dynamically selects operating points from this curve based on current load, power, temperature, and other telemetry
- **Overclocking, at its essence, means applying a positive frequency offset (`KilohertzDelta`) to each voltage point in this table**

The conservatism of the factory V-F curve means every voltage point has exploitable overclocking headroom:

```
Default Frequency  = f_default(voltage)
Maximum Frequency  = f_max(voltage)        ← constrained by physical delay & signal integrity
Stable Frequency   = min(f_max, actual reachable under power/thermal/voltage limits)

Overclocking Headroom = Stable Frequency - Default Frequency
```

##### 2.4.2 Loadline Mechanism

The Loadline is a critical characteristic of GPU power delivery:

| Type | Name | Description |
|---|---|---|
| DC Loadline | DC / Static Loadline | Steady-state voltage droop proportional to load current (V = V_set - I_load × R_ll) |
| AC Loadline | AC / Dynamic Loadline | Transient voltage fluctuations (overshoot/undershoot) during abrupt load changes |

**Key impacts**:
- Under heavy load, the actual effective core voltage (V_actual) is lower than the set voltage (V_set) due to Vdroop
- Excessive voltage droop can cause frequency instability or silent data corruption
- Motherboard/GPU **Load-Line Calibration (LLC)** adjusts this effect — higher LLC reduces droop but increases transient overshoot risk
- Optimized V-F curves must account for worst-case effective voltage under varying loads

##### 2.4.3 Optimization Constraints and Limiting Conditions

GPU performance optimization is simultaneously bounded by multiple constraint types:

| Constraint | Type | Effect When Triggered |
|---|---|---|
| **Power Limit (PL)** | Chip total power cap | GPU auto-reduces voltage and frequency |
| **Thermal Limit** | Junction temperature cap | GPU throttles (Thermal Throttling), reducing frequency |
| **Voltage Limit** | Hardware maximum allowed operating voltage | Caps the maximum settable frequency on the curve |
| **Current Limit (CEP)** | Supply circuit maximum current | Limits sustained high-load operation capability |
| **Reliability Wall** | Long-term voltage/temperature stress on silicon | Accelerated electromigration and aging effects |
| **Operating System TDR** | Windows Timeout Detection & Recovery | Driver reset on GPU hang, possible to BSOD on 50-series |

In practice, high-frequency/high-load scenarios most commonly hit **power limit or thermal limit first**, causing the actual operating point to deviate from the target set on the V-F curve. NVOC's dynamic resolution adaptive adjustment mechanism is specifically designed to address this.

##### 2.4.4 Underlying Basis for V-F Curve Optimization

The feasibility of V-F curve optimization rests on three facts:

1. **Individual Variance**: Silicon quality variation within the same GPU model results in 5%-15% spread in stable frequency capability per voltage point
2. **Voltage Point Independence**: The overclocking headroom at different voltage points is relatively independent (determined by physical delay characteristics specific to each voltage domain)
3. **Load Dependency**: Chip stability behavior differs across workload types — compute-intensive loads stress arithmetic pipelines, while memory-intensive loads stress the memory subsystem and interconnect

NVOC's autoscan exploits these properties to explore each GPU's individual stable overclocking limit on a per-voltage-point basis.

##### 2.4.5 Comprehensive Benefit Model: Power vs. Time Cost Tradeoff

Optimization benefits can be measured along two dimensions:

**Performance Gains**:
- Typical: 4% - 15% (varies by GPU model and workload)
- Some GPUs exceed 10% improvement under specific workloads
- Pure compute tasks (e.g., Hashcat NTLMv1) scale near-linearly with core frequency
- Graphics workloads (e.g., Black Myth: Wukong Benchmark) show diminishing returns after a saturation point

**Power Efficiency Improvements**:
- Power reduction of 12% - 30% without performance loss
- Some GPUs achieve > 20% improvement in energy efficiency ratio (performance per watt)
- A CPU-heavy 3D game (e.g., Rainbow Six Siege Benchmark) showed effective FPS gains even with GPU underclocking due to freed thermal/power budget

**Time Cost of Optimization**:
- Standard autoscan: several hours (full point-by-point sweep of all voltage points)
- Ultra-fast scan: ~1 hour (scans only 4 critical "hot zone" voltage points, interpolates the rest)

The scan duration is determined by: number of voltage points × per-point stress test time × (binary search steps + fluctuation cycles). The Relaxed Finite-Step Binary Approximation Algorithm is designed to minimize this while maintaining result quality.

---

### 3. From Theory to NVOC Implementation

| Theoretical Concept | NVOC Implementation |
|---|---|
| V-F curve per-point optimization | `set vfp autoscan` — powered by the Relaxed Finite-Step Binary Approximation Algorithm |
| Constraint handling | Dynamic resolution adaptation, power/thermal limit detection modules |
| Stability validation | cli-stressor suite (CUDA/PyTorch, OpenCL, Rust cuBLAS) with multi-precision verification |
| Loadline compensation | `fix_result` post-processing with light/heavy load margin correction |
| Individual chip exploration | Breakpoint-resume scanning + frequency fluctuation mechanism for result reliability |
| Efficiency analysis | Pre/post optimization power/performance comparison and visualization |
| Fault recovery | Multi-level fault handling (soft errors → app crashes → driver crashes → kernel panics → BSOD recovery) |

---

### Related Pages

- [[Autoscan-Workflow]] — Complete V-F curve autoscan workflow
- [[GPU-Support-Matrix]] — GPU generation compatibility and API capabilities
- [[Stress-Testing]] — Stress testing tools reference
- [[Auto-Optimizer-Guide]] — CLI command reference

---

<a id="chinese"></a>

## 中文

> **理论基础：GPU 性能优化与 V-F 曲线调优**
>
> 本章节阐述了 GPU 超频与 V-F 曲线自动优化的底层理论，帮助理解 NVOC 各工具的设计动机与工作原理。建议在阅读 [[Autoscan-Workflow]] 之前先了解本文内容。

---

### 1. 性能优化问题的来源

#### 1.1 为什么需要性能优化？

NVIDIA GPU 出厂时通常预留了较大的性能与能效调整空间，原因如下：

- **保守标定**：厂商基于最差情况（worst-case）的硅片、最严苛的散热条件和最广泛的负载类型设定出厂 V-F 曲线，以确保所有 GPU 在任意场景下都能稳定运行。
- **个体硅质差异（Silicon Lottery）**：同一型号的不同 GPU 芯片因制造工艺的微观波动，实际电压-频率容忍度存在显著差异。部分芯片可以在低于标称电压下达到目标频率（体质较好），或在高电压下运行远超默认的频率（超频潜力大）。
- **统一的 DVFS 策略**：出厂固件采用通用的动态电压频率调节（DVFS）策略，无法针对个体芯片特性或特定应用场景进行精细优化。

NVOC 的目标是：通过精细化参数调优，在满足特定约束（功耗墙、温度墙、电压墙）的前提下，**探索并实现 GPU 在极限性能与最优能效比之间的统一优化逻辑**。

#### 1.2 GPU 应用领域与优化需求

GPU 的应用领域已经从传统的图形渲染大幅扩展到众多计算密集型行业：

| 领域 | 典型负载 | 优化侧重 |
|---|---|---|
| 传统图形渲染 | 电脑游戏、影视与动画特效制作、CAD 计算机辅助设计、专业可视化（医学影像、科学数据可视化） | 帧率提升、渲染延迟降低 |
| 高性能计算（HPC） | 气象预报、计算流体动力学、分子动力学模拟、天体物理学、金融建模、石油和天然气勘探、密码分析 | 吞吐量最大化 |
| 人工智能（AI） | 深度学习训练与推理、大语言模型（LLM）、图像识别、语音识别、自然语言处理、推荐系统 | 算力利用率、能效比 |
| 数据安全与加密 | Hashcat 密码恢复、加密货币挖矿 | 核心频率极限 |
| 数据分析 | 大规模 ETL、图计算 | 显存带宽与 I/O 吞吐 |
| 云计算与虚拟化 | 多租户 GPU 共享 | 资源利用率、SLA 合规 |

GPU 应用领域的不断拓宽证明了其并行计算模型的普适性和高效性，同时也对 GPU 的性能、功耗和易用性提出了更高的要求。

#### 1.3 性能优化的必要性

##### 1.3.1 功耗与散热瓶颈

顶级 GPU 的峰值功耗可达数百瓦甚至更高。如此巨大的功耗不仅带来高昂的电费开销，更严峻的是由此产生的巨大热量。这就是所谓的"**功耗墙**"（Power Limit）现象——芯片功耗达到散热系统能力上限后，无法通过简单的提高频率来提升性能，GPU 反而会自动降频以保持在功耗和温度预算之内。DVFS 技术通过降低空闲或低负载时的电压与频率，可以直接减少功耗和发热，是缓解这一瓶颈的重要手段。

##### 1.3.2 经济成本考量

GPU 硬件本身，尤其是面向数据中心和专业应用的高端型号（如 NVIDIA A100 80GB，单价高达约 15,000 美元），价格非常昂贵。此外，大规模 GPU 集群的长期运行所消耗的电能也是一笔不容忽视的运营开支。因此，任何能够提高 GPU 能效比（即单位能量所能完成的计算量）或提升 GPU 资源利用率的优化措施，都能直接降低总体拥有成本（TCO），具有显著的经济价值。

##### 1.3.3 资源利用率最大化

在云计算、HPC 集群等多用户、多任务共享 GPU 资源的环境中，最大化 GPU 的利用率是提高投资回报率和运营效率的关键。然而，实际部署中 GPU 的平均利用率可能并不高，有时甚至低于 50%。性能调优，特别是动态的、自适应的调优策略，有助于根据实际负载情况更精细地匹配 GPU 的性能输出，在满足服务等级协议（SLA）的前提下服务更多用户或任务。

##### 1.3.4 传统 DVFS 的局限性

动态电压频率调整（DVFS）作为一种成熟的功耗管理技术，在 GPU 性能优化中扮演着核心角色。其基本原理是：当负载较低时降低电压和频率以显著减少动态功耗和静态泄漏功耗；当需要处理突发高计算负载时提升电压和频率以提供峰值性能；在给定功耗预算下寻找能实现最大性能的工作点。

然而，由于 GPU 应用的多样性和复杂性（图形渲染、HPC 模拟、AI 训练/推理等具有截然不同的计算特性和性能瓶颈），厂商默认的、基于准静态 V-F 曲线的 DVFS 策略往往难以达到理想效果。传统 DVFS 通常只会基于一条默认的 V-F 曲线，动态地依据一定负载判据和算法去寻求最佳工作点，却忽视了 **V-F 曲线本身的可优化性**——而这正是 NVOC 的关键切入点：**直接优化默认的 V-F 曲线本身，再配合 DVFS 策略进行调优，以获得最大程度的能效/性能提升**。

#### 1.4 文献综述与工程实践调研

计算机超频领域是一个比较小而精的研究方向，相关工程实践方面的文献资料相对较少。关键前期工作包括：

**AOA: Adaptive Overclocking Algorithm on CPU-GPU Heterogeneous Platforms**（CPU-GPU 异构平台的自适应超频算法）——该研究提出了一种针对 CPU-GPU 异构平台的自适应超频算法，旨在维持系统总能耗基本不变的前提下提升整体性能。其核心思想是通过动态协同调整 CPU 与 GPU 的频率及功耗分配以优化能效，引入了动态功率上限因子（k）和负载不均衡因子（W）两个关键参数。

实验结果：在较旧的服务器平台（Intel E5 2660 + Tesla K80）上，AOA 算法取得了最高 6.1% 的性能提升和约 4.4% 的节能效果。然而，在较新的消费级平台（Intel i9 + RTX 2080Ti）上，由于硬件本身对功耗更为敏感，该算法的性能提升有限（0.2%-0.6%），且未能展现出显著的功耗优化效果。

与本研究的核心区别：AOA 假设 CPU 与 GPU 总功耗恒定，通过动态优化功率分配来提升性能，这个假设与 NVOC 关注的应用场景不同。NVOC 主要聚焦于**直接优化 GPU 的 V-F 曲线本身**，在中高端桌面平台和服务器平台上实现硬件底层参数的改进。

---

### 2. 性能优化的理论基础

#### 2.1 信号完整性与传播延迟

GPU 芯片运行频率的物理上限，由数字电路的基本特性决定。

##### 2.1.1 CMOS 反相器模型

芯片内部逻辑电路的核心基本单元是 CMOS（互补式金属氧化物半导体）反相器。当输入信号发生翻转跳变（0→1 或 1→0）时，输出并不会瞬间切换，而需要经过一个由驱动晶体管电流能力和下游门电路负载电容共同决定的有限充放电时间——即**传播延迟**。

##### 2.1.2 频率-电压关系

更高的操作电压可以增大驱动晶体管的电流，从而加速晶体管开关速度（减小传播延迟），进而支持更高的工作频率。然而，这种关系并非线性：动态功耗的增长公式为 `C × V² × f`，其中电压 V 带来的功耗增长呈平方甚至更高次方关系。这意味着通过加压来换取频率提升的代价越来越大。

##### 2.1.3 信号完整性约束

当频率过高时，以下二次效应变得不可忽略：
- **信号反射**：传输线上阻抗不连续处产生的信号反射
- **串扰**：相邻信号线之间的电容耦合干扰
- **衰减**：长距离片上布线带来的电阻损耗

GPU 芯片内的**关键路径**（critical path）——即两个时序元件之间最长的组合逻辑路径——决定了该电压下频率的理论极限。简言之，**每个电压点存在一个由物理延迟决定的最高可用频率上限**，这构成了 V-F 曲线优化的物理基础。

#### 2.2 芯片功耗与温度特性

##### 2.2.1 静态功耗（漏电功耗）

静态功耗来源于晶体管即使在不翻转时也存在的漏电流：
- **亚阈值漏电**：晶体管名义上"关断"时流过的微弱电流
- **栅极漏电**：穿过栅氧化层的隧穿电流
- 漏电流随温度升高**呈指数增长**
- 在先进制程（7nm、5nm、4nm、3nm）中，漏电功耗占比越来越大

##### 2.2.2 动态功耗（开关功耗）

核心公式：`P_dynamic = α × C × V² × f`

其中：
- `α` = 活动因子（每个时钟周期内发生翻转的电路比例）
- `C` = 总负载电容
- `V` = 工作电压
- `f` = 开关频率

关键洞察：电压对功耗的影响极大（平方关系）。略微降压即可显著降低功耗——这正是**降压超频（Undervolting）**策略的物理基础。

##### 2.2.3 NVIDIA GPU 主要功能模块功耗与频率特性

| 模块 | 功耗特征 | 频率敏感性 |
|---|---|---|
| CUDA 核心 / 流式多处理器（SM） | 动态功耗的主要来源 | 高 |
| 显存控制器（MC） | 与显存频率和带宽强相关 | 中 |
| 显存颗粒（GDDR / HBM） | 较大的静态 + 动态功耗 | 中 |
| 光栅化单元（ROP） | 主要由渲染负载驱动 | 中 |
| 光线追踪核心（RT Core） | 高密度计算，功耗高 | 高 |
| Tensor Core | 矩阵乘加运算密集，功耗高 | 高 |
| L1 / L2 缓存 | 面积大，高温下漏电显著 | 低-中 |
| NVENC / NVDEC | 专用固定功能编解码器 | 低（固定功耗） |
| PCIe / NVLink 接口 | I/O 决定功耗 | 低-中 |

#### 2.3 芯片性能优化逻辑

V-F 曲线优化的核心矛盾是 **频率提升** 与 **功耗/热量增加** 之间的权衡关系：

```
性能 ↑ → 频率 ↑ → 电压需求 ↑ → 功耗 ↑↑ → 温度 ↑ → 漏电 ↑ → 稳定性 ↓
```

这形成了一个负反馈循环。由此衍生出两条优化路径：

1. **性能优先路径**：在给定散热/功耗约束内，最大化每个电压点的频率偏移，获取更多绝对性能
2. **能效优先路径（降压超频）**：在性能不降低的前提下，寻找每个频率点对应的最低可行电压，利用 V² 关系降低功耗

#### 2.4 GPU 超频模式与原理

##### 2.4.1 NVIDIA GPU 的动态电压-频率工作模式

从 Pascal（GTX 10 系）起，NVIDIA 引入 **GPU Boost 3.0** 技术。核心机制：

- 芯片内预置了一张 **V-F 曲线表**（VFP Table），定义了约 80 个电压点各自对应的默认核心频率
- 运行时，GPU 根据当前负载、功耗、温度等因素，在这条曲线上动态选取工作点运行
- **超频的本质：对该表中的每个电压点施加正的频率偏移（KilohertzDelta）**

出厂 V-F 曲线的保守性意味着每个电压点都存在可探索的超频空间：

```
默认频率 = f_default(voltage)
极限频率 = f_max(voltage)     ← 受物理延迟和信号完整性约束
稳定频率 = min(f_max, 受功耗/温度/电压墙约束的实际可达频率)

超频空间 = 稳定频率 - 默认频率
```

##### 2.4.2 处理器核心 Loadline 机制

Loadline（负载线）是 GPU 供电系统的核心特征：

| 类型 | 名称 | 描述 |
|---|---|---|
| DC Loadline（静态） | 直流负载线 | 稳态下的电压跌落，与负载电流成正比（V_actual = V_set - I_load × R_ll） |
| AC Loadline（动态） | 交流负载线 | 负载突变时的瞬态电压波动（过冲/下冲） |

**关键影响**：
- 高负载时，实际有效核心电压（V_actual）低于设定电压（V_set），即 Vdroop 现象
- 电压跌落过大可能导致频率不稳定或发生静默数据错误
- 主板/显卡的**负载线校准（LLC）**功能可调节此效应——提高 LLC 等级可减少压降，但会增加瞬态过冲风险
- 优化后的 V-F 曲线必须考虑最恶劣负载切换条件下的实际有效电压

##### 2.4.3 GPU 优化的限制与约束条件

GPU 性能优化同时受到以下多层次约束条件的制约：

| 约束类型 | 说明 | 触发后的影响 |
|---|---|---|
| **功耗墙（Power Limit）** | 芯片总功耗上限，由 vBIOS 或驱动设定 | GPU 自动降低电压和频率以限制功耗 |
| **温度墙（Thermal Limit）** | 芯片结温上限（通常 83°C-87°C） | 触发温度降频（Thermal Throttling），逐步降低频率 |
| **电压墙（Voltage Limit）** | 硬件允许的最高操作电压，由 vBIOS 锁定 | 限制 V-F 曲线上最大可设定频率 |
| **电流墙（Current Limit / CEP）** | 供电电路最大可承受电流 | 限制高负载下的持续运行能力 |
| **可靠性墙（Reliability）** | 长期高压/高温对芯片的加速老化效应（电迁移等） | 极限设置可能加速芯片性能退化 |
| **操作系统 TDR** | Windows 超时检测与恢复机制 | 检测到 GPU 无响应时重置驱动；50 系显卡可能蓝屏 |

实际操作中，高频高负载下最先触发的是**功耗墙或温度墙**，导致 GPU 实际工作点偏离曲线上设定的目标值。NVOC 的动态分辨率自适应调整机制正是为了解决这一问题而设计。

##### 2.4.4 V-F 曲线优化的底层依据

优化 V-F 曲线的可行性建立在以下三个事实之上：

1. **个体硅质差异**：同一型号 GPU 因制造工艺波动，每个电压点的稳定频率上限存在约 5%-15% 的个体间差异
2. **电压点独立性**：不同电压点的超频空间相对独立（由该电压域下的物理延迟特性决定），可逐个独立优化
3. **负载依赖性**：不同负载类型下的稳定性表现不同——计算密集型负载考验算术单元极限，访存密集型负载考验显存子系统极限

NVOC 的 autoscan 正是利用以上特性，通过逐电压点压力测试，探索每个个体 GPU 的稳定超频上限。

##### 2.4.5 性能优化的综合效益模型

优化收益可从两个维度综合衡量：

**性能提升**：
- 典型范围：4% - 15%（取决于 GPU 型号和具体负载类型）
- 部分 GPU 在特定负载下性能提升超过 10%
- 纯计算类任务（如 Hashcat NTLMv1）频率-性能缩放近乎线性
- 图形类任务（如《黑神话：悟空》Benchmark）在达到某饱和点后收益递减

**能效改善**：
- 性能不损失的前提下，功耗降低约 12% - 30%
- 部分 GPU 的单位性能功耗（能效比）改善超过 20%
- CPU 重负载类 3D 应用中，即使 GPU 降频也可能带来有效帧率提升（释放了更多热/功耗预算给 CPU）

**优化过程的时间成本**：
- 标准 autoscan：数小时（全电压点逐步扫描）
- 超快速扫描（Ultrafast）：约 1 小时（仅扫描 4 个关键"热点工作区间"后插值）

扫描耗时由以下因素决定：电压点数 × 每点压力测试时长 ×（二分搜索步数 + 涨落验证周期）。Relaxed Finite-Step Binary Approximation Algorithm 被设计用于在保证精度的前提下最小化此开销。

---

### 3. 从理论到 NVOC 实现

| 理论概念 | NVOC 中的具体实现 |
|---|---|
| V-F 曲线逐点优化 | `set vfp autoscan` — 基于 Relaxed Finite-Step Binary Approximation Algorithm |
| 约束条件处理 | 动态分辨率自适应调整、功耗墙/温度墙探测模块 |
| 稳定性验证 | cli-stressor 套件（CUDA/PyTorch、OpenCL、Rust cuBLAS），支持多精度旁路校验 |
| Loadline 效应补偿 | `fix_result` 后处理中的轻重载补偿修正（margin_bin） |
| 个体差异探索 | 断点续扫机制 + 主动频率涨落验证，保证结果可靠性 |
| 能效分析 | 优化前后能耗/性能对比与数据可视化导出 |
| 故障恢复 | 多级故障处理（软错误 → 应用崩溃 → 驱动崩溃 → 内核恐慌 → BSOD 自恢复） |

---

### 相关页面

- [[Autoscan-Workflow]] — V-F 曲线自动扫描完整流程
- [[GPU-Support-Matrix]] — GPU 世代兼容性与接口能力矩阵
- [[Stress-Testing]] — 压力测试工具详解
- [[Auto-Optimizer-Guide]] — CLI 命令参考手册
