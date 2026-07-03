# NVOC 工程成熟度审计 / Engineering Maturity Audit — 2026-07

> 审计范围:全 monorepo(Rust ×7 crate、Python ×3 前端/工具、CI/CD、发布、文档、安全治理)。
> 方法:静态走查 + 本地构建/测试验证 + CI 配置逐行审查。所有发现均附文件位置与证据。
>
> **English TL;DR** — Overall the repo is more mature than typical hobby tooling: path-filtered
> two-tier CI, a label-gated self-hosted GPU tier, hardened localhost HTTP surface in `srv`,
> lockfiles for both ecosystems, and a release tag guard. The highest-priority gaps found:
> (P0) the `ci:gpu` label persisted across fork pushes, letting later pushes run unreviewed
> code on self-hosted GPU runners; (P0) `nvoc-auto-optimizer` had 35 passing unit tests that
> CI never executed; (P1) `srv` has zero tests, panics (`unwrap`) throughout service startup,
> and its localhost control endpoint has no local-user authentication; (P1) no dependency-advisory
> auditing (cargo-audit / pip-audit) and no Dependabot version updates; (P1) the Linux systemd
> unit re-applies a V-F curve at every boot with no crash-loop guard. Fixes for the two P0 CI
> items plus supply-chain workflows ship in the same PR as this report; the rest are filed as issues.

## 总体评价

按成熟工程标准衡量,这个仓库**明显好于同类超频工具的平均水平**:

- CI 分两级(托管 CI + 自托管 GPU CI),按路径过滤、按组件拆 job,有 `ci-summary` 汇总门;
- GPU 写入类测试默认 `#[ignore]`、硬件门控,GPU CI 明确注释"永不在 CI 跑写入路径";
- `srv` 的 localhost HTTP 面做了 loopback 绑定、POST + `X-Requested-With` CSRF 防护、参数范围钳制(`srv/src/websrv.rs:8-19`);
- Rust/Python 双生态均有 lockfile,重复 crate 版本有专门审计 job;
- 有 `SECURITY.md`、release tag guard、runner 安全文档(`.github/RUNNERS.md`)。

短板集中在:**测试没有全部接入 CI、供应链审计缺失、srv 组件的健壮性、以及"开机自动应用超频"的故障恢复设计**。以下按紧急程度排列。

---

## P0 — 应立即处理

### P0-1 GPU CI:fork PR 打一次标签后,后续推送可在自托管 runner 上跑任意代码

- 位置:`.github/workflows/gpu-ci.yml`(`on.pull_request.types: [labeled, synchronize, reopened]` + guard 只检查标签存在)
- 问题:`ci:gpu` 标签一旦被维护者打上就**持续存在**;之后 fork 作者每次 `git push`(`synchronize` 事件)都会在自托管 Windows GPU runner 上执行其修改后的代码,无需任何复审。自托管 runner 上的任意代码执行意味着可窃取 runner 凭据、横向渗透 runner 所在网络、或直接对 GPU 执行写入。
- 修复(本 PR 已含):fork PR 仅在 `labeled` 事件当次运行——维护者每打一次标签即批准一个修订;同仓库分支 PR 保留推送自动重跑。
- 后续建议:在仓库设置中将 "Require approval for workflow runs from forks" 提到 **All outside collaborators**,双保险。

### P0-2 auto-optimizer 的 35 个单元测试从未在 CI 运行

- 位置:`.github/workflows/ci.yml` `rust-auto-optimizer` job(原先只有 Clippy + Build)
- 问题:autoscan/扫描策略是整个项目的核心安全逻辑(决定给 GPU 施加什么频率/电压),`auto-optimizer/src` 里有 35 个非 GPU 单元测试(scan_strategy、autoscan_config、scan_log 等),但 CI 只编译不测试。回归只能靠人工发现——#217 修的 autoscan-vfp panic 正是这类问题。
- 验证:本机 Linux `cargo test --package nvoc-auto-optimizer` → **35 passed, 0 failed**。
- 修复(本 PR 已含):在 windows job 中追加 `cargo test --all-targets --all-features`(硬件门控测试仍由 gpu-ci 单独跑)。

