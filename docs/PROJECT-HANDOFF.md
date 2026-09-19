# KM003C 工作台移交文档

更新日期：2026-09-19。源码基线：`7c3e248`；本次工作区修订已提交到 `main`，并发布为 `v0.1.0-20260919-2`。

## 接手结论

项目已经具备实时测量、会话录制与恢复、CSV/Parquet 导入导出、PD 报文分析、中英文界面和两套皮肤。当前优先级是解决用户反馈的操作与稳定性问题，尤其是设备离线记录、重新测量、退出报错和录制时设置页响应。

不要把旧聊天中的实施计划全部视为已完成。以当前代码、回归测试和真实运行记录为准。Obsidian 正式库的 2026-09-06 工作流审计也明确要求：安装、架构和签名通过不能等同 USB/PD 真机验证通过。

## 仓库与版本

| 项目 | 当前值 |
| --- | --- |
| 本机仓库 | `/Users/xueweixun/Documents/工具类/km003c-workbench-macos` |
| 用户仓库 | https://github.com/weixunkkkkk/km003c-workbench-macos |
| 工作分支 | `macos-workbench`，跟踪 `github/main` |
| 用户远程 | `github` |
| 上游远程 | `origin` → `okhsunrog/km003c-rs` |
| App 版本 | `0.1.0 (3)` |
| Cargo workspace 版本 | `0.3.0`，与 App 版本不同 |
| Bundle ID | `com.weixun.km003cworkbench` |
| 系统与架构 | macOS 11+，arm64 / x86_64 Universal |
| 可执行文件名 | `KM003CWorkbench` |

重要提交：`4d8694c` 修复后台逻辑调度并更新设置与皮肤；`f6ed197` 修复发布标签校验及二进制打包路径；`49f511c` 完成离线记录选择、记录管理和来源导出修订；`7c3e248` 同步发布规则和说明。

