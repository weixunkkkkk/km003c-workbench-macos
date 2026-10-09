# KM003C 工作台 v0.1.0 (6) 本地交付

日期：2026-10-09。按用户要求，将“评估是否合并 PR 15”窗口的已完成修复与本窗口的剩余修复合并，生成 Universal App、DMG，并安装到本地。

## 本次整合

- PR #15 已合并至 GitHub main：`fb5f276788d30afa831f063e7f27943660647472`。查询确认合并前 Linux、macOS、Windows、Rust 1.97 与 Python 检查全绿。
- 纳入 PR 的后台采样活动、停流重启和中断记录、分段保存时序、退出 USB 释放、字体与打包兼容修复。
- 纳入本窗口的仪表栏折叠、多标尺及真实 CSV 格式导入；无设备累计量的外部 CSV 明确标注为积分派生值，跨分段和未知长间隔不积分。
- 本地整合提交：`6cec1d7f7348369cfd6cd8d7f696c8d1df1b5a97`。本次没有推送本地增量、创建发布标签或上传新 Release；不直接关闭 Issue #9。

## 验证结果

- `cargo fmt --all -- --check`、`git diff --check` 通过。
- 完整工作区测试：254 项通过、4 项忽略；GUI 为 136 项通过、4 项忽略。
- `cargo clippy --locked --workspace --all-targets -- -D warnings` 通过。
- 发布版本与磁盘镜像工具 Python 测试分别 3 项、1 项通过。
- 独立运行真实 CSV 样本测试通过；365,092 点，末点 4,263.039858 秒，派生累计能量 45.937907 Wh、容量 2.332040 Ah。原文件 SHA-256 未改变，文件没有上传或复制入仓库。
- Apple Silicon 与 Intel Release 构建通过，Universal 包包含 `arm64` 和 `x86_64`；严格 ad-hoc 签名验证通过。没有 Apple Developer ID 签名或公证。
- 新 DMG 原路径 `hdiutil verify` 完整通过；SHA-256、只读挂载、镜像内版本、架构与签名检查通过。创建镜像需在沙箱外运行 macOS 磁盘工具。
- 安装前通过应用清单确认正式 App 未运行；保留旧 build 5 备份，再替换 `/Applications/KM003C 工作台.app`。安装后二进制与构建产物完全一致，签名通过。
- 从已安装 App 正常启动，在“诊断与关于”实际确认 `KM003C 工作台 v0.1.0 (6)`。
- 实际窗口只读导入上述 CSV，点数和累计值与测试相符，完整度明确为未知；折叠／展开仪表栏成功；右键两个时刻生成 M1/M2，显示 U/I/P 与差值；E/Q 开关和点击标尺固定读数成功。

## 文件与校验

- 安装 App：`/Applications/KM003C 工作台.app`。
- DMG：`dist/issue9-pr15-20261009-build6/KM003C-Workbench-v0.1.0-macOS-universal.dmg`。
- 校验文件：同名 `.dmg.sha256`。
- 旧 App 备份：`release/rollback/issue9-pr15-20261009-build5/KM003C 工作台.app`。

DMG SHA-256：`4615593f78eeef72f2e5587a9a96ac9e4fd9239a5f425b295d3f3b33053573ce`。

Universal 二进制 SHA-256：`39d836688d365e2dbf86b33a6969b84cb41632af7e7af450b56f18da3ebfa1d5`。

## 边界与后续

未发现 KM003C/KM002C 真机；本轮未执行 USB 后台十分钟录制、实际锁屏恢复、四档采样率或真实 PD 协商。用户提供的 Intel/macOS 13.7.8 Touch Bar/KVO 退出崩溃尚未在对应设备复现，退出流程加固不能作为该崩溃已修复的证明。

已兼容的是用户实际提供的 laPower 工程 CSV；未收到可确认的 POWER-Z Windows 导出样本，不宣称覆盖所有官方格式或已经通过 Windows 反向导入。

实际窗口验收期间 Mac 锁屏，后续 English、1024×700、两套皮肤和退出的 GUI 验收停止；保留已完成的启动、版本、导入与标尺证据，不将未执行项写为通过。已安装 App 停留在只读导入图表，可点击“返回实时”回到设备采样页。

仅清理本次隔离测试 App、演示存储与打包中间目录；保留正式录制、偏好、原 CSV、交付文件与旧 App 备份。