---

## P1 — 高优先级

### P1-1 供应链:无依赖漏洞审计、无版本更新机制、Actions 用可变 tag 固定

- 现状:无 `dependabot.yml`(此前只有 GitHub 自动的分组*安全*更新);CI 无 cargo-audit / pip-audit;所有第三方 Action 按 tag(`@v6`、`@v4`)而非 commit SHA 固定;`.github/actionlint.yaml` 存在但没有任何 job 执行 actionlint。
- 风险:RustSec/PyPA 通告发布后无人知晓;tag 可被上游重指(自托管 runner 场景下 Action 供应链攻击后果更重)。
- 修复(本 PR 已含):`dependabot.yml`(cargo / uv / github-actions / gitsubmodule)+ `supply-chain.yml`(cargo-audit、pip-audit、actionlint,每周定时 + lockfile 变更触发)。
- 后续建议:用 Dependabot 接管后,逐步把 Action 引用换成 SHA 固定(Dependabot 会维护 SHA 注释)。

### P1-2 srv:零测试 + 服务启动路径充满 `unwrap()`

- 位置:`srv/src/bin/nvoc_service.rs:90,102,106,133,151,184,208,210,211` 等(共 24 处 `unwrap/expect`);`srv/` 下 `#[test]` 数量为 **0**。
- 问题:以 SYSTEM 身份运行的 Windows 服务在 NVML 初始化失败、日志目录不可写、SCM 注册失败等完全可预期的环境问题下会直接 panic,而不是向 SCM 报告失败原因;服务崩溃后依赖 `service_failure_actions` 重启,可能形成无日志的崩溃循环。`websrv.rs` 的 `percent_decode`/`parse_query`/范围校验逻辑本身高度可测,却没有测试。
- 建议:启动路径改为 `Result` + SCM 错误上报;为 `websrv.rs` 补纯逻辑单元测试并纳入 CI(`cargo test -p nvoc-srv` 目前在 CI 也未运行)。

### P1-3 srv HTTP 控制端点缺少本机权限边界

- 位置:`srv/src/websrv.rs:104`(`127.0.0.1:14514`,无认证)
- 问题:loopback + CSRF 防护挡住了浏览器和外网,但**同机任意非特权用户/进程**都可以 `POST /oc_global` 命令 SYSTEM 服务写超频参数(±2 GHz 范围内)。在多用户机器 / 被低权限恶意软件感染的机器上,这是一次免费的权限跨越(低权限 → 硬件状态写入)。`SECURITY.md` 明确把"权限边界错误"列入安全范围,但 srv README/文档未描述该端点的安全模型。
- 建议(任选其一,按成本排序):① 启动时生成随机 token 写入仅管理员可读的文件,请求需带 `Authorization`;② 改用支持 ACL 的命名管道;③ 至少在文档中明示"本端点信任所有本机进程"。

### P1-4 systemd 开机自动导入 V-F 曲线,无崩溃回环保护

- 位置:`auto-optimizer/systemd/nvoc-vfp.service`(`ExecStart=... import-vfp /srv/nvoc/vfp.csv`)
- 问题:曲线略微不稳时,典型故障模式是"开机 → 应用曲线 → 桌面负载触发崩溃/重启 → 再次开机自动应用",用户可能需要进恢复模式才能解开。成熟超频工具(如 Afterburner 的 startup 延迟应用)都有"上次启动未干净关机则跳过应用"的保护。
- 建议:`ExecStart` 前检查/写入 marker 文件:启动时若发现上次 marker 未被干净清除则跳过导入并告警;干净关机(`ExecStop`)时清除 marker。另:`systemd-udev-settle.service` 已被 systemd 官方弃用,建议改用 `ConditionPathExists=/dev/nvidiactl` + udev 规则或重试循环(现有 `ExecStartPre` 轮询已可承担等待职责,可直接去掉 settle 依赖)。

---

## P2 — 中优先级

### P2-1 SECURITY.md 没有可操作的报告渠道

