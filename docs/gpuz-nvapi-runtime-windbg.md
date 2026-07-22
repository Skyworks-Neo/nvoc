# 动态定位 GPU-Z 的分路功耗/电压来源（WinDbg 完整流程）

## 目标
GPU-Z 传感器页显示 Board Power Draw / GPU Chip Power Draw / MVDDC Power Draw /
PWR_SRC Power Draw / PWR_SRC Voltage / 16-Pin Power / 16-Pin Voltage 等分路读数，
这些值**随负载实时跳动**。静态分析无法确定它们的来源，必须动态看 GPU-Z 到底调用了什么。
本流程用 WinDbg hook `nvapi_QueryInterface`，记录 GPU-Z 解析/调用的 ID，从而锁定来源。

## 关键背景（静态分析已确认的事实，避免重复踩坑）

1. **唯一钩点**：`nvapi_QueryInterface(uint32 id)` —— 所有 nvapi*.dll 都导出该符号（32/64 位同名）。
   GPU-Z 通过它把 32 位 ID 解析成函数指针，再调用对应 handler。
   - 64 位实现：`nvapi64_impl.dll!nvapi_QueryInterface`（基址 `0x180000000`，VA `0x180107720`）。
   - 真正的分发表 `off_1804DE000`：每条 16 字节 `[u64 func_ptr][u64 id]`，共 **2337** 条；
     其中 **1487 条** 不在我们 `nvid.rs`（857 条）里（见 `nvapi64_impl_qi_table.txt` / `unknown_ids.txt`）。

2. **GPU-Z 进程位数**：你观察到 GPU-Z 用 `nvapi.dll` / `nvapi_impl.dll` / `nvppe.dll`（均 x86），不用 `nvapi64*`。
   ⇒ **GPU-Z 是 32 位 (WoW64) 进程**。下面按 **WOW64** 给主流程，附 64 位差异。

3. **已实测排除的陷阱（重要）**：
   - 候选 ID `0x7457cab5`（handler `0x180238CC0`）虽然结构与已知功耗 handler 同族（同 72B/v1 版本魔数、
     同 `0x60B30` RM 缓冲、同 RM 分发器），但**实测不是实时读**：返回的 32 字节在重复读取和负载变化下
     完全不变（即使管理员权限），且非管理员返回 `NVAPI_INVALID_USER_PRIVILEGE`。结论：它是**确定性、
     特权、非实时的 blob**（capability/key/descriptor），**不是** GPU-Z 的分路实时读数。已记入
     `nvid.rs` 的 `Unknown_7457CAB5`。
   - 教训：**结构相似 ≠ 语义相同**。25 个候选（`gpu_control_rm_unknown_ids.txt`）里任何静态挑出的，
     都必须用"负载下字节是否变化"动态验证，不能凭相似性直接封装。

4. **两条 RM 通道（解释为何某些 ID 需管理员）**：
   - 普通 `\\.\NvDevice`：公开 NVAPI 读取（功耗拓扑 `0x20880B33`、电压轨等）走这条，普通用户可读，明文。
   - 管理 `\\.\NvAdminDevice`（`sub_180389CA0`，`CreateFileA`+`DeviceIoControl`）：需管理员权限。
   - 驱动内核侧也会对特定 RM 子命令做特权校验（即使走普通通道），返回 `NVAPI_INVALID_USER_PRIVILEGE`。
   - **GPU-Z 之所以能读特权路：它以管理员权限运行**。

---

## 方法总览（按推荐度）

- **方法 A（最推荐）**：日志断点 hook `nvapi_QueryInterface`，不暂停只记录每次解析的 `(id, 返回的函数指针)`。
  跑几秒得到 GPU-Z 解析的全部 ID 清单，和候选表求交 → 锁定实际使用的 ID。
- **方法 B**：对方法 A 锁定的 handler 入口下断点，dump 入参结构体，对照 GPU-Z 界面变化（加载时哪路上升）
  反推字段语义。
- **方法 C（最省事，零脚本）**：用 API Monitor 或 Frida 替代手敲 WinDbg（见末尾）。

---

## 完整流程（方法 A，WOW64 / 32 位 GPU-Z）

### 0. 准备
- 装 **WinDbg（Classic 或 WinDbg Preview）** + Debugging Tools for Windows。
- **以管理员权限启动 WinDbg**（GPU-Z 进程若也需管理员读特权路，附加方也需管理员才能完整看到）。
- 先让 GPU-Z 正常运行、停在传感器页（能看到那几路数值在跳）。

