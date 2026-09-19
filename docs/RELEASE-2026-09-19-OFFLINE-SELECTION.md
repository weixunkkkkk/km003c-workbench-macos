# KM003C 工作台 v0.1.0 · 离线选择与记录管理修订

日期：2026-09-19
App 版本：`0.1.0 (3)`
发布标签：[`v0.1.0-20260919-2`](https://github.com/weixunkkkkk/km003c-workbench-macos/releases/tag/v0.1.0-20260919-2)

## 更新规则

- App 的用户可见版本仍保持 `0.1.0`；同一 App 版本的可分发修订使用 `v0.1.0-YYYYMMDD` 日期标签，同日重复构建追加 `-N` 序号，不覆盖历史 Release。
- DMG 文件名保持 `KM003C-Workbench-v0.1.0-macOS-universal.dmg`，每个日期版 Release 单独保存对应的 `.sha256` 文件。
- 源码标签、Release 说明、中文安装说明和 `CHANGELOG.md` 必须在发布前同步；Release 正文应列出更新内容、验证结果和未完成的真机边界。
- 本次修订不改变 `km003c-lib` 公共 API、CSV/Parquet 23 列契约、USB 请求或积分口径。

## 本次更新

- 记录管理从设置内容中独立出来，恢复录制和设备离线记录使用固定动作列。
- 离线目录刷新按文件名和数据地址保留选择；所选记录消失时回退到首条，空目录清除选择。
- 设备离线入口在已连接设备时可用；下载失败会清理旧离线视图和导出来源。
- 导入文件支持返回实时数据和另存原格式副本；副本写入不会覆盖目标文件，并同步保留 `.km003c.json` 元数据。
- 离线摘要与最后采样点存在有限舍入或末间隔差异时，保留原始采样并显示差异提示，不虚构补点。
- 设置分类、表单网格、图表统计、紧凑窗口和双语文案重新对齐；恢复列表和长文件名不会撑大窗口。
- `APP_BUILD` 更新为 `3`，增加只读 `offline_snapshot` 示例和项目移交文档。

## 验证

- `cargo fmt --all -- --check` 和 `git diff --check` 通过。
- `cargo test --locked` 通过：GUI 99 项通过、3 项需要设备或外部验收文件的测试忽略；库、集成和文档测试全部通过。
- `cargo clippy --workspace --all-targets --locked -- -D warnings` 通过。
- `Scripts/verify_dmg.sh` 通过：DMG 完整性、`Info.plist`、`arm64 + x86_64` Universal 二进制及 ad-hoc 签名均正常。
- 当前 DMG SHA-256：
  `5505f1f6f8606a952b6c2223316a1c39b32d174f4b545eab28d1d68e74dd077c`。

## 仍需真机验收

尚未用真实 KM003C 完成锁屏续录、主动睡眠唤醒、拔插续录、四档采样率、设备离线目录刷新/下载重试及完整 USB PD 协商。演示数据、自动化测试和 DMG 验证不能替代这些链路。

## 回滚

如果发现回归，停止分发本标签，回退到
[`v0.1.0-20260908`](https://github.com/weixunkkkkk/km003c-workbench-macos/releases/tag/v0.1.0-20260908) 或更早的 `v0.1.0`；用户本地可移除新 App 后重新安装旧 DMG。用户配置、日志和 Pending 恢复录制不属于发布资产，不应随 App 回滚删除。
