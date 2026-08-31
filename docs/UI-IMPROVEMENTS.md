# KM003C Workbench UI 修改指令

目标文件：`km003c-egui/src/main.rs`（另有 `theme.rs` 常量可用，勿新增硬编码色值）

> 说明：工作区中第 1、2、3 条可能已被部分实施，动手前先 `git diff` 确认现状，避免重复修改。

## 1. 设置窗口改为居中弹窗

`show_settings_window`：

- `.anchor(egui::Align2::RIGHT_TOP, [-12.0, 58.0])` → `.anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])`
- 窗口 Id 从 `"settings_drawer"` 改为 `"settings_window"`（重置 egui 持久化位置）
- 尺寸：`default_width(440.0)`、`min_width(400.0)`、`max_width(560.0)`
- `max_height` 改为 `(ctx.content_rect().height() - 120.0).max(420.0)`

## 2. 设置分组卡片化 + 表单对齐

- 新增模块级自由函数：

```rust
fn settings_section(ui: &mut egui::Ui, title: &str, default_open: bool, add_contents: impl FnOnce(&mut egui::Ui)) {
    let frame_width = ui.available_width();
    egui::Frame::NONE
        .fill(theme::PANEL_RAISED)
        .stroke(egui::Stroke::new(1.0, theme::DIVIDER))
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.set_min_width((frame_width - 20.0).max(120.0));
            egui::CollapsingHeader::new(egui::RichText::new(title).strong())
                .id_salt(("settings_section", title))
                .default_open(default_open)
                .show(ui, add_contents);
        });
    ui.add_space(6.0);
}
```

- `show_settings_content` 里 8 个分组（数据源 / 设备信息 / 录制 / 可恢复录制 / 图表 / 数据质量 / 设备离线记录 / 关于）统一改用 `settings_section`；删除"常用设置"大标题，只保留一行 small muted 说明。
- 默认展开：数据源、录制、图表；其余默认收起。
- 表单行用 `egui::Grid::new(...).num_columns(2).spacing([12.0, 6.0])` 对齐：
  - 录制组：文件格式 / 功率阈值 / 持续时间
  - 图表组：默认时间窗 / 曲线降噪
  - label 统一 `egui::RichText::new(label).color(theme::MUTED_TEXT)`，控件第二列左对齐
- "数据质量"分组 label 同样改 muted 色，与"设备信息"一致。
- 降噪说明挂在 ComboBox response 上：`show_ui` 返回 `Option<InnerResponse>`，用 `if let Some(inner) = ... { inner.response.on_hover_text("五点中值滤波只改变屏幕曲线；游标、统计、录制和导出始终使用原始采样。"); }`
- "关于"组三行说明合并为一行 small muted："基于 km003c-rs · MIT / Apache-2.0 · 不是 ChargerLAB 官方软件"，日志路径保留一行。

## 3. 读数卡统一（`instrument_card`）

- 圆角 `CornerRadius::same(7)` → `same(6)`，与"录制累计 / 当前协议 / 信号线"面板一致。
- 无统计数据时不再渲染三个"—"占位，改为两行居中提示：
  - "最小 · 平均 · 最大"（small，`theme::MUTED_TEXT`）
  - "记录后显示统计"（small，`theme::MUTED_TEXT.gamma_multiply(0.6)`）
- `set_min_height` 保持不变，避免布局跳动。

## 4. 图表 Y 轴跟随曲线开关（`show_combined_monitor_chart`）

- 将 `let visible_series = self.visible_series;` 移到构建 `axes` 之前。
- `custom_y_axes` 只放入当前可见序列对应的轴：隐藏功率曲线时右侧功率轴同步隐藏；电压 / 电流同理。
- 序列可见性逻辑已保证至少一条曲线可见，无需额外兜底。

## 5. 信号线芯片等宽

- 非 compact 模式下 D+ / D− / CC1 / CC2 四个芯片宽度按 `(面板内宽 - 18.0 列间距) / 2 - 12.0 芯片内边距` 计算，传给 `signal_value` 新增 `min_width: f32` 参数，在 Frame 内 `ui.set_min_width(min_width)`，消除内容长短导致的参差。

## 约束

- 只改 UI 层，不改录制 / 导出 / 统计的数据逻辑。
- 颜色一律用 `theme.rs` 常量（`PANEL` / `PANEL_RAISED` / `DIVIDER` / `MUTED_TEXT` / `VOLTAGE` / `CURRENT` / `POWER` / `RECORDING`）。
- 圆角统一 6px（大面板 8px 仅图表主区保留）。
- 改完 `cargo check -p km003c-egui` 必须通过。