GitHub 手动验证运行 [34184683051](https://github.com/weixunkkkkk/km003c-workbench-macos/actions/runs/34184683051) 已查询确认成功：版本校验、Linux、Windows、macOS Universal 构建全部通过。该运行是 `workflow_dispatch`，发布步骤按设计跳过。

已公开发行：[v0.1.0-20260908](https://github.com/weixunkkkkk/km003c-workbench-macos/releases/tag/v0.1.0-20260908)。附件为 `KM003C-Workbench-v0.1.0-macOS-universal.dmg` 与校验文件，DMG SHA-256：

```text
d60b87cac14e246445c6c7f630e74fee252379a5ad53e871b6760750e60a1d87
```

最新公开发行：[v0.1.0-20260919-2](https://github.com/weixunkkkkk/km003c-workbench-macos/releases/tag/v0.1.0-20260919-2)。当前 DMG 位于 `dist/offline-selection-20260919/`，SHA-256：

```text
5505f1f6f8606a952b6c2223316a1c39b32d174f4b545eab28d1d68e74dd077c
```

安装位置是 `/Applications/KM003C 工作台.app`。2026-09-19 已重新安装并启动验收，确认“更多 → 重新测量”入口存在。发行包使用 ad-hoc 签名，尚未 Apple 公证。

## 2026-09-19 用户反馈与处理状态

| 反馈 | 定位与当前处理 | 后续验收 |
| --- | --- | --- |
| On-device 无法加载 | 原选项仅在已下载数据后可用；工作区修订允许已连接设备进入“数据与设备”，触发目录读取，并默认展开离线记录区 | 真机刷新目录、选择、下载、显示；录制期间的互斥提示仍需完善 |
| 只有 Resume，不能开始新测量 | 增加“更多 → 重新测量 / New measurement”；旧会话封口留在恢复目录，等待写盘完成后重置为 Idle | 录制、暂停、导入、断线状态分别检查；旧数据必须可恢复 |
| 退出出现 unexpected close | 尚未取得对应崩溃报告，未宣称修复 | 保存完整报错、时间及 DiagnosticReports；区分 OS 崩溃与恢复清单的 Interrupted 标签 |
| Active protocol 为 unconfirmed | 当前实现要求捕获 Source Capabilities → Request → Accept → PS_RDY；启动晚于协商时可能缺报文 | 从接线前启动采集重测；保留报文日志，不能按 VBUS 猜协议 |
| Recording 时设置迟钝 | 恢复页原先每帧枚举磁盘并读取 manifest；工作区修订为后台线程扫描、五秒缓存，同一时刻只运行一次扫描 | 大量恢复记录、1000 SPS 下测响应；仍应评估图表计算开销 |

本表中的“工作区修订”已包含在 2026-09-19 本地安装包和 `v0.1.0-20260919-2` Release 中。继续工作时先看 `git status`、最新提交和本次测试结果。

本次验证：格式检查、`git diff --check`、GUI crate 全部测试（99 通过，3 项设备/外部文件相关测试跳过）及 `cargo clippy --workspace --all-targets --locked -- -D warnings` 通过。新增离线目录选择、来源导出、原格式副本和窗口布局回归测试。Universal Release、DMG 挂载与完整性、签名和远端下载校验通过；已检查本地启动待机画面、记录管理和重新测量菜单。当前未发现 KM003C，真机尚未验证；未找到用户所述退出错误对应的 panic 或 KM003C 崩溃报告。

本地交付记录（2026-09-19）：

- DMG：`dist/offline-selection-20260919/KM003C-Workbench-v0.1.0-macOS-universal.dmg`
- DMG SHA-256：`5505f1f6f8606a952b6c2223316a1c39b32d174f4b545eab28d1d68e74dd077c`
- 已安装 executable SHA-256：`177114ad6159e779ddeb1ea5801e7ba74a4da52ca3ad9721dfdc5b664035723e`
- 旧版备份：`release/rollback/install-20260919/KM003C 工作台.app`
- 安装前通过实际窗口确认没有录制，正常退出；未修改用户配置和 Pending 恢复数据。

## 代码地图

| 路径 | 职责 |
| --- | --- |
| `km003c-lib/` | USB 通信、设备协议、PD 底层解码 |
| `km003c-cli/` | 命令行工具 |
| `km003c-egui/src/main.rs` | App 状态、USB 任务、录制协调、图表、设置、快捷键；仍是较大文件 |
| `measurement.rs` | 设备序号连续性、缺失统计、梯形积分、有符号与绝对累计量 |
| `recording.rs` | 后台写盘、23 列数据、写盘事件与汇总 |
| `recording_session.rs` | manifest、分段合并、北京时间元数据、恢复发现 |
| `recording_import.rs` | CSV/Parquet 结构校验与异步导入 |
| `offline_view.rs` / `offline_export.rs` | 设备离线数据转换与导出 |
| `pd_decoder.rs` / `pd_connection.rs` / `pd_trace_view.rs` | 当前合同、连接状态及固件 trace 展示 |
| `preferences.rs` / `i18n.rs` / `theme.rs` | 持久化偏好、中英文文案及动态主题 |
| `sleep_assertion.rs` | 录制期间通过 `caffeinate -i` 防止空闲睡眠 |
| `assets/` | 嵌入的应用图标、日系壁纸等素材 |

运行路径：USB 任务 → `UsbMessage` → `App::logic` → `update_runtime` → `process_messages` → 积分及录制提交 → 后台写盘事件。`App::ui` 调用 `show_workbench` 绘制。

**后台处理必须保留在 `App::logic`。** 不得把采样消费、封口轮询、自动控制、重连或下一轮调度移回绘制函数。当前调度为采样时 16ms、空闲 100ms、积压立即重调度，每轮最多处理 64 条 USB 消息。

## 状态与数据约定

`RecordingPhase`：Idle、Recording、Paused、Finalizing、Saved、Interrupted、WaitingForReconnect、Recovering。设备采样与文件录制是两个独立状态：暂停文件录制时仍可更新实时读数。

`PauseReason` 区分手动、自动阈值与 USB 中断。只有自动暂停可以按阈值自动继续。意外断线续录需要匹配原设备序列号。

每个录制会话包含 `manifest.json` 和多个段；30 秒或 32,768 点先到者触发封口。保存后生成同名 `.km003c.json`，Parquet 同时携带元数据。最终数据与元数据校验完成前保留恢复副本。

CSV/Parquet 固定 23 列：

```text
elapsed_us, sample_index, sequence, marker, sample_rate_hz,
missing_samples, gap_duration_us, interpolated,
cumulative_missing_samples, cumulative_interpolated_duration_us,
discarded_sequence_samples, cumulative_discarded_sequence_samples,
vbus_uv, ibus_ua, power_uw, charge_uah, energy_uwh,
charge_throughput_uah, energy_throughput_uwh,
cc1_uv, cc2_uv, dp_uv, dm_uv
```

累计容量和能量使用绝对值积分，净量保留方向。图表滤波只作用于展示，不改原始点、统计和导出。旧文件缺少墙钟元数据时显示时间未知，不能使用文件修改时间代替。

偏好采用 `serde(default)` 兼容旧版。演示与正式模式隔离；测试可使用 `KM003C_STORAGE_ROOT`。正式数据目录：

```text
~/Library/Application Support/com.weixun.km003cworkbench/
  logs/
  Recordings/Pending/
```

## 验证证据与边界

详细历史证据见 [后台录制修复与验收](BACKGROUND-RECORDING-FIX.md)。该记录中的格式检查、完整测试和严格 Clippy 是此前版本结果，不自动适用于本次未提交修订。

- 无 UI 绘制回归：61 秒输入、三个 CSV 段、123 点、23 列，检查自动控制与无重复累计。
- 演示：最小化及隐藏超过两分钟，后台段持续更新；保存重导入 16087 点与元数据一致。
- 真机 50 SPS：6 分 42.52 秒，20105 点，之后 USB `e00002c0` 且设备从系统列表消失；进入等待重连并成功保存。缺失 21 点，最大间隔 440ms，完整度 99.895657%。
- 尚未完整验收：真机十分钟连续后台、2/10/1000 SPS、锁屏十分钟、主动睡眠唤醒、断线后续录、完整 PD 协商链路。

本地历史证据在 `release/verification/background-fix-20260907`，回滚备份在 `release/rollback/background-fix-20260907`；这些是忽略的本地文件，新克隆不会包含，使用前确认存在。

## 构建、发布与安装

从仓库根目录执行：

```bash
cargo fmt --all -- --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
python3 -m unittest discover -s Scripts -p test_release_version.py
./Scripts/build_release.sh
```

`build_release.sh` 依次执行 `package_app.sh`、`make_dmg.sh`、`verify_dmg.sh`。构建需要 Rust（当前 Cargo 声明最低 1.97）、Xcode 命令行工具以及两个 macOS Rust target。产物在 `dist/`。

发布标签从 `Distribution/Info.plist` 的 App 版本校验，支持 `v0.1.0`、`v0.1.0-YYYYMMDD` 和同日构建序号 `v0.1.0-YYYYMMDD-N`。不能拿 workspace 的 `0.3.0` 校验应用标签。当前 DMG 名称仍固定含 `v0.1.0`；以后升级 App 版本时检查 plist、打包脚本和文件名的一致性。

GitHub workflow 构建的是跨平台二进制压缩包，标签触发时生成草稿 Release；手动触发只验证、不发布。macOS App/DMG 由本地脚本生成，不能将 CI 的 tar.gz 当作 DMG。

安装前检查应用是否正在录制，先保存或安全封口、正常退出，再备份并替换 `/Applications/KM003C 工作台.app`。保留用户配置和 Pending 目录。安装后核对 binary hash、Universal 架构、签名、实际启动；不要仅用版本号判断新旧，因为此前多次修订沿用 `0.1.0 (1)`。

## 接手顺序

### 后续修订：短记录末间隔（2026-09-19）

- 真机 A01.d：18 点，间隔 10 秒，摘要时长 170 秒，偏移 29952；两次只读快照一致。末点为 `-98173 µAh / -896105 µWh`，摘要为 `-100581 µAh / -918378 µWh`。额外检查紧邻 16 字节均为 FF，不作为采样导入。
- 仅 10 ppm 的兼容策略不足。新增末间隔检查：容量和能量差额必须同时沿最后一步的方向、各自不超过最后一步增量，且对应间隔比例相差不超过 5%。这是保守一致性策略，不宣称已完整逆向固件时序；不删除原有长度和明显错配检查。
- 不补点、不修改原始行；摘要和末点分别保留，UI 明示差额与导出原始采样的口径。下载失败会清除过期离线视图及导出来源，防止导出上一次记录。
- 真机快照已加入回归；离线 8 项测试通过，GUI 93 项通过、3 项跳过，严格 Clippy 与格式检查通过。只读诊断入口：`cargo run -p km003c-lib --example offline_snapshot`，使用前正常退出其他 KM003C 客户端。
- 已安装本地并完成真实 GUI“刷新目录 → 下载 A01 → 导出 Parquet → 再导入”：18 点、170 秒、末点容量 `-98173 µAh`、能量 `-896105 µWh` 对应显示一致。文件位于 `release/KM003C-A01-离线修复验证-20260919.parquet`。本次验证针对当前唯一记录，不证明已删除的历史五条记录均可读取。
- 最新 Universal DMG SHA-256：`e391571a7992aa89bc4274953709da749c6ec4e318ba0dcd24ae7158ca924204`；安装副本逐文件一致性、签名和 DMG 挂载校验通过。回滚：`release/rollback/offline-tail-20260919/KM003C 工作台.app`。下方两次更早构建的 hash 仅用于历史追溯。

### 后续修订：离线下载末点校验与工具栏入口

以下改动已重新打包并安装（2026-09-19），替代上述较早的本地 DMG：

- 当前 DMG SHA-256：`c4e71ec16cd9e3a84a2b276d5729d8467bbfcb852f23b43b0d027bff7f560f62`。
- Universal 双架构、DMG 验证、签名验证和安装副本逐文件一致性通过；旧版备份位于 `release/rollback/offline-fix-20260919/KM003C 工作台.app`。
- 安装后已启动并连接设备，实际窗口显示“恢复视图 → 离线导入 → 更多”。随后观察到离线图表已加载 371 点、时长 01:01:40.0、累计容量约 5.1818 Ah、累计能量约 100.5795 Wh；未由代理操作下载，验收范围为实际界面观察。

- 用户实测报错：末点 `5181760 µAh / 100579515 µWh`，摘要 `5181772 µAh / 100579761 µWh`。旧代码使用严格相等导致下载失败。
- `offline.rs` 将累计值一致性校验改为最多 10 ppm（0.001%）或 1 个整数单位的容差；分别检查容量和能量，拒绝异号和超限差异。长度检查保留。该阈值是兼容策略，并非已证实的固件时序规则。
- 不改写元数据或原始采样；容差内差异写入日志，下载状态提示显示两项差值。新增正负向实测数值、边界与溢出测试。
- 主工具栏“恢复视图”后增加“离线导入 / On-device import”，进入设备记录页面并请求目录；紧凑模式仍保留入口。
- 合成末点回归通过；实际界面已观察到离线记录加载成功，尚未逐字段比对完整 USB 下载内容及导出结果。

1. 阅读本文件、`git status` 和 `git diff`，保留工作区未提交改动。
2. 优先验证本次重新测量的状态转换与恢复数据保留；再检查离线目录真实 USB 请求。
3. 收集退出报错的确切日志和 macOS 崩溃报告，定位后再修复。
4. 连接测试仪，从协商前开始采集，记录完整 PD 事件以判断 unconfirmed 的具体原因。
5. 使用多个恢复会话和 1000 SPS，检查设置页延迟与后台扫描刷新；如仍卡顿，测量图表计算和写盘协调耗时。
6. 两种语言、两套皮肤、1024×700 与 1280×820 检查；完成相关测试后再打包、安装和发布。

参考文档：[中文说明](../README.zh-CN.md)、[2026-09-08 发行说明](RELEASE-2026-09-08.md)、[UI 改进记录](UI-IMPROVEMENTS.md)、[安装说明](../Distribution/安装说明.md)。
