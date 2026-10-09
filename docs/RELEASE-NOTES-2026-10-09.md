# KM003C 工作台 · 后台稳定性、CSV 导入与界面对齐

App `0.1.0 (7)` · 发布标签 `v0.1.0-20261009`

## 本次更新

- 整合已合并 PR #15：后台采样活动、停流重启与中断标记、分段保存时序、有界退出和字体兼容等修订。
- 仪表栏可以折叠，图表支持多个时间标尺，比较 U/I/P/E/Q 读数和差值；主轨与累计轨共享标尺时刻。
- 支持已提供的 laPower 工程 CSV 格式。没有设备累计量时，能量与容量标明为积分派生量，不跨分段和未知长间隔积分，缺少质量信息时显示未知。
- 修复“全程／标尺／固定游标”控件高度错位；统一 32px 高和 8px 间距，长文件名不改变控件位置。
- 会话累计的标签、方向、数字和单位使用固定列；数字右对齐、单位左对齐。
- 设置语言下拉框最多 180px 宽，简中与 English 使用相同布局尺寸。

## 数据与验证

现有录制、恢复会话、偏好和 23 列 CSV/Parquet 契约不变，无需迁移。用户的原始 CSV、偏好和恢复目录不随版本上传。

本地已检查 Universal 双架构、DMG 完整性、严格 ad-hoc 签名和 SHA-256。正式安装版关于页确认 build 7，实际导入 365,092 点 CSV，并检查图表控件和累计量对齐。中英文与三种窗口尺寸的导入布局几何回归检查通过。

CI 检查可在[Actions](https://github.com/weixunkkkkk/km003c-workbench-macos/actions)查看；详细本地证据见 [build 6 整合记录](RELEASE-2026-10-09-ISSUE9-PR15.md) 和 [build 7 安装验收](RELEASE-2026-10-09-ALIGNMENT.md)。

尚未复验真机 USB、四档采样率、长时间后台／锁屏和完整 PD 协商；Intel/macOS 13.7.8 Touch Bar/KVO 原退出崩溃未在对应机器上确认修复。兼容范围是已验证的 laPower CSV，不代表所有 POWER-Z Windows 导出格式。Issue #9 不因本次发布直接结案。

## 下载、安装与回退

下载本页附件 `KM003C-Workbench-v0.1.0-macOS-universal.dmg`。安装前结束录制并正常退出旧版，再将“KM003C 工作台.app”拖入 Applications。macOS 包支持 Apple Silicon 和 Intel，使用 ad-hoc 签名，未 Apple Developer ID 公证。

该 DMG 与用户本地安装的 build 7 包字节一致，SHA-256：

```text
797dabc4f5787032ba3b96f6843aa958fbd012f46e6d43e5249011fbde8b2699
```

下载同名 `.dmg.sha256` 至 DMG 同目录，执行：

```bash
shasum -a 256 -c KM003C-Workbench-v0.1.0-macOS-universal.dmg.sha256
```

旧发布 [v0.1.0-20261004](https://github.com/weixunkkkkk/km003c-workbench-macos/releases/tag/v0.1.0-20261004) 保留，可回退应用；不删除偏好、日志或待恢复录制。固件升级仍未提供，本项目不是 ChargerLAB 官方软件。