"请私下向维护者报告"没有给出任何联系方式。建议开启 GitHub **Private Vulnerability Reporting** 并在 SECURITY.md 链接 `Security → Advisories → Report a vulnerability`。

### P2-2 README 子模块指引与固定 SHA 冲突

`README.md` Quick Start 让用户 `cd nvapi-rs && git checkout v0.2.x`,这会把子模块从超级仓库固定的 commit(`954021…`)挪到分支尖端——用户构建的代码与 CI 验证的代码可能不一致。`git submodule update --init` 已经检出正确 commit,该步骤应删除。另外 `.gitmodules` 的相对 URL(`../nvapi-rs.git`)会让 fork 用户(其账号下无 nvapi-rs 兄弟仓库)克隆失败,建议改为绝对 URL。

### P2-3 死配置:`auto-optimizer/.github/workflows/rust.yml`

GitHub Actions 只识别仓库根目录的 `.github/workflows/`,该嵌套 workflow 永远不会运行,徒增误导,应删除或移出。

### P2-4 Python 端异常吞噬面偏大

`gui/src` + `tui/nvoc_tui` 共 52 处 `except:` / `except Exception:`。GUI 侧围绕超频写入的静默失败尤其危险(用户以为应用成功)。建议:分批收敛 + 在 ruff 配置启用 `E722`(bare except)与 `BLE001`(blind except)并对存量用 `# noqa` 显式豁免,防止新增。GUI 内存泄漏已有 #190 跟踪,broad-except 收敛可与 #185 的 GUI 重构逐 tab 同步做。

### P2-5 Rust 生产路径 `unwrap` 缺少 lint 门禁

`cli/src` 32 处、`auto-optimizer/src` 33 处、`srv/src` 24 处(含测试代码)。`core` 只有 2 处,证明团队做得到。建议在写路径 crate(core/srv/auto-optimizer)启用 `#![warn(clippy::unwrap_used, clippy::expect_used)]` 并在 CI `-D warnings` 下渐进收敛(测试模块豁免)。

### P2-6 requirements.txt 双源维护

`gui/requirements.txt` 与 `gui/pyproject.toml` 靠注释约定"手工保持同步"(`cli-stressor-opencl` 同)。建议 CI 加一个比对步骤,或用 `uv export` 生成 requirements.txt,消除漂移可能。

---

## P3 — 低优先级 / 观察项

| 项 | 说明 |
|---|---|
| 无发布产物流水线 | 只有 tag guard,没有构建/签名/checksum 的 release workflow;用户全部从源码构建。鉴于产物是"能写 GPU 的二进制",发布流水线应包含 SHA256SUMS 与构建溯源(SLSA provenance / attestations)。建议在决定发布节奏后再上(见 issue)。 |
| 无 CHANGELOG / 版本纪律 | 所有 crate 停在 0.1.0;发布前需要版本与变更记录约定。 |
| 无 CODEOWNERS / issue & PR 模板 | 贡献者规模扩大前补齐即可。 |
| README 仓库布局节 | 只列了 5 个组件,与上方 10 组件的 canonical 表不一致。 |
| 无覆盖率度量 | 可选 `cargo llvm-cov` + pytest `--cov`,先有数字再谈目标。 |
| GUI/TUI 解析器重复 | 已由 #185 跟踪(共享 auto-optimizer stdout 解析)。 |

## 做得好的(维持现状)

- GPU 写入测试的三层门控(`#[ignore]` → gpu-ci 标签 → 写入类仅人工监督 bench)设计清晰,`RUNNERS.md` 甚至考虑了 TDR 注册表与带外重启;
- `websrv.rs` 的 CSRF/范围钳制注释详尽,`GPU_INDEX_MAX`、OC delta 边界都有依据说明;
- 重复 crate 版本审计 job(`rust-crate-versions`)是超出平均水平的供应链意识;
- 文档双语且指定了 canonical 来源,避免了常见的双语漂移。

---

*审计执行:2026-07-03。本报告与 P0 修复、供应链 workflow 同 PR 提交;P1-P3 项在 issue 中逐项跟踪。*