### 1. 附加到 GPU-Z
```
File → Attach to a Process
  ⚠️ 不要勾 "Noninvasive"（要下断点必须 invasive）
  → 选 GPU-Z 进程（如 gpuz.exe / GPU-Z.2.x.exe）
```
或命令行（管理员）：
```
windbg -pn gpuz.exe        # 进程名按你实际 exe
```
> 32 位进程下，WinDbg 的 32 位引擎接管；提示符下 `.effmach` 应显示 `Machine type: x86`。
> 若不是：`.load wow64exts` 然后 `.effmach x86`。

### 2. 解析符号 / 模块基址
```
.reload /f nvapi.dll          ; GPU-Z 实际加载的是 nvapi.dll（32 位）
lm m nvapi*                   ; 列出已加载的 nvapi 相关模块及基址
```
确认 `nvapi.dll`（或 `nvapi_impl.dll`）的基址。设断点（用模块名+导出名最稳）：
```
bp nvapi!nvapi_QueryInterface
```
> 若报 "not code" / 找不到符号，换 `bm nvapi!nvapi_QueryInterface`，或先 `u nvapi!nvapi_QueryInterface l8`
> 看入口，用绝对地址 `bp <基址+偏移>`。

### 3. 改成日志断点（核心：不暂停，只记录 id 和返回的 handler 指针）
先确认 ABI（`nvapi_QueryInterface(uint32 id)` 的 id 在哪）：
```
u nvapi!nvapi_QueryInterface l8
```
- 若开头是 `mov eax/ecx, [esp+4]` 或 `mov ecx, ...` → id 在寄存器/栈，按下面对应版本。

**cdecl 版（id 在 `[esp+4]`，返回值在 `@eax`）**：
```
.logopen C:\gpuz_qi.log
bp nvapi!nvapi_QueryInterface ".printf \"tid=%x id=0x%x\\n\", @tid, poi(@esp+4); gu; .printf \"  ->handler=0x%p\\n\", @eax; g"
g
```
**fastcall 版（id 在 `@ecx`，返回值在 `@eax`）**：
```
.logopen C:\gpuz_qi.log
bp nvapi!nvapi_QueryInterface ".printf \"tid=%x id=0x%x\\n\", @tid, @ecx; gu; .printf \"  ->handler=0x%p\\n\", @eax; g"
```
说明：
- `gu` = step out（走到 QueryInterface 返回后），此时 `@eax` 是返回的 handler 函数指针。
- `g` = 继续运行不暂停。GPU-Z 几乎不被卡，日志里刷出所有被解析的 ID。

### 4. 收集日志（让传感器刷新几秒）
```
g                            ; 放行
（等 3~5 秒，让 GPU-Z 刷新传感器若干次）
Ctrl+Break                  ; 暂停
.logclose
```
日志样例：
```
tid=1234 id=0x7457cab5
  ->handler=0x73a238c0
tid=1234 id=0xedcf624e
  ->handler=0x73a235f10
...
```
`handler` 的低位（`…238c0` / `…235f10`）可直接和候选表（`gpu_control_rm_unknown_ids.txt` 的 `func_va` 低位）比对。

### 5. 和候选表求交（锁定 GPU-Z 实际用的 ID）
PowerShell（离开 WinDbg）：
```powershell
$gpuz = Select-String -Path C:\gpuz_qi.log 'id=0x([0-9a-f]+)' |
        ForEach-Object { ('0x' + ($_.Matches[0].Value -replace 'id=0x','')) } |
        Sort-Object -Unique
$cand = Get-Content D:\git-repo\nvoc\gpu_control_rm_unknown_ids.txt |
        Where-Object { $_ -notmatch '^#' } |
        ForEach-Object { ($_ -split '\t')[0] }
Compare-Object $cand $gpuz -IncludeEqual -ExcludeDifferent
```
**校验**：日志里应出现 `0xedcf624e`（已知功耗拓扑）——说明钩点正确。

### 6. 关键判别：哪些 ID 是"实时分路读"
对交集中的每个 ID，判断它是不是 GPU-Z 的分路实时读（而非像 `0x7457cab5` 那样的静态 blob）：
- **看调用频率**：日志里每秒（传感器刷新周期）都被解析的 ID → 实时读候选。
- **负载对照**：在 GPU 满载 vs 空载两种状态下各跑一次方法 A，**只出现在/频繁于刷新窗口、且 handler
  返回值会变**的 ID 才是实时读。
- 仅出现一次、或返回值不变的 ID → 静态描述，排除（同 `0x7457cab5` 教训）。

---

