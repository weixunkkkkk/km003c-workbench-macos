# KM003C 工作台

简体中文 | [English README](README.md)

KM003C 工作台是一款面向 macOS 的 USB-C 电源测试仪桌面软件。它保留
`km003c-rs` 的 Rust USB 通信、测量积分、USB PD 解码和离线记录能力，使用
Rust/egui 提供适合长时间采样的中文仪表界面。

> 本项目不是 ChargerLAB 官方软件。它基于开源的
> [km003c-rs](https://github.com/okhsunrog/km003c-rs)，并参考了公开的
> [WITRN-RS](https://github.com/KHWLGH/WITRN-RS) 交互思路；没有复制其源代码或
> 受 GPL-3.0 保护的实现。

## 主要功能

- **实时监控**：同时显示 VBUS 电压、IBUS 电流和功率，数字读数、最小值、平均值和最大值使用等宽字体，便于观察微小变化。
- **录制与恢复**：点击开始后立即录制，可暂停、继续和保存；录制期间会安全写入恢复目录，拔线、锁屏或应用异常退出后可以查看、保存或继续录制。
- **累计统计**：按实际写入区间统计录制时长、采样点、累计能量、累计容量和带方向净能量。累计能量使用功率绝对值积分，正向或反向测量都不会错误显示为零。
- **实时图表**：电压、电流、功率合并显示，共享时间轴和联动游标；游标表格会显示最近采样点的真实 V/A/W 数值。
- **全程跟随**：录制时导航条自动跟随最新采样；也可以拖动、缩放到任意时间段，点击“回到最新”恢复跟随。
- **实际/相对刻度**：实际刻度保留 V/A/W 工程单位；相对量程用于比较不同数量级的曲线。自适应范围采用稳健窗口，尖峰保留在原始数据中。
- **自定义曲线**：主图固定 V/I/P，D+、D−、CC1、CC2、电阻、累计量和净量等通道可在高级分析中自行开启。
- **USB PD 分析**：只有完整捕获 Source Capabilities、Request、Accept 和 PS_RDY 后，才标记当前固定档、PPS、EPR/AVS 合同；无法确认时不会根据电压猜测 QC、VOOC 或 UFCS。
- **离线记录**：读取设备保存的离线记录，或导入 KM003C 专用 CSV/Parquet。导入会校验 23 列字段、类型和时间顺序。
- **数据导出**：CSV 和 Parquet 继续保持原有 23 列契约，并生成包含北京时间起止时间、时长和完整度的伴随元数据。
- **自动控制**：可选按功率、电流或电压阈值自动暂停和恢复，带回差和持续时间防抖；默认关闭。
- **中英文界面**：设置中可切换简体中文和 English，协议名称、单位和原始字段保留行业常用英文写法。
- **演示模式**：使用 `--demo` 启动确定性模拟数据，界面会明确标注演示数据；正常启动不会自动进入演示模式。

## 系统要求

- macOS 11 或更高版本
- Apple Silicon 或 Intel Mac（Universal App）
- 一台通过 USB 连接的 POWER-Z KM003C

应用默认跳过 USB reset；如果设备需要，可在设置中手动启用高级 USB reset。录制时的锁屏保护只阻止系统空闲睡眠，合盖或用户主动睡眠仍可能中断 USB 链路。

## 安装与运行

### 使用 DMG

从 GitHub Releases 下载 Universal DMG，将“KM003C 工作台.app”拖到“应用程序”。当前版本使用 ad-hoc 签名、尚未进行 Developer ID 公证；首次打开时如果 macOS 显示安全提示，请在 Finder 中右键应用并选择“打开”。

### 从源码运行

```bash
git clone https://github.com/weixunkkkkk/km003c-workbench-macos.git
cd km003c-workbench-macos
cargo run -p km003c-egui --bin KM003CWorkbench
```

不连接设备也可以检查界面：

```bash
cargo run -p km003c-egui --bin KM003CWorkbench -- --demo
```

构建 macOS Universal App 和 DMG：

```bash
./Scripts/package_app.sh
./Scripts/make_dmg.sh
./Scripts/verify_dmg.sh
```

生成的 App 和 DMG 位于本地 `dist/`，不会被提交到源码仓库。

## 录制文件格式

CSV/Parquet 仍是 23 列稳定格式，包含设备相对时间、序号、VBUS、IBUS、功率、
CC1/CC2、D+/D−、累计容量和能量等字段。原始采样不会因为图表降噪而被删除；
显示层的中值滤波和降采样只影响波形呈现。

录制文件旁边会保存 `.km003c.json` 元数据，记录 UTC 和北京时间的开始、结束、
保存时间、有效时长、暂停/断线区间、采样率、点数和数据完整度。没有真实时间元数据
的旧文件会显示“记录时间未知”，不会使用文件修改时间伪造记录时间。

## 本地数据位置

应用配置、日志和待恢复录制保存在：

```text
~/Library/Application Support/com.weixun.km003cworkbench/
```

其中日志位于 `logs/`，待恢复录制位于 `Recordings/Pending/`。设备数据、序列号和录制内容不会写入普通偏好配置。

## 开源许可

本项目沿用核心仓库的双许可证，用户可以选择：

- [MIT License](LICENSE-MIT)
- [Apache License 2.0](LICENSE-APACHE)

协议解析和测量核心来自
[okhsunrog/km003c-rs](https://github.com/okhsunrog/km003c-rs)。WITRN-RS 仅作为公开功能和交互参考，具体差异与许可证边界见
[`Distribution/WITRN-RS-参考迁移.md`](Distribution/WITRN-RS-参考迁移.md)。

## 验证状态

协议解析、测量、录制、导入导出和 GUI 单元测试可以在没有设备时运行。Universal
构建、DMG 完整性、Info.plist 和 ad-hoc 签名已经过本地验证；KM003C 真机上的
USB 锁屏续录、拔插恢复和完整协议协商仍需要连接设备进行验收。

欢迎提交 Issue 或 Pull Request。
