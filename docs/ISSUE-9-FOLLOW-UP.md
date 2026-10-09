# Issue #9 剩余项实施与验证

基线：`50178e3`，2026-10-09。PR #15 不直接合并，Issue #9 不直接结案。

## 范围与顺序

1. 批准 PR #15 等待中的 CI，核对各平台结果；不放宽 Actions 权限。
2. 加固现有退出流程：先封口录制，等待 USB 释放确认，再结束应用；等待必须有上限。
3. 复用现有仪表栏和游标读数，加入折叠与多标尺。标尺在切换数据源、开始新会话和清空数据时清除；缩放、暂停、皮肤切换不清除。标尺在主轨与累计轨共享时间，读数不覆盖绘图区。
4. 按真实 CSV 样本增加明确格式分支，不改变本机 23 列 CSV/Parquet 契约；不猜测不明列的单位。外部 CSV 缺少设备累计量时，计算结果标明为派生值；分段间不积分，质量信息未知，不伪造完整度。

## 已收到证据

- CSV 样本：`lapower_data_2026-10-06T10-18-09-660Z … .csv`。含前导信息、`Time(D.hh:mm:ss.ms)`、`Voltage(V)`、`Current(A)`、`Power(W)`、信号线、`RelativeTime(s)`、`Timestamp(ms)`、`RecordingSegment` 和 `SampleInterval(ms)`。365,092 点，末点约 4263.04 秒。仅按这一格式确认兼容，不将它泛称为所有 POWER-Z CSV。
- 崩溃截图：Intel x86_64，macOS 13.7.8，v0.1.0 build 1；主线程 `EXC_BAD_INSTRUCTION / SIGILL`。可见异常栈经过 Foundation KVO `removeObserver` 与 AppKit `_NSTouchBarFinderObservation invalidate`。这是窗口响应链/Touch Bar 清理线索，不能仅凭截图认定 USB 释放是根因。

## 完成判据

- 相关单元测试、完整测试、严格 Clippy 与格式检查通过。
- 用真实 CSV 导入，核对点数、首末时间、工程单位、累计口径及原文件未被改写。
- 演示窗口检查中英文、1024×700 与1280×820，折叠后测量和录制继续，多标尺可添加、删除、比较，源切换不遗留旧标尺。
- 退出等待测试覆盖确认、积压消息、无连接和超时；实际启动/退出无新的异常。
- 原 Intel/macOS 13 Touch Bar 崩溃与 Windows 软件反向导入，未在对应真实环境执行前保持“尚未验证”。

不新增依赖、偏好字段、固件功能或无关 UI 重写；不安装或覆盖当前正式 App。