## 方法 B：dump handler 的结构体，确定通道语义
对方法 A 锁定的实时读 ID（设为 `0xXXXXXXXX`，handler 在 `nvapi_impl.dll` 基址 + 偏移）：
```
? nvapi_impl + 0x<偏移>           ; 算运行时绝对地址
bp <绝对地址>
g                                  ; 命中
; handler 原型通常是 int(GpuHandle a1, Struct* a2)。32 位：a2 在 [esp+8]
r @esp
dd poi(@esp+8) l20                 ; dump 结构体前 0x80 字节；首 DWORD = 版本魔数
; 单步过 RM 调用后，看 a2 被写回的字段
```
- 结构体首 DWORD = 版本魔数（高 16=version，低 16=size）；用静态表（`65608`/`67236`/`73764`…）核对结构体类型。
- 遍历 `a2` 的 count + 每通道记录，**对照 GPU-Z 界面**（给 GPU 加载，看哪一路数值上升 = 当前 handler 的输出）。
- 把每个 handler 的"版本魔数 + 字段布局"记下 → 这就是封装所需的精确结构体。

---

## 64 位 GPU-Z 的差异（若你的 GPU-Z 其实是 64 位）
- 附加后 `.effmach` 显示 `x64`；模块换 `nvapi64_impl.dll`。
- `nvapi_QueryInterface(uint32 id)` 为 x64 fastcall：id 在 `@ecx`，返回值 `@rax`：
  ```
  .logopen C:\gpuz_qi64.log
  bp nvapi64_impl!nvapi_QueryInterface ".printf \"tid=%x id=0x%x\\n\", @tid, @ecx; gu; .printf \"  ->handler=0x%p\\n\", @rax; g"
  g
  ```
- 已知 VA（基址 `0x180000000`）：`nvapi_QueryInterface=0x180107720`、功耗 `0xedcf624e→0x180235F10`、
  静态 blob `0x7457cab5→0x180238CC0`（已排除），其余见 `gpu_control_rm_unknown_ids.txt`。

---

## 替代方案（比手敲 WinDbg 省事）

### API Monitor（图形化，零脚本）
- 对 `nvapi.dll` 的 `nvapi_QueryInterface` 打钩，图形化列出每次调用的 `id` 参数与返回值。

### Frida（一段 JS，自动适配 32/64 位）
```js
const m = Process.getModuleByName('nvapi.dll');          // 或 nvapi64_impl.dll
const qi = m.findExportByName('nvapi_QueryInterface');
const seen = new Set();
Interceptor.attach(qi, {
  onEnter(a){ this.id = a[0].toUInt32(); },
  onLeave(r){
    if (seen.has(this.id)) return;        // 只打印每个 ID 第一次
    seen.add(this.id);
    send({ id: '0x' + this.id.toString(16), ptr: r.toString() });
  }
});
```
```
frida -p <gpuz pid> -l hook.js
```
Frida 对 32/64 位自动适配，比 WinDbg 断点流畅。**推荐先用 Frida 快速拿 ID 清单，再用 WinDbg 做结构体 dump。**

---

## 成功判据 & 下一步
- 方法 A 日志出现 `0xedcf624e` ⇒ 钩点正确（校验通过）。
- 交集中**每秒调用、且负载下返回值变化**的 ID ⇒ 那就是 GPU-Z 的分路实时功耗/电压来源。
- 用方法 B 拿到结构体布局后：
  1. 在 `nvapi-rs/sys/src/nvid.rs` 把对应 `Unknown_XXXXXXXX` 改名为真实语义（如 `NvAPI_GPU_GetPowerRails`）。
  2. 在 `nvapi-rs/sys/src/gpu/power.rs` 按布局定义 `NV_..._V1` 结构体（`nvversion!` 的 size 断言把关）。
  3. 按 `nvid.rs → power.rs → clock.rs(RawConversion) → src/gpu.rs(PhysicalGpu) → hi/src/gpu.rs(GpuStatus)`
     一级级封装（参照 P2/P3 已有模式）。
  4. 本机 `cargo test -- --ignored` 验证负载下数值变化，确认语义。
- 若所有动态命中的 ID 都不是明文实时读（都像 `0x7457cab5` 那样静态/特权）→ GPU-Z 的分路值可能来自
  **NVML**（非 NVAPI）或自有换算，转向查 NVML 的 per-rail 接口（`nvmlDeviceGet*`）。

## 产物归档
- `C:\gpuz_qi.log`（或 `.logclose` 后的文件）：原始 ID 解析日志。
- `nvapi64_impl_qi_table.txt` / `unknown_ids.txt` / `gpu_control_rm_unknown_ids.txt`：静态 ID 表（已生成）。
- 把"ID ↔ Board/Chip/MVDDC/PWR_SRC/16-Pin"映射回填 `nvid.rs` 并按链路封装。
